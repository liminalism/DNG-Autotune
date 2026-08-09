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

/// A per-pixel EV correction field the renderer can consume, whatever produced
/// it. Both the Gaussian-surround local-tone operator ([`LocalToneMap`]) and the
/// edge-aware HDR operator ([`HdrMap`]) hand `tone::render` the same shape — one
/// scalar EV offset per pixel — so the render path stays a single code path.
pub trait CorrectionField: Sync {
    fn corrections_ev(&self) -> &[f32];
    fn dimensions(&self) -> (usize, usize);
}

impl CorrectionField for LocalToneMap {
    fn corrections_ev(&self) -> &[f32] {
        &self.corrections_ev
    }
    fn dimensions(&self) -> (usize, usize) {
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

/// The per-pixel weight that withholds a local-tone correction where the
/// highlight was reconstructed rather than recorded.
///
/// `uncertainty` is the raw-domain map `highlight::reconstruct_with_confidence_
/// and_uncertainty` emits, aligned to `pixels` and already carried through the
/// orientation transform. When it is absent (callers that never ran
/// reconstruction, and the unit tests) the same quantity is approximated from
/// the developed pixel itself — the old behaviour, kept only as a fallback,
/// because post-white-balance values are a poorer stand-in for "was this
/// channel at the sensor's clip point?" than the raw-domain map.
#[inline]
fn synthesis_gate_at(uncertainty: Option<&[f32]>, index: usize, source: [f32; 3]) -> f32 {
    match uncertainty {
        Some(map) => crate::highlight::synthesis_gate(map[index]),
        None => crate::highlight::synthesis_gate_from_confidence([
            crate::highlight::clip_confidence(source[0]),
            crate::highlight::clip_confidence(source[1]),
            crate::highlight::clip_confidence(source[2]),
        ]),
    }
}

/// Build the full-resolution local EV correction field.
pub fn build(
    image: &LinearImage,
    strength: f32,
    noise_floor_ev: Option<f32>,
    highlight_uncertainty: Option<&[f32]>,
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
    ensure!(
        highlight_uncertainty.is_none_or(|map| map.len() == image.pixels.len()),
        "highlight uncertainty map must match the image it gates"
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
        .enumerate()
        .zip(noise_limited.par_iter_mut())
        .zip(normalized.par_iter())
        .zip(image.pixels.par_iter())
        .for_each(
            |((((index, correction), noise_limited), normalized_pixel), source)| {
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

                // Slice 6: r-gated detail — suppress local contrast where the
                // highlight was reconstructed rather than recorded, blended
                // continuously so the 1→2 clipped boundary does not become a
                // luminance contour. The gate weight comes from the same
                // raw-domain uncertainty map the renderer's chroma path uses, so
                // all three consumers move together when its tuning changes.
                // Median stays near zero (median_base_ev) and the correction is
                // never applied per-channel — one EV gain for all three.
                {
                    let u = synthesis_gate_at(highlight_uncertainty, index, *source);
                    // Gate both lifts and compressions; 0.85 retains the
                    // existing strength so --local-tone 0.35 HDR sky/ground
                    // separation is preserved on trusted pixels.
                    value *= 1.0 - u * 0.85;
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

// --- HDR-like single-frame local tone (Slice 8) ---------------------------
//
// `docs/white-blowout-advice.md` §"Where HDR belongs": what balances sky against
// ground on a single frame is edge-aware local tone mapping, not an HDR *output*
// container (`docs/HDR_EVALUATION.md` measured under 1 EV of unclipped headroom
// on the blown-sky cohort, so a wider container has nothing to hold). This is
// the controlled design the advice sketches:
//
//   L = log2(Y),  B = guided_filter(L),  D = L - B,
//   dEV = B_compressed + k*D - L,   one scalar RGB gain, applied in `tone::render`.
//
// Off by default and byte-identical when off, on the `--local-tone 0` precedent;
// it is deliberately *not* wired into the automatic profile until the halo test
// passes on real leaf/branch/ridge crops (advice's default-on gate).

/// Guided-filter window as a fraction of the shorter image side. A larger
/// divisor means a smaller window and a base that follows finer structure.
const HDR_GUIDED_RADIUS_DIVISOR: f32 = 64.0;
/// Regularisation of the self-guided filter, in the log2 (EV²) domain. An edge
/// of a few EV has variance far above this, so `a → 1` and the edge is kept;
/// flat regions fall below it, so `a → 0` and the base smooths. Tunable.
const HDR_GUIDED_EPS: f32 = 0.02;
/// Fraction of the base's dynamic range removed at full strength. 0.5 halves the
/// low-frequency EV span — the sky/ground separation the operator exists to buy.
const HDR_BASE_COMPRESSION: f32 = 0.5;
/// How much local (detail) contrast is retained. 1.0 keeps it whole, so the
/// compression acts only on the base, the classic Durand/Dorsey split.
const HDR_DETAIL_RETENTION: f32 = 1.0;
/// Asymmetric caps (advice: shadow +0.4–0.6 EV, highlight −0.6–0.9 EV).
const HDR_SHADOW_LIMIT_EV: f32 = 0.6;
const HDR_HIGHLIGHT_LIMIT_EV: f32 = 0.9;
/// How hard to pull the correction toward zero where highlight reconstruction
/// synthesised colour, so HDR never re-exposes an invented sky (Slice 6 gate).
const HDR_UNCERTAINTY_GATE: f32 = 0.85;

#[derive(Debug, Clone, Serialize)]
pub struct HdrReport {
    pub strength: f32,
    pub guided_radius_px: usize,
    pub guided_eps: f32,
    pub base_compression: f32,
    pub detail_retention: f32,
    pub shadow_limit_ev: f32,
    pub highlight_limit_ev: f32,
    /// Low-frequency (base) EV span, p95−p5, before compression: the frame's
    /// sky-band vs ground-band separation as the operator sees it.
    pub base_span_ev: f32,
    /// The same span after compression — what the operator narrows it to.
    pub compressed_base_span_ev: f32,
    pub correction_min_ev: f32,
    pub correction_median_ev: f32,
    pub correction_max_ev: f32,
    pub correction_p90_abs_ev: f32,
    pub shadow_lift_fraction: f32,
    pub highlight_compression_fraction: f32,
    pub noise_limited_fraction: f32,
}

/// Per-pixel EV offsets from the HDR operator, consumed by `tone::render`
/// exactly like [`LocalToneMap`].
pub struct HdrMap {
    width: usize,
    height: usize,
    corrections_ev: Vec<f32>,
    report: HdrReport,
}

impl HdrMap {
    pub fn corrections_ev(&self) -> &[f32] {
        &self.corrections_ev
    }
    pub fn report(&self) -> &HdrReport {
        &self.report
    }
    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}

impl CorrectionField for HdrMap {
    fn corrections_ev(&self) -> &[f32] {
        &self.corrections_ev
    }
    fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}

/// Self-guided edge-preserving filter (He et al., guide = input = `l`). Returns
/// the smoothed base in the same units as `l`.
fn self_guided_filter(l: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let count = l.len();
    let mut scratch = vec![0.0_f32; count];

    let mut mean_i = vec![0.0_f32; count];
    box_blur_2d(l, &mut mean_i, &mut scratch, width, height, radius);

    let ll: Vec<f32> = l.par_iter().map(|v| v * v).collect();
    let mut mean_ii = vec![0.0_f32; count];
    box_blur_2d(&ll, &mut mean_ii, &mut scratch, width, height, radius);
    drop(ll);

    // a = var / (var + eps); b = (1 - a) * mean_i
    let a: Vec<f32> = mean_ii
        .par_iter()
        .zip(mean_i.par_iter())
        .map(|(mii, mi)| {
            let var = (mii - mi * mi).max(0.0);
            var / (var + HDR_GUIDED_EPS)
        })
        .collect();
    drop(mean_ii);
    let b: Vec<f32> = a
        .par_iter()
        .zip(mean_i.par_iter())
        .map(|(a, mi)| (1.0 - a) * mi)
        .collect();
    drop(mean_i);

    let mut mean_a = vec![0.0_f32; count];
    box_blur_2d(&a, &mut mean_a, &mut scratch, width, height, radius);
    let mut mean_b = vec![0.0_f32; count];
    box_blur_2d(&b, &mut mean_b, &mut scratch, width, height, radius);
    drop(a);
    drop(b);

    mean_a
        .par_iter()
        .zip(mean_b.par_iter())
        .zip(l.par_iter())
        .map(|((ma, mb), lv)| ma * lv + mb)
        .collect()
}

/// Build the HDR-like EV correction field. Edge-aware base/detail split in log2
/// luminance, base range compressed toward its median, detail retained; one
/// scalar EV offset per pixel.
pub fn build_hdr(
    image: &LinearImage,
    strength: f32,
    noise_floor_ev: Option<f32>,
    highlight_uncertainty: Option<&[f32]>,
) -> Result<HdrMap> {
    ensure!(
        strength.is_finite() && (0.0..=1.0).contains(&strength),
        "hdr strength must be between 0 and 1"
    );
    ensure!(strength > 0.0, "hdr strength must be above zero");
    ensure!(
        image.width > 0
            && image.height > 0
            && image.pixels.len() == image.width.saturating_mul(image.height),
        "cannot build an hdr map for an empty or inconsistent image"
    );
    ensure!(
        highlight_uncertainty.is_none_or(|map| map.len() == image.pixels.len()),
        "highlight uncertainty map must match the image it gates"
    );

    let count = image.pixels.len();
    let (width, height) = (image.width, image.height);

    // Scene luminance normalised so mid-grey lands at MID_GRAY, then log2 EV
    // relative to mid-grey — the domain where a base/detail split is a tone
    // operation rather than a brightness one.
    let scene: Vec<f32> = image
        .pixels
        .par_iter()
        .map(|pixel| luminance(*pixel).max(0.0))
        .collect();
    let geometric_mean = log_average(&scene).max(LOG_EPSILON);
    let normalization = MID_GRAY / geometric_mean;
    let l: Vec<f32> = scene
        .par_iter()
        .map(|value| ((value * normalization).max(LOG_EPSILON) / MID_GRAY).log2())
        .collect();
    drop(scene);

    let radius = (width.min(height) as f32 / HDR_GUIDED_RADIUS_DIVISOR).round() as usize;
    let radius = radius.max(1);
    let base = self_guided_filter(&l, width, height, radius);

    let base_p5 = sampled_quantile(&base, 0.05);
    let base_p50 = sampled_quantile(&base, 0.5);
    let base_p95 = sampled_quantile(&base, 0.95);
    let base_span_ev = base_p95 - base_p5;
    let alpha = 1.0 - HDR_BASE_COMPRESSION * strength;
    let k = HDR_DETAIL_RETENTION;
    let compressed_base_span_ev = base_span_ev * alpha;

    let (corrections, noise_flags): (Vec<f32>, Vec<bool>) = (0..count)
        .into_par_iter()
        .map(|i| {
            let bv = base[i];
            let lv = l[i];
            let detail = lv - bv;
            let base_compressed = base_p50 + alpha * (bv - base_p50);
            // dEV = (Bc + k*D) - L. With k = 1 this is (alpha-1)*(B - median):
            // pixels whose base is above the median compress, below it lift,
            // and the median pixel is unmoved, so exposure does not drift.
            let ev = (base_compressed + k * detail) - lv;
            let mut value = if ev >= 0.0 {
                ev.min(HDR_SHADOW_LIMIT_EV)
            } else {
                -((-ev).min(HDR_HIGHLIGHT_LIMIT_EV))
            };

            let mut limited = false;
            if value > 0.0
                && let Some(floor) = noise_floor_ev.filter(|floor| floor.is_finite())
            {
                let scene_ev = (luminance(image.pixels[i]).max(LOG_EPSILON) / MID_GRAY).log2();
                let confidence = smoothstep((scene_ev - floor) / 2.0);
                limited = confidence < 0.999;
                value *= confidence;
            }

            // Do not re-expose synthesised highlight colour (Slice 6 gate),
            // off the same raw-domain uncertainty map as `build` and the
            // renderer's chroma path.
            let u = synthesis_gate_at(highlight_uncertainty, i, image.pixels[i]);
            value *= 1.0 - u * HDR_UNCERTAINTY_GATE;

            (value * strength, limited)
        })
        .unzip();

    let correction_min_ev = corrections.iter().copied().fold(f32::INFINITY, f32::min);
    let correction_max_ev = corrections
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let correction_median_ev = sampled_quantile(&corrections, 0.5);
    let absolute: Vec<f32> = corrections.iter().map(|value| value.abs()).collect();
    let correction_p90_abs_ev = sampled_quantile(&absolute, 0.9);
    let divisor = count as f32;
    let shadow_lift_fraction =
        corrections.iter().filter(|value| **value > 0.1).count() as f32 / divisor;
    let highlight_compression_fraction =
        corrections.iter().filter(|value| **value < -0.1).count() as f32 / divisor;
    let noise_limited_fraction =
        noise_flags.iter().filter(|value| **value).count() as f32 / divisor;

    Ok(HdrMap {
        width,
        height,
        corrections_ev: corrections,
        report: HdrReport {
            strength,
            guided_radius_px: radius,
            guided_eps: HDR_GUIDED_EPS,
            base_compression: HDR_BASE_COMPRESSION,
            detail_retention: HDR_DETAIL_RETENTION,
            shadow_limit_ev: HDR_SHADOW_LIMIT_EV,
            highlight_limit_ev: HDR_HIGHLIGHT_LIMIT_EV,
            base_span_ev,
            compressed_base_span_ev,
            correction_min_ev,
            correction_median_ev,
            correction_max_ev,
            correction_p90_abs_ev,
            shadow_lift_fraction,
            highlight_compression_fraction,
            noise_limited_fraction,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate must follow the raw-domain uncertainty map, not the developed
    /// pixel. This image never comes near the clip ramp, so the fallback proxy
    /// is zero everywhere and only a real map can suppress anything.
    #[test]
    fn hdr_gate_follows_the_reconstruction_map_on_dim_pixels() {
        let source = image(128, 32, |x, _| 0.01 * 16.0_f32.powf(x as f32 / 127.0));
        assert!(
            source
                .pixels
                .iter()
                .all(|p| p[0] < crate::highlight::CLIP_RAMP_LOW),
            "the fixture must stay below the clip ramp for this to prove anything"
        );
        let ungated = build_hdr(&source, 1.0, None, None).unwrap();
        let map: Vec<f32> = (0..source.pixels.len())
            .map(|i| if i % 128 < 64 { 1.0 } else { 0.0 })
            .collect();
        let gated = build_hdr(&source, 1.0, None, Some(&map)).unwrap();

        let (mut suppressed, mut counted) = (0usize, 0usize);
        for (i, (before, after)) in ungated
            .corrections_ev()
            .iter()
            .zip(gated.corrections_ev())
            .enumerate()
        {
            if i % 128 < 64 {
                if before.abs() > 0.05 {
                    counted += 1;
                    if after.abs() < before.abs() * 0.5 {
                        suppressed += 1;
                    }
                }
            } else {
                assert_eq!(before, after, "trusted pixels must be untouched");
            }
        }
        assert!(counted > 0, "fixture produced no corrections to gate");
        assert_eq!(suppressed, counted, "reconstructed pixels were not gated");
    }

    #[test]
    fn local_tone_gate_follows_the_reconstruction_map_on_dim_pixels() {
        let source = image(128, 32, |x, _| 0.01 * 16.0_f32.powf(x as f32 / 127.0));
        let ungated = build(&source, 1.0, None, None).unwrap();
        let map = vec![1.0_f32; source.pixels.len()];
        let gated = build(&source, 1.0, None, Some(&map)).unwrap();
        let before = ungated.report().correction_p90_abs_ev;
        let after = gated.report().correction_p90_abs_ev;
        assert!(before > 0.05, "fixture produced no corrections to gate");
        assert!(
            after < before * 0.2,
            "fully reconstructed frame should be almost entirely gated: {before} -> {after}"
        );
    }

    #[test]
    fn a_mismatched_uncertainty_map_is_rejected() {
        let source = image(16, 16, |_, _| MID_GRAY);
        let map = vec![0.0_f32; 9];
        assert!(build(&source, 1.0, None, Some(&map)).is_err());
        assert!(build_hdr(&source, 1.0, None, Some(&map)).is_err());
    }

    #[test]
    fn hdr_constant_image_produces_no_correction() {
        let map = build_hdr(&image(48, 48, |_, _| MID_GRAY), 1.0, None, None).unwrap();
        assert!(
            map.corrections_ev()
                .iter()
                .all(|value| value.abs() < 1.0e-4),
            "a flat field has no dynamic range to redistribute"
        );
    }

    #[test]
    fn hdr_median_correction_stays_near_zero() {
        // A broad low-frequency ramp: the base carries the range, so the
        // operator has plenty to compress — but must not drift exposure.
        let source = image(256, 64, |x, _| 0.01 * 64.0_f32.powf(x as f32 / 255.0));
        let map = build_hdr(&source, 1.0, None, None).unwrap();
        assert!(
            map.report().correction_median_ev.abs() < 0.1,
            "median correction drifted: {}",
            map.report().correction_median_ev
        );
    }

    #[test]
    fn hdr_lifts_shadows_and_compresses_highlights_within_limits() {
        let source = image(256, 64, |x, _| 0.01 * 64.0_f32.powf(x as f32 / 255.0));
        let map = build_hdr(&source, 1.0, None, None).unwrap();
        let report = map.report();
        assert!(report.shadow_lift_fraction > 0.0, "no shadow was lifted");
        assert!(
            report.highlight_compression_fraction > 0.0,
            "no highlight was compressed"
        );
        assert!(
            report.correction_max_ev <= HDR_SHADOW_LIMIT_EV + 1.0e-4,
            "shadow lift exceeded its cap: {}",
            report.correction_max_ev
        );
        assert!(
            report.correction_min_ev >= -HDR_HIGHLIGHT_LIMIT_EV - 1.0e-4,
            "highlight compression exceeded its cap: {}",
            report.correction_min_ev
        );
    }

    #[test]
    fn hdr_compresses_the_base_range() {
        let source = image(256, 64, |x, _| 0.01 * 64.0_f32.powf(x as f32 / 255.0));
        let report = build_hdr(&source, 1.0, None, None)
            .unwrap()
            .report()
            .clone();
        assert!(
            report.compressed_base_span_ev < report.base_span_ev,
            "base range was not compressed: {} -> {}",
            report.base_span_ev,
            report.compressed_base_span_ev
        );
    }

    #[test]
    fn hdr_ramp_stays_monotonic_after_correction() {
        let source = image(256, 16, |x, _| 0.002 * 512.0_f32.powf(x as f32 / 255.0));
        let map = build_hdr(&source, 1.0, None, None).unwrap();
        let row = &map.corrections_ev()[8 * 256..9 * 256];
        for x in 1..256 {
            let previous = source.pixels[8 * 256 + x - 1][0] * row[x - 1].exp2();
            let current = source.pixels[8 * 256 + x][0] * row[x].exp2();
            assert!(
                current + 1.0e-6 >= previous,
                "hdr correction reversed the ramp at {x}: {previous} -> {current}"
            );
        }
    }

    #[test]
    fn hdr_hard_edge_does_not_bleed_a_halo() {
        // Bright block beside a dark block. An edge-aware base keeps the
        // correction of dark pixels next to the edge close to that of dark
        // pixels in the interior: a halo is exactly the base of the bright
        // side leaking across and changing the dark side's correction.
        let width = 256;
        let source = image(width, 64, |x, _| if x < width / 2 { 0.02 } else { 1.5 });
        let map = build_hdr(&source, 1.0, None, None).unwrap();
        let row = &map.corrections_ev()[32 * width..33 * width];
        let interior = row[8]; // deep in the dark block
        let at_edge = row[width / 2 - 2]; // dark pixel adjacent to the edge
        assert!(
            (at_edge - interior).abs() < 0.15,
            "halo: dark-side correction moved {interior} -> {at_edge} near the edge"
        );
    }

    #[test]
    fn hdr_repeated_builds_are_identical() {
        let source = image(96, 80, |x, y| {
            0.01 + (x as f32 / 95.0) * 0.6 + (y % 4) as f32 * 0.02
        });
        let first = build_hdr(&source, 0.7, Some(-8.0), None).unwrap();
        let second = build_hdr(&source, 0.7, Some(-8.0), None).unwrap();
        assert_eq!(first.corrections_ev(), second.corrections_ev());
    }

    #[test]
    fn hdr_noise_floor_suppresses_shadow_lift() {
        let source = image(256, 64, |x, _| if x < 40 { 0.002 } else { 0.4 });
        let free = build_hdr(&source, 1.0, None, None).unwrap();
        let guarded = build_hdr(&source, 1.0, Some(-6.0), None).unwrap();
        let index = 32 * 256 + 4;
        assert!(
            guarded.corrections_ev()[index] <= free.corrections_ev()[index] + 1.0e-6,
            "noise floor did not restrain the shadow lift"
        );
        assert!(guarded.report().noise_limited_fraction > 0.0);
    }

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
        let map = build(&image(32, 32, |_, _| MID_GRAY), 1.0, None, None).unwrap();
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
            None,
        )
        .unwrap();
        assert!(dark.corrections_ev()[16 * 64 + 2] > 0.1);

        let bright = build(
            &image(64, 32, |x, _| if x < 12 { 1.5 } else { 0.08 }),
            1.0,
            None,
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
            None,
        )
        .unwrap();
        assert!(map.report().correction_min_ev >= -HIGHLIGHT_LIMIT_EV);
        assert!(map.report().correction_max_ev <= SHADOW_LIMIT_EV);
    }

    #[test]
    fn noise_floor_suppresses_shadow_lift() {
        let source = image(64, 32, |x, _| if x < 12 { 0.002 } else { 0.3 });
        let free = build(&source, 1.0, None, None).unwrap();
        let guarded = build(&source, 1.0, Some(-6.0), None).unwrap();
        let index = 16 * 64 + 2;
        assert!(guarded.corrections_ev()[index] < free.corrections_ev()[index]);
        assert!(guarded.report().noise_limited_fraction > 0.0);
    }

    #[test]
    fn repeated_builds_are_identical() {
        let source = image(48, 40, |x, y| {
            0.01 + (x as f32 / 47.0) * 0.5 + (y % 3) as f32 * 0.01
        });
        let first = build(&source, 0.7, Some(-8.0), None).unwrap();
        let second = build(&source, 0.7, Some(-8.0), None).unwrap();
        assert_eq!(first.corrections_ev(), second.corrections_ev());
    }

    #[test]
    fn smooth_ramp_stays_monotonic_after_local_correction() {
        let source = image(128, 16, |x, _| 0.002 * 256.0_f32.powf(x as f32 / 127.0));
        let map = build(&source, 1.0, None, None).unwrap();
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
        let map = build(&source, 1.0, None, None).unwrap();
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
