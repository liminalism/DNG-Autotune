//! Full-resolution, deterministic local tone adaptation.
//!
//! The scale selection follows the Reinhard local operator as reproduced by
//! Zhang et al.: compare adjacent Gaussian surrounds and keep the largest
//! contiguous scale whose normalized center-surround contrast stays below a
//! threshold. The selected surround is converted to a bounded EV dodge/burn
//! field instead of replacing the crate's established global tone curve.

use crate::analyze::luminance;
use crate::types::{LinearImage, MID_GRAY};
use anyhow::{Result, ensure};
use rayon::prelude::*;
use serde::Serialize;

const LOG_EPSILON: f32 = 1.0e-8;
const CONTRAST_EPSILON: f32 = 0.05;
const PHI: f32 = 8.0;
const SCALE_FACTOR: f32 = 1.6;
const CANDIDATE_SCALES: usize = 9;
const DEAD_BAND_EV: f32 = 0.15;
const SHADOW_LIMIT_EV: f32 = 1.0;
const HIGHLIGHT_LIMIT_EV: f32 = 0.75;
const REPORT_SAMPLES: usize = 250_000;

#[derive(Debug, Clone, Serialize)]
pub struct LocalToneReport {
    pub strength: f32,
    pub epsilon: f32,
    pub phi: f32,
    pub scale_count: usize,
    pub smallest_sigma_px: f32,
    pub largest_sigma_px: f32,
    pub shadow_limit_ev: f32,
    pub highlight_limit_ev: f32,
    pub correction_min_ev: f32,
    pub correction_median_ev: f32,
    pub correction_max_ev: f32,
    pub correction_p90_abs_ev: f32,
    pub shadow_lift_fraction: f32,
    pub highlight_compression_fraction: f32,
    pub noise_limited_fraction: f32,
    pub selected_scale_counts: Vec<usize>,
}

/// Per-pixel EV offsets consumed by the renderer.
pub struct LocalToneMap {
    width: usize,
    height: usize,
    corrections_ev: Vec<f32>,
    report: LocalToneReport,
}

impl LocalToneMap {
    pub fn corrections_ev(&self) -> &[f32] {
        &self.corrections_ev
    }

    pub fn report(&self) -> &LocalToneReport {
        &self.report
    }

    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}

#[inline]
fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn sampled_quantile(values: &[f32], quantile: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let stride = (values.len() / REPORT_SAMPLES.max(1)).max(1);
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
    let index = ((samples.len() - 1) as f32 * quantile.clamp(0.0, 1.0)).round() as usize;
    samples[index]
}

fn log_average(luminances: &[f32]) -> f32 {
    const CHUNK: usize = 4096;
    let partials: Vec<f64> = luminances
        .par_chunks(CHUNK)
        .map(|chunk| {
            chunk
                .iter()
                .map(|value| f64::from((value.max(0.0) + LOG_EPSILON).ln()))
                .sum::<f64>()
        })
        .collect();
    let sum: f64 = partials.into_iter().sum();
    (sum / luminances.len().max(1) as f64).exp() as f32
}

fn box_radii_for_gaussian(sigma: f32) -> [usize; 3] {
    let passes = 3.0_f32;
    let ideal = ((12.0 * sigma.max(0.01).powi(2) / passes) + 1.0).sqrt();
    let mut lower_width = ideal.floor() as usize;
    if lower_width.is_multiple_of(2) {
        lower_width = lower_width.saturating_sub(1);
    }
    lower_width = lower_width.max(1);
    let upper_width = lower_width + 2;
    let lower = lower_width as f32;
    let count_lower = (0.25 * passes * (lower + 3.0) - 3.0 * sigma.powi(2) / (lower + 1.0))
        .round()
        .clamp(0.0, passes) as usize;
    std::array::from_fn(|index| {
        let width = if index < count_lower {
            lower_width
        } else {
            upper_width
        };
        (width - 1) / 2
    })
}

fn blur_rows(source: &[f32], destination: &mut [f32], row_len: usize, radius: usize) {
    if radius == 0 {
        destination.copy_from_slice(source);
        return;
    }
    let weight = 1.0 / (radius * 2 + 1) as f32;
    source
        .par_chunks_exact(row_len)
        .zip(destination.par_chunks_exact_mut(row_len))
        .for_each(|(source, destination)| {
            let last = row_len - 1;
            let mut sum = 0.0_f64;
            for offset in 0..=radius * 2 {
                let index = offset.saturating_sub(radius).min(last);
                sum += f64::from(source[index]);
            }
            destination[0] = (sum as f32) * weight;
            for (x, value) in destination.iter_mut().enumerate().skip(1) {
                let remove = x.saturating_sub(radius + 1).min(last);
                let add = x.saturating_add(radius).min(last);
                sum += f64::from(source[add]) - f64::from(source[remove]);
                *value = (sum as f32) * weight;
            }
        });
}

fn transpose(source: &[f32], destination: &mut [f32], width: usize, height: usize) {
    destination
        .par_chunks_exact_mut(height)
        .enumerate()
        .for_each(|(x, column)| {
            for y in 0..height {
                column[y] = source[y * width + x];
            }
        });
}

fn box_blur_2d(
    source: &[f32],
    destination: &mut [f32],
    scratch: &mut [f32],
    width: usize,
    height: usize,
    radius: usize,
) {
    blur_rows(source, scratch, width, radius);
    transpose(scratch, destination, width, height);
    blur_rows(destination, scratch, height, radius);
    transpose(scratch, destination, height, width);
}

fn fast_gaussian_into(
    source: &[f32],
    destination: &mut [f32],
    scratch_a: &mut [f32],
    scratch_b: &mut [f32],
    width: usize,
    height: usize,
    sigma: f32,
) {
    let radii = box_radii_for_gaussian(sigma);
    box_blur_2d(source, destination, scratch_a, width, height, radii[0]);
    box_blur_2d(destination, scratch_b, scratch_a, width, height, radii[1]);
    box_blur_2d(scratch_b, destination, scratch_a, width, height, radii[2]);
}

/// Build the full-resolution local EV correction field.
pub fn build(
    image: &LinearImage,
    strength: f32,
    noise_floor_ev: Option<f32>,
) -> Result<LocalToneMap> {
    ensure!(
        strength.is_finite() && (0.0..=1.0).contains(&strength),
        "local tone strength must be between 0 and 1"
    );
    ensure!(strength > 0.0, "local tone strength must be above zero");
    ensure!(
        image.width > 0
            && image.height > 0
            && image.pixels.len() == image.width.saturating_mul(image.height),
        "cannot build a local tone map for an empty or inconsistent image"
    );

    let count = image.pixels.len();
    let mut normalized: Vec<f32> = image
        .pixels
        .par_iter()
        .map(|pixel| luminance(*pixel).max(0.0))
        .collect();
    let geometric_mean = log_average(&normalized).max(LOG_EPSILON);
    let normalization = MID_GRAY / geometric_mean;
    normalized
        .par_iter_mut()
        .for_each(|value| *value = (*value * normalization).max(LOG_EPSILON));

    let base_sigma = ((image.width.min(image.height) as f32) / 1024.0).max(1.0);
    let sigmas: Vec<f32> = (0..=CANDIDATE_SCALES)
        .map(|index| base_sigma * SCALE_FACTOR.powi(index as i32))
        .collect();

    let mut current = vec![0.0_f32; count];
    let mut next = vec![0.0_f32; count];
    let mut scratch_a = vec![0.0_f32; count];
    let mut scratch_b = vec![0.0_f32; count];
    fast_gaussian_into(
        &normalized,
        &mut current,
        &mut scratch_a,
        &mut scratch_b,
        image.width,
        image.height,
        sigmas[0],
    );
    fast_gaussian_into(
        &normalized,
        &mut next,
        &mut scratch_a,
        &mut scratch_b,
        image.width,
        image.height,
        sigmas[1],
    );

    let mut selected = current.clone();
    let mut selected_scale = vec![0_u8; count];
    let mut active = vec![true; count];

    for scale in 0..CANDIDATE_SCALES {
        let canonical_scale = SCALE_FACTOR.powi(scale as i32);
        let stabilizer = 2.0_f32.powf(PHI) * MID_GRAY / canonical_scale.powi(2);
        selected
            .par_iter_mut()
            .zip(selected_scale.par_iter_mut())
            .zip(active.par_iter_mut())
            .zip(current.par_iter())
            .zip(next.par_iter())
            .for_each(|((((selected, selected_scale), active), current), next)| {
                if !*active {
                    return;
                }
                let contrast =
                    (current - next).abs() / (stabilizer + current.max(0.0) + LOG_EPSILON);
                if contrast < CONTRAST_EPSILON {
                    *selected = *current;
                    *selected_scale = scale as u8;
                } else {
                    *active = false;
                }
            });

        if scale + 1 < CANDIDATE_SCALES {
            std::mem::swap(&mut current, &mut next);
            fast_gaussian_into(
                &normalized,
                &mut next,
                &mut scratch_a,
                &mut scratch_b,
                image.width,
                image.height,
                sigmas[scale + 2],
            );
        }
    }
    drop(current);
    drop(next);
    drop(scratch_a);
    drop(scratch_b);
    drop(active);

    selected.par_iter_mut().for_each(|value| {
        *value = (value.max(LOG_EPSILON) / MID_GRAY).log2();
    });
    let median_base_ev = sampled_quantile(&selected, 0.5);

    let mut noise_limited = vec![false; count];
    selected
        .par_iter_mut()
        .zip(noise_limited.par_iter_mut())
        .zip(normalized.par_iter())
        .zip(image.pixels.par_iter())
        .for_each(
            |(((correction, noise_limited), normalized_pixel), source)| {
                let base_ev = *correction;
                let difference = median_base_ev - base_ev;
                let magnitude = (difference.abs() - DEAD_BAND_EV).max(0.0);
                let mut value = if difference >= 0.0 {
                    magnitude.min(SHADOW_LIMIT_EV)
                } else {
                    -magnitude.min(HIGHLIGHT_LIMIT_EV)
                };

                if value > 0.0 {
                    if let Some(floor) = noise_floor_ev.filter(|floor| floor.is_finite()) {
                        let scene_ev = (luminance(*source).max(LOG_EPSILON) / MID_GRAY).log2();
                        let confidence = smoothstep((scene_ev - floor) / 2.0);
                        *noise_limited = confidence < 0.999;
                        value *= confidence;
                    }

                    let pixel_ev = (normalized_pixel.max(LOG_EPSILON) / MID_GRAY).log2();
                    let detail_ev = pixel_ev - base_ev;
                    value *= 1.0 - smoothstep((detail_ev - 1.0) / 2.0);
                }

                *correction = value * strength;
            },
        );

    let correction_min_ev = selected.iter().copied().fold(f32::INFINITY, f32::min);
    let correction_max_ev = selected.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let correction_median_ev = sampled_quantile(&selected, 0.5);
    let absolute: Vec<f32> = selected.iter().map(|value| value.abs()).collect();
    let correction_p90_abs_ev = sampled_quantile(&absolute, 0.9);
    let divisor = count as f32;
    let shadow_lift_fraction =
        selected.iter().filter(|value| **value > 0.1).count() as f32 / divisor;
    let highlight_compression_fraction =
        selected.iter().filter(|value| **value < -0.1).count() as f32 / divisor;
    let noise_limited_fraction =
        noise_limited.iter().filter(|value| **value).count() as f32 / divisor;
    let mut selected_scale_counts = vec![0_usize; CANDIDATE_SCALES];
    for scale in selected_scale {
        selected_scale_counts[usize::from(scale)] += 1;
    }

    Ok(LocalToneMap {
        width: image.width,
        height: image.height,
        corrections_ev: selected,
        report: LocalToneReport {
            strength,
            epsilon: CONTRAST_EPSILON,
            phi: PHI,
            scale_count: CANDIDATE_SCALES,
            smallest_sigma_px: sigmas[0],
            largest_sigma_px: sigmas[CANDIDATE_SCALES - 1],
            shadow_limit_ev: SHADOW_LIMIT_EV,
            highlight_limit_ev: HIGHLIGHT_LIMIT_EV,
            correction_min_ev,
            correction_median_ev,
            correction_max_ev,
            correction_p90_abs_ev,
            shadow_lift_fraction,
            highlight_compression_fraction,
            noise_limited_fraction,
            selected_scale_counts,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: usize, height: usize, value: impl Fn(usize, usize) -> f32) -> LinearImage {
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                pixels.push([value(x, y); 3]);
            }
        }
        LinearImage::new(width, height, pixels).unwrap()
    }

    #[test]
    fn constant_image_selects_the_largest_scale_and_stays_zero() {
        let map = build(&image(32, 32, |_, _| MID_GRAY), 1.0, None).unwrap();
        assert!(
            map.corrections_ev()
                .iter()
                .all(|value| value.abs() < 1.0e-6)
        );
        assert_eq!(
            map.report().selected_scale_counts[CANDIDATE_SCALES - 1],
            32 * 32
        );
    }

    #[test]
    fn dark_minority_is_lifted_and_bright_minority_is_compressed() {
        let dark = build(
            &image(64, 32, |x, _| if x < 12 { 0.02 } else { 0.3 }),
            1.0,
            None,
        )
        .unwrap();
        assert!(dark.corrections_ev()[16 * 64 + 2] > 0.1);

        let bright = build(
            &image(64, 32, |x, _| if x < 12 { 1.5 } else { 0.08 }),
            1.0,
            None,
        )
        .unwrap();
        assert!(bright.corrections_ev()[16 * 64 + 2] < -0.1);
    }

    #[test]
    fn corrections_stay_within_declared_limits() {
        let map = build(
            &image(96, 32, |x, _| match x / 32 {
                0 => 0.0001,
                1 => 0.18,
                _ => 20.0,
            }),
            1.0,
            None,
        )
        .unwrap();
        assert!(map.report().correction_min_ev >= -HIGHLIGHT_LIMIT_EV);
        assert!(map.report().correction_max_ev <= SHADOW_LIMIT_EV);
    }

    #[test]
    fn noise_floor_suppresses_shadow_lift() {
        let source = image(64, 32, |x, _| if x < 12 { 0.002 } else { 0.3 });
        let free = build(&source, 1.0, None).unwrap();
        let guarded = build(&source, 1.0, Some(-6.0)).unwrap();
        let index = 16 * 64 + 2;
        assert!(guarded.corrections_ev()[index] < free.corrections_ev()[index]);
        assert!(guarded.report().noise_limited_fraction > 0.0);
    }

    #[test]
    fn repeated_builds_are_identical() {
        let source = image(48, 40, |x, y| {
            0.01 + (x as f32 / 47.0) * 0.5 + (y % 3) as f32 * 0.01
        });
        let first = build(&source, 0.7, Some(-8.0)).unwrap();
        let second = build(&source, 0.7, Some(-8.0)).unwrap();
        assert_eq!(first.corrections_ev(), second.corrections_ev());
    }

    #[test]
    fn smooth_ramp_stays_monotonic_after_local_correction() {
        let source = image(128, 16, |x, _| 0.002 * 256.0_f32.powf(x as f32 / 127.0));
        let map = build(&source, 1.0, None).unwrap();
        let row = &map.corrections_ev()[8 * 128..9 * 128];
        for x in 1..128 {
            let previous = source.pixels[8 * 128 + x - 1][0] * row[x - 1].exp2();
            let current = source.pixels[8 * 128 + x][0] * row[x].exp2();
            assert!(
                current + 1.0e-7 >= previous,
                "local correction reversed the ramp at {x}: {previous} -> {current}"
            );
        }
    }

    #[test]
    fn hard_edge_does_not_create_plateau_overshoot() {
        let source = image(128, 32, |x, _| if x < 64 { 0.02 } else { 1.0 });
        let map = build(&source, 1.0, None).unwrap();
        let row = &map.corrections_ev()[16 * 128..17 * 128];
        for correction in row.iter().take(64) {
            let adjusted = 0.02 * correction.exp2();
            assert!((0.02..=0.04).contains(&adjusted));
        }
        for correction in row.iter().skip(64) {
            let adjusted = correction.exp2();
            assert!((0.5..=1.0).contains(&adjusted));
        }
    }
}
