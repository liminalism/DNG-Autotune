//! Per-image sensor noise estimation.
//!
//! The controller lifts shadows by a median of +1.37 EV across the Sony test
//! batch, and up to +5 EV, with no idea whether there is signal down there. A
//! noise estimate turns "how dark is the darkest percentile" into "how far down
//! is there still something worth showing".
//!
//! # Model
//!
//! kronander2013 §5 gives the standard sensor model: photoelectrons arrive as a
//! Poisson process, so their variance equals their mean, and the readout adds
//! signal-independent Gaussian noise. Carried through the gain into digital
//! numbers (their eqs. 8-12) that leaves variance affine in the signal:
//!
//! ```text
//! var(s) = shot_slope * s + read_variance
//! ```
//!
//! where `s` is the black-subtracted signal in DN. `shot_slope` is the gain in
//! DN per electron and `read_variance` is the readout floor. The paper
//! calibrates these from bias and flat frames; we have neither, so they are
//! estimated from the image itself in the manner of its reference [9] (Foi et
//! al., practical Poissonian-Gaussian modelling from single raw data).
//!
//! # Estimation
//!
//! Tiles are measured across the frame, giving many (mean, variance) pairs.
//! Plain tile variance is useless here: in a photograph it is dominated by
//! detail, not noise, and fitting it produced SNR=10 crossings above sensor
//! saturation. Instead each tile's noise is estimated from the second
//! difference along rows, `x[i-1] - 2*x[i] + x[i+1]`, which is exactly zero for
//! any linear ramp and so cancels smooth content. For independent noise of
//! variance `v` that combination has variance `6v`, giving `v = var(d) / 6`.
//!
//! Edges and texture still inflate some tiles, so within each brightness bin
//! the *low* percentile is taken as the noise level, and a line is fitted
//! through those per-bin representatives.
//!
//! Samples are drawn from a single CFA phase so that neighbouring samples are
//! the same colour; otherwise the mosaic itself would read as variance. The
//! phase is chosen from the camera's actual CFA layout so that it lands on a
//! *green* site. Sampling phase (0,0) blindly picks red on an RGGB sensor,
//! which under daylight carries roughly half the signal of green — on the Sony
//! corpus that starved the fit of brightness range and made 43 of 258 frames
//! return no estimate at all.

use rawler::cfa::CFAColor;
use rawler::rawimage::RawPhotometricInterpretation;
use rawler::{RawImage, RawImageData};
use serde::Serialize;

/// Samples per side of a measurement tile, within one colour plane.
const TILE: usize = 8;
/// Brightness bins used to separate flat tiles from textured ones.
///
/// Bins hold an equal *count* of tiles rather than covering equal ranges of
/// brightness. Fixed-width bins leave most of the range empty on an ordinary
/// frame — a photograph without bright highlights put every tile into the
/// bottom few bins and failed the minimum-bin check outright.
const BINS: usize = 20;
/// Quantile of each bin's variance taken as its noise level.
const FLAT_QUANTILE: f32 = 0.05;
/// Minimum tiles in a bin for it to contribute to the fit.
const MIN_TILES_PER_BIN: usize = 32;
/// Minimum bins with data for an estimate to be reported at all.
const MIN_BINS: usize = 4;
/// Ratio between the brightest and darkest bin needed to fit a slope at all.
///
/// Equal-count bins can always be formed, so this replaces the old failure mode
/// with an honest one: without about two stops of leverage the slope is not
/// determined, whatever the bin count says.
const MIN_LEVERAGE: f32 = 4.0;

/// Signal-to-noise ratios whose crossing points are reported.
const SNR_REPORTED: [f32; 2] = [1.0, 10.0];

/// A fitted noise model for one image.
#[derive(Debug, Clone, Serialize)]
pub struct NoiseEstimate {
    /// Slope of variance against signal, in DN per DN. Proportional to sensor
    /// gain, so it rises with ISO.
    pub shot_slope: f32,
    /// Readout variance floor, in DN squared.
    pub read_variance: f32,
    /// Scene EV, relative to middle grey, at which SNR falls to 1. Below this
    /// there is essentially no signal.
    pub snr1_ev: f32,
    /// Scene EV at which SNR falls to 10, a rough floor for clean shadow detail.
    pub snr10_ev: f32,
    /// Tiles that contributed to the fit.
    pub tiles_used: usize,
    /// White level minus black level, in DN. Retained because converting a
    /// *pooled* slope back into scene EV needs it, and re-deriving it would mean
    /// decoding the file again.
    pub saturation: f32,
}

/// Signal level, in DN above black, at which SNR reaches `target`.
///
/// Solving `s / sqrt(k1 * s + k2) = target` gives a quadratic in `s`:
/// `s^2 - target^2 * k1 * s - target^2 * k2 = 0`.
pub(crate) fn signal_at_snr(shot_slope: f32, read_variance: f32, target: f32) -> f32 {
    let t2 = target * target;
    let b = t2 * shot_slope;
    let c = t2 * read_variance;
    0.5 * (b + (b * b + 4.0 * c).max(0.0).sqrt())
}

/// Variance of independent noise recovered from a second difference.
const SECOND_DIFFERENCE_GAIN: f64 = 6.0;

/// Mean level and gradient-insensitive noise variance of one tile.
///
/// `rows` is the tile laid out row-major with `TILE` samples per row.
fn tile_statistics(rows: &[f32]) -> Option<(f32, f32)> {
    if rows.len() < TILE * 2 || TILE < 3 {
        return None;
    }

    let n = rows.len() as f64;
    let mean = rows.iter().map(|v| *v as f64).sum::<f64>() / n;

    // Second difference cancels constants and linear ramps. Texture is usually
    // anisotropic — foliage and fabric are far busier across one axis than the
    // other — so measure both directions and keep the quieter one.
    let mut horizontal = (0.0f64, 0u64);
    for row in rows.chunks_exact(TILE) {
        for window in row.windows(3) {
            let d = window[0] as f64 - 2.0 * window[1] as f64 + window[2] as f64;
            horizontal.0 += d * d;
            horizontal.1 += 1;
        }
    }

    let mut vertical = (0.0f64, 0u64);
    let height = rows.len() / TILE;
    for column in 0..TILE {
        for row in 1..height.saturating_sub(1) {
            let above = rows[(row - 1) * TILE + column] as f64;
            let here = rows[row * TILE + column] as f64;
            let below = rows[(row + 1) * TILE + column] as f64;
            let d = above - 2.0 * here + below;
            vertical.0 += d * d;
            vertical.1 += 1;
        }
    }

    if horizontal.1 == 0 && vertical.1 == 0 {
        return None;
    }

    let mean_square = |(sum, count): (f64, u64)| {
        if count == 0 {
            f64::INFINITY
        } else {
            sum / count as f64
        }
    };
    let quietest = mean_square(horizontal).min(mean_square(vertical));
    if !quietest.is_finite() {
        return None;
    }

    let variance = quietest / SECOND_DIFFERENCE_GAIN;
    Some((mean as f32, variance.max(0.0) as f32))
}

/// How a single colour plane is addressed within the sample buffer.
struct Plane {
    /// First row and column of the chosen phase.
    origin_y: usize,
    origin_x: usize,
    /// Distance between successive samples of the same colour.
    step_y: usize,
    step_x: usize,
    /// Component offset within a pixel, for already-demosaiced data.
    channel: usize,
}

/// Choose a green plane from the image's own CFA layout.
///
/// Green is the right choice on two counts: it carries the most signal, so the
/// fit gets the brightness range it needs, and a Bayer mosaic has twice as many
/// green sites as red or blue.
fn choose_plane(raw: &RawImage) -> Plane {
    let cpp = raw.cpp.max(1);

    match &raw.photometric {
        RawPhotometricInterpretation::Cfa(config) => {
            let cfa = &config.cfa;
            let (height, width) = (cfa.height.max(1), cfa.width.max(1));

            // First green site in the repeating pattern, else fall back to the
            // origin so an exotic layout still yields something.
            let mut origin = (0usize, 0usize);
            'search: for row in 0..height {
                for column in 0..width {
                    if cfa.cfa_color_at(row, column) == CFAColor::GREEN {
                        origin = (row, column);
                        break 'search;
                    }
                }
            }

            Plane {
                origin_y: origin.0,
                origin_x: origin.1,
                step_y: height,
                step_x: width,
                channel: 0,
            }
        }
        // Already demosaiced: read the green channel of every pixel.
        _ => Plane {
            origin_y: 0,
            origin_x: 0,
            step_y: 1,
            step_x: 1,
            channel: 1.min(cpp - 1),
        },
    }
}

/// Collect (mean, variance) pairs from tiles of a single colour plane.
fn collect_tiles(raw: &RawImage) -> Vec<(f32, f32)> {
    let samples: Vec<f32> = match &raw.data {
        RawImageData::Integer(values) => values.iter().map(|v| *v as f32).collect(),
        RawImageData::Float(values) => values.clone(),
    };

    let cpp = raw.cpp.max(1);
    let stride = raw.width * cpp;
    let plane = choose_plane(raw);

    let plane_width = raw.width.saturating_sub(plane.origin_x) / plane.step_x;
    let plane_height = raw.height.saturating_sub(plane.origin_y) / plane.step_y;
    if plane_width < TILE || plane_height < TILE {
        return Vec::new();
    }

    let mut tiles = Vec::new();
    let mut buffer = Vec::with_capacity(TILE * TILE);

    for tile_y in 0..plane_height / TILE {
        for tile_x in 0..plane_width / TILE {
            buffer.clear();
            for row in 0..TILE {
                let y = plane.origin_y + (tile_y * TILE + row) * plane.step_y;
                for column in 0..TILE {
                    let x = plane.origin_x + (tile_x * TILE + column) * plane.step_x;
                    let index = y * stride + x * cpp + plane.channel;
                    if let Some(value) = samples.get(index) {
                        buffer.push(*value);
                    }
                }
            }
            if let Some(statistics) = tile_statistics(&buffer) {
                tiles.push(statistics);
            }
        }
    }

    tiles
}

/// Fit `variance = shot_slope * signal + read_variance` to per-bin flat tiles.
///
/// Two departures from plain least squares, both because the data misbehaves:
///
/// * Bins hold equal *counts*, not equal brightness ranges. A frame with no
///   bright highlights puts every tile into the darkest few fixed-width bins.
/// * The fit is weighted by `1 / variance^2`. The sampling error of a variance
///   estimate grows with the variance itself, so unweighted least squares is
///   dominated by the brightest bins and drags the intercept negative.
///
/// The intercept is then clamped into `[0, darkest bin variance]`: the noise
/// floor cannot exceed the quietest thing actually measured, and cannot be
/// negative. That replaces fitting a parameter there is no data for with
/// measuring one.
fn fit(tiles: &[(f32, f32)], black: f32, saturation: f32) -> Option<(f32, f32, usize)> {
    if tiles.is_empty() {
        return None;
    }

    // Work in black-subtracted signal, and ignore anything at or near clipping:
    // saturated tiles have artificially low variance and would drag the fit down.
    let ceiling = saturation * 0.95;
    let mut usable: Vec<(f32, f32)> = tiles
        .iter()
        .map(|(mean, variance)| (mean - black, *variance))
        .filter(|(signal, variance)| {
            *signal > 0.0 && *signal < ceiling && variance.is_finite() && *variance > 0.0
        })
        .collect();

    if usable.len() < BINS * MIN_TILES_PER_BIN {
        return None;
    }

    usable.sort_by(|a, b| a.0.total_cmp(&b.0));

    // Equal-count bins; each contributes its median signal and the low quantile
    // of its variances, which is the flattest tile at that brightness.
    let per_bin = usable.len() / BINS;
    let mut points = Vec::with_capacity(BINS);
    let mut tiles_used = 0usize;

    for index in 0..BINS {
        let start = index * per_bin;
        let end = if index + 1 == BINS {
            usable.len()
        } else {
            start + per_bin
        };
        let slice = &usable[start..end];
        if slice.len() < MIN_TILES_PER_BIN {
            continue;
        }

        let signal = slice[slice.len() / 2].0;
        let mut variances: Vec<f32> = slice.iter().map(|(_, v)| *v).collect();
        variances.sort_by(f32::total_cmp);
        let position = ((variances.len() - 1) as f32 * FLAT_QUANTILE).round() as usize;
        points.push((signal, variances[position]));
        tiles_used += slice.len();
    }

    if points.len() < MIN_BINS {
        return None;
    }

    // Without real brightness leverage the slope is not determined. Say so
    // rather than reporting a number pulled out of a narrow range.
    let darkest = points.first()?;
    let brightest = points.last()?;
    if darkest.0 <= 0.0 || brightest.0 / darkest.0 < MIN_LEVERAGE {
        return None;
    }

    // Weighted least squares with weights 1 / variance^2.
    let (mut sw, mut swx, mut swy, mut swxx, mut swxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (signal, variance) in &points {
        let weight = 1.0 / ((*variance as f64) * (*variance as f64)).max(1.0e-12);
        let x = *signal as f64;
        let y = *variance as f64;
        sw += weight;
        swx += weight * x;
        swy += weight * y;
        swxx += weight * x * x;
        swxy += weight * x * y;
    }

    let denominator = sw * swxx - swx * swx;
    if denominator.abs() < 1.0e-12 {
        return None;
    }

    let slope = ((sw * swxy - swx * swy) / denominator) as f32;
    let intercept = ((swy * swxx - swx * swxy) / denominator) as f32;

    if !slope.is_finite() || !intercept.is_finite() {
        return None;
    }

    // The floor cannot be negative, nor higher than the quietest measurement.
    let read_variance = intercept.clamp(0.0, darkest.1);

    Some((slope.max(0.0), read_variance, tiles_used))
}

/// Scene EV, relative to middle grey, at which SNR reaches 1 and 10.
///
/// Shared by the per-frame fit and by pooled resolution in
/// [`crate::noiseprofile`], so both express the floor the same way.
pub(crate) fn crossings(shot_slope: f32, read_variance: f32, saturation: f32) -> (f32, f32) {
    let to_ev = |signal: f32| ((signal / saturation).max(1.0e-9) / crate::types::MID_GRAY).log2();
    (
        to_ev(signal_at_snr(shot_slope, read_variance, SNR_REPORTED[0])),
        to_ev(signal_at_snr(shot_slope, read_variance, SNR_REPORTED[1])),
    )
}

/// Estimate the noise model for `raw`, or `None` when the image gives too
/// little to fit.
pub fn estimate(raw: &RawImage) -> Option<NoiseEstimate> {
    let black = raw.blacklevel.as_vec();
    let white = raw.whitelevel.as_vec();
    let black = black.iter().sum::<f32>() / black.len().max(1) as f32;
    let white = white.iter().sum::<f32>() / white.len().max(1) as f32;
    let saturation = white - black;
    if !saturation.is_finite() || saturation <= 0.0 {
        return None;
    }

    let tiles = collect_tiles(raw);
    let (shot_slope, read_variance, tiles_used) = fit(&tiles, black, saturation)?;

    let (snr1_ev, snr10_ev) = crossings(shot_slope, read_variance, saturation);

    Some(NoiseEstimate {
        shot_slope,
        read_variance,
        snr1_ev,
        snr10_ev,
        tiles_used,
        saturation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snr_crossing_is_consistent_with_the_model() {
        let (slope, floor) = (2.0f32, 100.0f32);
        for target in [1.0f32, 4.0, 10.0] {
            let signal = signal_at_snr(slope, floor, target);
            let achieved = signal / (slope * signal + floor).sqrt();
            assert!(
                (achieved - target).abs() < 1.0e-3,
                "target {target}: got {achieved}"
            );
        }
    }

    /// With no shot component the model is pure read noise, so the SNR=1 point
    /// is exactly the noise standard deviation.
    #[test]
    fn read_noise_only_crosses_at_its_own_sigma() {
        let signal = signal_at_snr(0.0, 64.0, 1.0);
        assert!((signal - 8.0).abs() < 1.0e-3, "got {signal}");
    }

    #[test]
    fn brighter_signal_is_needed_for_higher_snr() {
        let low = signal_at_snr(1.5, 50.0, 1.0);
        let high = signal_at_snr(1.5, 50.0, 10.0);
        assert!(high > low * 10.0);
    }

    /// The whole point of the second difference: a smooth ramp must read as
    /// zero noise, where plain variance would report a large number.
    #[test]
    fn a_linear_ramp_reads_as_no_noise() {
        let ramp: Vec<f32> = (0..TILE * TILE).map(|i| (i % TILE) as f32 * 10.0).collect();
        let (mean, variance) = tile_statistics(&ramp).unwrap();
        assert!(mean > 0.0);
        assert!(variance < 1.0e-6, "ramp reported variance {variance}");
    }

    /// A known alternating pattern has an exactly computable second difference.
    /// Alternating by index makes every row and every column alternate, so both
    /// directions agree and the minimum is unambiguous.
    #[test]
    fn alternating_pattern_recovers_its_second_difference() {
        // +/-1 alternating: d = 1 - 2*(-1) + 1 = 4 at every position, so
        // mean(d^2) = 16 and the reported variance is 16/6.
        let pattern: Vec<f32> = (0..TILE * TILE)
            .map(|i| {
                let (row, column) = (i / TILE, i % TILE);
                if (row + column) % 2 == 0 { 1.0 } else { -1.0 }
            })
            .collect();
        let (_, variance) = tile_statistics(&pattern).unwrap();
        assert!((variance - 16.0 / 6.0).abs() < 1.0e-4, "got {variance}");
    }

    /// Vertical stripes are busy across rows but perfectly smooth down columns,
    /// so taking the quieter direction must report almost no noise.
    #[test]
    fn anisotropic_texture_is_measured_on_its_quiet_axis() {
        let stripes: Vec<f32> = (0..TILE * TILE)
            .map(|i| if (i % TILE) % 2 == 0 { 0.0 } else { 500.0 })
            .collect();
        let (_, variance) = tile_statistics(&stripes).unwrap();
        assert!(variance < 1.0e-6, "stripes reported variance {variance}");
    }

    #[test]
    fn tiny_tiles_are_rejected() {
        assert!(tile_statistics(&[1.0, 2.0]).is_none());
    }

    /// A synthetic field obeying the model must be recovered by the fit.
    #[test]
    fn fit_recovers_planted_parameters() {
        let (slope, floor) = (3.0f32, 200.0f32);
        let mut tiles = Vec::new();
        // Deterministic pseudo-noise so the test does not depend on an RNG.
        let mut state = 12345u32;
        let mut next = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / 16777216.0
        };
        for bin in 0..BINS {
            let signal = (bin as f32 + 0.5) * (1000.0 / BINS as f32);
            for _ in 0..MIN_TILES_PER_BIN * 3 {
                let expected = slope * signal + floor;
                // Spread above the true variance, as texture would do; the low
                // quantile should still land near the truth.
                let inflation = 1.0 + 1.5 * next();
                tiles.push((signal, expected * inflation));
            }
        }
        let (fitted_slope, fitted_floor, used) = fit(&tiles, 0.0, 1200.0).unwrap();
        assert!(used > 0);
        assert!(
            (fitted_slope - slope).abs() / slope < 0.35,
            "slope {fitted_slope} vs {slope}"
        );
        assert!(
            (fitted_floor - floor).abs() / floor < 0.6,
            "floor {fitted_floor} vs {floor}"
        );
    }

    #[test]
    fn too_little_data_yields_no_estimate() {
        assert!(fit(&[], 0.0, 100.0).is_none());
        assert!(fit(&[(10.0, 20.0); 5], 0.0, 100.0).is_none());
    }
}
