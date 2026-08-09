//! Experimental pre-demosaic highlight reconstruction.
//!
//! These methods are deliberately opt-in.  They work on the normalized CFA
//! mosaic, where a saturated reading is a lower bound and where measured sites
//! can still be kept exact.  `RawPyramid` is the small, clipping-aware baseline;
//! `Harmonic` adds connected-region colour-line fitting and an obstacle-
//! constrained harmonic solve.  Neither path changes the unattended `Current`
//! estimator.

use anyhow::{Result, ensure};
use clap::ValueEnum;
use faer::Side;
use faer::prelude::{Col, Solve, SparseColMat};
use faer::sparse::Triplet;
use rawler::CFA;
use rawler::cfa::CFAColor;
use serde::Serialize;
use std::collections::VecDeque;

const VALID_CONFIDENCE_MAX: f32 = 0.5;
const EPSILON: f32 = 1.0e-8;
const PYRAMID_KERNEL: [f32; 5] = [1.0, 4.0, 6.0, 4.0, 1.0];
const HARMONIC_SWEEPS: usize = 240;
const HARMONIC_POLISH_SWEEPS: usize = 60;
const DIRECT_SOLVE_MAX_UNKNOWNS: usize = 1 << 14;
const GUIDE_K: f32 = 0.15;
const WEIGHT_FLOOR: f32 = 1.0e-4;
const KNEE_LOW: f32 = 0.80;
const KNEE_HIGH: f32 = 0.995;
const KNEE_BINS: usize = 24;
const KNEE_MIN_VOTES: usize = 100;

/// Highlight estimator selected by the batch CLI.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HighlightMethod {
    /// Existing post-demosaic, per-pixel estimator.
    #[default]
    Current,
    /// Clipping-aware multiscale colour reconstruction on Bayer quads.
    RawPyramid,
    /// Region-wise colour-line transport plus an obstacle harmonic solve.
    Harmonic,
}

impl HighlightMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::RawPyramid => "raw_pyramid",
            Self::Harmonic => "harmonic",
        }
    }

    pub const fn is_spatial(self) -> bool {
        !matches!(self, Self::Current)
    }

    /// Conservative transient reserve until the 24 MP RSS gate is rerun.
    pub const fn extra_bytes_per_pixel(self) -> u64 {
        match self {
            Self::Current => 0,
            Self::RawPyramid => 24,
            Self::Harmonic => 48,
        }
    }
}

/// What a pre-demosaic estimator did before its counters are folded into the
/// public highlight report.
#[derive(Debug, Default, Clone, Serialize)]
pub struct RawHighlightReport {
    pub method: HighlightMethod,
    pub clipped_cfa_sites: usize,
    pub reconstructed_cfa_sites: usize,
    pub connected_regions: usize,
    pub knee_corrected_sites: usize,
    pub mean_fit_quality: f32,
    pub fully_clipped_cores: usize,
    pub solver_fallbacks: usize,
    pub max_lift: f32,
}

#[derive(Clone)]
struct Grid {
    width: usize,
    height: usize,
    value: Vec<[f32; 3]>,
    floor: Vec<[f32; 3]>,
    valid: Vec<[bool; 3]>,
}

#[derive(Clone)]
struct PyramidLevel {
    width: usize,
    height: usize,
    value: Vec<[f32; 3]>,
    weight: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, Copy)]
struct LineFit {
    guides: [usize; 2],
    slopes: [f32; 2],
    guide_count: usize,
    intercept: f32,
    r2: f32,
    trusted_mass: f32,
}

#[derive(Default)]
struct Region {
    cells: Vec<usize>,
    boundary: Vec<usize>,
}

#[inline]
fn channel(color: CFAColor) -> Option<usize> {
    match color {
        CFAColor::RED => Some(0),
        CFAColor::GREEN => Some(1),
        CFAColor::BLUE => Some(2),
        _ => None,
    }
}

#[inline]
fn mirror(index: isize, length: usize) -> usize {
    if length <= 1 {
        return 0;
    }
    let period = (2 * length - 2) as isize;
    let folded = index.rem_euclid(period);
    if folded < length as isize {
        folded as usize
    } else {
        (period - folded) as usize
    }
}

#[inline]
fn confidence_at(samples: &[f32], confidence: Option<&[f32]>, index: usize) -> f32 {
    confidence
        .map(|map| map[index])
        .unwrap_or_else(|| crate::highlight::clip_confidence(samples[index]))
        .clamp(0.0, 1.0)
}

fn build_grid(
    samples: &[f32],
    confidence: Option<&[f32]>,
    width: usize,
    height: usize,
    cfa: &CFA,
) -> Grid {
    let grid_width = width.div_ceil(2);
    let grid_height = height.div_ceil(2);
    let mut sum = vec![[0.0_f32; 3]; grid_width * grid_height];
    let mut floor = vec![[0.0_f32; 3]; grid_width * grid_height];
    let mut trusted_sum = vec![[0.0_f32; 3]; grid_width * grid_height];
    let mut count = vec![[0_u8; 3]; grid_width * grid_height];
    let mut trusted_count = vec![[0_u8; 3]; grid_width * grid_height];

    for y in 0..height {
        for x in 0..width {
            let source = y * width + x;
            let Some(c) = channel(cfa.cfa_color_at(y, x)) else {
                continue;
            };
            let cell = (y / 2) * grid_width + x / 2;
            let value = samples[source];
            sum[cell][c] += value;
            floor[cell][c] = floor[cell][c].max(value);
            count[cell][c] += 1;
            if confidence_at(samples, confidence, source) < VALID_CONFIDENCE_MAX {
                trusted_sum[cell][c] += value;
                trusted_count[cell][c] += 1;
            }
        }
    }

    let mut value = vec![[0.0_f32; 3]; grid_width * grid_height];
    let mut valid = vec![[false; 3]; grid_width * grid_height];
    for i in 0..value.len() {
        for c in 0..3 {
            if trusted_count[i][c] > 0 {
                value[i][c] = trusted_sum[i][c] / f32::from(trusted_count[i][c]);
                valid[i][c] = true;
            } else if count[i][c] > 0 {
                value[i][c] = sum[i][c] / f32::from(count[i][c]);
            }
        }
    }

    Grid {
        width: grid_width,
        height: grid_height,
        value,
        floor,
        valid,
    }
}

fn base_level(grid: &Grid) -> PyramidLevel {
    PyramidLevel {
        width: grid.width,
        height: grid.height,
        value: grid.value.clone(),
        weight: grid
            .valid
            .iter()
            .map(|valid| valid.map(|v| if v { 1.0 } else { 0.0 }))
            .collect(),
    }
}

/// Five-tap, mirrored, clipping-aware Gaussian reduction.  Invalid samples do
/// not enter either the numerator or denominator.
fn reduce(previous: &PyramidLevel) -> PyramidLevel {
    let width = previous.width.div_ceil(2);
    let height = previous.height.div_ceil(2);
    let mut value = vec![[0.0_f32; 3]; width * height];
    let mut weight = vec![[0.0_f32; 3]; width * height];

    for y in 0..height {
        for x in 0..width {
            let out = y * width + x;
            for (ky, &kernel_y) in PYRAMID_KERNEL.iter().enumerate() {
                let sy = mirror(2 * y as isize + ky as isize - 2, previous.height);
                for (kx, &kernel_x) in PYRAMID_KERNEL.iter().enumerate() {
                    let sx = mirror(2 * x as isize + kx as isize - 2, previous.width);
                    let source = sy * previous.width + sx;
                    let kernel = kernel_y * kernel_x;
                    for c in 0..3 {
                        let w = kernel * previous.weight[source][c];
                        value[out][c] += previous.value[source][c] * w;
                        weight[out][c] += w;
                    }
                }
            }
            for c in 0..3 {
                if weight[out][c] > EPSILON {
                    value[out][c] /= weight[out][c];
                    // A propagated value has one vote at the next scale.  The
                    // absolute kernel mass is irrelevant after renormalisation.
                    weight[out][c] = 1.0;
                }
            }
        }
    }

    PyramidLevel {
        width,
        height,
        value,
        weight,
    }
}

fn build_pyramid(grid: &Grid) -> Vec<PyramidLevel> {
    let mut levels = vec![base_level(grid)];
    while levels
        .last()
        .is_some_and(|level| level.width > 1 || level.height > 1)
    {
        let next = reduce(levels.last().expect("a base level exists"));
        levels.push(next);
    }
    levels
}

fn knee_reference_line(grid: &Grid, target: usize, guide: usize) -> Option<LineFit> {
    let pairs: Vec<(f32, f32)> = grid
        .value
        .iter()
        .zip(&grid.valid)
        .filter(|(value, valid)| {
            valid[target]
                && valid[guide]
                && (0.45..0.78).contains(&value[target])
                && value[guide] < KNEE_LOW
        })
        .map(|(value, _)| (value[guide], value[target]))
        .collect();
    if pairs.len() < KNEE_MIN_VOTES {
        return None;
    }
    let n = pairs.len() as f64;
    let mean_x = pairs.iter().map(|p| f64::from(p.0)).sum::<f64>() / n;
    let mean_y = pairs.iter().map(|p| f64::from(p.1)).sum::<f64>() / n;
    let var_x = pairs
        .iter()
        .map(|p| (f64::from(p.0) - mean_x).powi(2))
        .sum::<f64>();
    let var_y = pairs
        .iter()
        .map(|p| (f64::from(p.1) - mean_y).powi(2))
        .sum::<f64>();
    if var_x <= 1.0e-12 || var_y <= 1.0e-12 {
        return None;
    }
    let covariance = pairs
        .iter()
        .map(|p| (f64::from(p.0) - mean_x) * (f64::from(p.1) - mean_y))
        .sum::<f64>();
    let slope = covariance / (var_x + 1.0e-3 * var_x / n);
    let intercept = mean_y - slope * mean_x;
    let residual = pairs
        .iter()
        .map(|p| (f64::from(p.1) - (slope * f64::from(p.0) + intercept)).powi(2))
        .sum::<f64>();
    let r2 = (1.0 - residual / var_y).clamp(0.0, 1.0) as f32;
    (r2 > 0.85 && slope.is_finite() && slope.abs() < 64.0).then_some(LineFit {
        guides: [guide, guide],
        slopes: [slope as f32, 0.0],
        guide_count: 1,
        intercept: intercept as f32,
        r2,
        trusted_mass: 1.0,
    })
}

/// Infer a raise-only inverse for a smooth sensor shoulder. Hard-clipped sites
/// are deliberately excluded; this stage may recover roll-off but cannot
/// manufacture information beyond the white level.
fn apply_sensor_knee(samples: &mut [f32], width: usize, height: usize, cfa: &CFA) -> usize {
    let original = samples.to_vec();
    let grid = build_grid(&original, None, width, height, cfa);
    let mut lifts = [[0.0_f32; KNEE_BINS]; 3];
    let mut accepted = [[false; KNEE_BINS]; 3];

    for target in 0..3 {
        let Some(line) = (0..3)
            .filter(|&guide| guide != target)
            .filter_map(|guide| knee_reference_line(&grid, target, guide))
            .max_by(|a, b| a.r2.total_cmp(&b.r2))
        else {
            continue;
        };
        let guide = line.guides[0];
        let mut votes: [Vec<f32>; KNEE_BINS] = std::array::from_fn(|_| Vec::new());
        for value in &grid.value {
            if !(KNEE_LOW..KNEE_HIGH).contains(&value[target]) || value[guide] >= KNEE_HIGH {
                continue;
            }
            let bin = (((value[target] - KNEE_LOW) / (KNEE_HIGH - KNEE_LOW) * KNEE_BINS as f32)
                .floor() as usize)
                .min(KNEE_BINS - 1);
            let predicted = line.slopes[0] * value[guide] + line.intercept;
            let lift = predicted - value[target];
            if lift.is_finite() {
                votes[bin].push(lift);
            }
        }
        let mut previous = KNEE_LOW;
        for (bin, bin_votes) in votes.iter_mut().enumerate() {
            if bin_votes.len() < KNEE_MIN_VOTES {
                continue;
            }
            bin_votes.sort_by(f32::total_cmp);
            let median = bin_votes[bin_votes.len() / 2];
            let mut deviations: Vec<f32> = bin_votes
                .iter()
                .map(|value| (value - median).abs())
                .collect();
            deviations.sort_by(f32::total_cmp);
            let mad = deviations[deviations.len() / 2];
            let standard_error = 1.4826 * mad / (bin_votes.len() as f32).sqrt();
            if median <= (2.0 * standard_error).max(1.0e-5) {
                continue;
            }
            let center = KNEE_LOW + (bin as f32 + 0.5) * (KNEE_HIGH - KNEE_LOW) / KNEE_BINS as f32;
            let corrected = (center + median).max(previous);
            lifts[target][bin] = corrected - center;
            accepted[target][bin] = true;
            previous = corrected;
        }
    }

    let mut corrected_sites = 0_usize;
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            let Some(c) = channel(cfa.cfa_color_at(y, x)) else {
                continue;
            };
            let value = original[i];
            if !(KNEE_LOW..KNEE_HIGH).contains(&value) {
                continue;
            }
            let bin = (((value - KNEE_LOW) / (KNEE_HIGH - KNEE_LOW) * KNEE_BINS as f32).floor()
                as usize)
                .min(KNEE_BINS - 1);
            if accepted[c][bin] {
                let corrected = value + lifts[c][bin];
                if corrected > samples[i] {
                    samples[i] = corrected;
                    corrected_sites += 1;
                }
            }
        }
    }
    corrected_sites
}

fn coarse_color(levels: &[PyramidLevel], x: usize, y: usize) -> [f32; 3] {
    let mut answer = [0.0_f32; 3];
    let mut found = [false; 3];
    for (depth, level) in levels.iter().enumerate().skip(1) {
        let lx = (x >> depth).min(level.width - 1);
        let ly = (y >> depth).min(level.height - 1);
        let i = ly * level.width + lx;
        for c in 0..3 {
            if !found[c] && level.weight[i][c] > 0.0 {
                answer[c] = level.value[i][c];
                found[c] = true;
            }
        }
        if found == [true; 3] {
            break;
        }
    }
    answer
}

fn pyramid_prediction(grid: &Grid, levels: &[PyramidLevel]) -> Vec<[f32; 3]> {
    let mut prediction = grid.value.clone();
    for y in 0..grid.height {
        for x in 0..grid.width {
            let i = y * grid.width + x;
            if grid.valid[i] == [true; 3] {
                continue;
            }
            let low = coarse_color(levels, x, y);
            let mut ratios = [0.0_f32; 3];
            let mut ratio_count = 0;
            for (c, low_channel) in low.iter().copied().enumerate() {
                if grid.valid[i][c] && low_channel > EPSILON {
                    ratios[ratio_count] = (grid.value[i][c] / low_channel).max(0.0);
                    ratio_count += 1;
                }
            }
            let scale = match ratio_count {
                0 => (0..3)
                    .filter(|&c| low[c] > EPSILON)
                    .map(|c| grid.floor[i][c] / low[c])
                    .fold(1.0_f32, f32::max),
                1 => ratios[0],
                2 => (ratios[0] + ratios[1]) * 0.5,
                _ => {
                    ratios.sort_by(f32::total_cmp);
                    ratios[1]
                }
            };
            for (c, low_channel) in low.iter().copied().enumerate() {
                if !grid.valid[i][c] {
                    prediction[i][c] = (low_channel * scale).max(grid.floor[i][c]);
                }
            }
        }
    }
    prediction
}

fn regions(grid: &Grid) -> Vec<Region> {
    let mut seen = vec![false; grid.value.len()];
    let mut output = Vec::new();
    for seed in 0..grid.value.len() {
        if seen[seed] || grid.valid[seed] == [true; 3] {
            continue;
        }
        seen[seed] = true;
        let mut queue = VecDeque::from([seed]);
        let mut region = Region::default();
        while let Some(i) = queue.pop_front() {
            region.cells.push(i);
            let x = i % grid.width;
            let y = i / grid.width;
            let mut touches_boundary = false;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx < 0 || ny < 0 || nx >= grid.width as isize || ny >= grid.height as isize {
                        continue;
                    }
                    let n = ny as usize * grid.width + nx as usize;
                    if grid.valid[n] == [true; 3] {
                        touches_boundary = true;
                    } else if !seen[n] {
                        seen[n] = true;
                        queue.push_back(n);
                    }
                }
            }
            if touches_boundary {
                region.boundary.push(i);
            }
        }
        output.push(region);
    }
    output
}

fn fit_line(grid: &Grid, region: &Region, target: usize, guide: usize) -> Option<LineFit> {
    let mut pairs = Vec::new();
    let mut min_x = grid.width;
    let mut min_y = grid.height;
    let mut max_x = 0_usize;
    let mut max_y = 0_usize;
    for &i in &region.cells {
        let x = i % grid.width;
        let y = i / grid.width;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    // The colour line needs an intensity baseline, not just one boundary
    // column. Sixteen Bayer quads is the published minimum guide scale and is
    // still local enough not to turn a landscape's vegetation into a sky prior.
    const PAD: usize = 16;
    min_x = min_x.saturating_sub(PAD);
    min_y = min_y.saturating_sub(PAD);
    max_x = (max_x + PAD).min(grid.width - 1);
    max_y = (max_y + PAD).min(grid.height - 1);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let outside = y * grid.width + x;
            if grid.valid[outside][target] && grid.valid[outside][guide] {
                pairs.push((grid.value[outside][guide], grid.value[outside][target]));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pairs.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    if pairs.len() < 4 {
        return None;
    }
    let n = pairs.len() as f64;
    let mean_x = pairs.iter().map(|p| f64::from(p.0)).sum::<f64>() / n;
    let mean_y = pairs.iter().map(|p| f64::from(p.1)).sum::<f64>() / n;
    let var_x = pairs
        .iter()
        .map(|p| (f64::from(p.0) - mean_x).powi(2))
        .sum::<f64>();
    let var_y = pairs
        .iter()
        .map(|p| (f64::from(p.1) - mean_y).powi(2))
        .sum::<f64>();
    if var_x <= 1.0e-12 || var_y <= 1.0e-12 {
        return None;
    }
    let covariance = pairs
        .iter()
        .map(|p| (f64::from(p.0) - mean_x) * (f64::from(p.1) - mean_y))
        .sum::<f64>();
    let ridge = 1.0e-3 * var_x / n;
    let slope = covariance / (var_x + ridge);
    if !slope.is_finite() || slope.abs() >= 64.0 {
        return None;
    }
    let intercept = mean_y - slope * mean_x;
    let residual = pairs
        .iter()
        .map(|p| (f64::from(p.1) - (slope * f64::from(p.0) + intercept)).powi(2))
        .sum::<f64>();
    let r2 = (1.0 - residual / var_y).clamp(0.0, 1.0) as f32;
    let trusted_mass = region
        .cells
        .iter()
        .filter(|&&i| grid.valid[i][guide])
        .count() as f32
        / region.cells.len().max(1) as f32;
    (r2 > 0.25).then_some(LineFit {
        guides: [guide, guide],
        slopes: [slope as f32, 0.0],
        guide_count: 1,
        intercept: intercept as f32,
        r2,
        trusted_mass,
    })
}

fn fit_two_guides(grid: &Grid, region: &Region, target: usize) -> Option<LineFit> {
    let guides: Vec<usize> = (0..3).filter(|&c| c != target).collect();
    let [g0, g1] = [guides[0], guides[1]];
    let mut min_x = grid.width;
    let mut min_y = grid.height;
    let mut max_x = 0_usize;
    let mut max_y = 0_usize;
    for &i in &region.cells {
        let x = i % grid.width;
        let y = i / grid.width;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    const PAD: usize = 16;
    min_x = min_x.saturating_sub(PAD);
    min_y = min_y.saturating_sub(PAD);
    max_x = (max_x + PAD).min(grid.width - 1);
    max_y = (max_y + PAD).min(grid.height - 1);
    let mut samples = Vec::new();
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let i = y * grid.width + x;
            if grid.valid[i][target] && grid.valid[i][g0] && grid.valid[i][g1] {
                samples.push((grid.value[i][g0], grid.value[i][g1], grid.value[i][target]));
            }
        }
    }
    if samples.len() < 8 {
        return None;
    }
    let n = samples.len() as f64;
    let mean0 = samples.iter().map(|v| f64::from(v.0)).sum::<f64>() / n;
    let mean1 = samples.iter().map(|v| f64::from(v.1)).sum::<f64>() / n;
    let mean_y = samples.iter().map(|v| f64::from(v.2)).sum::<f64>() / n;
    let mut c00 = 0.0_f64;
    let mut c01 = 0.0_f64;
    let mut c11 = 0.0_f64;
    let mut cy0 = 0.0_f64;
    let mut cy1 = 0.0_f64;
    let mut var_y = 0.0_f64;
    for &(x0, x1, y) in &samples {
        let x0 = f64::from(x0) - mean0;
        let x1 = f64::from(x1) - mean1;
        let y = f64::from(y) - mean_y;
        c00 += x0 * x0;
        c01 += x0 * x1;
        c11 += x1 * x1;
        cy0 += x0 * y;
        cy1 += x1 * y;
        var_y += y * y;
    }
    if var_y <= 1.0e-12 {
        return None;
    }
    let ridge = 1.0e-3 * (c00 + c11) * 0.5 / n;
    c00 += ridge;
    c11 += ridge;
    let determinant = c00 * c11 - c01 * c01;
    if determinant.abs() <= 1.0e-15 {
        return None;
    }
    let slope0 = (cy0 * c11 - cy1 * c01) / determinant;
    let slope1 = (cy1 * c00 - cy0 * c01) / determinant;
    if !slope0.is_finite() || !slope1.is_finite() || slope0.abs() >= 64.0 || slope1.abs() >= 64.0 {
        return None;
    }
    let intercept = mean_y - slope0 * mean0 - slope1 * mean1;
    let residual = samples
        .iter()
        .map(|&(x0, x1, y)| {
            (f64::from(y) - (slope0 * f64::from(x0) + slope1 * f64::from(x1) + intercept)).powi(2)
        })
        .sum::<f64>();
    let r2 = (1.0 - residual / var_y).clamp(0.0, 1.0) as f32;
    let trusted_mass = region
        .cells
        .iter()
        .filter(|&&i| grid.valid[i][g0] && grid.valid[i][g1])
        .count() as f32
        / region.cells.len().max(1) as f32;
    (r2 > 0.25).then_some(LineFit {
        guides: [g0, g1],
        slopes: [slope0 as f32, slope1 as f32],
        guide_count: 2,
        intercept: intercept as f32,
        r2,
        trusted_mass,
    })
}

fn best_fits(grid: &Grid, region: &Region) -> [Option<LineFit>; 3] {
    std::array::from_fn(|target| {
        (0..3)
            .filter(|&guide| guide != target)
            .filter_map(|guide| fit_line(grid, region, target, guide))
            .chain(fit_two_guides(grid, region, target))
            .max_by(|a, b| {
                let score = |fit: &LineFit| fit.r2 * (0.25 + 0.75 * fit.trusted_mass);
                score(a).total_cmp(&score(b))
            })
    })
}

fn depth_map(grid: &Grid, region: &Region) -> Vec<f32> {
    let mut depth = vec![f32::INFINITY; grid.value.len()];
    let mut queue = VecDeque::new();
    for &i in &region.boundary {
        depth[i] = 1.0;
        queue.push_back(i);
    }
    while let Some(i) = queue.pop_front() {
        let x = i % grid.width;
        let y = i / grid.width;
        let next = depth[i] + 1.0;
        for (dx, dy) in [(1_isize, 0_isize), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as isize + dx;
            let ny = y as isize + dy;
            if nx < 0 || ny < 0 || nx >= grid.width as isize || ny >= grid.height as isize {
                continue;
            }
            let n = ny as usize * grid.width + nx as usize;
            if next < depth[n] && region.cells.binary_search(&n).is_ok() {
                depth[n] = next;
                queue.push_back(n);
            }
        }
    }
    depth
}

fn fully_clipped_components(grid: &Grid, region: &Region) -> Vec<Region> {
    let mut in_region = vec![false; grid.value.len()];
    for &i in &region.cells {
        in_region[i] = true;
    }
    let mut seen = vec![false; grid.value.len()];
    let mut cores = Vec::new();
    for &seed in &region.cells {
        if seen[seed] || grid.valid[seed] != [false; 3] {
            continue;
        }
        seen[seed] = true;
        let mut queue = VecDeque::from([seed]);
        let mut core = Region::default();
        while let Some(i) = queue.pop_front() {
            core.cells.push(i);
            let x = i % grid.width;
            let y = i / grid.width;
            let mut boundary = false;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx < 0 || ny < 0 || nx >= grid.width as isize || ny >= grid.height as isize {
                        continue;
                    }
                    let n = ny as usize * grid.width + nx as usize;
                    if !in_region[n] || grid.valid[n] != [false; 3] {
                        boundary = true;
                    } else if !seen[n] {
                        seen[n] = true;
                        queue.push_back(n);
                    }
                }
            }
            if boundary {
                core.boundary.push(i);
            }
        }
        core.cells.sort_unstable();
        cores.push(core);
    }
    cores
}

fn dome_model(grid: &Grid, region: &Region, current: &[[f32; 3]]) -> (f32, f32) {
    let mut slopes = Vec::new();
    let mut rim_levels = Vec::new();
    for &inside in &region.boundary {
        let x = inside % grid.width;
        let y = inside / grid.width;
        for (dx, dy) in [(1_isize, 0_isize), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as isize + dx;
            let ny = y as isize + dy;
            if nx < 0 || ny < 0 || nx >= grid.width as isize || ny >= grid.height as isize {
                continue;
            }
            let outside = ny as usize * grid.width + nx as usize;
            if region.cells.binary_search(&outside).is_err() {
                let outer = current[outside].iter().sum::<f32>() / 3.0;
                let nx2 = nx + dx;
                let ny2 = ny + dy;
                let next_outer = if nx2 >= 0
                    && ny2 >= 0
                    && nx2 < grid.width as isize
                    && ny2 < grid.height as isize
                {
                    let n2 = ny2 as usize * grid.width + nx2 as usize;
                    current[n2].iter().sum::<f32>() / 3.0
                } else {
                    outer
                };
                let slope = outer - next_outer;
                if slope.is_finite() && slope > 0.0 {
                    slopes.push(slope);
                    rim_levels.push(outer + slope);
                }
            }
        }
    }
    if slopes.is_empty() {
        let floor = region
            .boundary
            .iter()
            .map(|&i| grid.floor[i].iter().sum::<f32>() / 3.0)
            .fold(0.0_f32, f32::max);
        (floor, 0.0)
    } else {
        slopes.sort_by(f32::total_cmp);
        rim_levels.sort_by(f32::total_cmp);
        (
            rim_levels[rim_levels.len() / 2],
            slopes[slopes.len() / 2].clamp(0.0, 0.25),
        )
    }
}

fn fit_data(line: LineFit, current: &[[f32; 3]], index: usize) -> f32 {
    line.intercept
        + line.slopes[0] * current[index][line.guides[0]]
        + if line.guide_count == 2 {
            line.slopes[1] * current[index][line.guides[1]]
        } else {
            0.0
        }
}

/// Solve the pinned harmonic system directly for a bounded region.  Only the
/// lower triangle is assembled, and all arithmetic handed to the sparse
/// factorization is f64.  Obstacle projection is applied when the solution is
/// copied back.
fn solve_region_direct(
    grid: &Grid,
    region: &Region,
    fits: [Option<LineFit>; 3],
    current: &mut [[f32; 3]],
) -> bool {
    if region.cells.len() > DIRECT_SOLVE_MAX_UNKNOWNS {
        return false;
    }
    let mut candidate = current.to_vec();
    let mut channels = [0_usize, 1, 2];
    // Channels with the most missing data are evaluated last so they can use
    // the already solved guides.
    channels.sort_by_key(|&target| {
        region
            .cells
            .iter()
            .filter(|&&i| !grid.valid[i][target])
            .count()
    });

    for target in channels {
        let unknown: Vec<usize> = region
            .cells
            .iter()
            .copied()
            .filter(|&i| !grid.valid[i][target])
            .collect();
        if unknown.is_empty() {
            continue;
        }
        if unknown.len() > DIRECT_SOLVE_MAX_UNKNOWNS {
            return false;
        }
        let mut row_of = vec![usize::MAX; grid.value.len()];
        for (row, &cell) in unknown.iter().enumerate() {
            row_of[cell] = row;
        }
        let mut triplets = Vec::with_capacity(unknown.len() * 3);
        let mut rhs = vec![0.0_f64; unknown.len()];
        for (row, &i) in unknown.iter().enumerate() {
            let x = i % grid.width;
            let y = i / grid.width;
            let line = fits[target];
            let guide = line.map_or_else(
                || {
                    (0..3)
                        .filter(|&c| c != target)
                        .max_by(|&a, &b| candidate[i][a].total_cmp(&candidate[i][b]))
                        .unwrap_or((target + 1) % 3)
                },
                |fit| fit.guides[0],
            );
            let guide_scale = candidate[i][guide].abs().max(0.05);
            let mut diagonal = 1.0e-8_f64;
            for (dx, dy) in [(1_isize, 0_isize), (-1, 0), (0, 1), (0, -1)] {
                let nx = mirror(x as isize + dx, grid.width);
                let ny = mirror(y as isize + dy, grid.height);
                let n = ny * grid.width + nx;
                if n == i {
                    continue;
                }
                let delta = (candidate[i][guide] - candidate[n][guide]).abs();
                let weight = (-(delta / (GUIDE_K * guide_scale)).powi(2))
                    .exp()
                    .max(WEIGHT_FLOOR);
                diagonal += f64::from(weight);
                let other_row = row_of[n];
                if other_row == usize::MAX {
                    rhs[row] += f64::from(weight * candidate[n][target]);
                } else if other_row < row {
                    triplets.push(Triplet::new(row, other_row, -f64::from(weight)));
                }
            }
            if let Some(line) = line {
                let weight = 8.0 * line.r2 / (1.05 - line.r2).max(0.05);
                diagonal += f64::from(weight);
                rhs[row] +=
                    f64::from(weight * fit_data(line, &candidate, i).max(grid.floor[i][target]));
            }
            triplets.push(Triplet::new(row, row, diagonal));
        }

        let Ok(matrix) = SparseColMat::<usize, f64>::try_new_from_triplets(
            unknown.len(),
            unknown.len(),
            &triplets,
        ) else {
            return false;
        };
        let Ok(cholesky) = matrix.sp_cholesky(Side::Lower) else {
            return false;
        };
        let rhs = Col::from_fn(unknown.len(), |row| rhs[row]);
        let solution = cholesky.solve(&rhs);
        for (row, &i) in unknown.iter().enumerate() {
            let value = solution[row] as f32;
            if !value.is_finite() {
                return false;
            }
            candidate[i][target] = value.max(grid.floor[i][target]);
        }
    }

    for &i in &region.cells {
        current[i] = candidate[i];
    }
    true
}

fn keep_unfitted_partial_cells(
    grid: &Grid,
    region: &Region,
    fits: [Option<LineFit>; 3],
    current: &mut [[f32; 3]],
) {
    for &i in &region.cells {
        if grid.valid[i] == [false; 3] {
            continue;
        }
        for (target, fit) in fits.iter().enumerate() {
            if !grid.valid[i][target] && fit.is_none() {
                // A partially clipped coloured highlight still has a measured
                // hue. Without a trusted line, its saturated sample is the
                // only defensible lower bound; a spatial guess would import a
                // neighbour's colour.
                current[i][target] = grid.floor[i][target];
            }
        }
    }
}

fn apply_luminance_domes(grid: &Grid, cores: &[Region], current: &mut [[f32; 3]]) {
    for core in cores {
        let depth = depth_map(grid, core);
        let (rim, slope) = dome_model(grid, core, current);
        for &i in &core.cells {
            let old_luma = current[i].iter().sum::<f32>() / 3.0;
            let floor_luma = grid.floor[i].iter().sum::<f32>() / 3.0;
            let target_luma = (rim + slope * (depth[i] - 1.0).max(0.0)).max(floor_luma);
            let scale = target_luma / old_luma.max(EPSILON);
            for c in 0..3 {
                current[i][c] = (current[i][c] * scale).max(grid.floor[i][c]);
            }
        }
    }
}

fn harmonic_prediction(
    grid: &Grid,
    pyramid: &[[f32; 3]],
    all_regions: &[Region],
) -> (Vec<[f32; 3]>, f32, usize, usize) {
    let mut current = pyramid.to_vec();
    let mut fit_quality_sum = 0.0_f64;
    let mut fit_count = 0_usize;
    let mut full_cores = 0_usize;
    let mut solver_fallbacks = 0_usize;

    for region in all_regions {
        let fits = best_fits(grid, region);
        for fit in fits.iter().flatten() {
            fit_quality_sum += f64::from(fit.r2);
            fit_count += 1;
        }
        let cores = fully_clipped_components(grid, region);
        full_cores += cores.len();

        if solve_region_direct(grid, region, fits, &mut current) {
            apply_luminance_domes(grid, &cores, &mut current);
            keep_unfitted_partial_cells(grid, region, fits, &mut current);
            continue;
        }
        solver_fallbacks += 1;

        let mut next = current.clone();
        for _ in 0..(HARMONIC_SWEEPS + HARMONIC_POLISH_SWEEPS) {
            for &i in &region.cells {
                let x = i % grid.width;
                let y = i / grid.width;
                for (target, fit) in fits.iter().copied().enumerate() {
                    if grid.valid[i][target] {
                        continue;
                    }
                    let guide = fit.map_or_else(
                        || {
                            (0..3)
                                .filter(|&c| c != target)
                                .max_by(|&a, &b| current[i][a].total_cmp(&current[i][b]))
                                .unwrap_or((target + 1) % 3)
                        },
                        |line| line.guides[0],
                    );
                    let guide_scale = current[i][guide].abs().max(0.05);
                    let mut numerator = 0.0_f32;
                    let mut denominator = 0.0_f32;
                    for (dx, dy) in [(1_isize, 0_isize), (-1, 0), (0, 1), (0, -1)] {
                        let nx = mirror(x as isize + dx, grid.width);
                        let ny = mirror(y as isize + dy, grid.height);
                        let n = ny * grid.width + nx;
                        let delta = (current[i][guide] - current[n][guide]).abs();
                        let w = (-(delta / (GUIDE_K * guide_scale)).powi(2))
                            .exp()
                            .max(WEIGHT_FLOOR);
                        numerator += current[n][target] * w;
                        denominator += w;
                    }
                    if let Some(line) = fit {
                        let data = fit_data(line, &current, i).max(grid.floor[i][target]);
                        // A near-perfect colour line is stronger evidence than
                        // four neighbouring guesses. The denominator softens
                        // rapidly near the R² gate, so weak fits cannot paint an
                        // occluder's hue through the region.
                        let w = 8.0 * line.r2 / (1.05 - line.r2).max(0.05);
                        numerator += data * w;
                        denominator += w;
                    }
                    next[i][target] =
                        (numerator / denominator.max(EPSILON)).max(grid.floor[i][target]);
                }
            }
            std::mem::swap(&mut current, &mut next);
        }
        apply_luminance_domes(grid, &cores, &mut current);
        keep_unfitted_partial_cells(grid, region, fits, &mut current);
    }

    let mean_fit = if fit_count == 0 {
        0.0
    } else {
        (fit_quality_sum / fit_count as f64) as f32
    };
    (current, mean_fit, full_cores, solver_fallbacks)
}

struct ApplyContext<'a> {
    original: &'a [f32],
    confidence: Option<&'a [f32]>,
    width: usize,
    height: usize,
    cfa: &'a CFA,
    grid_width: usize,
}

fn apply_prediction(
    samples: &mut [f32],
    prediction: &[[f32; 3]],
    context: ApplyContext<'_>,
) -> (usize, f32) {
    let mut reconstructed = 0_usize;
    let mut max_lift = 0.0_f32;
    for y in 0..context.height {
        for x in 0..context.width {
            let i = y * context.width + x;
            if confidence_at(context.original, context.confidence, i) < VALID_CONFIDENCE_MAX {
                continue;
            }
            let Some(c) = channel(context.cfa.cfa_color_at(y, x)) else {
                continue;
            };
            let cell = (y / 2) * context.grid_width + x / 2;
            let target = prediction[cell][c].max(context.original[i]);
            if target > samples[i] {
                max_lift = max_lift.max(target - samples[i]);
                samples[i] = target;
                reconstructed += 1;
            }
        }
    }
    (reconstructed, max_lift)
}

/// Reconstruct a normalized RGB Bayer mosaic in place.
pub fn reconstruct_cfa(
    samples: &mut [f32],
    width: usize,
    height: usize,
    cfa: &CFA,
    confidence: Option<&[f32]>,
    method: HighlightMethod,
) -> Result<RawHighlightReport> {
    ensure!(
        method.is_spatial(),
        "current highlight reconstruction is post-demosaic"
    );
    ensure!(
        cfa.is_rgb(),
        "spatial highlight reconstruction requires an RGB CFA"
    );
    ensure!(
        samples.len() == width.saturating_mul(height),
        "CFA buffer length does not match its dimensions"
    );
    let confidence = confidence.filter(|map| map.len() == samples.len());
    let original = samples.to_vec();
    let clipped_cfa_sites = (0..samples.len())
        .filter(|&i| confidence_at(&original, confidence, i) >= VALID_CONFIDENCE_MAX)
        .count();
    if clipped_cfa_sites == 0 {
        return Ok(RawHighlightReport {
            method,
            ..RawHighlightReport::default()
        });
    }

    let knee_corrected_sites = if method == HighlightMethod::Harmonic {
        apply_sensor_knee(samples, width, height, cfa)
    } else {
        0
    };
    let grid = build_grid(samples, confidence, width, height, cfa);
    let pyramid = build_pyramid(&grid);
    let pyramid_prediction = pyramid_prediction(&grid, &pyramid);
    let all_regions = regions(&grid);
    let (prediction, mean_fit_quality, fully_clipped_cores, solver_fallbacks) = match method {
        HighlightMethod::RawPyramid => (pyramid_prediction, 0.0, 0, 0),
        HighlightMethod::Harmonic => {
            let (prediction, fit, cores, fallbacks) =
                harmonic_prediction(&grid, &pyramid_prediction, &all_regions);
            (prediction, fit, cores, fallbacks)
        }
        HighlightMethod::Current => unreachable!("checked above"),
    };
    let (reconstructed_cfa_sites, max_lift) = apply_prediction(
        samples,
        &prediction,
        ApplyContext {
            original: &original,
            confidence,
            width,
            height,
            cfa,
            grid_width: grid.width,
        },
    );

    Ok(RawHighlightReport {
        method,
        clipped_cfa_sites,
        reconstructed_cfa_sites,
        connected_regions: all_regions.len(),
        knee_corrected_sites,
        mean_fit_quality,
        fully_clipped_cores,
        solver_fallbacks,
        max_lift,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rggb() -> CFA {
        CFA::new("RGGB")
    }

    fn mosaic(width: usize, height: usize, field: impl Fn(usize, usize) -> [f32; 3]) -> Vec<f32> {
        let cfa = rggb();
        (0..height)
            .flat_map(|y| {
                let cfa = cfa.clone();
                let field = &field;
                (0..width).map(move |x| {
                    let rgb = field(x, y);
                    rgb[channel(cfa.cfa_color_at(y, x)).unwrap()]
                })
            })
            .collect()
    }

    #[test]
    fn no_clip_is_bit_exact_for_both_methods() {
        let width = 32;
        let height = 24;
        let source = mosaic(width, height, |x, y| {
            let v = 0.1 + 0.6 * (x + y) as f32 / (width + height) as f32;
            [v * 0.7, v * 0.9, v]
        });
        for method in [HighlightMethod::RawPyramid, HighlightMethod::Harmonic] {
            let mut candidate = source.clone();
            let report = reconstruct_cfa(&mut candidate, width, height, &rggb(), None, method)
                .expect("valid CFA");
            assert_eq!(candidate, source);
            assert_eq!(report.reconstructed_cfa_sites, 0);
        }
    }

    #[test]
    fn measured_sites_are_exact_and_reconstruction_respects_floors() {
        let width = 64;
        let height = 32;
        let truth = mosaic(width, height, |x, _| {
            let v = 0.65 + 0.9 * x as f32 / (width - 1) as f32;
            [v * 0.72, v, v * 0.96]
        });
        let captured: Vec<f32> = truth.iter().map(|v| v.min(1.0)).collect();
        for method in [HighlightMethod::RawPyramid, HighlightMethod::Harmonic] {
            let mut candidate = captured.clone();
            reconstruct_cfa(&mut candidate, width, height, &rggb(), None, method)
                .expect("valid CFA");
            for i in 0..candidate.len() {
                assert!(candidate[i].is_finite() && candidate[i] >= 0.0);
                assert!(candidate[i] + 1.0e-7 >= captured[i]);
                if captured[i] < crate::highlight::CLIP_RAMP_LOW {
                    assert_eq!(candidate[i], captured[i], "trusted site {i} moved");
                }
            }
        }
    }

    #[test]
    fn harmonic_reconstruction_is_deterministic() {
        let width = 48;
        let height = 48;
        let truth = mosaic(width, height, |x, y| {
            let dx = x as f32 - width as f32 * 0.5;
            let dy = y as f32 - height as f32 * 0.5;
            let v = 0.7 + 0.8 * (-(dx * dx + dy * dy) / 300.0).exp();
            [v * 0.74, v, v * 0.95]
        });
        let captured: Vec<f32> = truth.iter().map(|v| v.min(1.0)).collect();
        let mut first = captured.clone();
        let mut second = captured;
        let first_report = reconstruct_cfa(
            &mut first,
            width,
            height,
            &rggb(),
            None,
            HighlightMethod::Harmonic,
        )
        .unwrap();
        let second_report = reconstruct_cfa(
            &mut second,
            width,
            height,
            &rggb(),
            None,
            HighlightMethod::Harmonic,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first_report.fully_clipped_cores,
            second_report.fully_clipped_cores
        );
        assert!(first_report.fully_clipped_cores > 0);
        assert_eq!(first_report.solver_fallbacks, 0);
    }

    #[test]
    fn analytic_sensor_knee_is_inverted_without_touching_hard_clip() {
        let width = 512;
        let height = 128;
        let truth = mosaic(width, height, |x, _| {
            let intensity = 0.35 + 1.45 * x as f32 / (width - 1) as f32;
            [0.44 * intensity, intensity, 0.61 * intensity]
        });
        let shoulder = |value: f32| {
            if value <= KNEE_LOW {
                value
            } else {
                (KNEE_LOW + 0.65 * (value - KNEE_LOW)).min(1.0)
            }
        };
        let mut captured: Vec<f32> = truth.iter().copied().map(shoulder).collect();
        let before_knee = captured.clone();
        let corrected = apply_sensor_knee(&mut captured, width, height, &rggb());
        assert!(corrected > 0, "the verified knee should engage");
        let mut squared = 0.0_f64;
        let mut count = 0_usize;
        for (i, (&candidate, &reference)) in captured.iter().zip(&truth).enumerate() {
            let observed = shoulder(reference);
            if (KNEE_LOW..KNEE_HIGH).contains(&observed) && candidate > observed {
                squared += f64::from(candidate - reference).powi(2);
                count += 1;
            }
            if before_knee[i] >= KNEE_HIGH {
                assert_eq!(
                    candidate, before_knee[i],
                    "hard-clipped site {i} moved in the knee stage"
                );
            }
        }
        let rmse = (squared / count.max(1) as f64).sqrt() as f32;
        assert!(count > 0);
        assert!(rmse <= 0.003, "sensor-knee inverse RMSE {rmse}");
    }

    #[test]
    fn occluded_sky_does_not_change_pixels_beyond_the_demosaic_support() {
        use rawler::imgop::{Dim2, Point, Rect};

        let width = 96;
        let height = 64;
        let cfa = rggb();
        let truth = mosaic(width, height, |x, y| {
            if y >= 42 + ((x as f32 * 0.17).sin() * 2.0) as usize {
                let value = 0.12 + 0.03 * ((x * 11 + y * 7) % 9) as f32 / 8.0;
                [value * 0.45, value, value * 0.32]
            } else {
                let value = 0.72 + 0.95 * x as f32 / (width - 1) as f32;
                [value * 0.56, value, value * 0.92]
            }
        });
        let captured: Vec<f32> = truth.iter().map(|value| value.min(1.0)).collect();
        let confidence: Vec<f32> = captured
            .iter()
            .map(|value| crate::highlight::clip_confidence(*value))
            .collect();
        let roi = Rect::new(Point::zero(), Dim2::new(width, height));
        let baseline = crate::demosaic::demosaic_bayer(
            &captured,
            width,
            height,
            &cfa,
            roi,
            crate::demosaic::DemosaicMethod::Rcd,
        );
        let mut reconstructed = captured.clone();
        reconstruct_cfa(
            &mut reconstructed,
            width,
            height,
            &cfa,
            Some(&confidence),
            HighlightMethod::Harmonic,
        )
        .unwrap();
        let candidate = crate::demosaic::demosaic_bayer(
            &reconstructed,
            width,
            height,
            &cfa,
            roi,
            crate::demosaic::DemosaicMethod::Rcd,
        );

        let mut outside_changes = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let near_mask = (-3..=3).any(|dy| {
                    (-3..=3).any(|dx| {
                        let nx = x as isize + dx;
                        let ny = y as isize + dy;
                        nx >= 0
                            && ny >= 0
                            && nx < width as isize
                            && ny < height as isize
                            && confidence[ny as usize * width + nx as usize] >= VALID_CONFIDENCE_MAX
                    })
                });
                if !near_mask {
                    let i = y * width + x;
                    let a = crate::oklab::from_linear_srgb(baseline[i]);
                    let b = crate::oklab::from_linear_srgb(candidate[i]);
                    outside_changes.push((a.a - b.a).hypot(a.b - b.b));
                }
            }
        }
        outside_changes.sort_by(f32::total_cmp);
        let p99 = outside_changes[((outside_changes.len() - 1) as f32 * 0.99).round() as usize];
        assert!(p99 <= 0.005, "outside-mask p99 OKLab a/b change {p99}");
    }
}
