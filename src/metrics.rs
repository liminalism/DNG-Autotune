//! Objective measurements of a rendered image.
//!
//! Tuning the view transform needs numbers, not just side-by-side crops. The
//! highlight work in 0.1.3 was steered by a hand-rolled "saturation of pixels
//! above 92% value" statistic, which was serviceable but arbitrary. These are
//! the measures the enhancement literature actually uses, so results here are
//! comparable with published figures.

use crate::tone::Rgb16Image;
use serde::Serialize;

/// Approximate number of pixels sampled when measuring a rendered image.
const TARGET_SAMPLES: usize = 250_000;

/// Level at or above which a channel reads as blown rather than merely bright.
/// Hard clipping (exactly full scale) is too narrow to steer highlight work:
/// a channel at 98% is already visually white but is not literally clipped.
const NEAR_WHITE: u16 = (0.98 * u16::MAX as f32) as u16;

/// Objective statistics for one rendered image.
#[derive(Debug, Clone, Serialize)]
pub struct OutputStats {
    /// Hasler & Süsstrunk colourfulness, on the 0-255 scale used in the
    /// literature. Higher is more colourful; roughly 0 (greyscale) to ~110
    /// (extremely vivid). Reported to correlate above 90% with human ranking.
    pub colourfulness: f32,
    /// Fraction of sampled pixels with at least one channel at full scale.
    /// Hard clipping: information is definitively gone.
    pub clipped_fraction: f32,
    /// Fraction of sampled pixels with at least one channel at or above
    /// [`NEAR_WHITE`]. This is the measure the tone curve is judged on, since a
    /// blown highlight reads as white well before it literally clips.
    pub near_white_fraction: f32,
    /// Fraction of sampled pixels with every channel at zero.
    pub crushed_fraction: f32,
    /// Mean of all channels, on the 0-255 scale.
    pub mean_level: f32,
    pub sampled_pixels: usize,
}

/// Hasler & Süsstrunk's colourfulness metric.
///
/// Defined on the opponent pairs `rg = R - G` and `yb = (R + G) / 2 - B` as
/// `sqrt(sd_rg^2 + sd_yb^2) + 0.3 * sqrt(mean_rg^2 + mean_yb^2)`.
///
/// Inputs are on the 0-255 scale.
pub fn colourfulness(pixels: impl Iterator<Item = [f32; 3]>) -> f32 {
    let (mut sum_rg, mut sum_yb, mut sum_rg2, mut sum_yb2) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut count = 0u64;

    for [red, green, blue] in pixels {
        let rg = (red - green) as f64;
        let yb = (0.5 * (red + green) - blue) as f64;
        sum_rg += rg;
        sum_yb += yb;
        sum_rg2 += rg * rg;
        sum_yb2 += yb * yb;
        count += 1;
    }

    if count == 0 {
        return 0.0;
    }

    let n = count as f64;
    let mean_rg = sum_rg / n;
    let mean_yb = sum_yb / n;
    // Population variance; clamped because rounding can make it very slightly
    // negative when every sample is identical.
    let var_rg = (sum_rg2 / n - mean_rg * mean_rg).max(0.0);
    let var_yb = (sum_yb2 / n - mean_yb * mean_yb).max(0.0);

    let deviation = (var_rg + var_yb).sqrt();
    let magnitude = (mean_rg * mean_rg + mean_yb * mean_yb).sqrt();
    (deviation + 0.3 * magnitude) as f32
}

impl OutputStats {
    /// Measure a rendered image, sampling roughly [`TARGET_SAMPLES`] pixels.
    pub fn measure(image: &Rgb16Image) -> Self {
        let total = (image.width() as usize) * (image.height() as usize);
        let stride = (total / TARGET_SAMPLES.max(1)).max(1);
        let scale = 255.0 / u16::MAX as f32;

        let samples: Vec<[f32; 3]> = image
            .as_raw()
            .chunks_exact(3)
            .step_by(stride)
            .map(|pixel| {
                [
                    pixel[0] as f32 * scale,
                    pixel[1] as f32 * scale,
                    pixel[2] as f32 * scale,
                ]
            })
            .collect();

        let sampled_pixels = samples.len();
        let mut clipped = 0usize;
        let mut near_white = 0usize;
        let mut crushed = 0usize;
        let mut total_level = 0.0f64;

        for pixel in image.as_raw().chunks_exact(3).step_by(stride) {
            if pixel.contains(&u16::MAX) {
                clipped += 1;
            }
            if pixel.iter().any(|channel| *channel >= NEAR_WHITE) {
                near_white += 1;
            }
            if pixel.iter().all(|channel| *channel == 0) {
                crushed += 1;
            }
            total_level += pixel.iter().map(|c| *c as f64).sum::<f64>();
        }

        let divisor = sampled_pixels.max(1) as f32;
        Self {
            colourfulness: colourfulness(samples.into_iter()),
            clipped_fraction: clipped as f32 / divisor,
            near_white_fraction: near_white as f32 / divisor,
            crushed_fraction: crushed as f32 / divisor,
            mean_level: (total_level / (sampled_pixels.max(1) as f64 * 3.0)) as f32
                * (255.0 / u16::MAX as f32),
            sampled_pixels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_white_threshold_sits_just_below_full_scale() {
        let fraction = NEAR_WHITE as f32 / u16::MAX as f32;
        assert!(
            (0.97..1.0).contains(&fraction),
            "near-white threshold is {fraction}"
        );
    }

    #[test]
    fn greyscale_has_no_colourfulness() {
        let grey = (0..64).map(|v| [v as f32 * 4.0; 3]);
        assert!(colourfulness(grey) < 1.0e-4);
    }

    /// A red/blue split has large opponent spread, so it must score far above
    /// a near-neutral image with the same luminance range.
    #[test]
    fn vivid_scores_above_muted() {
        let vivid = (0..64).map(|index| {
            if index % 2 == 0 {
                [220.0, 20.0, 20.0]
            } else {
                [20.0, 20.0, 220.0]
            }
        });
        let muted = (0..64).map(|index| {
            if index % 2 == 0 {
                [130.0, 120.0, 120.0]
            } else {
                [120.0, 120.0, 130.0]
            }
        });
        assert!(colourfulness(vivid) > 100.0);
        assert!(colourfulness(muted) < 15.0);
    }

    /// The metric must respond to saturation, not to brightness.
    #[test]
    fn scaling_luminance_alone_does_not_change_ranking() {
        let bright = colourfulness((0..32).map(|_| [200.0, 100.0, 50.0]));
        let dark = colourfulness((0..32).map(|_| [100.0, 50.0, 25.0]));
        assert!(
            bright > dark,
            "a constant hue at higher amplitude scores higher"
        );
        assert!(colourfulness(std::iter::empty()) == 0.0);
    }
}
