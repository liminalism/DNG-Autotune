//! Output sharpening, applied to the rendered image.
//!
//! `docs/PLAN.md` listed this last in its quality set on the grounds that
//! it is the most taste-dependent and least likely to ruin a frame if deferred.
//! The 98-pair scorecard in `docs/STATUS.md` then made it the largest *measured*
//! gap: this program loses `average_gradient` to the camera on 64 of 98 frames,
//! median 3.71 against 4.59, and the reason is simply that every camera sharpens
//! its JPEG output and until now this program did not sharpen at all.
//!
//! # What it does, and what it deliberately does not
//!
//! An unsharp mask on **luminance only**, at a one-pixel radius — capture
//! sharpening, not a creative effect. The correction is added equally to the
//! three channels, so it moves luminance and leaves the colour difference
//! between channels untouched: sharpening cannot shift a hue or pull a pixel
//! sideways in colour. That is the mirror image of [`crate::chroma`], which
//! moves colour and provably cannot touch luminance. Between them the two
//! filters partition the signal, and neither can undo the other.
//!
//! Three guards, each answering a way unsharp masking normally goes wrong:
//!
//! - **Noise.** Sharpening is an amplifier and does not know grain from
//!   texture, so the amount fades out on frames the fitted noise model calls
//!   dirty, using the same `snr10_ev` that drives chroma denoising.
//! - **Small differences.** A soft shrinkage suppresses corrections below the
//!   noise floor, so flat areas stay flat instead of acquiring a crawling
//!   texture.
//! - **Headroom.** The correction may spend only part of the distance to black
//!   or white, so sharpening can never be what clips a highlight or crushes a
//!   shadow. `docs/STATUS.md` records this as the standing rule for anything
//!   applied after the tone curve.

use crate::tone::Rgb16Image;
use rayon::prelude::*;
use serde::Serialize;

/// Peak unsharp amount, on a clean frame.
///
/// Swept against the paired corpus: the target is the camera's median
/// `average_gradient`, and overshooting it buys halos rather than detail.
const AMOUNT: f32 = 0.75;
/// Radius of the blur the mask is taken against, in pixels.
///
/// One pixel — a 3x3 kernel. This is capture sharpening, restoring the acutance
/// demosaicing costs; a larger radius produces the visible edge halo that makes
/// a render look processed.
const RADIUS: usize = 1;
/// Corrections below roughly this fraction of full scale are suppressed.
///
/// A soft knee rather than a hard threshold, so there is no level at which
/// texture abruptly starts being sharpened.
const SHRINK_KNEE: f32 = 0.004;
/// Largest fraction of the remaining distance to black or white that the
/// correction may spend, measured on the outermost channel.
const HEADROOM_FRACTION: f32 = 0.5;

/// What the sharpener did, for the sidecar.
#[derive(Debug, Clone, Serialize)]
pub struct SharpenReport {
    /// Amount actually used after the noise fade.
    pub amount: f32,
    pub radius: usize,
    /// Fraction of sampled pixels whose correction was limited by headroom.
    pub headroom_limited_fraction: f32,
    /// Mean absolute correction, on the 0-1 scale.
    pub mean_correction: f32,
}

/// Rec.709 luminance of a rendered pixel, on the 0-1 scale.
#[inline]
fn luma(pixel: &[u16]) -> f32 {
    (0.2126 * f32::from(pixel[0]) + 0.7152 * f32::from(pixel[1]) + 0.0722 * f32::from(pixel[2]))
        / f32::from(u16::MAX)
}

/// Amount for a frame whose SNR=10 crossing is `snr10_ev`.
///
/// Reuses the chroma module's noise ramp rather than inventing a second one:
/// the frames too noisy to sharpen are exactly the frames noisy enough to need
/// chroma denoising, and having one ramp means one thing to re-tune.
pub fn automatic_amount(snr10_ev: Option<f32>) -> f32 {
    let noise = snr10_ev.map_or(0.0, crate::chroma::automatic_strength);
    AMOUNT * (1.0 - noise)
}

/// Sharpen `image` in place. Returns `None` when nothing was done.
pub fn apply(image: &mut Rgb16Image, snr10_ev: Option<f32>, scale: f32) -> Option<SharpenReport> {
    let amount = automatic_amount(snr10_ev) * scale;
    if !amount.is_finite() || amount <= 0.0 {
        return None;
    }
    let width = image.width() as usize;
    let height = image.height() as usize;
    if width < 3 || height < 3 {
        return None;
    }

    // The correction is computed from the untouched render into its own plane,
    // then applied. Sharpening in place would let already-sharpened neighbours
    // feed the mask of the pixels after them.
    let raw = image.as_raw();
    let mut correction = vec![0.0f32; width * height];
    correction
        .par_chunks_exact_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let y0 = y.saturating_sub(RADIUS);
            let y1 = (y + RADIUS).min(height - 1);
            for (x, out) in row.iter_mut().enumerate() {
                let x0 = x.saturating_sub(RADIUS);
                let x1 = (x + RADIUS).min(width - 1);
                let mut sum = 0.0f32;
                let mut count = 0.0f32;
                for sample_y in y0..=y1 {
                    for sample_x in x0..=x1 {
                        let index = (sample_y * width + sample_x) * 3;
                        sum += luma(&raw[index..index + 3]);
                        count += 1.0;
                    }
                }
                let index = (y * width + x) * 3;
                let centre = luma(&raw[index..index + 3]);
                let detail = centre - sum / count;
                // Soft shrinkage: proportional for large detail, quadratic for
                // small, so noise-scale corrections are suppressed smoothly.
                let magnitude = detail.abs();
                let shrunk = detail * (magnitude / (magnitude + SHRINK_KNEE));
                *out = amount * shrunk;
            }
        });

    let mut headroom_limited = 0usize;
    let mut correction_sum = 0.0f64;
    let raw = image.as_mut();
    for (index, delta) in correction.iter().enumerate() {
        let pixel = &mut raw[index * 3..index * 3 + 3];
        // Headroom is measured on the outermost *channel*, not on luminance.
        // Clipping is per channel, and the correction is added to all three, so
        // a pixel whose luminance has room can still have a channel that does
        // not — a blue sky at luma 0.9 with its blue channel at 0.99. Guarding
        // on luminance cost 20 frames of the highlight win the first time this
        // shipped; it is the same per-channel lesson as 0.1.12's chroma cap.
        let brightest = f32::from(*pixel.iter().max().unwrap_or(&0)) / f32::from(u16::MAX);
        let darkest = f32::from(*pixel.iter().min().unwrap_or(&0)) / f32::from(u16::MAX);
        let upper = (1.0 - brightest) * HEADROOM_FRACTION;
        let lower = -darkest * HEADROOM_FRACTION;
        let limited = delta.clamp(lower, upper);
        if (limited - delta).abs() > 1.0e-9 {
            headroom_limited += 1;
        }
        correction_sum += f64::from(limited.abs());
        let scaled = limited * f32::from(u16::MAX);
        for channel in pixel.iter_mut() {
            *channel = (f32::from(*channel) + scaled).clamp(0.0, f32::from(u16::MAX)) as u16;
        }
    }

    let total = correction.len().max(1);
    Some(SharpenReport {
        amount,
        radius: RADIUS,
        headroom_limited_fraction: headroom_limited as f32 / total as f32,
        mean_correction: (correction_sum / total as f64) as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn edge(width: u32, height: u32) -> Rgb16Image {
        ImageBuffer::from_fn(width, height, |x, _| {
            let level: u16 = if x < width / 2 { 12_000 } else { 40_000 };
            Rgb([level; 3])
        })
    }

    #[test]
    fn a_noisy_frame_is_not_sharpened() {
        assert_eq!(automatic_amount(Some(2.0)), 0.0);
        let mut image = edge(32, 32);
        let before = image.clone();
        assert!(apply(&mut image, Some(2.0), 1.0).is_none());
        assert_eq!(image.as_raw(), before.as_raw());
    }

    #[test]
    fn a_zero_scale_disables_it() {
        let mut image = edge(32, 32);
        let before = image.clone();
        assert!(apply(&mut image, Some(-8.0), 0.0).is_none());
        assert_eq!(image.as_raw(), before.as_raw());
    }

    /// An edge must gain local contrast — that is the entire point.
    #[test]
    fn an_edge_gains_acutance() {
        let mut image = edge(64, 32);
        apply(&mut image, Some(-8.0), 1.0).unwrap();
        let at = |x: u32| image.get_pixel(x, 16).0[1];
        // Just left of the transition should darken, just right should brighten.
        assert!(at(31) < 12_000, "dark side of the edge did not deepen");
        assert!(at(32) > 40_000, "light side of the edge did not lift");
    }

    /// A flat field has no detail, so it must come out untouched rather than
    /// acquiring texture from the shrinkage or the arithmetic.
    #[test]
    fn a_flat_field_is_unchanged() {
        let mut image: Rgb16Image = ImageBuffer::from_pixel(32, 32, Rgb([20_000_u16; 3]));
        let before = image.clone();
        apply(&mut image, Some(-8.0), 1.0).unwrap();
        assert_eq!(image.as_raw(), before.as_raw());
    }

    /// Sharpening moves luminance and must leave the colour difference between
    /// channels alone, so it cannot shift a hue.
    #[test]
    fn colour_differences_are_preserved() {
        let mut image: Rgb16Image = ImageBuffer::from_fn(64, 32, |x, _| {
            let level: u16 = if x < 32 { 10_000 } else { 30_000 };
            Rgb([level + 4_000, level, level.saturating_sub(3_000)])
        });
        let before = image.clone();
        apply(&mut image, Some(-8.0), 1.0).unwrap();
        for (after, original) in image.pixels().zip(before.pixels()) {
            // Only compare where nothing hit the 0..65535 ends.
            if after.0.iter().all(|c| *c > 0 && *c < u16::MAX) {
                let difference = |p: &Rgb<u16>| {
                    (
                        i32::from(p.0[0]) - i32::from(p.0[1]),
                        i32::from(p.0[1]) - i32::from(p.0[2]),
                    )
                };
                assert_eq!(difference(after), difference(original));
            }
        }
    }

    /// Sharpening must never be what clips a highlight or crushes a shadow —
    /// the standing rule for anything applied after the tone curve. A pixel
    /// already at an extreme may stay there; no other pixel may be driven to
    /// one.
    #[test]
    fn sharpening_cannot_clip_or_crush() {
        // Hard edges hard against both ends of the range, which is the worst
        // case for an unsharp mask: the overshoot has nowhere to go.
        let mut image: Rgb16Image = ImageBuffer::from_fn(96, 32, |x, _| {
            let level: u16 = match x / 32 {
                0 => 40,
                1 => u16::MAX - 40,
                _ => 30_000,
            };
            Rgb([level; 3])
        });
        let before = image.clone();
        apply(&mut image, Some(-8.0), 1.0).unwrap();

        for (after, original) in image.pixels().zip(before.pixels()) {
            for channel in 0..3 {
                if original.0[channel] > 0 {
                    assert!(
                        after.0[channel] > 0,
                        "sharpening crushed {} to zero",
                        original.0[channel]
                    );
                }
                if original.0[channel] < u16::MAX {
                    assert!(
                        after.0[channel] < u16::MAX,
                        "sharpening clipped {} to full scale",
                        original.0[channel]
                    );
                }
            }
        }
    }

    #[test]
    fn repeated_runs_are_identical() {
        let run = || {
            let mut image = edge(64, 48);
            apply(&mut image, Some(-8.0), 1.0);
            image.into_raw()
        };
        assert_eq!(run(), run());
    }
}
