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

/// Fraction of full scale a pixel must reach before its saturation is measured.
///
/// `(max - min) / max` is a ratio of two small numbers in the deep shadows,
/// where sensor and codec noise dominate both, so including those pixels
/// measures the noise rather than the rendering.
const SATURATION_FLOOR: f32 = 0.05;

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
    /// Mean of `(max - min) / max` over sampled pixels above
    /// [`SATURATION_FLOOR`]: how saturated the rendering is, independently of
    /// how bright it is.
    ///
    /// [`colourfulness`] answers the literature's question — "how colourful
    /// does this look" — but it moves with exposure as well as with chroma, so
    /// two renderings of one scene at different brightness are not comparable
    /// on it. This is a ratio, so a uniform gain leaves it unchanged, which is
    /// what makes it the right measure for steering the chroma path against a
    /// reference rendering.
    pub mean_saturation: f32,
    /// Shannon entropy of sampled 8-bit luminance values.
    pub luminance_entropy: f32,
    /// Mean adjacent-pixel luminance gradient on the 0-255 scale.
    pub average_gradient: f32,
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
        let mut saturation_sum = 0.0f64;
        let mut saturation_count = 0usize;
        let saturation_floor = SATURATION_FLOOR * u16::MAX as f32;
        let mut luminance_histogram = [0_u64; 256];

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
            let maximum = pixel.iter().copied().max().unwrap_or(0) as f32;
            if maximum >= saturation_floor {
                let minimum = pixel.iter().copied().min().unwrap_or(0) as f32;
                saturation_sum += f64::from((maximum - minimum) / maximum);
                saturation_count += 1;
            }
            total_level += pixel.iter().map(|c| *c as f64).sum::<f64>();
            let luminance =
                0.2126 * pixel[0] as f32 + 0.7152 * pixel[1] as f32 + 0.0722 * pixel[2] as f32;
            let bin = (luminance * (255.0 / u16::MAX as f32))
                .round()
                .clamp(0.0, 255.0) as usize;
            luminance_histogram[bin] += 1;
        }

        let divisor = sampled_pixels.max(1) as f32;
        let luminance_entropy = luminance_histogram
            .iter()
            .filter(|count| **count > 0)
            .map(|count| {
                let probability = *count as f32 / divisor;
                -probability * probability.log2()
            })
            .sum();

        let width = image.width() as usize;
        let height = image.height() as usize;
        let spatial_stride = (((width * height) as f64 / TARGET_SAMPLES as f64)
            .sqrt()
            .ceil() as usize)
            .max(1);
        let luminance_at = |x: usize, y: usize| {
            let index = (y * width + x) * 3;
            let raw = image.as_raw();
            (0.2126 * raw[index] as f32
                + 0.7152 * raw[index + 1] as f32
                + 0.0722 * raw[index + 2] as f32)
                * (255.0 / u16::MAX as f32)
        };
        let mut gradient_sum = 0.0_f64;
        let mut gradient_count = 0_usize;
        if width > 1 && height > 1 {
            for y in (0..height - 1).step_by(spatial_stride) {
                for x in (0..width - 1).step_by(spatial_stride) {
                    let center = luminance_at(x, y);
                    let dx = center - luminance_at(x + 1, y);
                    let dy = center - luminance_at(x, y + 1);
                    gradient_sum += f64::from(((dx * dx + dy * dy) * 0.5).sqrt());
                    gradient_count += 1;
                }
            }
        }

        Self {
            colourfulness: colourfulness(samples.into_iter()),
            clipped_fraction: clipped as f32 / divisor,
            near_white_fraction: near_white as f32 / divisor,
            crushed_fraction: crushed as f32 / divisor,
            mean_level: (total_level / (sampled_pixels.max(1) as f64 * 3.0)) as f32
                * (255.0 / u16::MAX as f32),
            mean_saturation: (saturation_sum / saturation_count.max(1) as f64) as f32,
            luminance_entropy,
            average_gradient: (gradient_sum / gradient_count.max(1) as f64) as f32,
            sampled_pixels,
        }
    }

    /// Measure an 8-bit rendering that this program did not produce.
    ///
    /// Deliberately a separate function rather than a widening of the 16-bit
    /// buffer into [`measure`]: a full-resolution phone JPEG is 50 megapixels,
    /// so promoting it to 16 bits would cost 300 MB at exactly the point in the
    /// pipeline where the RAW developer is about to allocate its own peak. The
    /// arithmetic below is the 0-255 counterpart of `measure`, statistic for
    /// statistic, so the two are directly comparable.
    pub fn measure_rgb8(image: &image::RgbImage) -> Self {
        let total = (image.width() as usize) * (image.height() as usize);
        let stride = (total / TARGET_SAMPLES.max(1)).max(1);
        let near_white = 0.98 * 255.0;
        let saturation_floor = SATURATION_FLOOR * 255.0;

        let samples: Vec<[f32; 3]> = image
            .as_raw()
            .chunks_exact(3)
            .step_by(stride)
            .map(|pixel| [pixel[0] as f32, pixel[1] as f32, pixel[2] as f32])
            .collect();

        let sampled_pixels = samples.len();
        let mut clipped = 0usize;
        let mut blown = 0usize;
        let mut crushed = 0usize;
        let mut total_level = 0.0f64;
        let mut saturation_sum = 0.0f64;
        let mut saturation_count = 0usize;
        let mut luminance_histogram = [0_u64; 256];

        for pixel in &samples {
            if pixel.contains(&255.0) {
                clipped += 1;
            }
            if pixel.iter().any(|channel| *channel >= near_white) {
                blown += 1;
            }
            if pixel.iter().all(|channel| *channel == 0.0) {
                crushed += 1;
            }
            let maximum = pixel[0].max(pixel[1]).max(pixel[2]);
            if maximum >= saturation_floor {
                let minimum = pixel[0].min(pixel[1]).min(pixel[2]);
                saturation_sum += f64::from((maximum - minimum) / maximum);
                saturation_count += 1;
            }
            total_level += pixel.iter().map(|c| f64::from(*c)).sum::<f64>();
            let luminance = 0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2];
            luminance_histogram[luminance.round().clamp(0.0, 255.0) as usize] += 1;
        }

        let divisor = sampled_pixels.max(1) as f32;
        let luminance_entropy = luminance_histogram
            .iter()
            .filter(|count| **count > 0)
            .map(|count| {
                let probability = *count as f32 / divisor;
                -probability * probability.log2()
            })
            .sum();

        let width = image.width() as usize;
        let height = image.height() as usize;
        let spatial_stride = (((width * height) as f64 / TARGET_SAMPLES as f64)
            .sqrt()
            .ceil() as usize)
            .max(1);
        let luminance_at = |x: usize, y: usize| {
            let index = (y * width + x) * 3;
            let raw = image.as_raw();
            0.2126 * raw[index] as f32
                + 0.7152 * raw[index + 1] as f32
                + 0.0722 * raw[index + 2] as f32
        };
        let mut gradient_sum = 0.0_f64;
        let mut gradient_count = 0_usize;
        if width > 1 && height > 1 {
            for y in (0..height - 1).step_by(spatial_stride) {
                for x in (0..width - 1).step_by(spatial_stride) {
                    let center = luminance_at(x, y);
                    let dx = center - luminance_at(x + 1, y);
                    let dy = center - luminance_at(x, y + 1);
                    gradient_sum += f64::from(((dx * dx + dy * dy) * 0.5).sqrt());
                    gradient_count += 1;
                }
            }
        }

        Self {
            colourfulness: colourfulness(samples.into_iter()),
            clipped_fraction: clipped as f32 / divisor,
            near_white_fraction: blown as f32 / divisor,
            crushed_fraction: crushed as f32 / divisor,
            mean_level: (total_level / (sampled_pixels.max(1) as f64 * 3.0)) as f32,
            mean_saturation: (saturation_sum / saturation_count.max(1) as f64) as f32,
            luminance_entropy,
            average_gradient: (gradient_sum / gradient_count.max(1) as f64) as f32,
            sampled_pixels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

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

    /// Saturation must read as a ratio: neutral is 0, a single lit channel is 1,
    /// and neither answer may depend on how bright the pixel is.
    #[test]
    fn mean_saturation_is_a_brightness_invariant_ratio() {
        let grey = ImageBuffer::from_pixel(8, 8, Rgb([20_000_u16; 3]));
        assert!(OutputStats::measure(&grey).mean_saturation < 1.0e-6);

        for level in [12_000_u16, 60_000] {
            let red = ImageBuffer::from_pixel(8, 8, Rgb([level, 0, 0]));
            assert!((OutputStats::measure(&red).mean_saturation - 1.0).abs() < 1.0e-6);
        }

        let half = ImageBuffer::from_pixel(8, 8, Rgb([40_000_u16, 20_000, 20_000]));
        assert!((OutputStats::measure(&half).mean_saturation - 0.5).abs() < 1.0e-4);
    }

    /// Pixels in the deep shadows are excluded, so codec noise down there cannot
    /// masquerade as colour.
    #[test]
    fn mean_saturation_ignores_pixels_below_the_floor() {
        let level = (SATURATION_FLOOR * u16::MAX as f32) as u16 - 1;
        let dark = ImageBuffer::from_pixel(8, 8, Rgb([level, 0, 0]));
        assert_eq!(OutputStats::measure(&dark).mean_saturation, 0.0);
    }

    /// A reference JPEG is measured on the 0-255 path, our own render on the
    /// 0-65535 one. The two only mean anything side by side if they agree on
    /// identical content.
    #[test]
    fn the_eight_and_sixteen_bit_paths_agree() {
        let eight = ImageBuffer::from_fn(64, 48, |x, y| {
            Rgb([(x * 4) as u8, (y * 5) as u8, ((x + y) * 2) as u8])
        });
        let sixteen: Rgb16Image = ImageBuffer::from_fn(64, 48, |x, y| {
            let pixel = eight.get_pixel(x, y).0;
            Rgb([
                u16::from(pixel[0]) * 257,
                u16::from(pixel[1]) * 257,
                u16::from(pixel[2]) * 257,
            ])
        });

        let from_eight = OutputStats::measure_rgb8(&eight);
        let from_sixteen = OutputStats::measure(&sixteen);

        assert!((from_eight.colourfulness - from_sixteen.colourfulness).abs() < 0.01);
        assert!((from_eight.mean_level - from_sixteen.mean_level).abs() < 0.01);
        assert!((from_eight.mean_saturation - from_sixteen.mean_saturation).abs() < 1.0e-4);
        assert!((from_eight.average_gradient - from_sixteen.average_gradient).abs() < 0.01);
        assert!((from_eight.luminance_entropy - from_sixteen.luminance_entropy).abs() < 0.01);
    }

    #[test]
    fn flat_image_has_zero_gradient_and_entropy() {
        let image = ImageBuffer::from_pixel(16, 16, Rgb([32_768_u16; 3]));
        let stats = OutputStats::measure(&image);
        assert!(stats.average_gradient < 1.0e-6);
        assert!(stats.luminance_entropy < 1.0e-6);
    }

    #[test]
    fn tonal_steps_have_entropy_and_gradient() {
        let image = ImageBuffer::from_fn(16, 16, |x, _| {
            let level = (x * 4096) as u16;
            Rgb([level; 3])
        });
        let stats = OutputStats::measure(&image);
        assert!(stats.average_gradient > 1.0);
        assert!(stats.luminance_entropy > 3.0);
    }
}
