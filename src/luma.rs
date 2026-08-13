//! Luminance noise reduction, scaled by the frame's own noise model.
//!
//! [`crate::chroma`] removes the colour speckle that makes a high-ISO frame look
//! broken, but it deliberately leaves luminance untouched — "luma denoising is
//! the part that destroys texture". That is the right default for a mildly noisy
//! frame. It is the wrong default for a night frame: at ISO 12800 the paired
//! Sony corpus renders a sky full of luminance grain the camera has removed, and
//! the moment the night tone map ([`crate::localtone::build_hdr`]) lifts the
//! shadows that grain is *amplified*, not hidden. So this module reduces luma
//! noise as well — but only on frames noisy enough to need it, and only as hard
//! as the measured noise justifies, so a clean frame is byte-identical and a
//! textured daytime frame is barely touched.
//!
//! # Why it is safe to run before the tone map and not before analysis
//!
//! Unlike chroma denoise, this filter *does* change luminance, so it cannot run
//! before [`crate::analyze`] without moving the exposure the controller solves
//! for. It therefore runs after analysis and before the tone map and render:
//! the exposure decision is taken on the original luminance and is byte-identical
//! to a run without this module, while the tone map, the night HDR operator and
//! the final render all see the cleaned luminance.
//!
//! # Method
//!
//! Each pixel is split into its luminance `Y` and colour difference `C = rgb - Y`
//! exactly as in [`crate::chroma`], and only `Y` is filtered; `C` is added back
//! unchanged, so the filter cannot shift a hue or a saturation — it can only
//! soften a luminance edge, which is the one thing it is allowed to do.
//!
//! `Y` is filtered in the log2-EV domain (`t = log2(Y / grey)`) for two reasons.
//! First, one regularisation constant then means the same thing across the many
//! stops a night scene spans. Second, photon noise has standard deviation
//! proportional to `sqrt(Y)`, so in the EV domain its amplitude grows *toward the
//! shadows* — filtering there automatically spends most of its effort on the
//! dark, lifted regions where the grain is worst and least on the bright regions
//! where detail matters, with no explicit mask.
//!
//! The smoother is a self-guided (He, Sun and Tang) edge-preserving filter: in a
//! window it fits `t ~= a * t + b`, so where the window has real structure
//! (`var >> epsilon`) `a -> 1` and the edge is kept, and where it is flat
//! (`var < epsilon`) `a -> 0` and the output collapses to the window mean, which
//! is where the grain dies. The result is blended by a strength that ramps with
//! the frame's SNR=10 crossing, so nothing switches on abruptly.

use crate::analyze::luminance;
use crate::types::{LinearImage, MID_GRAY};
use rayon::prelude::*;
use serde::Serialize;

/// `snr10_ev` at or below which the frame is clean enough to leave alone.
///
/// Set a touch more conservatively than [`crate::chroma`]'s `-3.5`: luma
/// filtering is the one that can cost texture, so it starts later, near the
/// ISO 500-1000 band, and every base-ISO frame takes the old code path exactly,
/// allocating nothing.
const INERT_SNR10_EV: f32 = -3.0;
/// `snr10_ev` at which the filter reaches full strength — the same anchor
/// [`crate::chroma`] uses, just below the ISO 8000-12800 band's crossing, which
/// is unambiguously broken without help.
const FULL_SNR10_EV: f32 = -0.5;
/// Largest fraction of the smoothed luminance ever mixed in.
///
/// Capped below 1.0 on purpose: full replacement is what turns a noisy photo
/// into a smeared one, and the camera's own over-smoothing on these frames is
/// exactly the failure this stays short of. At the cap the frame keeps a fifth
/// of its original luminance micro-texture.
const MAX_BLEND: f32 = 0.8;
/// Guided-filter window radius, in full-resolution pixels.
///
/// Deliberately small: luminance grain is high-frequency, and a wider window
/// starts to reach real texture. Coarse low-frequency luma blotching is left to
/// a later multi-scale pass, exactly as [`crate::chroma`] leaves the equivalent
/// chroma blotch to its guided stage.
const RADIUS: usize = 3;
/// Multiple of the frame's own measured flat-region noise variance used as the
/// self-guided filter's regularisation.
///
/// A fixed regularisation cannot work across brightness: in the log2-EV domain a
/// photon-noise wobble of a fixed number of electrons is a fraction of a stop in
/// the highlights but well over a stop in the deep shadows a night frame lives
/// in, so a constant small `epsilon` would mistake shadow grain for structure
/// and leave it untouched. Instead `epsilon` is set to a multiple of the noise
/// variance the frame actually shows — a low percentile of the local EV variance,
/// which the flat regions supply — so a real edge (variance far above the noise)
/// is kept while the grain (variance at the noise floor) is averaged away, on
/// clean and noisy frames alike.
const NOISE_EPS_MULTIPLE: f32 = 4.0;
/// Percentile of the local EV-variance map taken as the flat-region noise floor.
/// Low enough to land in genuinely flat regions rather than texture.
const NOISE_FLOOR_PERCENTILE: f32 = 0.2;
/// Absolute floor for the regularisation, so an already-flat plane cannot drive
/// `epsilon` to zero and divide-by-nothing.
const MIN_EPS: f32 = 1.0e-6;
/// Target sample count for the deterministic variance percentile.
const PERCENTILE_SAMPLES: usize = 200_000;
/// Floor for the luminance carried into the log domain, so a black or slightly
/// negative out-of-gamut pixel cannot produce a non-finite EV.
const LUMA_FLOOR: f32 = 1.0e-6;

/// What the filter did, for the sidecar.
#[derive(Debug, Clone, Serialize)]
pub struct LumaDenoiseReport {
    /// 0 to 1, before any user scale.
    pub strength: f32,
    /// Blur radius used, in pixels.
    pub radius: usize,
    /// Guided-filter regularisation, in EV².
    pub epsilon: f32,
    /// The noise measurement that chose the strength.
    pub snr10_ev: f32,
    /// Mean absolute luminance correction actually applied, in EV. The size of
    /// the grain that was removed, plus whatever real micro-texture went with it.
    pub mean_abs_correction_ev: f32,
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

/// One separable box pass of a scalar plane. Direct summation, not a running
/// total: the window is tiny, so the cost is negligible, and a running sum would
/// accumulate float drift across a row and make the result depend on width.
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

/// Deterministic low percentile of `values`, taken over a strided subsample so
/// the cost does not grow with the image. The stride and `total_cmp` sort make
/// the result independent of thread scheduling.
fn sampled_percentile(values: &[f32], percentile: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let stride = (values.len() / PERCENTILE_SAMPLES.max(1)).max(1);
    let mut samples: Vec<f32> = values
        .iter()
        .step_by(stride)
        .copied()
        .filter(|value| value.is_finite())
        .collect();
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_unstable_by(f32::total_cmp);
    let index = ((samples.len() - 1) as f32 * percentile.clamp(0.0, 1.0)).round() as usize;
    samples[index]
}

/// Self-guided edge-preserving filter (He et al., guide = input = `ev`), with
/// `epsilon` self-calibrated to the frame's own flat-region noise variance.
///
/// Returns the smoothed plane in the same EV units as the input, and the
/// `epsilon` it chose (for the sidecar).
fn guided_ev(ev: &[f32], width: usize, height: usize, radius: usize) -> (Vec<f32>, f32) {
    let mean = box_blur_plane(ev, width, height, radius);
    let squared: Vec<f32> = ev.par_iter().map(|v| v * v).collect();
    let mean_squared = box_blur_plane(&squared, width, height, radius);

    let variance: Vec<f32> = mean_squared
        .par_iter()
        .zip(&mean)
        .map(|(mean_sq, mean)| (mean_sq - mean * mean).max(0.0))
        .collect();

    // Calibrate the regularisation to the noise the frame actually shows: the
    // low percentile of the local variance is the flat-region grain, and a real
    // edge sits far above it.
    let noise_variance = sampled_percentile(&variance, NOISE_FLOOR_PERCENTILE);
    let epsilon = (NOISE_EPS_MULTIPLE * noise_variance).max(MIN_EPS);

    let slope: Vec<f32> = variance
        .par_iter()
        .map(|variance| variance / (variance + epsilon))
        .collect();
    let intercept: Vec<f32> = slope
        .par_iter()
        .zip(&mean)
        .map(|(a, mean)| (1.0 - a) * mean)
        .collect();

    let mean_slope = box_blur_plane(&slope, width, height, radius);
    let mean_intercept = box_blur_plane(&intercept, width, height, radius);

    let smoothed = mean_slope
        .par_iter()
        .zip(&mean_intercept)
        .zip(ev)
        .map(|((a, b), value)| a * value + b)
        .collect();
    (smoothed, epsilon)
}

/// Reduce the luminance noise of `image` in place, leaving its colour untouched.
///
/// `scale` multiplies the automatic strength; 1.0 is the automatic decision and
/// 0.0 disables the filter entirely. Returns `None` when nothing was done, in
/// which case `image` has not been written and nothing was allocated.
pub fn apply(
    image: &mut LinearImage,
    snr10_ev: Option<f32>,
    scale: f32,
) -> Option<LumaDenoiseReport> {
    let snr10_ev = snr10_ev.filter(|value| value.is_finite())?;
    let strength = (automatic_strength(snr10_ev) * scale).clamp(0.0, 1.0);
    if strength <= 0.0 {
        return None;
    }
    let (width, height) = (image.width, image.height);
    if width < 2 || height < 2 {
        return None;
    }
    let blend = strength * MAX_BLEND;

    // Luminance of each pixel, and its EV relative to middle grey. A pixel whose
    // luminance is non-positive (pure black, or an out-of-gamut negative) carries
    // no signal to denoise, so it is pinned to the darkest EV and its correction
    // is forced to zero below.
    let luma: Vec<f32> = image.pixels.par_iter().map(|p| luminance(*p)).collect();
    let ev: Vec<f32> = luma
        .par_iter()
        .map(|y| (y.max(LUMA_FLOOR) / MID_GRAY).log2())
        .collect();

    let (smoothed, epsilon) = guided_ev(&ev, width, height, RADIUS);

    // Apply the luminance change as a single additive delta to all three
    // channels, which preserves `rgb - Y` — the colour — exactly.
    let mean_abs_correction_ev = std::sync::atomic::AtomicU64::new(0);
    let counted = std::sync::atomic::AtomicU64::new(0);
    image
        .pixels
        .par_iter_mut()
        .zip(luma.par_iter())
        .zip(ev.par_iter())
        .zip(smoothed.par_iter())
        .for_each(|(((pixel, y), ev), smoothed)| {
            if *y <= LUMA_FLOOR || !y.is_finite() {
                return;
            }
            let target_ev = ev + blend * (smoothed - ev);
            let new_y = MID_GRAY * target_ev.exp2();
            if !new_y.is_finite() {
                return;
            }
            let delta = new_y - *y;
            pixel[0] += delta;
            pixel[1] += delta;
            pixel[2] += delta;
            mean_abs_correction_ev.fetch_add(
                ((target_ev - ev).abs() * 1.0e6) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });

    let counted = counted.load(std::sync::atomic::Ordering::Relaxed).max(1);
    let mean_abs_correction_ev =
        (mean_abs_correction_ev.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1.0e6)
            / counted as f32;

    Some(LumaDenoiseReport {
        strength,
        radius: RADIUS,
        epsilon,
        snr10_ev,
        mean_abs_correction_ev,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_from(width: usize, height: usize, pixels: Vec<[f32; 3]>) -> LinearImage {
        LinearImage::new(width, height, pixels).unwrap()
    }

    /// Deterministic pseudo-noise in `[-1, 1)`.
    fn noise(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((*seed >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0
    }

    /// A flat grey field with luminance grain: the high-ISO shadow, reduced to
    /// something a test can assert on.
    fn grainy(width: usize, height: usize, base: f32, amplitude: f32) -> LinearImage {
        let mut seed = 0x1234_5678u32;
        let pixels = (0..width * height)
            .map(|_| {
                let swing = noise(&mut seed) * amplitude;
                [base + swing, base + swing, base + swing]
            })
            .collect();
        image_from(width, height, pixels)
    }

    #[test]
    fn a_clean_frame_is_left_completely_alone() {
        let mut image = grainy(48, 48, 0.05, 0.01);
        let before = image.pixels.clone();
        assert!(apply(&mut image, Some(-8.0), 1.0).is_none());
        assert_eq!(image.pixels, before);
    }

    #[test]
    fn a_missing_or_broken_noise_estimate_disables_the_filter() {
        let mut image = grainy(48, 48, 0.05, 0.01);
        let before = image.pixels.clone();
        assert!(apply(&mut image, None, 1.0).is_none());
        assert!(apply(&mut image, Some(f32::NAN), 1.0).is_none());
        assert!(apply(&mut image, Some(2.0), 0.0).is_none());
        assert_eq!(image.pixels, before);
    }

    /// Colour must not move at all: this filter is allowed to change luminance
    /// and nothing else. `rgb - Y` is the colour, checked per pixel.
    #[test]
    fn colour_is_preserved_exactly() {
        // A noisy field that also carries a colour, so a colour shift would show.
        let mut seed = 0x9e37_79b1u32;
        let pixels: Vec<[f32; 3]> = (0..64 * 64)
            .map(|_| {
                let s = noise(&mut seed) * 0.02;
                [0.10 + s, 0.06 + s, 0.03 + s]
            })
            .collect();
        let mut image = image_from(64, 64, pixels.clone());
        let before_colour: Vec<[f32; 3]> = pixels
            .iter()
            .map(|p| {
                let y = luminance(*p);
                [p[0] - y, p[1] - y, p[2] - y]
            })
            .collect();
        assert!(apply(&mut image, Some(2.0), 1.0).is_some());
        for (pixel, colour) in image.pixels.iter().zip(before_colour) {
            let y = luminance(*pixel);
            for channel in 0..3 {
                assert!(
                    ((pixel[channel] - y) - colour[channel]).abs() < 1.0e-5,
                    "colour moved: {} vs {}",
                    pixel[channel] - y,
                    colour[channel]
                );
            }
        }
    }

    /// The point of the filter: luminance grain on a flat field is reduced.
    #[test]
    fn grain_is_reduced_at_high_noise() {
        let base = 0.05f32;
        let mut image = grainy(96, 96, base, 0.02);
        let spread = |img: &LinearImage| {
            let ys: Vec<f32> = img.pixels.iter().map(|p| luminance(*p)).collect();
            let mean = ys.iter().sum::<f32>() / ys.len() as f32;
            (ys.iter().map(|y| (y - mean).powi(2)).sum::<f32>() / ys.len() as f32).sqrt()
        };
        let before = spread(&image);
        apply(&mut image, Some(2.0), 1.0).unwrap();
        let after = spread(&image);
        assert!(
            after < before * 0.6,
            "grain not reduced enough: {before} -> {after}"
        );
    }

    /// A real luminance edge must survive: the filter smooths grain, not
    /// structure. Without an edge-aware term a box blur would bleed the two
    /// halves into each other.
    #[test]
    fn a_luminance_edge_survives() {
        let (width, height) = (128, 96);
        let pixels: Vec<[f32; 3]> = (0..width * height)
            .map(|index| {
                if index % width < width / 2 {
                    [0.02, 0.02, 0.02]
                } else {
                    [0.40, 0.40, 0.40]
                }
            })
            .collect();
        let mut image = image_from(width, height, pixels);
        apply(&mut image, Some(2.0), 1.0).unwrap();
        let at = |x: usize| luminance(image.pixels[(height / 2) * width + x]);
        // Well inside each half, away from the transition, the levels must hold.
        assert!(at(width / 4) < 0.06, "dark half lifted: {}", at(width / 4));
        assert!(
            at(width - width / 4) > 0.34,
            "bright half pulled down: {}",
            at(width - width / 4)
        );
    }

    /// A smooth luminance ramp is signal, not noise, and must be preserved:
    /// the guided filter is linear in a ramp, so the output tracks it.
    #[test]
    fn a_smooth_ramp_is_preserved() {
        let (width, height) = (96, 64);
        let pixels: Vec<[f32; 3]> = (0..width * height)
            .map(|index| {
                let x = (index % width) as f32 / (width - 1) as f32;
                let v = 0.02 + x * 0.30;
                [v, v, v]
            })
            .collect();
        let before: Vec<f32> = pixels.iter().map(|p| luminance(*p)).collect();
        let mut image = image_from(width, height, pixels);
        apply(&mut image, Some(2.0), 1.0).unwrap();
        for (pixel, original) in image.pixels.iter().zip(before) {
            assert!(
                (luminance(*pixel) - original).abs() < 0.02,
                "ramp moved from {original} to {}",
                luminance(*pixel)
            );
        }
    }

    #[test]
    fn strength_ramps_rather_than_switching() {
        assert_eq!(automatic_strength(-8.0), 0.0);
        assert_eq!(automatic_strength(INERT_SNR10_EV), 0.0);
        assert_eq!(automatic_strength(FULL_SNR10_EV), 1.0);
        assert_eq!(automatic_strength(4.0), 1.0);
        assert!(automatic_strength(-2.0) < automatic_strength(-1.0));
    }

    #[test]
    fn repeated_runs_are_identical() {
        let run = || {
            let mut image = grainy(64, 48, 0.05, 0.02);
            apply(&mut image, Some(1.0), 1.0);
            image.pixels
        };
        assert_eq!(run(), run());
    }
}
