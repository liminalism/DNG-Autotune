//! Chroma noise reduction, scaled by the frame's own noise model.
//!
//! `docs/PLAN.md` §4 puts profiled denoising second in the 0.2 quality list and
//! says to do "chroma first — chroma noise is what makes high-ISO look broken;
//! luma denoising can be gentler". The 42 Sony pairs added in 0.1.14 turned that
//! from a plausible claim into a measured one: at ISO 1600 and above this
//! program renders at 2.2x the camera's `mean_saturation`, and a 1:1 crop shows
//! why — the extra "saturation" is red and green speckle in the shadows, which
//! the camera removes and we did not.
//!
//! # Why chroma only
//!
//! Human spatial acuity for colour is far below that for luminance, so chroma
//! can be averaged over several pixels with no visible loss of detail while
//! removing most of what makes a high-ISO frame look broken. Luma denoising is
//! the part that destroys texture, and it is deliberately not done here.
//!
//! The decomposition makes that guarantee exact rather than approximate. Each
//! pixel is split into its luminance `Y` and a colour difference `C = rgb - Y`.
//! By construction `luminance(C) == 0`, and blurring is linear, so the blurred
//! `C` also has zero luminance and `Y + blur(C)` has exactly the luminance it
//! started with. **This filter cannot change luminance at all**, which is what
//! lets it run by default: it can dull a colour edge, but it can never soften
//! detail, shift exposure, or disturb the tone controller's statistics.
//!
//! # Strength
//!
//! Driven by `snr10_ev` from [`crate::noise`] — the scene EV at which signal to
//! noise falls to 10 — because that is a measured property of this frame rather
//! than a lookup on a metadata field that some files do not carry. Across the
//! corpus it separates the cases cleanly:
//!
//! ```text
//! Sony ISO 100-200      -6.28      ProShot, all       -5.71
//! Sony ISO 500-1000     -3.63      Sony ISO 8000+     -1.00
//! Sony ISO 1600-2500    -3.08      Sony ISO 12800+    +1.94
//! ```
//!
//! Below [`INERT_SNR10_EV`] the filter allocates nothing and returns `None`, so
//! every base-ISO frame renders exactly as it did before this module existed.

use crate::analyze::luminance;
use crate::types::LinearImage;
use rayon::prelude::*;
use serde::Serialize;

/// `snr10_ev` at or below which the frame is clean enough to leave alone.
///
/// Set so that base-ISO Sony (-6.28) and every ProShot frame (-5.71 median,
/// -2.66 worst) sit at or near zero strength, and so that the frames the pairs
/// showed to be broken do not. A frame below this threshold takes the old code
/// path exactly, allocating nothing.
const INERT_SNR10_EV: f32 = -3.5;
/// `snr10_ev` at which the filter reaches full strength.
///
/// Placed just below the ISO 8000-12800 band's -1.00 rather than above it. That
/// band is unambiguously broken without the filter — 2.17x the camera's
/// saturation — and running it at 0.68 strength left it at 1.53 where full
/// strength reaches 1.35, so there is no reason to hold back on frames that
/// noisy.
const FULL_SNR10_EV: f32 = -0.5;
/// Strength at or above which the luminance-guided stage also runs.
///
/// The box stage alone handles speckle, which is all a mildly noisy frame has.
/// The guided stage exists for the low-frequency blotching that only appears
/// once the sensor is being pushed, and it costs several passes over the image,
/// so it is not spent on frames that do not need it.
const GUIDED_MIN_STRENGTH: f32 = 0.35;
/// Support of the guided stage, in full-resolution pixels.
///
/// Deliberately far wider than [`MAX_RADIUS`] — reaching blotches is the whole
/// point, and unlike a box blur the guided filter's cost does not grow with it.
const GUIDED_RADIUS: usize = 128;
/// Resolution divisor for computing the guided filter's coefficients.
///
/// The coefficients are smooth by construction, so they lose nothing by being
/// fitted on a decimated image and interpolated back; this is He and Sun's fast
/// guided filter. It also averages sensor noise down before the fit, which makes
/// the local variance a better estimate of real structure.
const GUIDED_SUBSAMPLE: usize = 4;
/// Regularisation, in units of squared [`guide_value`].
///
/// Sets where the filter stops believing the guide: local guide variance below
/// this reads as "no real structure here" and the colour is replaced by the
/// window mean. Swept over the high-ISO corpus.
const GUIDED_EPSILON: f32 = 1.0e-3;

/// Blur radius in pixels at full strength.
///
/// Chroma is averaged over at most a 13x13 window. Larger radii start to bleed
/// colour across edges between saturated regions, which is the one artifact this
/// filter can produce.
///
/// It does not remove everything. At ISO 65535 the filter runs at full radius
/// and still sits at 1.40x the camera's saturation, and a 1:1 crop shows why:
/// what survives is *low-frequency* chroma blotching, on a scale larger than the
/// window. A box wide enough to reach it would be wide enough to smear colour
/// across real edges, so the fix is a different filter — a multi-scale or
/// edge-aware one — rather than a bigger number here.
const MAX_RADIUS: usize = 6;

/// What the filter did, for the sidecar.
#[derive(Debug, Clone, Serialize)]
pub struct ChromaDenoiseReport {
    /// 0 to 1, before any user scale.
    pub strength: f32,
    /// Blur radius actually used, in pixels.
    pub radius: usize,
    /// Whether the luminance-guided stage ran as well as the box stage.
    pub guided: bool,
    /// The noise measurement that chose the strength.
    pub snr10_ev: f32,
    /// Mean absolute colour difference from luminance, before and after. The
    /// drop is the chroma noise that was removed, plus whatever real colour
    /// detail went with it.
    pub mean_chroma_before: f32,
    pub mean_chroma_after: f32,
}

/// Hermite ramp on `[0, 1]`, clamped outside. Matches `tone::smoothstep`.
#[inline]
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Automatic strength for a frame whose SNR=10 crossing is `snr10_ev`.
///
/// Ramped rather than switched, so two frames a third of a stop apart in noise
/// do not render visibly differently.
pub fn automatic_strength(snr10_ev: f32) -> f32 {
    if !snr10_ev.is_finite() {
        return 0.0;
    }
    smoothstep((snr10_ev - INERT_SNR10_EV) / (FULL_SNR10_EV - INERT_SNR10_EV))
}

/// One separable box pass over a plane of 3-component chroma.
///
/// Direct summation rather than a running total: the window is at most 13 wide,
/// so the cost is small, and a running sum accumulates float drift across a
/// row — which would make the result depend on image width in the last decimal.
fn box_blur(
    source: &[[f32; 3]],
    destination: &mut [[f32; 3]],
    width: usize,
    height: usize,
    radius: usize,
    horizontal: bool,
) {
    destination
        .par_chunks_exact_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                let (mut sum, mut count) = ([0.0f32; 3], 0.0f32);
                let (centre, limit) = if horizontal { (x, width) } else { (y, height) };
                let start = centre.saturating_sub(radius);
                let end = (centre + radius + 1).min(limit);
                for position in start..end {
                    let index = if horizontal {
                        y * width + position
                    } else {
                        position * width + x
                    };
                    let pixel = source[index];
                    sum[0] += pixel[0];
                    sum[1] += pixel[1];
                    sum[2] += pixel[2];
                    count += 1.0;
                }
                *out = [sum[0] / count, sum[1] / count, sum[2] / count];
            }
        });
}

/// Separable box blur of a scalar plane, used by the guided stage.
fn box_blur_plane(plane: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let mut horizontal = vec![0.0f32; plane.len()];
    horizontal
        .par_chunks_exact_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                let start = x.saturating_sub(radius);
                let end = (x + radius + 1).min(width);
                let mut sum = 0.0f32;
                for position in start..end {
                    sum += plane[y * width + position];
                }
                *out = sum / (end - start) as f32;
            }
        });

    let mut vertical = vec![0.0f32; plane.len()];
    vertical
        .par_chunks_exact_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let start = y.saturating_sub(radius);
            let end = (y + radius + 1).min(height);
            for (x, out) in row.iter_mut().enumerate() {
                let mut sum = 0.0f32;
                for position in start..end {
                    sum += horizontal[position * width + x];
                }
                *out = sum / (end - start) as f32;
            }
        });
    vertical
}

/// Box-average a plane down by `factor`, discarding any ragged final rows and
/// columns. Deterministic and exact for a given input.
fn downsample(
    plane: &[f32],
    width: usize,
    height: usize,
    factor: usize,
) -> (Vec<f32>, usize, usize) {
    let small_width = width / factor;
    let small_height = height / factor;
    let mut small = vec![0.0f32; small_width * small_height];
    small
        .par_chunks_exact_mut(small_width.max(1))
        .enumerate()
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                let mut sum = 0.0f32;
                for dy in 0..factor {
                    for dx in 0..factor {
                        sum += plane[(y * factor + dy) * width + (x * factor + dx)];
                    }
                }
                *out = sum / (factor * factor) as f32;
            }
        });
    (small, small_width, small_height)
}

/// Bilinear sample of a low-resolution plane at a full-resolution pixel centre.
#[inline]
fn sample_bilinear(
    plane: &[f32],
    small_width: usize,
    small_height: usize,
    x: usize,
    y: usize,
    factor: usize,
) -> f32 {
    // Centre of the full-res pixel expressed in low-res coordinates.
    let fx = ((x as f32 + 0.5) / factor as f32 - 0.5).clamp(0.0, (small_width - 1) as f32);
    let fy = ((y as f32 + 0.5) / factor as f32 - 0.5).clamp(0.0, (small_height - 1) as f32);
    let x0 = fx.floor() as usize;
    let y0 = fy.floor() as usize;
    let x1 = (x0 + 1).min(small_width - 1);
    let y1 = (y0 + 1).min(small_height - 1);
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let top = plane[y0 * small_width + x0] * (1.0 - tx) + plane[y0 * small_width + x1] * tx;
    let bottom = plane[y1 * small_width + x0] * (1.0 - tx) + plane[y1 * small_width + x1] * tx;
    top * (1.0 - ty) + bottom * ty
}

/// Guide value for the edge-aware stage.
///
/// Scene-linear luminance spans far too many stops for a single regularisation
/// constant to mean the same thing in the shadows and the highlights, so the
/// guide is compressed the same way the view transform compresses: a bounded,
/// monotone `Y / (Y + grey)`. Edges keep their ordering, and one `epsilon` is
/// meaningful across the frame.
#[inline]
fn guide_value(luminance: f32) -> f32 {
    let y = luminance.max(0.0);
    y / (y + crate::types::MID_GRAY)
}

/// Luminance-guided smoothing of the colour difference, after He, Sun and Tang's
/// guided filter, computed at reduced resolution after He and Sun's fast variant.
///
/// This is the stage that reaches the noise a box blur cannot. **No linear
/// low-pass filter can remove low-frequency chroma noise**: it occupies the same
/// spatial band as real colour, so any kernel wide enough to average it away is
/// wide enough to destroy the colour too. Separating them needs a prior, and
/// luminance is the right one — a real colour boundary almost always coincides
/// with a luminance boundary, and sensor chroma noise never does.
///
/// The filter solves, per window, the linear model `chroma ~= a * guide + b`
/// that best fits the data, then evaluates it per pixel. Where the guide has
/// structure, `a` is large and colour follows the luminance edge; where the
/// guide is flat — a shadow, a sky — `a` collapses toward zero and the output
/// becomes the window mean, which is exactly the aggressive averaging that kills
/// a blotch. `epsilon` sets where that transition sits.
///
/// Cost is independent of the radius, which is the whole reason for using it:
/// the equivalent joint-bilateral filter at this support would be thousands of
/// taps per pixel.
fn guided_chroma(
    chroma: &[[f32; 3]],
    luma: &[f32],
    output: &mut [[f32; 3]],
    width: usize,
    height: usize,
    radius: usize,
    epsilon: f32,
) {
    let factor = GUIDED_SUBSAMPLE;
    if width / factor < 2 || height / factor < 2 {
        output.copy_from_slice(chroma);
        return;
    }

    let guide: Vec<f32> = luma.iter().map(|y| guide_value(*y)).collect();
    let (small_guide, small_width, small_height) = downsample(&guide, width, height, factor);
    let small_radius = (radius / factor).max(1);

    let mean_guide = box_blur_plane(&small_guide, small_width, small_height, small_radius);
    let guide_squared: Vec<f32> = small_guide.iter().map(|g| g * g).collect();
    let mean_guide_squared =
        box_blur_plane(&guide_squared, small_width, small_height, small_radius);
    let variance: Vec<f32> = mean_guide_squared
        .iter()
        .zip(&mean_guide)
        .map(|(squared, mean)| (squared - mean * mean).max(0.0))
        .collect();

    for channel in 0..3 {
        let plane: Vec<f32> = chroma.iter().map(|c| c[channel]).collect();
        let (small_plane, _, _) = downsample(&plane, width, height, factor);
        let mean_plane = box_blur_plane(&small_plane, small_width, small_height, small_radius);
        let cross: Vec<f32> = small_guide
            .iter()
            .zip(&small_plane)
            .map(|(g, p)| g * p)
            .collect();
        let mean_cross = box_blur_plane(&cross, small_width, small_height, small_radius);

        let slope: Vec<f32> = mean_cross
            .iter()
            .zip(&mean_guide)
            .zip(&mean_plane)
            .zip(&variance)
            .map(|(((cross, guide_mean), plane_mean), var)| {
                (cross - guide_mean * plane_mean) / (var + epsilon)
            })
            .collect();
        let intercept: Vec<f32> = mean_plane
            .iter()
            .zip(&slope)
            .zip(&mean_guide)
            .map(|((plane_mean, a), guide_mean)| plane_mean - a * guide_mean)
            .collect();

        let mean_slope = box_blur_plane(&slope, small_width, small_height, small_radius);
        let mean_intercept = box_blur_plane(&intercept, small_width, small_height, small_radius);

        output
            .par_chunks_exact_mut(width)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, out) in row.iter_mut().enumerate() {
                    let a = sample_bilinear(&mean_slope, small_width, small_height, x, y, factor);
                    let b =
                        sample_bilinear(&mean_intercept, small_width, small_height, x, y, factor);
                    out[channel] = a * guide[y * width + x] + b;
                }
            });
    }

    // The three channels were fitted independently, so the result no longer has
    // exactly zero luminance the way the input did. Projecting it back costs one
    // subtraction and restores the guarantee this whole module rests on.
    output.par_iter_mut().for_each(|c| {
        let residual = luminance(*c);
        c[0] -= residual;
        c[1] -= residual;
        c[2] -= residual;
    });
}

/// Average absolute colour difference, as a single number for the sidecar.
fn mean_chroma(pixels: &[[f32; 3]]) -> f32 {
    let total: f64 = pixels
        .iter()
        .map(|c| f64::from(c[0].abs() + c[1].abs() + c[2].abs()))
        .sum();
    (total / (pixels.len().max(1) as f64 * 3.0)) as f32
}

/// Smooth the colour of `image` in place, leaving its luminance untouched.
///
/// `scale` multiplies the automatic strength; 1.0 is the automatic decision and
/// 0.0 disables the filter entirely. Returns `None` when nothing was done, in
/// which case `image` has not been read or written and nothing was allocated.
pub fn apply(
    image: &mut LinearImage,
    snr10_ev: Option<f32>,
    scale: f32,
) -> Option<ChromaDenoiseReport> {
    let snr10_ev = snr10_ev.filter(|value| value.is_finite())?;
    let strength = (automatic_strength(snr10_ev) * scale).clamp(0.0, 1.0);
    if strength <= 0.0 {
        return None;
    }
    let radius = ((strength * MAX_RADIUS as f32).round() as usize).max(1);
    let (width, height) = (image.width, image.height);
    if width < 2 || height < 2 {
        return None;
    }

    // Split into luminance and colour difference. `luminance(chroma)` is zero by
    // construction, and every step below is linear, so it stays zero.
    let mut chroma: Vec<[f32; 3]> = Vec::with_capacity(image.pixels.len());
    let mut luma: Vec<f32> = Vec::with_capacity(image.pixels.len());
    for pixel in &image.pixels {
        let y = luminance(*pixel);
        luma.push(y);
        chroma.push([pixel[0] - y, pixel[1] - y, pixel[2] - y]);
    }
    let mean_chroma_before = mean_chroma(&chroma);

    let mut scratch = vec![[0.0f32; 3]; chroma.len()];
    box_blur(&chroma, &mut scratch, width, height, radius, true);
    box_blur(&scratch, &mut chroma, width, height, radius, false);

    // Second stage, on frames noisy enough to have blotches rather than just
    // speckle. It reads the box-smoothed chroma rather than the raw one: the
    // guided filter fits a local linear model, and fitting it to speckle would
    // put the speckle into the coefficients.
    let guided = strength >= GUIDED_MIN_STRENGTH;
    if guided {
        guided_chroma(
            &chroma,
            &luma,
            &mut scratch,
            width,
            height,
            GUIDED_RADIUS,
            GUIDED_EPSILON,
        );
        std::mem::swap(&mut chroma, &mut scratch);
    }
    let mean_chroma_after = mean_chroma(&chroma);

    // Blend by strength as well as scaling the radius, so a frame just past the
    // threshold changes slightly rather than jumping to a full 3x3 average.
    image
        .pixels
        .par_iter_mut()
        .zip(chroma.par_iter())
        .zip(luma.par_iter())
        .for_each(|((pixel, smoothed), y)| {
            for channel in 0..3 {
                let original = pixel[channel] - y;
                pixel[channel] = y + original + strength * (smoothed[channel] - original);
            }
        });

    Some(ChromaDenoiseReport {
        strength,
        radius,
        guided,
        snr10_ev,
        mean_chroma_before,
        mean_chroma_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_from(width: usize, height: usize, pixels: Vec<[f32; 3]>) -> LinearImage {
        LinearImage::new(width, height, pixels).unwrap()
    }

    /// Alternating colour speckle on a flat grey field: exactly the high-ISO
    /// failure, reduced to something a test can assert on.
    fn speckled(width: usize, height: usize) -> LinearImage {
        let pixels = (0..width * height)
            .map(|index| {
                let swing = if index % 2 == 0 { 0.06 } else { -0.06 };
                [0.18 + swing, 0.18, 0.18 - swing]
            })
            .collect();
        image_from(width, height, pixels)
    }

    #[test]
    fn a_clean_frame_is_left_completely_alone() {
        let mut image = speckled(32, 32);
        let before = image.pixels.clone();
        assert!(apply(&mut image, Some(-8.0), 1.0).is_none());
        assert_eq!(image.pixels, before);
    }

    #[test]
    fn a_missing_or_broken_noise_estimate_disables_the_filter() {
        let mut image = speckled(32, 32);
        let before = image.pixels.clone();
        assert!(apply(&mut image, None, 1.0).is_none());
        assert!(apply(&mut image, Some(f32::NAN), 1.0).is_none());
        assert!(apply(&mut image, Some(2.0), 0.0).is_none());
        assert_eq!(image.pixels, before);
    }

    /// The guarantee that lets this run by default: colour may move, luminance
    /// may not. Checked per pixel, not on average.
    #[test]
    fn luminance_is_preserved_exactly() {
        let mut image = speckled(48, 40);
        let before: Vec<f32> = image.pixels.iter().map(|p| luminance(*p)).collect();
        assert!(apply(&mut image, Some(2.0), 1.0).is_some());
        for (pixel, original) in image.pixels.iter().zip(before) {
            assert!(
                (luminance(*pixel) - original).abs() < 1.0e-5,
                "luminance moved from {original} to {}",
                luminance(*pixel)
            );
        }
    }

    #[test]
    fn speckle_is_reduced_at_high_noise() {
        let mut image = speckled(48, 40);
        let report = apply(&mut image, Some(2.0), 1.0).unwrap();
        assert!(
            report.mean_chroma_after < report.mean_chroma_before * 0.5,
            "chroma {} -> {}",
            report.mean_chroma_before,
            report.mean_chroma_after
        );
    }

    /// A flat colour field has no chroma noise to remove, so the filter must be
    /// a no-op on it even at full strength — otherwise it would be desaturating
    /// real colour rather than noise.
    #[test]
    fn a_uniform_colour_is_unchanged() {
        let mut image = image_from(32, 32, vec![[0.30, 0.18, 0.09]; 32 * 32]);
        let before = image.pixels.clone();
        apply(&mut image, Some(2.0), 1.0).unwrap();
        for (after, original) in image.pixels.iter().zip(before) {
            for channel in 0..3 {
                assert!((after[channel] - original[channel]).abs() < 1.0e-6);
            }
        }
    }

    #[test]
    fn strength_ramps_rather_than_switching() {
        assert_eq!(automatic_strength(-8.0), 0.0);
        assert_eq!(automatic_strength(INERT_SNR10_EV), 0.0);
        assert_eq!(automatic_strength(FULL_SNR10_EV), 1.0);
        assert_eq!(automatic_strength(4.0), 1.0);
        let middle = automatic_strength((INERT_SNR10_EV + FULL_SNR10_EV) * 0.5);
        assert!((middle - 0.5).abs() < 1.0e-6, "midpoint strength {middle}");
        assert!(automatic_strength(-2.0) < automatic_strength(-1.0));
    }

    /// A slow colour drift across a flat-luminance field: the low-frequency
    /// blotching a box blur provably cannot reach, because it lives in the same
    /// spatial band as real colour. Only the guided stage can, and only because
    /// the luminance guide says there is no real structure here.
    #[test]
    fn the_guided_stage_reaches_blotches_a_box_blur_cannot() {
        // Tall enough that the blotch repeats several times inside the guided
        // filter's support; a frame barely wider than the support would spend
        // most of its rows in clamped windows and understate the effect.
        let (width, height) = (96, 256);
        // The swing direction is chosen to have exactly zero luminance, which is
        // what a colour difference always is here — so the luminance guide is
        // genuinely flat and has no reason to protect the blotch. A swing that
        // moved luminance would be real structure, and preserving it would be
        // correct behaviour rather than a failure.
        let swing_direction = [1.0f32, 0.0, -0.2126 / 0.0722];
        debug_assert!(luminance(swing_direction).abs() < 1.0e-6);
        let blotched = |_x: usize, y: usize| {
            // One full cycle down the frame — far wider than MAX_RADIUS.
            // Four cycles down the frame: a period of 64 px, far wider than
            // MAX_RADIUS's 13 px window and well inside the guided support.
            let swing = 0.02 * (y as f32 / height as f32 * 4.0 * std::f32::consts::TAU).sin();
            [
                0.18 + swing * swing_direction[0],
                0.18 + swing * swing_direction[1],
                0.18 + swing * swing_direction[2],
            ]
        };
        let pixels: Vec<[f32; 3]> = (0..width * height)
            .map(|index| blotched(index % width, index / width))
            .collect();

        // Compare the two filters directly, at identical strength, so the
        // result is about the filter and not about the blend.
        let luma: Vec<f32> = pixels.iter().map(|p| luminance(*p)).collect();
        let chroma: Vec<[f32; 3]> = pixels
            .iter()
            .zip(&luma)
            .map(|(p, y)| [p[0] - y, p[1] - y, p[2] - y])
            .collect();

        let mut scratch = vec![[0.0f32; 3]; chroma.len()];
        let mut boxed = chroma.clone();
        box_blur(&boxed, &mut scratch, width, height, MAX_RADIUS, true);
        box_blur(&scratch, &mut boxed, width, height, MAX_RADIUS, false);

        let mut guided = vec![[0.0f32; 3]; chroma.len()];
        guided_chroma(
            &chroma,
            &luma,
            &mut guided,
            width,
            height,
            GUIDED_RADIUS,
            GUIDED_EPSILON,
        );

        let spread = |plane: &[[f32; 3]]| {
            let values: Vec<f32> = plane.iter().map(|c| c[0] - c[2]).collect();
            let mean = values.iter().sum::<f32>() / values.len() as f32;
            (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt()
        };
        let original = spread(&chroma);
        assert!(
            spread(&boxed) > original * 0.8,
            "a box blur is not supposed to reach this: {} of {}",
            spread(&boxed),
            original
        );
        assert!(
            spread(&guided) < original * 0.35,
            "guided {} should be far below the original {}",
            spread(&guided),
            original
        );

        // And it must still be reachable through the public path.
        let mut image = image_from(width, height, pixels);
        assert!(apply(&mut image, Some(2.0), 1.0).unwrap().guided);
    }

    /// Where the luminance guide has real structure, colour must survive. This
    /// is the whole reason for using a guide rather than a wider box.
    #[test]
    fn colour_survives_a_luminance_edge() {
        let (width, height) = (160, 128);
        let pixels: Vec<[f32; 3]> = (0..width * height)
            .map(|index| {
                if index % width < width / 2 {
                    [0.40, 0.10, 0.10] // bright red half
                } else {
                    [0.02, 0.02, 0.08] // dark blue half
                }
            })
            .collect();
        let mut image = image_from(width, height, pixels.clone());
        apply(&mut image, Some(2.0), 1.0).unwrap();

        // Sample well inside each half, away from the transition.
        let at = |x: usize| image.pixels[(height / 2) * width + x];
        let left = at(width / 4);
        let right = at(width - width / 4);
        let red_bias = |p: [f32; 3]| p[0] - p[2];
        assert!(
            red_bias(left) > 0.20,
            "the red half lost its colour: {left:?}"
        );
        assert!(
            red_bias(right) < 0.0,
            "the blue half picked up red across the edge: {right:?}"
        );
    }

    /// The guarantee has to survive the guided stage too, which fits the three
    /// channels independently and so needs an explicit re-projection.
    #[test]
    fn luminance_is_preserved_through_the_guided_stage() {
        let (width, height) = (96, 80);
        let pixels: Vec<[f32; 3]> = (0..width * height)
            .map(|index| {
                let x = (index % width) as f32 / width as f32;
                let swing = if index % 3 == 0 { 0.05 } else { -0.04 };
                [0.1 + x * 0.4 + swing, 0.1 + x * 0.4, 0.1 + x * 0.4 - swing]
            })
            .collect();
        let mut image = image_from(width, height, pixels);
        let before: Vec<f32> = image.pixels.iter().map(|p| luminance(*p)).collect();
        let report = apply(&mut image, Some(2.0), 1.0).unwrap();
        assert!(report.guided);
        for (pixel, original) in image.pixels.iter().zip(before) {
            assert!(
                (luminance(*pixel) - original).abs() < 1.0e-5,
                "luminance moved from {original} to {}",
                luminance(*pixel)
            );
        }
    }

    #[test]
    fn repeated_runs_are_identical() {
        let run = || {
            let mut image = speckled(64, 48);
            apply(&mut image, Some(1.0), 1.0);
            image.pixels
        };
        assert_eq!(run(), run());
    }
}
