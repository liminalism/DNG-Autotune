//! Local, multi-illuminant white balance.
//!
//! After fierro2009, which treats the brightest regions of a frame as the
//! *lights* illuminating it, clusters them, and corrects each pixel by a blend
//! of their white points weighted by spatial and chromatic proximity. That
//! addresses mixed lighting — window light against tungsten, say — which a
//! single as-shot white balance cannot.
//!
//! # Three deliberate departures from the paper
//!
//! **Opt-in.** `docs/KNOWN_LIMITATIONS.md` records that sunsets, stage light,
//! LEDs and underwater scenes are *intentionally* left un-neutralized. Making
//! this automatic would silently reverse that decision, so it runs only when
//! asked for, and its strength is a dial.
//!
//! **Deterministic clustering.** The paper clusters with K-means seeded at
//! random plus a Gap statistic, and states plainly that "repeatability is not
//! granted". This crate promises deterministic output, so lights are found by
//! quantizing the chromaticity plane onto a fixed grid and merging cells in a
//! fixed order. The same input always gives the same lights.
//!
//! **Chromatic only.** The paper notes its correction "will always increase the
//! lightness of the pixel". Here exposure is the tone controller's job, so each
//! white point is normalized to unit luminance before use — and a weighted
//! blend of unit-luminance white points has unit luminance too, since
//! luminance is linear (checked by `white_points_carry_unit_luminance` below).
//!
//! That is *not* enough to make the correction exposure-neutral, though: the
//! correction below **divides** each channel by the blended white point, and
//! the reciprocal of a unit-luminance vector does not itself carry unit
//! luminance in general. Concretely, for weights `c_i` summing to 1 and
//! `w_i > 0` with `sum(c_i * w_i) == 1`, convexity of `1/x` gives (Jensen)
//! `sum(c_i / w_i) >= 1 / sum(c_i * w_i) == 1`, with equality only when every
//! `w_i` is equal, i.e. `w` is neutral. So dividing by a non-neutral,
//! unit-luminance white point *raises* the pixel's luminance, in proportion to
//! how non-neutral the local light is — an exposure change hiding inside a
//! colour operator, which is exactly what this module claims not to do.
//! `apply` therefore rescales every corrected pixel back to its
//! pre-correction luminance after the division (see `luminance_restore_gain`);
//! that restore step, not the unit-luminance normalization by itself, is what
//! actually makes the correction move colour without moving exposure.
//!
//! # Why it only acts on mixed lighting
//!
//! Rawler has already applied the camera's as-shot white balance, so the
//! dominant illuminant is handled. Running a white-patch correction on top of
//! that would fight it. The correction is therefore applied only when two or
//! more chromatically distinct lights are found; with one light the paper's
//! method degenerates to plain von Kries, which is what as-shot already did.

use crate::analyze::luminance;
use crate::types::LinearImage;
use rayon::prelude::*;
use serde::Serialize;

/// Fractions of the brightest pixels treated as lights, tried in order.
///
/// fierro2009 uses the top 5%, and notes that for some images this "does not
/// provide enough information and the segmentation step produces only a single
/// class of highlight areas, resulting in a pure Von Kries method", prescribing
/// "the repetition of the thresholding with a lower value each time, until more
/// than one class is found". Two illuminants of unequal brightness are exactly
/// that case: at 5% only the brighter one is sampled.
///
/// The escalation copes with illuminants whose brightness ranges overlap, which
/// is the normal case. It cannot help when one illuminant is uniformly brighter
/// than the other across the whole frame with no overlap at all — then no
/// threshold admits both, and only the brighter is found.
const LIGHT_FRACTIONS: [f32; 4] = [0.05, 0.12, 0.25, 0.45];
/// Resolution of the chromaticity grid used for clustering, per axis.
const CHROMA_GRID: usize = 24;
/// Chromatic distance under which two grid cells are merged into one light.
const MERGE_DISTANCE: f32 = 0.055;
/// Chromatic distance at which a light stops influencing a pixel.
const CHROMA_REACH: f32 = 0.35;
/// Lights holding less than this share of the sampled bright pixels are noise.
const MIN_LIGHT_SHARE: f32 = 0.08;
/// Most lights kept, strongest first.
const MAX_LIGHTS: usize = 4;
/// Two lights this far apart chromatically count as genuinely different.
const DISTINCT_DISTANCE: f32 = 0.02;
/// Chromatic distance from neutral beyond which a bright region is treated as a
/// coloured *object* rather than an illuminant to be neutralized.
///
/// fierro2009 assumes the brightest regions are lights whose cast should be
/// removed. For a campfire, a sunset or a neon sign that assumption inverts the
/// picture: on the test corpus, a fire at roughly 0.39 from neutral was read as
/// an orange cast and "corrected" until the flames came out green. A mixed
/// tungsten/daylight interior sits nearer 0.12, so a threshold here separates
/// the case the method is for from the case it destroys.
const MAX_ILLUMINANT_CAST: f32 = 0.22;
/// Neutral chromaticity: equal parts red, green and blue.
const NEUTRAL: (f32, f32) = (1.0 / 3.0, 1.0 / 3.0);
/// Approximate pixels sampled when searching for lights.
const TARGET_SAMPLES: usize = 200_000;
/// Regularization used when restoring a pixel's luminance after the
/// per-channel division in [`apply`]; see [`luminance_restore_gain`].
///
/// Small next to any real pixel value (linear samples on the test corpus run
/// from about 0.01 to a few), so it leaves the restore for ordinary pixels
/// unaffected to well under float roundoff, while still bounding the gain
/// applied to pixels whose luminance sits at or near zero.
const LUMINANCE_RESTORE_EPS: f32 = 1.0e-4;

/// One detected illuminant.
#[derive(Debug, Clone, Serialize)]
pub struct Light {
    /// Centroid in normalized image coordinates, both axes 0..1.
    pub x: f32,
    pub y: f32,
    /// White point, normalized to unit luminance.
    pub white_point: [f32; 3],
    /// Share of sampled bright pixels belonging to this light.
    pub share: f32,
}

/// What the estimator found.
#[derive(Debug, Clone, Serialize)]
pub struct LocalWhiteBalance {
    pub lights: Vec<Light>,
    /// Largest chromatic distance between any two lights. The correction is
    /// skipped below [`DISTINCT_DISTANCE`].
    pub separation: f32,
    /// Whether a correction was actually applied.
    pub applied: bool,
    pub strength: f32,
}

/// Chromaticity of a colour as `(r, b)` fractions of the channel sum.
///
/// Two coordinates suffice because the three fractions sum to one. Neutral sits
/// at `(1/3, 1/3)`.
#[inline]
fn chromaticity(rgb: [f32; 3]) -> Option<(f32, f32)> {
    let sum = rgb[0] + rgb[1] + rgb[2];
    if !sum.is_finite() || sum <= 1.0e-6 {
        return None;
    }
    Some((rgb[0] / sum, rgb[2] / sum))
}

#[inline]
fn chroma_distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dr = a.0 - b.0;
    let db = a.1 - b.1;
    (dr * dr + db * db).sqrt()
}

/// One accumulated chromaticity grid cell.
#[derive(Clone, Copy, Default)]
struct Cell {
    count: u32,
    sum_r: f64,
    sum_g: f64,
    sum_b: f64,
    sum_x: f64,
    sum_y: f64,
}

/// Find the lights in `image`, widening the brightness threshold until more
/// than one is found or the thresholds run out.
///
/// Returns `None` when the frame has too little bright area to say anything.
pub fn detect(image: &LinearImage) -> Option<Vec<Light>> {
    let mut fallback = None;
    for fraction in LIGHT_FRACTIONS {
        match detect_at(image, fraction) {
            Some(lights) if lights.len() >= 2 => return Some(lights),
            Some(lights) => fallback = Some(lights),
            None => {}
        }
    }
    fallback
}

/// Find lights using one specific brightness threshold.
fn detect_at(image: &LinearImage, light_fraction: f32) -> Option<Vec<Light>> {
    let total = image.width.saturating_mul(image.height);
    if total == 0 || image.width < 8 || image.height < 8 {
        return None;
    }
    let stride = (total / TARGET_SAMPLES.max(1)).max(1);

    // Luminance threshold for "this is a light": the top LIGHT_FRACTION.
    let mut luminances: Vec<f32> = image
        .pixels
        .iter()
        .step_by(stride)
        .map(|pixel| luminance(*pixel))
        .filter(|value| value.is_finite() && *value > 0.0)
        .collect();
    if luminances.len() < 256 {
        return None;
    }
    luminances.sort_unstable_by(f32::total_cmp);
    let index = ((luminances.len() - 1) as f32 * (1.0 - light_fraction)) as usize;
    let threshold = luminances[index];

    let mut grid = vec![Cell::default(); CHROMA_GRID * CHROMA_GRID];
    let mut bright = 0u32;

    for (offset, pixel) in image.pixels.iter().enumerate().step_by(stride) {
        let value = luminance(*pixel);
        if !value.is_finite() || value < threshold {
            continue;
        }
        let Some((r, b)) = chromaticity(*pixel) else {
            continue;
        };

        let column = ((r * CHROMA_GRID as f32) as usize).min(CHROMA_GRID - 1);
        let row = ((b * CHROMA_GRID as f32) as usize).min(CHROMA_GRID - 1);
        let cell = &mut grid[row * CHROMA_GRID + column];

        cell.count += 1;
        cell.sum_r += pixel[0] as f64;
        cell.sum_g += pixel[1] as f64;
        cell.sum_b += pixel[2] as f64;
        cell.sum_x += ((offset % image.width) as f64) / image.width as f64;
        cell.sum_y += ((offset / image.width) as f64) / image.height as f64;
        bright += 1;
    }

    if bright < 128 {
        return None;
    }

    // Order cells by population, breaking ties by index so the result cannot
    // depend on iteration order.
    let mut order: Vec<usize> = (0..grid.len()).filter(|i| grid[*i].count > 0).collect();
    order.sort_by(|a, b| grid[*b].count.cmp(&grid[*a].count).then(a.cmp(b)));

    // Agglomerate: each cell joins the first accepted light within
    // MERGE_DISTANCE, otherwise starts a new one.
    let mut lights: Vec<(Cell, (f32, f32))> = Vec::new();
    for index in order {
        let cell = grid[index];
        let column = index % CHROMA_GRID;
        let row = index / CHROMA_GRID;
        let position = (
            (column as f32 + 0.5) / CHROMA_GRID as f32,
            (row as f32 + 0.5) / CHROMA_GRID as f32,
        );

        match lights
            .iter_mut()
            .find(|(_, centre)| chroma_distance(*centre, position) < MERGE_DISTANCE)
        {
            Some((accumulated, _)) => {
                accumulated.count += cell.count;
                accumulated.sum_r += cell.sum_r;
                accumulated.sum_g += cell.sum_g;
                accumulated.sum_b += cell.sum_b;
                accumulated.sum_x += cell.sum_x;
                accumulated.sum_y += cell.sum_y;
            }
            None => lights.push((cell, position)),
        }
    }

    let mut found: Vec<Light> = lights
        .into_iter()
        .filter_map(|(cell, _)| {
            let share = cell.count as f32 / bright as f32;
            if share < MIN_LIGHT_SHARE {
                return None;
            }
            let n = cell.count as f64;
            let mean = [
                (cell.sum_r / n) as f32,
                (cell.sum_g / n) as f32,
                (cell.sum_b / n) as f32,
            ];
            // Unit luminance, so the correction cannot change exposure.
            let scale = luminance(mean);
            if !scale.is_finite() || scale <= 1.0e-8 {
                return None;
            }
            let white_point = [mean[0] / scale, mean[1] / scale, mean[2] / scale];

            // Reject strongly coloured lights: they are the subject, not a cast.
            let chroma = chromaticity(white_point)?;
            if chroma_distance(chroma, NEUTRAL) > MAX_ILLUMINANT_CAST {
                return None;
            }

            Some(Light {
                x: (cell.sum_x / n) as f32,
                y: (cell.sum_y / n) as f32,
                white_point,
                share,
            })
        })
        .collect();

    found.sort_by(|a, b| b.share.total_cmp(&a.share));
    found.truncate(MAX_LIGHTS);

    (!found.is_empty()).then_some(found)
}

/// Largest chromatic distance between any two lights.
pub fn separation(lights: &[Light]) -> f32 {
    let mut worst = 0.0f32;
    for (index, a) in lights.iter().enumerate() {
        for b in &lights[index + 1..] {
            let (Some(ca), Some(cb)) = (chromaticity(a.white_point), chromaticity(b.white_point))
            else {
                continue;
            };
            worst = worst.max(chroma_distance(ca, cb));
        }
    }
    worst
}

/// The white point acting on one pixel: lights blended by spatial and
/// chromatic proximity, per fierro2009's `cf = alpha * beta`.
#[inline]
fn blended_white_point(
    lights: &[Light],
    light_chroma: &[(f32, f32)],
    x: f32,
    y: f32,
    pixel: [f32; 3],
) -> [f32; 3] {
    let pixel_chroma = chromaticity(pixel);

    let mut weights = [0.0f32; MAX_LIGHTS];
    let mut total = 0.0f32;

    for (index, light) in lights.iter().enumerate() {
        // Spatial proximity, normalized on the image diagonal.
        let dx = x - light.x;
        let dy = y - light.y;
        let spatial = 1.0 - ((dx * dx + dy * dy).sqrt() / std::f32::consts::SQRT_2).min(1.0);

        // Chromatic proximity between the pixel and this light.
        let chromatic = match pixel_chroma {
            Some(chroma) => {
                1.0 - (chroma_distance(chroma, light_chroma[index]) / CHROMA_REACH).min(1.0)
            }
            None => 1.0,
        };

        let weight = spatial * chromatic;
        weights[index] = weight;
        total += weight;
    }

    // A pixel far from every light in both position and colour: fall back to
    // equal influence rather than dividing by zero.
    if total <= 1.0e-6 {
        let share = 1.0 / lights.len() as f32;
        let mut blended = [0.0f32; 3];
        for light in lights {
            for (channel, value) in blended.iter_mut().enumerate() {
                *value += light.white_point[channel] * share;
            }
        }
        return blended;
    }

    let mut blended = [0.0f32; 3];
    for (index, light) in lights.iter().enumerate() {
        let share = weights[index] / total;
        for (channel, value) in blended.iter_mut().enumerate() {
            *value += light.white_point[channel] * share;
        }
    }
    blended
}

/// Gain that restores a pixel's luminance from `after` back to `before`.
///
/// The per-channel division in [`apply`] changes a pixel's luminance unless
/// the divisor happens to be neutral (see the module doc). Multiplying the
/// whole pixel by `before / after` restores it exactly, because luminance is
/// linear: `luminance(k * p) == k * luminance(p)`.
///
/// That plain ratio is not safe as written: `after` can be zero, and once
/// channels are allowed to go negative (planned for the owned-colour path) a
/// pixel can approach zero luminance from either side, not just from above.
/// Clamping `after` away from zero would dodge the blow-up but jumps the
/// moment the clamp engages — worse, clamping via `after.abs().max(eps)`
/// loses the sign at `after == 0`, so the gain would flip discontinuously as
/// `after` crosses zero. Instead this uses the damped reciprocal
/// `x / (x^2 + eps^2)`: it agrees with `1/x` to within `(eps/x)^2` once `|x|`
/// clears `eps`, and — being an odd, smooth function of `after` — passes
/// through zero continuously instead of jumping, so there is no seam in the
/// restored image at the pixels where the guard actually matters.
///
/// The one exact special case is `before == after`: there the division did
/// not move the pixel's luminance at all (typically because the local white
/// point was already neutral, or `strength` dialed it to neutral), so the
/// true restore gain is exactly 1 — not the regularized formula's
/// approximation of it — and using the exact value here is what keeps a
/// strictly neutral correction a bit-exact no-op, matching what the
/// undivided-by-luminance code already got right.
///
/// `before` or `after` being non-finite means the pixel was already garbage
/// going in (not something this operator can cause); the gain is left at 1
/// so the restore step does not manufacture a new NaN out of an existing one.
#[inline]
fn luminance_restore_gain(before: f32, after: f32) -> f32 {
    if !before.is_finite() || !after.is_finite() {
        return 1.0;
    }
    if before == after {
        return 1.0;
    }
    before * after / (after * after + LUMINANCE_RESTORE_EPS * LUMINANCE_RESTORE_EPS)
}

/// Apply a local white balance to `image` in place.
///
/// `strength` interpolates between no correction and the full blend. Returns
/// what was found and whether it acted.
pub fn apply(image: &mut LinearImage, strength: f32) -> Option<LocalWhiteBalance> {
    let strength = strength.clamp(0.0, 1.0);
    let lights = detect(image)?;
    let separation = separation(&lights);

    // As-shot white balance already handled the dominant illuminant. Only
    // genuinely mixed lighting is worth correcting; anything else would be
    // fighting a correction the camera already made.
    let applied = lights.len() >= 2 && separation >= DISTINCT_DISTANCE && strength > 0.0;

    if applied {
        let light_chroma: Vec<(f32, f32)> = lights
            .iter()
            .map(|light| chromaticity(light.white_point).unwrap_or((1.0 / 3.0, 1.0 / 3.0)))
            .collect();

        let width = image.width;
        let inverse_width = 1.0 / width as f32;
        let inverse_height = 1.0 / image.height as f32;

        image
            .pixels
            .par_iter_mut()
            .enumerate()
            .for_each(|(offset, pixel)| {
                let x = (offset % width) as f32 * inverse_width;
                let y = (offset / width) as f32 * inverse_height;
                let white = blended_white_point(&lights, &light_chroma, x, y, *pixel);
                let before = luminance(*pixel);

                for channel in 0..3 {
                    // Dial between neutral and the blended white point. This
                    // is where `strength` acts, before the division; the
                    // luminance restore below sees only its result, so it
                    // composes correctly at every strength — including 0,
                    // where the divisor is exactly 1.0 for every channel, the
                    // pixel is untouched, and `luminance_restore_gain` takes
                    // its `before == after` fast path.
                    let divisor = 1.0 + (white[channel] - 1.0) * strength;
                    if divisor > 1.0e-4 {
                        pixel[channel] /= divisor;
                    }
                }

                // The division above is not luminance-preserving on its own
                // (see the module doc); restore it so the operator moves
                // colour without moving exposure.
                let after = luminance(*pixel);
                let gain = luminance_restore_gain(before, after);
                for channel in pixel.iter_mut() {
                    *channel *= gain;
                }
            });
    }

    Some(LocalWhiteBalance {
        lights,
        separation,
        applied,
        strength,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_from(
        width: usize,
        height: usize,
        f: impl Fn(usize, usize) -> [f32; 3],
    ) -> LinearImage {
        let pixels = (0..width * height)
            .map(|index| f(index % width, index / width))
            .collect();
        LinearImage::new(width, height, pixels).unwrap()
    }

    #[test]
    fn chromaticity_is_neutral_for_grey() {
        let (r, b) = chromaticity([0.4, 0.4, 0.4]).unwrap();
        assert!((r - 1.0 / 3.0).abs() < 1.0e-6);
        assert!((b - 1.0 / 3.0).abs() < 1.0e-6);
        assert!(chromaticity([0.0, 0.0, 0.0]).is_none());
    }

    /// A frame lit by one colour must yield one light, so the correction is
    /// skipped and as-shot white balance is left alone.
    #[test]
    fn a_single_illuminant_is_not_corrected() {
        let mut image = image_from(128, 128, |x, _| {
            let level = if x < 64 { 0.05 } else { 0.9 };
            [level * 1.1, level, level * 0.8]
        });
        let before = image.pixels.clone();
        let result = apply(&mut image, 1.0).unwrap();

        assert!(!result.applied, "one illuminant must not be corrected");
        assert_eq!(image.pixels, before, "pixels must be untouched");
    }

    /// Two chromatically distinct bright regions must be found as two lights.
    #[test]
    fn two_illuminants_are_detected_and_corrected() {
        // Left half lit warm, right half lit cool; both bright enough to count.
        // Warm left, cool right, each shading from dark to bright down the
        // frame so their luminance ranges overlap, as real mixed lighting does.
        // Warm is 8.8% brighter for equal level, so the top 5% still sees only
        // warm and the threshold has to widen.
        let mut image = image_from(128, 128, |x, y| {
            let level = 0.3 + 0.7 * (y as f32 / 127.0);
            if x < 64 {
                [level * 1.30, level, level * 0.70]
            } else {
                [level * 0.70, level, level * 1.30]
            }
        });
        let result = apply(&mut image, 1.0).unwrap();

        assert!(result.lights.len() >= 2, "got {:?}", result.lights.len());
        assert!(result.separation > DISTINCT_DISTANCE);
        assert!(result.applied);
    }

    /// Correcting must move colour, not exposure: each white point carries unit
    /// luminance, so a blend of them does too.
    #[test]
    fn white_points_carry_unit_luminance() {
        let image = image_from(128, 128, |x, y| {
            let level = 0.3 + 0.7 * (y as f32 / 127.0);
            if x < 64 {
                [level * 1.30, level, level * 0.70]
            } else {
                [level * 0.70, level, level * 1.30]
            }
        });
        for light in detect(&image).unwrap() {
            let value = luminance(light.white_point);
            assert!((value - 1.0).abs() < 1.0e-4, "luminance {value}");
        }
    }

    /// The bug this module used to have, at the level of the maths: the
    /// *reciprocal* of a unit-luminance, non-neutral white point does not
    /// itself carry unit luminance. `white_points_carry_unit_luminance` above
    /// shows a light's own white point is unit luminance; this shows dividing
    /// by it is not exposure-neutral on its own — which is why `apply` now
    /// restores luminance after the division instead of relying on the
    /// unit-luminance normalization alone.
    #[test]
    fn dividing_by_a_unit_luminance_white_point_is_not_luminance_preserving() {
        let raw = [1.3f32, 1.0, 0.7]; // an arbitrary non-neutral colour
        let scale = luminance(raw);
        let white_point = [raw[0] / scale, raw[1] / scale, raw[2] / scale];
        assert!(
            (luminance(white_point) - 1.0).abs() < 1.0e-6,
            "sanity: white point must be unit luminance"
        );

        let reciprocal = [
            1.0 / white_point[0],
            1.0 / white_point[1],
            1.0 / white_point[2],
        ];
        let reciprocal_luminance = luminance(reciprocal);
        assert!(
            (reciprocal_luminance - 1.0).abs() > 1.0e-3,
            "a non-neutral white point's reciprocal should not be unit \
             luminance, but got {reciprocal_luminance}"
        );
        // Jensen's inequality on the convex 1/x: the reciprocal's luminance
        // can only be pulled up from 1, never down. This is the exposure
        // increase the module doc now describes, and `apply` corrects for.
        assert!(reciprocal_luminance > 1.0, "got {reciprocal_luminance}");
    }

    /// When the division does not move a pixel's luminance at all (the
    /// divisor was exactly 1.0 for every channel — a neutral local white
    /// point, or `strength == 0`), the restore must be an exact no-op, not
    /// merely close: this is the case the pre-restore code already got
    /// right, and it must not regress to a near-miss.
    #[test]
    fn neutral_division_is_an_exact_restore_no_op() {
        let pixel = [0.42f32, 0.17, 0.83];
        let before = luminance(pixel);
        let after = luminance(pixel); // stand-in for a divisor of exactly 1.0
        assert_eq!(luminance_restore_gain(before, after), 1.0);
    }

    /// The owned-colour path (`src/color.rs`, `--raw-color-path owned`) stops
    /// clipping out-of-gamut colours, so a pixel's luminance can be negative,
    /// or `before`/`after` can straddle zero on opposite sides. The gain must
    /// stay finite in both cases and must not flip sign discontinuously at
    /// the crossing — see the damped-reciprocal reasoning on
    /// `luminance_restore_gain`.
    #[test]
    fn restore_gain_is_finite_across_negative_and_near_zero_luminance() {
        // Negative target luminance reached from a positive `after`: finite,
        // and the restored luminance actually lands on `before`.
        let gain = luminance_restore_gain(-0.4, 0.6);
        assert!(gain.is_finite());
        assert!((gain * 0.6 - (-0.4)).abs() < 1.0e-3, "gain {gain}");

        // `after` exactly zero: the plain ratio `before / after` would be
        // infinite; the regularized gain must not be.
        assert!(luminance_restore_gain(0.5, 0.0).is_finite());

        // `after` sampled just below and just above zero: `x / (x^2 + eps^2)`
        // is an odd function of `after`, so the gain should be (anti)symmetric
        // across the crossing rather than jumping — the near-zero-luminance
        // pixel gets a large but finite, smoothly sign-flipping multiplier on
        // either side, not a step.
        let just_below = luminance_restore_gain(0.5, -1.0e-5);
        let just_above = luminance_restore_gain(0.5, 1.0e-5);
        assert!(just_below.is_finite() && just_above.is_finite());
        assert!(
            (just_below + just_above).abs() < 1.0e-2,
            "gain should flip sign smoothly across after == 0: {just_below} vs {just_above}"
        );
    }

    /// Correcting must preserve each pixel's own luminance, across both
    /// strongly saturated and near-neutral colours — not just the (unit
    /// luminance, by construction) light white points themselves.
    #[test]
    fn correction_preserves_luminance_for_saturated_and_near_neutral_colours() {
        let mut image = image_from(128, 128, |x, y| {
            let level = 0.3 + 0.7 * (y as f32 / 127.0);
            let (r_ratio, b_ratio) = match x / 32 {
                0 => (1.35, 0.65), // strongly warm
                1 => (0.65, 1.35), // strongly cool
                2 => (1.04, 0.97), // near neutral, warm-leaning
                _ => (0.97, 1.04), // near neutral, cool-leaning
            };
            [level * r_ratio, level, level * b_ratio]
        });
        let before = image.pixels.clone();
        let result = apply(&mut image, 1.0).unwrap();
        assert!(result.applied, "expected a correction to run");

        for (index, (pre, post)) in before.iter().zip(image.pixels.iter()).enumerate() {
            let l0 = luminance(*pre);
            let l1 = luminance(*post);
            assert!(
                (l0 - l1).abs() < 1.0e-4,
                "pixel {index}: {pre:?} -> {post:?}, luminance {l0} vs {l1}"
            );
        }
    }

    /// The restore must compose correctly at every strength, not just full
    /// strength: `strength` is baked into the divisor before the restore
    /// runs, so the restore should hold for whatever pixel that divisor
    /// produced.
    #[test]
    fn correction_preserves_luminance_at_partial_strength() {
        let mut image = image_from(128, 128, |x, y| {
            let level = 0.3 + 0.7 * (y as f32 / 127.0);
            if x < 64 {
                [level * 1.30, level, level * 0.70]
            } else {
                [level * 0.70, level, level * 1.30]
            }
        });
        let before = image.pixels.clone();
        let result = apply(&mut image, 0.5).unwrap();
        assert!(result.applied);

        for (pre, post) in before.iter().zip(image.pixels.iter()) {
            let l0 = luminance(*pre);
            let l1 = luminance(*post);
            assert!((l0 - l1).abs() < 1.0e-4, "luminance {l0} vs {l1}");
        }
    }

    #[test]
    fn zero_strength_changes_nothing() {
        let mut image = image_from(128, 128, |x, y| {
            let level = 0.3 + 0.7 * (y as f32 / 127.0);
            if x < 64 {
                [level * 1.30, level, level * 0.70]
            } else {
                [level * 0.70, level, level * 1.30]
            }
        });
        let before = image.pixels.clone();
        let result = apply(&mut image, 0.0).unwrap();
        assert!(!result.applied);
        assert_eq!(image.pixels, before);
    }

    /// The same input must give the same lights every time; the paper's own
    /// clustering does not guarantee this.
    #[test]
    fn detection_is_deterministic() {
        let image = image_from(160, 120, |x, y| {
            let warm = ((x / 8) + (y / 8)) % 2 == 0;
            let level = 0.4 + 0.6 * ((x * 7 + y * 13) % 11) as f32 / 11.0;
            if warm {
                [level * 1.3, level, level * 0.7]
            } else {
                [level * 0.7, level, level * 1.3]
            }
        });
        let first = detect(&image).unwrap();
        for _ in 0..4 {
            let again = detect(&image).unwrap();
            assert_eq!(first.len(), again.len());
            for (a, b) in first.iter().zip(&again) {
                assert_eq!(a.white_point, b.white_point);
                assert_eq!(a.share, b.share);
            }
        }
    }

    /// A campfire is an illuminant, but neutralizing it turns the flames green.
    /// Strongly coloured lights must be rejected outright.
    #[test]
    fn a_strongly_coloured_light_is_not_treated_as_a_cast() {
        // Deep orange flame against a dim neutral surround.
        let mut image = image_from(128, 128, |x, y| {
            let level = 0.3 + 0.7 * (y as f32 / 127.0);
            if x < 64 {
                [level * 2.20, level * 0.85, level * 0.30]
            } else {
                [level * 0.05, level * 0.05, level * 0.05]
            }
        });
        let before = image.pixels.clone();

        // Rejecting every light leaves nothing to report, so `apply` may return
        // None; either way the frame must come back untouched.
        if let Some(result) = apply(&mut image, 1.0) {
            for light in &result.lights {
                let chroma = chromaticity(light.white_point).unwrap();
                assert!(
                    chroma_distance(chroma, NEUTRAL) <= MAX_ILLUMINANT_CAST,
                    "kept a light {:.3} from neutral",
                    chroma_distance(chroma, NEUTRAL)
                );
            }
            assert!(!result.applied, "a fire must not be white balanced away");
        }
        assert_eq!(image.pixels, before, "the fire must be left alone");
    }

    #[test]
    fn tiny_images_are_declined() {
        let image = image_from(4, 4, |_, _| [0.5; 3]);
        assert!(detect(&image).is_none());
    }
}
