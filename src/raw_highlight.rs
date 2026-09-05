//! Experimental pre-demosaic highlight reconstruction.
//!
//! These methods work on the normalized CFA
//! mosaic, where a saturated reading is a lower bound and where measured sites
//! can still be kept exact.  `RawPyramid` is the small, clipping-aware baseline;
//! `Harmonic` adds connected-region colour-line fitting and an obstacle-
//! constrained harmonic solve and is the versioned automatic estimator.

use anyhow::{Result, ensure};
use clap::ValueEnum;
use faer::Side;
use faer::prelude::{Col, Solve, SparseColMat};
use faer::sparse::Triplet;
use rawler::CFA;
use rawler::cfa::CFAColor;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::VecDeque;

const VALID_CONFIDENCE_MAX: f32 = 0.5;
const EPSILON: f32 = 1.0e-8;
const PYRAMID_KERNEL: [f32; 5] = [1.0, 4.0, 6.0, 4.0, 1.0];
const HARMONIC_SWEEPS: usize = 240;
const HARMONIC_POLISH_SWEEPS: usize = 60;
const DIRECT_SOLVE_MAX_UNKNOWNS: usize = 1 << 14;
const MAX_AFFINE_EXTRAPOLATION_SPANS: f32 = 3.0;
const HIGH_GAIN_COLOR_SLOPE: f32 = 2.5;
const GUIDE_K: f32 = 0.15;
const WEIGHT_FLOOR: f32 = 1.0e-4;
const KNEE_LOW: f32 = 0.80;
const KNEE_HIGH: f32 = 0.995;
const KNEE_BINS: usize = 24;
const KNEE_MIN_VOTES: usize = 100;

/// Highlight estimator selected by the automatic profile or an explicit caller.
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

    /// Transient reserve `memory.rs` sizes `--jobs` against, on top of
    /// `memory::PEAK_BYTES_PER_PIXEL`, for what a spatial estimator adds to a
    /// frame's high-water mark.
    ///
    /// These were 24 and 64, fitted when the solver allocated a full-frame
    /// working copy and a grid-sized row map per connected clipped region and
    /// its transient was the tallest thing in the process. Since the 2026-09-05
    /// allocation pass it is not: the solver's whole peak now sits below the
    /// chroma stage's, and what it adds to the frame's maximum is what is
    /// measured here. Same worst-case flag set as the `memory.rs` sweep,
    /// `--spatial-highlight-floor 0` so the solve cannot decline:
    ///
    /// | frame | pixels | `raw_pyramid` | `harmonic` |
    /// |---|---|---|---|
    /// | `_DSC1139.ARW` | 24.3 MP | +0.16 B/px | +1.89 B/px |
    /// | `_DSC1289.ARW` (623 593 clipped sites) | 24.3 MP | +0.25 B/px | +0.97 B/px |
    /// | `_DSC1105.ARW` | 10.5 MP | +0.75 B/px | +0.26 B/px |
    ///
    /// 8 is roughly four times the worst of those. It is deliberately not 0:
    /// the reserve is the line item that makes a future solver's growth visible
    /// in the budget rather than in an OOM kill, and 8 B/px on a 24 megapixel
    /// frame is 195 MB — enough to absorb a new full-grid buffer or two.
    pub const fn extra_bytes_per_pixel(self) -> u64 {
        match self {
            Self::Current => 0,
            Self::RawPyramid => 8,
            Self::Harmonic => 8,
        }
    }
}

/// Fraction of CFA sites that must read as clipped before a spatial estimator
/// is worth its cost.
///
/// The spatial solvers are the expensive part of the whole program: on `_DSC1289`
/// (24 MP, 623 593 clipped CFA sites) the harmonic path costs ~7.31 s of wall
/// clock and ~1056 MB of peak RSS at `--jobs 1`, down from ~21.9 s and ~1334 MB
/// before the 2026-09-05 allocation work (`CHANGELOG.md`). Almost all of that is
/// paid whether or not there is anything to reconstruct, because `build_grid`,
/// `build_pyramid` and `pyramid_prediction` are full-image passes.
/// A frame with a few hundred isolated clipped sites gets nothing back for it,
/// and the post-demosaic `Current` estimator — the default for the whole life of
/// the project before the harmonic promotion — already handles isolated clipped
/// pixels.
///
/// `1e-5` is 0.001% of sites, i.e. about 240 sites on a 24 MP mosaic. The
/// supporting corpus measurement is `docs/HDR_EVALUATION.md`, which classified
/// every pixel of all 58 files in `raw/raw_3rd_batch` by clipped-channel count:
/// the two-or-more-channel fraction has a **median of 0.0003%** (3e-6, below
/// this floor), a **p90 of 1.9%** and a **maximum of 12.4%**, and the frames
/// that were independently flagged by eye as having a highlight defect are the
/// nine above 1% — three orders of magnitude above the floor. So the floor
/// separates "nothing to reconstruct" from every frame the corpus has ever
/// shown a highlight problem on, with a wide margin on both sides.
///
/// Swept 2026-08-27 on all 381 CFA files (`docs/evidence/spatial-floor-sweep-20260827/`):
/// 99 frames at zero sites, 71 in (0, 1e-5) with 1–224 sites, 211 at or above
/// the floor. The 29-frame "empty band" was a reporting artifact. 1e-5 is kept
/// as this "few hundred isolated sites" cutoff; a quality A/B of those 71
/// would be needed to move it. The floor continues to gate on CFA sites, not
/// on two-or-more-channel demosaiced pixels. `clipped_cfa_sites` is counted
/// for every spatial pass, including a declined one, and folded onto the
/// Current sidecar report.
pub const DEFAULT_SPATIAL_CLIPPED_FLOOR: f32 = 1.0e-5;

/// Parameters that define one pre-demosaic reconstruction pass.
#[derive(Debug, Clone, Copy)]
pub struct ReconstructionOptions {
    pub white_balance: [f32; 3],
    pub strength: f32,
    pub method: HighlightMethod,
    /// Decline the spatial solve below this fraction of clipped CFA sites and
    /// fall back to the post-demosaic estimator. `0.0` always solves, which is
    /// what the ablation paths and the `--spatial-highlight-floor 0` override
    /// use; it reproduces the pre-floor behaviour exactly.
    pub clipped_fraction_floor: f32,
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
    confidence: Vec<[f32; 3]>,
    trust: Vec<[f32; 3]>,
    // Compatibility topology for the older luminance initializer. Final
    // chromaticity and application authority is continuous; it never reads
    // this thresholded view.
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
    guide_min: [f32; 2],
    guide_max: [f32; 2],
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

/// Grid-sized scratch buffers reused across every region of one frame.
///
/// These used to be allocated inside the per-region helpers, so their cost
/// scaled with the number of connected clipped regions rather than with how
/// much of the frame was clipped. On a 24 megapixel frame with 69 regions and
/// 4 267 clipped photosites that is roughly 20 GB of `vec![...;
/// grid.value.len()]` allocated, zeroed and (for the solver's working copy)
/// copied — the largest single cost in the whole pipeline, and most of the
/// process's minor page faults. Every buffer here is left in its neutral state
/// by whoever borrows it, so reusing one is indistinguishable from a fresh
/// allocation.
struct RegionScratch {
    /// `usize::MAX` everywhere except while a direct solve is numbering rows.
    row_of: Vec<usize>,
    /// `false` everywhere between uses.
    in_region: Vec<bool>,
    /// `false` everywhere between uses.
    seen: Vec<bool>,
    /// `f32::INFINITY` everywhere between uses.
    depth: Vec<f32>,
    /// Cells a direct solve has overwritten, so a failed solve can restore the
    /// buffer the iterative fallback expects to find untouched.
    undo: Vec<(usize, usize, f32)>,
}

impl RegionScratch {
    fn new(cells: usize) -> Self {
        Self {
            row_of: vec![usize::MAX; cells],
            in_region: vec![false; cells],
            seen: vec![false; cells],
            depth: vec![f32::INFINITY; cells],
            undo: Vec::new(),
        }
    }
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

/// Per-cell sums for [`build_grid`], kept together so one rayon task can own
/// a whole grid row without six separate mutable borrows.
#[derive(Clone, Copy, Default)]
struct CellAccumulator {
    sum: [f32; 3],
    floor: [f32; 3],
    weighted_sum: [f32; 3],
    confidence_sum: [f32; 3],
    trust_sum: [f32; 3],
    count: [u8; 3],
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
    let mut floor = vec![[0.0_f32; 3]; grid_width * grid_height];
    let mut value = vec![[0.0_f32; 3]; grid_width * grid_height];
    let mut grid_confidence = vec![[0.0_f32; 3]; grid_width * grid_height];
    let mut trust = vec![[0.0_f32; 3]; grid_width * grid_height];
    let mut valid = vec![[false; 3]; grid_width * grid_height];

    // A Bayer quad maps to exactly one grid row, so a task can own grid row
    // `gy` together with source rows `2*gy` and `2*gy+1` and touch nothing
    // else: the accumulation order inside a cell is still row-major over that
    // quad, so the f32 sums are the ones the serial loop produced. Summing and
    // finalising in the same pass keeps the accumulators to one row per thread
    // instead of a whole extra grid (~384 MB on a 24 megapixel frame, live
    // alongside the five planes below).
    value
        .par_chunks_mut(grid_width)
        .zip(floor.par_chunks_mut(grid_width))
        .zip(grid_confidence.par_chunks_mut(grid_width))
        .zip(trust.par_chunks_mut(grid_width))
        .zip(valid.par_chunks_mut(grid_width))
        .enumerate()
        .for_each_init(
            || vec![CellAccumulator::default(); grid_width],
            |cells, (grid_y, ((((value, floor), grid_confidence), trust), valid))| {
                cells.fill(CellAccumulator::default());
                for y in (2 * grid_y)..(2 * grid_y + 2).min(height) {
                    for x in 0..width {
                        let source = y * width + x;
                        let Some(c) = channel(cfa.cfa_color_at(y, x)) else {
                            continue;
                        };
                        let cell = &mut cells[x / 2];
                        let sample = samples[source];
                        let clip = confidence_at(samples, confidence, source);
                        let site_trust = (1.0 - clip).powi(2);
                        cell.sum[c] += sample;
                        cell.floor[c] = cell.floor[c].max(sample);
                        cell.weighted_sum[c] += sample * site_trust;
                        cell.confidence_sum[c] += clip;
                        cell.trust_sum[c] += site_trust;
                        cell.count[c] += 1;
                    }
                }
                for (i, cell) in cells.iter().enumerate() {
                    floor[i] = cell.floor;
                    for c in 0..3 {
                        if cell.count[c] > 0 {
                            let n = f32::from(cell.count[c]);
                            grid_confidence[i][c] = cell.confidence_sum[c] / n;
                            trust[i][c] = cell.trust_sum[c] / n;
                            value[i][c] = if cell.trust_sum[c] > EPSILON {
                                cell.weighted_sum[c] / cell.trust_sum[c]
                            } else {
                                cell.sum[c] / n
                            };
                            // This threshold is compatibility topology for the
                            // older pyramid/line-fit luminance initializer. The
                            // joint chroma solve and final per-site application
                            // use continuous trust and confidence instead. Mean
                            // confidence deliberately keeps a Bayer quad with
                            // one clean and one clipped green from being treated
                            // as wholly measured.
                            valid[i][c] = grid_confidence[i][c] < VALID_CONFIDENCE_MAX;
                        }
                    }
                }
            },
        );

    Grid {
        width: grid_width,
        height: grid_height,
        value,
        floor,
        confidence: grid_confidence,
        trust,
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

    // Each output row reads `previous` only, so a row per task is deterministic
    // and the tap order inside a pixel is unchanged.
    value
        .par_chunks_mut(width)
        .zip(weight.par_chunks_mut(width))
        .enumerate()
        .for_each(|(y, (value_row, weight_row))| {
            for (x, (value, weight)) in value_row.iter_mut().zip(weight_row.iter_mut()).enumerate()
            {
                for (ky, &kernel_y) in PYRAMID_KERNEL.iter().enumerate() {
                    let sy = mirror(2 * y as isize + ky as isize - 2, previous.height);
                    for (kx, &kernel_x) in PYRAMID_KERNEL.iter().enumerate() {
                        let sx = mirror(2 * x as isize + kx as isize - 2, previous.width);
                        let source = sy * previous.width + sx;
                        let kernel = kernel_y * kernel_x;
                        for c in 0..3 {
                            let w = kernel * previous.weight[source][c];
                            value[c] += previous.value[source][c] * w;
                            weight[c] += w;
                        }
                    }
                }
                for c in 0..3 {
                    if weight[c] > EPSILON {
                        value[c] /= weight[c];
                        // A propagated value has one vote at the next scale.
                        // The absolute kernel mass is irrelevant after
                        // renormalisation.
                        weight[c] = 1.0;
                    }
                }
            }
        });

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
        guide_min: [pairs.first().map_or(0.0, |pair| pair.0), 0.0],
        guide_max: [pairs.last().map_or(0.0, |pair| pair.0), 0.0],
        guide_count: 1,
        intercept: intercept as f32,
        r2,
        trusted_mass: 1.0,
    })
}

/// Infer a raise-only inverse for a smooth sensor shoulder. Hard-clipped sites
/// are deliberately excluded; this stage may recover roll-off but cannot
/// manufacture information beyond the white level.
///
/// `original` is the caller's untouched copy of `samples`; this stage reads the
/// pre-knee value at every site while writing the lifted one, and the caller
/// already holds exactly that copy, so taking it here avoids a second
/// full-mosaic clone (~97 MB on a 24 megapixel frame).
fn apply_sensor_knee(
    samples: &mut [f32],
    original: &[f32],
    confidence: Option<&[f32]>,
    width: usize,
    height: usize,
    cfa: &CFA,
) -> usize {
    debug_assert_eq!(samples.len(), original.len());
    let grid = build_grid(original, None, width, height, cfa);
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

    // One row per task: every site reads `original` and writes only its own
    // `samples[i]`, and the total is a sum of per-row counts, so the split
    // changes neither the pixels nor the reported figure.
    let corrected_sites: usize = samples
        .par_chunks_mut(width)
        .enumerate()
        .map(|(y, row)| {
            let mut corrected_sites = 0_usize;
            for (x, sample) in row.iter_mut().enumerate() {
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
                    // An explicit per-site zero means "do not touch"; honour that
                    // as a veto. Do not scale the lift by clip confidence: this
                    // stage inverts a measured shoulder over [KNEE_LOW,
                    // KNEE_HIGH), which begins well below CLIP_RAMP_LOW, so a clip
                    // ramp would zero the correction across most of the stage's own
                    // range and apply a fraction of an analytic inverse above it.
                    // `accepted[c][bin]` is already the evidence gate.
                    if confidence.is_some_and(|map| map[i] <= 0.0) {
                        continue;
                    }
                    let corrected = value + lifts[c][bin];
                    if corrected > *sample {
                        *sample = corrected;
                        corrected_sites += 1;
                    }
                }
            }
            corrected_sites
        })
        .sum();
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
    let affected = |i: usize| grid.confidence[i].iter().any(|&c| c > 0.0);
    let mut seen = vec![false; grid.value.len()];
    let mut output = Vec::new();
    for seed in 0..grid.value.len() {
        if seen[seed] || !affected(seed) {
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
                    if !affected(n) {
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
        guide_min: [pairs.first().map_or(0.0, |pair| pair.0), 0.0],
        guide_max: [pairs.last().map_or(0.0, |pair| pair.0), 0.0],
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
    // A two-guide fit is only identifiable when the guides contain genuinely
    // independent information.  Bayer channels on a smooth constant-colour
    // ramp are almost perfectly collinear: the normal equations may still be
    // numerically invertible, but large cancelling slopes then extrapolate an
    // arbitrary colour once one guide clips.  Judge independence before the
    // ridge term masks that degeneracy.
    let predictor_independence = (c00 * c11 - c01 * c01) / (c00 * c11).max(f64::MIN_POSITIVE);
    if !predictor_independence.is_finite() || predictor_independence < 0.01 {
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
        guide_min: [
            samples
                .iter()
                .map(|value| value.0)
                .fold(f32::INFINITY, f32::min),
            samples
                .iter()
                .map(|value| value.1)
                .fold(f32::INFINITY, f32::min),
        ],
        guide_max: [
            samples
                .iter()
                .map(|value| value.0)
                .fold(f32::NEG_INFINITY, f32::max),
            samples
                .iter()
                .map(|value| value.1)
                .fold(f32::NEG_INFINITY, f32::max),
        ],
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

/// Continuous confidence for the harmonic path's per-channel luminance
/// extrapolation. Unlike `best_fits`, this never turns a channel into a
/// valid/invalid vote: every sample contributes with the same pairwise trust
/// used by the joint chromaticity solve.
fn continuous_luminance_support(grid: &Grid, region: &Region) -> [f32; 3] {
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
    // `weight` is symmetric in (target, guide) and covariance^2 / (var_x *
    // var_y) is invariant under swapping x and y, so pass (a, b) and pass
    // (b, a) compute the identical f32. Evaluate each unordered pair once.
    let pair = |target: usize, guide: usize| -> f32 {
        let mut sum_w = 0.0_f64;
        let mut sum_x = 0.0_f64;
        let mut sum_y = 0.0_f64;
        let mut sum_xx = 0.0_f64;
        let mut sum_yy = 0.0_f64;
        let mut sum_xy = 0.0_f64;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let i = y * grid.width + x;
                let weight = f64::from(grid.trust[i][target] * grid.trust[i][guide]);
                let x = f64::from(grid.value[i][guide]);
                let y = f64::from(grid.value[i][target]);
                sum_w += weight;
                sum_x += weight * x;
                sum_y += weight * y;
                sum_xx += weight * x * x;
                sum_yy += weight * y * y;
                sum_xy += weight * x * y;
            }
        }
        if sum_w <= f64::from(EPSILON) {
            return 0.0;
        }
        let var_x = sum_xx - sum_x * sum_x / sum_w;
        let var_y = sum_yy - sum_y * sum_y / sum_w;
        if var_x <= 1.0e-12 || var_y <= 1.0e-12 {
            return 0.0;
        }
        let covariance = sum_xy - sum_x * sum_y / sum_w;
        (covariance * covariance / (var_x * var_y)).clamp(0.0, 1.0) as f32
    };
    let rg = pair(0, 1);
    let rb = pair(0, 2);
    let gb = pair(1, 2);
    [rg.max(rb), rg.max(gb), rb.max(gb)]
}

/// Breadth-first distance from the region's rim, written into `depth`.
///
/// `depth` arrives all-`INFINITY` and is left that way by [`clear_depth`] once
/// the caller has read it; only the region's own cells are ever written.
fn depth_map(grid: &Grid, region: &Region, depth: &mut [f32]) {
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
}

/// Undo [`depth_map`]: only rim and region cells can have been written.
fn clear_depth(region: &Region, depth: &mut [f32]) {
    for &i in region.boundary.iter().chain(&region.cells) {
        depth[i] = f32::INFINITY;
    }
}

fn fully_clipped_components(
    grid: &Grid,
    region: &Region,
    scratch: &mut RegionScratch,
) -> Vec<Region> {
    let RegionScratch {
        in_region, seen, ..
    } = scratch;
    for &i in &region.cells {
        in_region[i] = true;
    }
    let mut cores = Vec::new();
    for &seed in &region.cells {
        // Domes are reserved for hard-clipped cores with no surviving local
        // evidence. A soft sensor shoulder (even above the old 0.5 binary
        // threshold) remains ordinary harmonic territory and must not gain an
        // extrapolated peak merely because continuous confidence is non-zero.
        if seen[seed] || grid.confidence[seed].iter().any(|&c| c < 1.0 - EPSILON) {
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
                    if !in_region[n] || grid.confidence[n].iter().any(|&c| c < 1.0 - EPSILON) {
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
    // `seen` is only ever set inside the region, so clearing the region clears
    // the buffer for the next one.
    for &i in &region.cells {
        in_region[i] = false;
        seen[i] = false;
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

fn soft_extrapolation_limit(value: f32, measured_min: f32, measured_max: f32) -> f32 {
    let span = (measured_max - measured_min).max(EPSILON);
    let transition = 0.5 * span;
    let upper = measured_max + MAX_AFFINE_EXTRAPOLATION_SPANS * span;
    let lower = measured_min - MAX_AFFINE_EXTRAPOLATION_SPANS * span;
    let soft_upper = |sample: f32, limit: f32| {
        let start = limit - transition;
        if sample <= start {
            return sample;
        }
        let t = ((sample - start) / (2.0 * transition)).clamp(0.0, 1.0);
        start + transition * (2.0 * t - t * t)
    };
    if value < lower + transition {
        -soft_upper(-value, -lower)
    } else {
        soft_upper(value, upper)
    }
}

fn fit_data(line: LineFit, current: &[[f32; 3]], index: usize, bound_extrapolation: bool) -> f32 {
    let high_gain_color_fit = line.slopes[..line.guide_count]
        .iter()
        .any(|slope| slope.abs() > HIGH_GAIN_COLOR_SLOPE);
    let bounded_guide = |slot: usize| {
        if !bound_extrapolation || !high_gain_color_fit {
            return current[index][line.guides[slot]];
        }
        soft_extrapolation_limit(
            current[index][line.guides[slot]],
            line.guide_min[slot],
            line.guide_max[slot],
        )
    };
    line.intercept
        + line.slopes[0] * bounded_guide(0)
        + if line.guide_count == 2 {
            line.slopes[1] * bounded_guide(1)
        } else {
            0.0
        }
}

fn usable_white_balance(white_balance: [f32; 3]) -> [f32; 3] {
    // Match the owned colour transform: a missing first as-shot coefficient
    // means the file recorded no white balance at all, rather than one bad
    // channel in an otherwise usable triplet.
    if white_balance[0].is_nan() {
        return [1.0; 3];
    }
    white_balance.map(|gain| {
        if gain.is_finite() && gain > EPSILON {
            gain
        } else {
            1.0
        }
    })
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
    scratch: &mut RegionScratch,
) -> bool {
    // Bound the system by what it actually solves for. Gating on
    // `region.cells.len()` rejected regions on padding that is not even
    // unknown, which the widened `affected` predicate in `regions()` made
    // common: a region that fits comfortably would fall into the 300-sweep
    // iterative path on trusted cells alone. The per-channel check below is
    // the real guard; this one only avoids assembling a system that cannot be
    // used.
    let widest_unknown = (0..3)
        .map(|target| {
            region
                .cells
                .iter()
                .filter(|&&i| !grid.valid[i][target])
                .count()
        })
        .max()
        .unwrap_or(0);
    if widest_unknown > DIRECT_SOLVE_MAX_UNKNOWNS {
        return false;
    }
    // Edited in place. This used to work on a full-frame `current.to_vec()`
    // per region — 144 MB copied for a few thousand solved cells — but the only
    // writes are to `unknown`, and the old code copied exactly those cells back
    // on success. Recording them instead lets a failed solve restore the buffer
    // the iterative fallback expects, at the cost of one entry per solved cell.
    scratch.undo.clear();
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
            roll_back(current, &mut scratch.undo);
            return false;
        }
        let row_of = &mut scratch.row_of;
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
                        .max_by(|&a, &b| current[i][a].total_cmp(&current[i][b]))
                        .unwrap_or((target + 1) % 3)
                },
                |fit| fit.guides[0],
            );
            let guide_scale = current[i][guide].abs().max(0.05);
            let mut diagonal = 1.0e-8_f64;
            for (dx, dy) in [(1_isize, 0_isize), (-1, 0), (0, 1), (0, -1)] {
                let nx = mirror(x as isize + dx, grid.width);
                let ny = mirror(y as isize + dy, grid.height);
                let n = ny * grid.width + nx;
                if n == i {
                    continue;
                }
                let delta = (current[i][guide] - current[n][guide]).abs();
                let weight = (-(delta / (GUIDE_K * guide_scale)).powi(2))
                    .exp()
                    .max(WEIGHT_FLOOR);
                diagonal += f64::from(weight);
                let other_row = row_of[n];
                if other_row == usize::MAX {
                    rhs[row] += f64::from(weight * current[n][target]);
                } else if other_row < row {
                    triplets.push(Triplet::new(row, other_row, -f64::from(weight)));
                }
            }
            if let Some(line) = line {
                let weight = 8.0 * line.r2 / (1.05 - line.r2).max(0.05);
                diagonal += f64::from(weight);
                rhs[row] += f64::from(
                    weight
                        * fit_data(line, current, i, grid.valid[i] != [false; 3])
                            .max(grid.floor[i][target]),
                );
            }
            triplets.push(Triplet::new(row, row, diagonal));
        }
        // Left neutral for the next channel and the next region.
        for &cell in &unknown {
            row_of[cell] = usize::MAX;
        }

        let Ok(matrix) = SparseColMat::<usize, f64>::try_new_from_triplets(
            unknown.len(),
            unknown.len(),
            &triplets,
        ) else {
            roll_back(current, &mut scratch.undo);
            return false;
        };
        let Ok(cholesky) = matrix.sp_cholesky(Side::Lower) else {
            roll_back(current, &mut scratch.undo);
            return false;
        };
        let rhs = Col::from_fn(unknown.len(), |row| rhs[row]);
        let solution = cholesky.solve(&rhs);
        for (row, &i) in unknown.iter().enumerate() {
            let value = solution[row] as f32;
            if !value.is_finite() {
                roll_back(current, &mut scratch.undo);
                return false;
            }
            scratch.undo.push((i, target, current[i][target]));
            current[i][target] = value.max(grid.floor[i][target]);
        }
    }

    true
}

/// Restore every cell a failed direct solve wrote, newest first.
fn roll_back(current: &mut [[f32; 3]], undo: &mut Vec<(usize, usize, f32)>) {
    for (i, target, value) in undo.drain(..).rev() {
        current[i][target] = value;
    }
}

fn apply_luminance_domes(
    grid: &Grid,
    cores: &[Region],
    current: &mut [[f32; 3]],
    depth: &mut [f32],
) {
    for core in cores {
        depth_map(grid, core, depth);
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
        clear_depth(core, depth);
    }
}

fn harmonic_prediction(
    grid: &Grid,
    pyramid: Vec<[f32; 3]>,
    all_regions: &[Region],
    _white_balance: [f32; 3],
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, f32, usize, usize) {
    // Taken by value: the pyramid prediction is this solve's starting point and
    // has no other reader once the harmonic arm is chosen, so copying it would
    // only add a second full grid (~72 MB) to the frame's high-water mark.
    let mut current = pyramid;
    let mut luminance_support = vec![[1.0_f32; 3]; grid.value.len()];
    let mut scratch = RegionScratch::new(grid.value.len());
    let mut fit_quality_sum = 0.0_f64;
    let mut fit_count = 0_usize;
    let mut full_cores = 0_usize;
    let mut solver_fallbacks = 0_usize;

    for region in all_regions {
        let fits = best_fits(grid, region);
        let continuous_support = continuous_luminance_support(grid, region);
        for &i in &region.cells {
            let fully_clipped_fallback = if grid.confidence[i]
                .iter()
                .all(|&confidence| confidence >= 1.0 - EPSILON)
            {
                1.0
            } else {
                0.0
            };
            for (target, support) in continuous_support.iter().copied().enumerate() {
                // Continuous attenuation replaces the deleted hard overwrite,
                // but it still needs defensible colour-line evidence. When no
                // fit clears the established sample/conditioning/R² gates, a
                // partial cell receives no neighbour-imported lift and lands
                // at its sensor floor through recomposition. Only a genuinely
                // fully clipped core may use the smooth neutral fallback.
                let supported = if fits[target].is_some() { support } else { 0.0 };
                luminance_support[i][target] = supported.max(fully_clipped_fallback);
            }
        }
        for fit in fits.iter().flatten() {
            fit_quality_sum += f64::from(fit.r2);
            fit_count += 1;
        }
        let cores = fully_clipped_components(grid, region, &mut scratch);
        full_cores += cores.len();

        if solve_region_direct(grid, region, fits, &mut current, &mut scratch) {
            apply_luminance_domes(grid, &cores, &mut current, &mut scratch.depth);
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
                        let data = fit_data(line, &current, i, grid.valid[i] != [false; 3])
                            .max(grid.floor[i][target]);
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
        apply_luminance_domes(grid, &cores, &mut current, &mut scratch.depth);
    }

    let mean_fit = if fit_count == 0 {
        0.0
    } else {
        (fit_quality_sum / fit_count as f64) as f32
    };
    (
        current,
        luminance_support,
        mean_fit,
        full_cores,
        solver_fallbacks,
    )
}

#[inline]
fn log_chromaticity(pixel: [f32; 3], white_balance: [f32; 3]) -> [f32; 2] {
    let balanced =
        std::array::from_fn::<_, 3, _>(|c| (pixel[c].max(EPSILON) * white_balance[c]).max(EPSILON));
    [
        (balanced[0] / balanced[1]).ln(),
        (balanced[2] / balanced[1]).ln(),
    ]
}

#[inline]
fn chroma_cross(origin: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - origin[0]) * (b[1] - origin[1]) - (a[1] - origin[1]) * (b[0] - origin[0])
}

fn chroma_hull(mut points: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup_by(|a, b| (a[0] - b[0]).abs() < 1.0e-6 && (a[1] - b[1]).abs() < 1.0e-6);
    if points.len() <= 2 {
        return points;
    }
    let mut lower = Vec::with_capacity(points.len());
    for &point in &points {
        while lower.len() >= 2
            && chroma_cross(lower[lower.len() - 2], lower[lower.len() - 1], point) <= 0.0
        {
            lower.pop();
        }
        lower.push(point);
    }
    let mut upper = Vec::with_capacity(points.len());
    for &point in points.iter().rev() {
        while upper.len() >= 2
            && chroma_cross(upper[upper.len() - 2], upper[upper.len() - 1], point) <= 0.0
        {
            upper.pop();
        }
        upper.push(point);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn closest_on_segment(point: [f32; 2], a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    let edge = [b[0] - a[0], b[1] - a[1]];
    let length_squared = edge[0] * edge[0] + edge[1] * edge[1];
    if length_squared <= EPSILON {
        return a;
    }
    let t = (((point[0] - a[0]) * edge[0] + (point[1] - a[1]) * edge[1]) / length_squared)
        .clamp(0.0, 1.0);
    [a[0] + t * edge[0], a[1] + t * edge[1]]
}

fn nearest_chroma_envelope(point: [f32; 2], hull: &[[f32; 2]]) -> [f32; 2] {
    if hull.len() >= 3
        && (0..hull.len())
            .all(|i| chroma_cross(hull[i], hull[(i + 1) % hull.len()], point) >= -1.0e-6)
    {
        return point;
    }
    let mut nearest = hull[0];
    let mut distance_squared = f32::INFINITY;
    let edges = if hull.len() == 1 { 1 } else { hull.len() };
    for i in 0..edges {
        let candidate = closest_on_segment(point, hull[i], hull[(i + 1) % hull.len()]);
        let squared = (point[0] - candidate[0]).powi(2) + (point[1] - candidate[1]).powi(2);
        if squared < distance_squared {
            nearest = candidate;
            distance_squared = squared;
        }
    }
    nearest
}

/// Give one continuous raw-domain estimator authority over chromaticity for a
/// complete affected component. The older affine/harmonic result supplies
/// luminance, spatial detail, and a weak initialization only. Pairwise sensor
/// evidence remains continuous through `t_c=(1-c_c)^2`, including the R/B
/// constraint that survives when green is clipped.
fn joint_log_chromaticity(
    grid: &Grid,
    all_regions: &[Region],
    luminance_prediction: &[[f32; 3]],
    luminance_support: &[[f32; 3]],
    white_balance: [f32; 3],
) -> Vec<[f32; 3]> {
    const SWEEPS: usize = 160;
    const DATA_WEIGHT: f32 = 12.0;
    const PRIOR_WEIGHT: f32 = 0.02;

    let white_balance = usable_white_balance(white_balance);
    let initial_uv: Vec<[f32; 2]> = luminance_prediction
        .iter()
        .map(|&pixel| log_chromaticity(pixel, white_balance))
        .collect();
    let mut current_uv = initial_uv.clone();
    let mut next_uv = current_uv.clone();
    let guide: Vec<f32> = luminance_prediction
        .iter()
        .map(|pixel| {
            (pixel[0] * white_balance[0]
                + pixel[1] * white_balance[1]
                + pixel[2] * white_balance[2])
                / 3.0
        })
        .collect();

    // One cell's share of the Jacobi system. Every field reads only `grid`,
    // `guide` and `initial_uv`, so all of it is invariant across sweeps.
    struct CellSystem {
        index: u32,
        a: f32,
        d: f32,
        off: f32,
        determinant: f32,
        rhs_u: f32,
        rhs_v: f32,
        neighbors: [u32; 4],
        weights: [f32; 4],
        neighbor_count: u8,
        has_data_authority: bool,
    }

    for region in all_regions {
        // Hoist the sweep-invariant work out of the sweep. The body below used
        // to recompute about thirty transcendentals per cell on every one of up
        // to SWEEPS passes -- five log_chromaticity calls and four exp/sqrt
        // groups -- although only `current_uv[n]` varies between sweeps.
        // Accumulation order is unchanged, so the result is bit-identical.
        // Regions are connected components and this pass writes nothing, so it
        // parallelises cleanly; `with_min_len` keeps small regions sequential.
        let systems: Vec<CellSystem> = region
            .cells
            .par_iter()
            .with_min_len(256)
            .map(|&i| {
                let x = i % grid.width;
                let y = i / grid.width;
                let observed = log_chromaticity(grid.value[i], white_balance);
                let trust = grid.trust[i];
                let w_rg = DATA_WEIGHT * trust[0] * trust[1];
                let w_bg = DATA_WEIGHT * trust[2] * trust[1];
                let w_rb = DATA_WEIGHT * trust[0] * trust[2];

                let mut a = PRIOR_WEIGHT + w_rg + w_rb;
                let mut d = PRIOR_WEIGHT + w_bg + w_rb;
                let off = -w_rb;
                let rhs_u = PRIOR_WEIGHT * initial_uv[i][0]
                    + w_rg * observed[0]
                    + w_rb * (observed[0] - observed[1]);
                let rhs_v = PRIOR_WEIGHT * initial_uv[i][1] + w_bg * observed[1]
                    - w_rb * (observed[0] - observed[1]);

                let mut neighbors = [0_u32; 4];
                let mut weights = [0.0_f32; 4];
                let mut neighbor_count = 0_usize;
                let guide_scale = guide[i].abs().max(0.05);
                for (dx, dy) in [(1_isize, 0_isize), (-1, 0), (0, 1), (0, -1)] {
                    let nx = mirror(x as isize + dx, grid.width);
                    let ny = mirror(y as isize + dy, grid.height);
                    let n = ny * grid.width + nx;
                    if n == i {
                        continue;
                    }
                    let delta = (guide[i] - guide[n]).abs();
                    let neighbor_observed = log_chromaticity(grid.value[n], white_balance);
                    let ratios = [
                        (
                            observed[0],
                            neighbor_observed[0],
                            trust[0] * trust[1],
                            grid.trust[n][0] * grid.trust[n][1],
                        ),
                        (
                            observed[1],
                            neighbor_observed[1],
                            trust[2] * trust[1],
                            grid.trust[n][2] * grid.trust[n][1],
                        ),
                        (
                            observed[0] - observed[1],
                            neighbor_observed[0] - neighbor_observed[1],
                            trust[0] * trust[2],
                            grid.trust[n][0] * grid.trust[n][2],
                        ),
                    ];
                    let mut chroma_delta = 0.0_f32;
                    let mut chroma_mass = 0.0_f32;
                    for (here, there, here_weight, there_weight) in ratios {
                        let evidence = (here_weight * there_weight).sqrt();
                        chroma_delta += evidence * (here - there).abs();
                        chroma_mass += evidence;
                    }
                    let chroma_affinity = if chroma_mass > EPSILON {
                        (-(chroma_delta / chroma_mass / 0.20).powi(2)).exp()
                    } else {
                        // Different saturated colours can have no common
                        // trusted pair (for example R-clipped beside
                        // G-clipped). In that case the harmonic initializer is
                        // only an edge detector: a large chroma edge blocks
                        // coupling, while the much smaller clip-state bias on
                        // a smooth sky still passes through.
                        let initial_delta = (initial_uv[i][0] - initial_uv[n][0])
                            .hypot(initial_uv[i][1] - initial_uv[n][1]);
                        (-(initial_delta / 0.50).powi(2)).exp()
                    };
                    let spatial = ((-(delta / (GUIDE_K * guide_scale)).powi(2)).exp()
                        * chroma_affinity)
                        .max(WEIGHT_FLOOR);
                    a += spatial;
                    d += spatial;
                    neighbors[neighbor_count] = n as u32;
                    weights[neighbor_count] = spatial;
                    neighbor_count += 1;
                }

                CellSystem {
                    index: i as u32,
                    a,
                    d,
                    off,
                    determinant: a * d - off * off,
                    rhs_u,
                    rhs_v,
                    neighbors,
                    weights,
                    neighbor_count: neighbor_count as u8,
                    has_data_authority: w_rg + w_bg + w_rb > EPSILON,
                }
            })
            .collect();

        for _ in 0..SWEEPS {
            let mut max_delta = 0.0_f32;
            for system in &systems {
                let i = system.index as usize;
                let mut rhs_u = system.rhs_u;
                let mut rhs_v = system.rhs_v;
                for slot in 0..system.neighbor_count as usize {
                    let n = system.neighbors[slot] as usize;
                    rhs_u += system.weights[slot] * current_uv[n][0];
                    rhs_v += system.weights[slot] * current_uv[n][1];
                }

                if system.determinant <= EPSILON || !system.determinant.is_finite() {
                    next_uv[i] = current_uv[i];
                    continue;
                }
                let solved = [
                    ((rhs_u * system.d - system.off * rhs_v) / system.determinant).clamp(-8.0, 8.0),
                    ((system.a * rhs_v - system.off * rhs_u) / system.determinant).clamp(-8.0, 8.0),
                ];
                max_delta = max_delta
                    .max((solved[0] - current_uv[i][0]).abs())
                    .max((solved[1] - current_uv[i][1]).abs());
                next_uv[i] = solved;
            }
            // Publish only this region's own cells. Swapping the whole
            // buffers would leave an already-finished region reading back as
            // either its final or its one-sweep-stale value, decided by the
            // parity of the sweep counts taken by later, unrelated regions.
            for &i in &region.cells {
                current_uv[i] = next_uv[i];
            }
            if max_delta < 1.0e-5 {
                break;
            }
        }

        // The joint field normally owns chromaticity without a binary clamp.
        // One pathological case has no such authority: every pairwise data
        // weight is zero and every spatial edge has collapsed to WEIGHT_FLOOR.
        // There the weak affine/harmonic initializer would otherwise be about
        // 98% of the system and could invent a hue unsupported by any measured
        // boundary. Bound only that evidence-free state in the same log-chroma
        // coordinates as the joint solve; ordinary continuous-confidence cells
        // remain entirely unconstrained by this compatibility guard.
        let boundary_hull = chroma_hull(
            region
                .boundary
                .iter()
                .filter(|&&i| {
                    let trust = grid.trust[i];
                    trust[0] * trust[1] + trust[2] * trust[1] + trust[0] * trust[2] > EPSILON
                })
                .map(|&i| log_chromaticity(grid.value[i], white_balance))
                .collect(),
        );
        if !boundary_hull.is_empty() {
            for system in &systems {
                if system.has_data_authority
                    || system.weights[..system.neighbor_count as usize]
                        .iter()
                        .any(|&weight| weight > WEIGHT_FLOOR)
                {
                    continue;
                }
                let i = system.index as usize;
                current_uv[i] = nearest_chroma_envelope(current_uv[i], &boundary_hull);
            }
        }
    }

    let mut output = luminance_prediction.to_vec();
    let deep_probe: Vec<usize> = std::env::var("RAW_AUTOTUNE_PROBE_GRID")
        .ok()
        .map(|spec| {
            spec.split(';')
                .filter_map(|pair| {
                    let mut it = pair.split(',');
                    let x = it.next()?.trim().parse::<usize>().ok()?;
                    let y = it.next()?.trim().parse::<usize>().ok()?;
                    (x < grid.width && y < grid.height).then_some(y * grid.width + x)
                })
                .collect()
        })
        .unwrap_or_default();
    for (region_id, region) in all_regions.iter().enumerate() {
        // A component containing several real colours must not let one colour
        // line become another's missing channel. Measure component
        // heterogeneity from the same confidence-weighted pairwise evidence;
        // it only attenuates unsupported lift and never supplies a colour.
        let mut moments = [[0.0_f64; 3]; 3];
        for &i in &region.cells {
            let uv = log_chromaticity(grid.value[i], white_balance);
            let ratios = [uv[0], uv[1], uv[0] - uv[1]];
            let weights = [
                grid.trust[i][0] * grid.trust[i][1],
                grid.trust[i][2] * grid.trust[i][1],
                grid.trust[i][0] * grid.trust[i][2],
            ];
            for pair in 0..3 {
                let w = f64::from(weights[pair]);
                let value = f64::from(ratios[pair]);
                moments[pair][0] += w;
                moments[pair][1] += w * value;
                moments[pair][2] += w * value * value;
            }
        }
        let chroma_std = moments
            .iter()
            .filter(|m| m[0] > f64::from(EPSILON))
            .map(|m| {
                let mean = m[1] / m[0];
                (m[2] / m[0] - mean * mean).max(0.0).sqrt() as f32
            })
            .fold(0.0_f32, f32::max);
        let component_coherence = (-(chroma_std / 0.35).powi(2)).exp();
        if !deep_probe.is_empty() {
            for &p in &deep_probe {
                if region.cells.contains(&p) {
                    eprintln!(
                        "DEEPPROBE cell {p} ({},{}) region {region_id} cells {} boundary {} chroma_std {chroma_std:.4} coherence {component_coherence:.4}",
                        p % grid.width,
                        p / grid.width,
                        region.cells.len(),
                        region.boundary.len()
                    );
                }
            }
        }
        for &i in &region.cells {
            let q = [current_uv[i][0].exp(), 1.0, current_uv[i][1].exp()];
            let observed = std::array::from_fn::<_, 3, _>(|c| grid.value[i][c] * white_balance[c]);
            let harmonic =
                std::array::from_fn::<_, 3, _>(|c| luminance_prediction[i][c] * white_balance[c]);
            let trusted_numerator = (0..3)
                .map(|c| grid.trust[i][c] * q[c] * observed[c])
                .sum::<f32>();
            let trusted_denominator = (0..3).map(|c| grid.trust[i][c] * q[c] * q[c]).sum::<f32>();
            // Preserve the harmonic path's white-balanced mean luminance when
            // chromaticity changes. This is the explicit luma/chroma ownership
            // split: the joint solve may rotate q, but it cannot independently
            // manufacture a brighter spatial field.
            let harmonic_scalar = harmonic.iter().sum::<f32>() / q.iter().sum::<f32>().max(EPSILON);
            let scalar = if trusted_denominator > EPSILON {
                let trusted_scalar = trusted_numerator / trusted_denominator;
                let trusted_mass = grid.trust[i].iter().sum::<f32>();
                let authority = trusted_mass / (trusted_mass + 0.05);
                (authority * trusted_scalar + (1.0 - authority) * harmonic_scalar)
                    .min(1.2 * harmonic_scalar)
            } else {
                // Only a fully clipped core with no pairwise evidence reaches
                // this path. The harmonic luma field is a smooth neutral-safe
                // fallback; boundary chromaticity still arrives spatially.
                harmonic_scalar
            };
            // The harmonic/pyramid estimator owns the spatial-luminance
            // envelope. Joint chromaticity may redistribute a requested lift,
            // but must not turn that weak colour prior into extra peak energy.
            let luminance_envelope = (0..3)
                .map(|c| harmonic[c] / q[c].max(EPSILON))
                .fold(f32::INFINITY, f32::min);
            let scalar = scalar.min(luminance_envelope);
            // The heterogeneity guard used to attenuate toward the clipped
            // floor. But the floor's chroma is the one colour known to be
            // wrong — it records which channel clipped first, not the scene —
            // and on mixed indoor-outdoor components the guard therefore
            // painted whole openings magenta
            // (docs/evidence/phase4-backlit-20260827/ablation-grid.png). The
            // guard still discounts the joint chroma exactly as before in
            // incoherent components; its fallback is now neutral chroma at
            // the harmonic luminance — the least-commitment colour the sensor
            // evidence supports there, and how reference renderers treat
            // unknowable blown colour. Coherent components are unchanged in
            // the coherence→1 limit.
            //
            // The neutral fallback is only reached in proportion to
            // `luminance_support`, because the recomposition below starts from
            // `grid.floor` — and that base carries the same wrong chroma the
            // fallback exists to remove. Where the colour-line evidence is
            // partial (support around 0.55 on _DSC1282's canopy sky) an
            // incoherent component therefore kept most of the clipped floor's
            // magenta even after the target was neutralized. So the base is
            // neutralized the same way, to the *least-commitment* neutral:
            // every clipped floor is a lower bound on its channel, so the
            // dimmest neutral consistent with all three bounds is
            // `clip_neutral`, the largest white-balanced floor. It claims no
            // luminance the clip does not already prove, and a channel that
            // attains the maximum keeps exactly its own floor. `neutral` can
            // no longer sit below that base, so the support blend spans a
            // non-negative interval as before.
            let clip_neutral = (0..3)
                .map(|c| grid.floor[i][c] * white_balance[c])
                .fold(0.0_f32, f32::max);
            let neutral_luminance = (harmonic.iter().sum::<f32>() / 3.0).max(clip_neutral);
            for c in 0..3 {
                let predicted = (scalar * q[c] / white_balance[c]).max(grid.floor[i][c]);
                let neutral = (neutral_luminance / white_balance[c]).max(grid.floor[i][c]);
                let neutral_base = (clip_neutral / white_balance[c]).max(grid.floor[i][c]);
                let base = component_coherence * grid.floor[i][c]
                    + (1.0 - component_coherence) * neutral_base;
                let target =
                    component_coherence * predicted + (1.0 - component_coherence) * neutral;
                output[i][c] = base + luminance_support[i][c] * (target - base).max(0.0);
            }
            if !deep_probe.is_empty() && deep_probe.contains(&i) {
                eprintln!(
                    "DEEPPROBE cell {i} q {q:?} scalar {scalar:.5} clip_neutral {clip_neutral:.5} neutral_lum {neutral_luminance:.5} wb {white_balance:?} obs {observed:?} harmonic {harmonic:?} floor {:?} support {:?} out {:?}",
                    grid.floor[i], luminance_support[i], output[i]
                );
            }
        }
    }
    output
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
            let confidence = confidence_at(context.original, context.confidence, i);
            if confidence == 0.0 {
                continue;
            }
            let Some(c) = channel(context.cfa.cfa_color_at(y, x)) else {
                continue;
            };
            let cell = (y / 2) * context.grid_width + x / 2;
            let lift = (prediction[cell][c] - context.original[i]).max(0.0);
            let target = context.original[i] + confidence * lift;
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
/// Diagnostic stage dump, active only when `RAW_AUTOTUNE_HIGHLIGHT_DEBUG`
/// names a directory. White-balanced, tone-compressed to make chroma errors
/// visible; values above clip roll off instead of wrapping. Never touches the
/// reconstruction itself.
fn debug_dump_grid(stage: &str, grid: &Grid, data: &[[f32; 3]], white_balance: [f32; 3]) {
    let Ok(directory) = std::env::var("RAW_AUTOTUNE_HIGHLIGHT_DEBUG") else {
        return;
    };
    let white_balance = usable_white_balance(white_balance);
    let mut img = image::RgbImage::new(grid.width as u32, grid.height as u32);
    for y in 0..grid.height {
        for x in 0..grid.width {
            let v = data[y * grid.width + x];
            let encode = |value: f32, gain: f32| -> u8 {
                let balanced = (value * gain).max(0.0);
                // Reinhard-style rolloff so >1 values stay distinguishable.
                let compressed = balanced / (1.0 + 0.5 * balanced);
                ((compressed / (1.0 / 1.5)).min(1.0).powf(1.0 / 2.2) * 255.0) as u8
            };
            img.put_pixel(
                x as u32,
                y as u32,
                image::Rgb([
                    encode(v[0], white_balance[0]),
                    encode(v[1], white_balance[1]),
                    encode(v[2], white_balance[2]),
                ]),
            );
        }
    }
    let _ = std::fs::create_dir_all(&directory);
    let _ = img.save(format!("{directory}/{stage}.png"));
}

/// Companion to [`debug_dump_grid`]: per-channel trust as an RGB map.
fn debug_dump_trust(grid: &Grid) {
    let Ok(directory) = std::env::var("RAW_AUTOTUNE_HIGHLIGHT_DEBUG") else {
        return;
    };
    let mut img = image::RgbImage::new(grid.width as u32, grid.height as u32);
    for y in 0..grid.height {
        for x in 0..grid.width {
            let t = grid.trust[y * grid.width + x];
            img.put_pixel(
                x as u32,
                y as u32,
                image::Rgb([
                    (t[0] * 255.0) as u8,
                    (t[1] * 255.0) as u8,
                    (t[2] * 255.0) as u8,
                ]),
            );
        }
    }
    let _ = std::fs::create_dir_all(&directory);
    let _ = img.save(format!("{directory}/04-trust.png"));
}

pub fn reconstruct_cfa(
    samples: &mut [f32],
    width: usize,
    height: usize,
    cfa: &CFA,
    confidence: Option<&[f32]>,
    options: ReconstructionOptions,
) -> Result<RawHighlightReport> {
    let ReconstructionOptions {
        white_balance,
        strength,
        method,
        clipped_fraction_floor,
    } = options;
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
    let strength = strength.clamp(0.0, 1.0);
    // One pass, not two: both counts read the same per-sample confidence, and
    // at 24 M samples the second scan is pure duplicate work. A count and an
    // "any" are both order-independent, so splitting the scan across the pool
    // reports the same pair the serial walk did.
    let (clipped_cfa_sites, has_reconstruction_authority) = (0..samples.len())
        .into_par_iter()
        .map(|i| {
            let site = confidence_at(samples, confidence, i);
            (usize::from(site >= VALID_CONFIDENCE_MAX), site > 0.0)
        })
        .reduce(|| (0_usize, false), |a, b| (a.0 + b.0, a.1 || b.1));
    if !has_reconstruction_authority || strength == 0.0 {
        return Ok(RawHighlightReport {
            method,
            clipped_cfa_sites,
            ..RawHighlightReport::default()
        });
    }

    // Archive-speed gate. Everything below this point is full-image work whose
    // cost does not scale with how much of the frame is actually clipped, so a
    // frame with essentially no clipped sites pays ~20 s and ~1.3 GB for a
    // result indistinguishable from the cheap post-demosaic estimator. Declining
    // here reports `Current`, and `color::develop` reads the *reported* method,
    // so the frame takes the whole `Current` path and is byte-identical to an
    // explicit `--highlight-method current` run. The samples are untouched.
    if clipped_fraction_floor > 0.0
        && (clipped_cfa_sites as f32) < clipped_fraction_floor * samples.len() as f32
    {
        return Ok(RawHighlightReport {
            method: HighlightMethod::Current,
            clipped_cfa_sites,
            ..RawHighlightReport::default()
        });
    }

    // Taken here rather than before the gate above: this is the first point at
    // which the pre-reconstruction mosaic is actually read, and a declined
    // frame would otherwise pay ~97 MB and a full copy for nothing.
    let original = samples.to_vec();

    // Build one full-strength target, then blend the complete knee + spatial
    // result once below. This keeps strength 1 byte-identical while giving an
    // eventual fractional API one linear, stage-order-independent meaning.
    let knee_corrected_sites = if method == HighlightMethod::Harmonic {
        apply_sensor_knee(samples, &original, confidence, width, height, cfa)
    } else {
        0
    };
    let grid = build_grid(samples, confidence, width, height, cfa);
    let pyramid = build_pyramid(&grid);
    let pyramid_prediction = pyramid_prediction(&grid, &pyramid);
    // The level stack is the coarse-colour source for that one call and nothing
    // else. Returning it here (~190 MB on a 24 megapixel frame) keeps it out of
    // the harmonic solve, which is where the process's high-water mark sits.
    drop(pyramid);
    let all_regions = regions(&grid);
    debug_dump_grid("00-grid-value", &grid, &grid.value, white_balance);
    debug_dump_grid("01-pyramid", &grid, &pyramid_prediction, white_balance);
    debug_dump_trust(&grid);
    let probe_cells: Vec<(usize, usize)> = std::env::var("RAW_AUTOTUNE_PROBE_GRID")
        .ok()
        .map(|spec| {
            spec.split(';')
                .filter_map(|pair| {
                    let mut it = pair.split(',');
                    Some((
                        it.next()?.trim().parse::<usize>().ok()?,
                        it.next()?.trim().parse::<usize>().ok()?,
                    ))
                })
                .filter(|&(x, y)| x < grid.width && y < grid.height)
                .collect()
        })
        .unwrap_or_default();
    for &(x, y) in &probe_cells {
        let i = y * grid.width + x;
        eprintln!(
            "GRIDPROBE ({x},{y}) value {:?} trust {:?} valid {:?} floor {:?} pyramid {:?}",
            grid.value[i], grid.trust[i], grid.valid[i], grid.floor[i], pyramid_prediction[i],
        );
    }
    let (prediction, mean_fit_quality, fully_clipped_cores, solver_fallbacks) = match method {
        HighlightMethod::RawPyramid => (pyramid_prediction, 0.0, 0, 0),
        HighlightMethod::Harmonic => {
            let (luminance, support, fit, cores, fallbacks) =
                harmonic_prediction(&grid, pyramid_prediction, &all_regions, white_balance);
            debug_dump_grid("02-harmonic", &grid, &luminance, white_balance);
            for &(x, y) in &probe_cells {
                let i = y * grid.width + x;
                eprintln!(
                    "GRIDPROBE ({x},{y}) harmonic {:?} support {:?}",
                    luminance[i], support[i]
                );
            }
            let prediction =
                joint_log_chromaticity(&grid, &all_regions, &luminance, &support, white_balance);
            debug_dump_grid("03-joint", &grid, &prediction, white_balance);
            for &(x, y) in &probe_cells {
                let i = y * grid.width + x;
                eprintln!("GRIDPROBE ({x},{y}) joint {:?}", prediction[i]);
            }
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
    if std::env::var("RAW_AUTOTUNE_HIGHLIGHT_DEBUG").is_ok() {
        // Rebuild a grid from the applied mosaic so the dump shows what the
        // demosaic will actually receive, not the idealized prediction.
        let applied = build_grid(samples, confidence, width, height, cfa);
        debug_dump_grid("05-applied", &applied, &applied.value, white_balance);
        let mut unweighted = vec![[0.0_f32; 3]; applied.value.len()];
        let mut counts = vec![[0_u8; 3]; applied.value.len()];
        for y in 0..height {
            for x in 0..width {
                if let Some(c) = channel(cfa.cfa_color_at(y, x)) {
                    let cell = (y / 2) * applied.width + x / 2;
                    unweighted[cell][c] += samples[y * width + x];
                    counts[cell][c] += 1;
                }
            }
        }
        for (cell, count) in unweighted.iter_mut().zip(&counts) {
            for c in 0..3 {
                if count[c] > 0 {
                    cell[c] /= f32::from(count[c]);
                }
            }
        }
        debug_dump_grid(
            "06-applied-unweighted",
            &applied,
            &unweighted,
            white_balance,
        );
    }
    if strength < 1.0 {
        for (sample, &source) in samples.iter_mut().zip(&original) {
            *sample = source + strength * (*sample - source);
        }
    }
    let max_lift = strength * max_lift;

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
            let report = reconstruct_cfa(
                &mut candidate,
                width,
                height,
                &rggb(),
                None,
                ReconstructionOptions {
                    white_balance: [1.0; 3],
                    strength: 1.0,
                    method,
                    clipped_fraction_floor: 0.0,
                },
            )
            .expect("valid CFA");
            assert_eq!(candidate, source);
            assert_eq!(report.reconstructed_cfa_sites, 0);
        }
    }

    /// The archive-speed floor: a frame with a handful of clipped sites must
    /// decline the solve, leave every sample untouched, and *say* it declined by
    /// reporting `Current`, because `color::develop` routes the post-demosaic
    /// estimator off that field. A frame over the floor must be unaffected by
    /// the floor's existence.
    #[test]
    fn the_spatial_floor_declines_sparse_clipping_and_reports_current() {
        let width = 64;
        let height = 64;
        // One clipped site in 4096, i.e. a clipped fraction of 2.4e-4.
        let mut sparse = mosaic(width, height, |x, y| {
            let v = 0.1 + 0.4 * (x + y) as f32 / (width + height) as f32;
            [v * 0.7, v * 0.9, v]
        });
        sparse[width * (height / 2) + width / 2] = 1.0;
        let before = sparse.clone();

        // Below the floor: declined, untouched, reported as `Current`, and the
        // clipped count is still measured so a future sweep has the statistic.
        let mut candidate = sparse.clone();
        let declined = reconstruct_cfa(
            &mut candidate,
            width,
            height,
            &rggb(),
            None,
            ReconstructionOptions {
                white_balance: [1.0; 3],
                strength: 1.0,
                method: HighlightMethod::Harmonic,
                clipped_fraction_floor: 1.0e-3,
            },
        )
        .expect("valid CFA");
        assert_eq!(candidate, before, "a declined frame must not move");
        assert_eq!(declined.method, HighlightMethod::Current);
        assert_eq!(declined.clipped_cfa_sites, 1);
        assert_eq!(declined.reconstructed_cfa_sites, 0);

        // The same frame with the floor at the shipped default, which this
        // fraction clears: the solve runs and reports the method it was asked
        // for. Whether it changes a sample is the estimator's business, not the
        // gate's.
        let mut solved = sparse.clone();
        let ran = reconstruct_cfa(
            &mut solved,
            width,
            height,
            &rggb(),
            None,
            ReconstructionOptions {
                white_balance: [1.0; 3],
                strength: 1.0,
                method: HighlightMethod::Harmonic,
                clipped_fraction_floor: DEFAULT_SPATIAL_CLIPPED_FLOOR,
            },
        )
        .expect("valid CFA");
        assert_eq!(ran.method, HighlightMethod::Harmonic);

        // And `0` is the documented always-solve override.
        let mut forced = sparse;
        let unfloored = reconstruct_cfa(
            &mut forced,
            width,
            height,
            &rggb(),
            None,
            ReconstructionOptions {
                white_balance: [1.0; 3],
                strength: 1.0,
                method: HighlightMethod::Harmonic,
                clipped_fraction_floor: 0.0,
            },
        )
        .expect("valid CFA");
        assert_eq!(unfloored.method, HighlightMethod::Harmonic);
        assert_eq!(
            forced, solved,
            "the floor must not change what the solver does"
        );
    }

    #[test]
    fn split_green_quad_uses_continuous_value_but_is_not_wholly_measured() {
        let samples = vec![0.30, 0.93, 0.96, 0.40];
        let confidence = vec![0.0, 0.10, 1.0, 0.0];
        let grid = build_grid(&samples, Some(&confidence), 2, 2, &rggb());

        assert_eq!(grid.floor[0][1], 0.96);
        assert_eq!(grid.confidence[0][1], 0.55);
        assert!(!grid.valid[0][1]);
        assert_eq!(grid.value[0][1], 0.93);
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
            reconstruct_cfa(
                &mut candidate,
                width,
                height,
                &rggb(),
                None,
                ReconstructionOptions {
                    white_balance: [1.0; 3],
                    strength: 1.0,
                    method,
                    clipped_fraction_floor: 0.0,
                },
            )
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
    fn zero_confidence_and_zero_strength_are_exact_no_ops() {
        let width = 48;
        let height = 48;
        // A frame the reconstructor genuinely acts on. A flat buffer would trip
        // `reconstruct_cfa`'s own early return, so the assertions below would
        // prove only that the early return works -- not the per-pixel
        // invariant, which lives past it.
        let truth = mosaic(width, height, |x, _| {
            let value = 0.60 + 0.95 * x as f32 / (width - 1) as f32;
            [value * 0.74, value, value * 0.95]
        });
        let captured: Vec<f32> = truth.iter().map(|value| value.min(1.0)).collect();
        let options = |strength| ReconstructionOptions {
            white_balance: [2.3632812, 1.0, 1.8085938],
            strength,
            method: HighlightMethod::Harmonic,
            clipped_fraction_floor: 0.0,
        };

        // Unless the fixture moves at full strength, neither case below means
        // anything.
        let mut moved = captured.clone();
        reconstruct_cfa(&mut moved, width, height, &rggb(), None, options(1.0)).unwrap();
        assert!(
            moved != captured,
            "the fixture must exercise reconstruction"
        );

        // Strength zero, on a frame that otherwise changes.
        let mut candidate = captured.clone();
        reconstruct_cfa(&mut candidate, width, height, &rggb(), None, options(0.0)).unwrap();
        assert_eq!(candidate, captured, "strength 0 moved a sample");

        // Per pixel: sites handed an explicit zero stay byte-exact while the
        // sites interleaved with them reconstruct normally.
        let confidence: Vec<f32> = captured
            .iter()
            .enumerate()
            .map(|(i, &value)| {
                if i % 3 == 0 {
                    0.0
                } else {
                    crate::highlight::clip_confidence(value)
                }
            })
            .collect();
        let mut candidate = captured.clone();
        reconstruct_cfa(
            &mut candidate,
            width,
            height,
            &rggb(),
            Some(&confidence),
            options(1.0),
        )
        .unwrap();
        let mut pinned = 0_usize;
        for i in (0..captured.len()).step_by(3) {
            assert_eq!(candidate[i], captured[i], "zero-confidence site {i} moved");
            pinned += 1;
        }
        assert!(pinned > 0);
        assert!(
            candidate != captured,
            "the rest of the frame must still reconstruct"
        );
    }

    #[test]
    fn fractional_strength_blends_the_complete_full_target_once() {
        let width = 48;
        let height = 48;
        let truth = mosaic(width, height, |x, _| {
            let value = 0.60 + 0.95 * x as f32 / (width - 1) as f32;
            [value * 0.74, value, value * 0.95]
        });
        let captured: Vec<f32> = truth.iter().map(|value| value.min(1.0)).collect();
        let options = |strength| ReconstructionOptions {
            white_balance: [2.3632812, 1.0, 1.8085938],
            strength,
            method: HighlightMethod::Harmonic,
            clipped_fraction_floor: 0.0,
        };

        let mut full = captured.clone();
        reconstruct_cfa(&mut full, width, height, &rggb(), None, options(1.0)).unwrap();
        let mut half = captured.clone();
        reconstruct_cfa(&mut half, width, height, &rggb(), None, options(0.5)).unwrap();

        assert_ne!(full, captured, "the fixture must exercise reconstruction");
        for i in 0..captured.len() {
            assert_eq!(half[i], captured[i] + 0.5 * (full[i] - captured[i]));
        }
    }

    /// Two connected components never share a 4-neighbour, so the joint solve
    /// for one cannot legitimately depend on another. It did: the sweep loop
    /// swapped the whole `current`/`next` buffers, so a finished region read
    /// back as final or one-sweep-stale according to the parity of the sweep
    /// counts later regions happened to take. Region order is the cheapest
    /// probe for that -- it is invariant now and was not before.
    #[test]
    fn joint_chromaticity_does_not_depend_on_the_order_of_other_regions() {
        let width = 64;
        let height = 48;
        let white_balance = [2.3632812, 1.0, 1.8085938];
        let blob = |x: usize, y: usize, cx: f32, cy: f32| {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            (-(dx * dx + dy * dy) / 40.0).exp()
        };
        let truth = mosaic(width, height, |x, y| {
            let value = 0.42 + 1.10 * (blob(x, y, 14.0, 24.0) + blob(x, y, 49.0, 22.0));
            [value * 0.74, value, value * 0.95]
        });
        let captured: Vec<f32> = truth.iter().map(|value| value.min(1.0)).collect();

        let grid = build_grid(&captured, None, width, height, &rggb());
        let pyramid = build_pyramid(&grid);
        let pyramid_prediction = pyramid_prediction(&grid, &pyramid);
        let mut all_regions = regions(&grid);
        assert!(
            all_regions.len() >= 2,
            "the fixture needs several regions, found {}",
            all_regions.len()
        );
        let (luminance, support, ..) = harmonic_prediction(
            &grid,
            pyramid_prediction.clone(),
            &all_regions,
            white_balance,
        );

        let forward =
            joint_log_chromaticity(&grid, &all_regions, &luminance, &support, white_balance);
        all_regions.reverse();
        let reversed =
            joint_log_chromaticity(&grid, &all_regions, &luminance, &support, white_balance);
        assert_eq!(
            forward, reversed,
            "a region's field moved with region order"
        );
    }

    #[test]
    fn evidence_free_joint_cell_cannot_invent_chroma_beyond_its_boundary() {
        let boundary = [0.30, 0.60, 1.00];
        let value = vec![boundary, boundary, boundary, [1.0, 0.60, 1.00]];
        let grid = Grid {
            width: 4,
            height: 1,
            floor: value.clone(),
            value,
            confidence: vec![[0.0; 3], [0.0; 3], [0.0; 3], [1.0; 3]],
            trust: vec![[1.0; 3], [1.0; 3], [1.0; 3], [0.0; 3]],
            valid: vec![[true; 3], [true; 3], [true; 3], [false; 3]],
        };
        let region = Region {
            cells: vec![3],
            boundary: vec![0, 1, 2],
        };
        let luminance = vec![boundary, boundary, boundary, [2.4, 0.60, 1.00]];
        let support = vec![[1.0; 3]; 4];

        let current = joint_log_chromaticity(&grid, &[region], &luminance, &support, [1.0; 3]);
        assert_eq!(current[3][1], 0.60, "measured green moved");
        assert_eq!(current[3][2], 1.00, "measured blue moved");
        assert!(
            current[3][0] < 1.2,
            "unsupported magenta lift survived: {:?}",
            current[3]
        );
        assert!(current[3][0] >= grid.floor[3][0]);
    }

    #[test]
    fn partial_support_in_an_incoherent_component_does_not_keep_the_clipped_chroma() {
        // Two measured cells of very different colour make the component
        // incoherent (chroma_std well past 0.35), so the heterogeneity guard
        // hands the third cell to the neutral fallback. That cell has a
        // measured red and a clipped green and blue, and only half the
        // colour-line support, which is exactly _DSC1282's canopy sky. The
        // white-balanced blue floor proves 2.0, so green must reach it rather
        // than stay near its own 1.0 clip and render magenta.
        let white_balance = [2.0_f32, 1.0, 2.0];
        let value = vec![
            [0.10, 0.50, 0.10],
            [0.50, 0.10, 0.10],
            [0.40, 1.00, 1.00],
            [0.20, 0.30, 0.20],
        ];
        let grid = Grid {
            width: 4,
            height: 1,
            floor: value.clone(),
            value,
            confidence: vec![[0.0; 3], [0.0; 3], [0.0, 1.0, 1.0], [0.0; 3]],
            trust: vec![[1.0; 3], [1.0; 3], [1.0, 0.0, 0.0], [1.0; 3]],
            valid: vec![[true; 3], [true; 3], [true, false, false], [true; 3]],
        };
        let region = Region {
            cells: vec![0, 1, 2],
            boundary: vec![3],
        };
        let luminance = grid.value.clone();
        let support = vec![[0.5_f32; 3]; 4];

        let current = joint_log_chromaticity(&grid, &[region], &luminance, &support, white_balance);
        let balanced: [f32; 3] = std::array::from_fn(|c| current[2][c] * white_balance[c]);
        assert!(
            balanced[1] > 1.8,
            "clipped green stayed near its own clip: {balanced:?}"
        );
        let spread = balanced.iter().copied().fold(f32::MIN, f32::max)
            - balanced.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            spread < 0.1,
            "the neutral fallback still carried clipped chroma: {balanced:?}"
        );
        for c in 0..3 {
            assert!(
                current[2][c] >= grid.floor[2][c],
                "a channel fell below its floor"
            );
        }
    }

    #[test]
    fn prediction_application_is_continuous_below_the_old_clip_gate() {
        let width = 2;
        let height = 2;
        let original = vec![1.0_f32; width * height];
        let confidence = vec![0.0, 0.25, 0.5, 1.0];
        let mut candidate = original.clone();
        let (changed, max_lift) = apply_prediction(
            &mut candidate,
            &[[2.0; 3]],
            ApplyContext {
                original: &original,
                confidence: Some(&confidence),
                width,
                height,
                cfa: &rggb(),
                grid_width: 1,
            },
        );
        assert_eq!(candidate, vec![1.0, 1.25, 1.5, 2.0]);
        assert_eq!(changed, 3);
        assert_eq!(max_lift, 1.0);
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
            ReconstructionOptions {
                white_balance: [1.0; 3],
                strength: 1.0,
                method: HighlightMethod::Harmonic,
                clipped_fraction_floor: 0.0,
            },
        )
        .unwrap();
        let second_report = reconstruct_cfa(
            &mut second,
            width,
            height,
            &rggb(),
            None,
            ReconstructionOptions {
                white_balance: [1.0; 3],
                strength: 1.0,
                method: HighlightMethod::Harmonic,
                clipped_fraction_floor: 0.0,
            },
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
    fn a_failed_direct_solve_leaves_the_buffer_untouched() {
        // The direct solve edits `current` in place instead of working on a
        // full-frame copy, so the iterative fallback only sees the state it used
        // to see if every write is rolled back when a later channel fails. This
        // fixture solves one channel and then poisons the next one's right-hand
        // side, which is the order the rollback exists for.
        let width = 5;
        let mut value: Vec<[f32; 3]> = (0..width)
            .map(|x| {
                let intensity = 0.2 + x as f32 * 0.1;
                [intensity, intensity * 1.1, intensity * 0.9]
            })
            .collect();
        // A known neighbour with no green reading: channel 1's system inherits
        // the NaN through its right-hand side and cannot produce a finite
        // solution, while channel 0 (solved first, fewer unknowns) can.
        value[width - 2][1] = f32::NAN;
        let mut valid = vec![[true; 3]; width];
        valid[width - 1] = [false, false, true];
        let grid = Grid {
            width,
            height: 1,
            floor: vec![[0.0; 3]; width],
            value: value.clone(),
            confidence: valid
                .iter()
                .map(|channels| channels.map(|v| if v { 0.0 } else { 1.0 }))
                .collect(),
            trust: valid
                .iter()
                .map(|channels| channels.map(|v| if v { 1.0 } else { 0.0 }))
                .collect(),
            valid,
        };
        let region = Region {
            cells: (0..width).collect(),
            boundary: vec![0, width - 1],
        };
        let mut scratch = RegionScratch::new(grid.value.len());
        let mut current = value.clone();
        let solved = solve_region_direct(&grid, &region, [None; 3], &mut current, &mut scratch);
        assert!(!solved, "a non-finite system must be reported as unsolved");
        for (after, before) in current.iter().zip(&value) {
            for c in 0..3 {
                assert_eq!(
                    after[c].is_nan(),
                    before[c].is_nan(),
                    "a failed solve must not change the buffer"
                );
                if !before[c].is_nan() {
                    assert_eq!(
                        after[c], before[c],
                        "a failed solve must not change the buffer"
                    );
                }
            }
        }
        // And the scratch is handed back neutral, or the next region would read
        // this region's row numbering.
        assert!(scratch.row_of.iter().all(|&row| row == usize::MAX));
        assert!(scratch.undo.is_empty());
    }

    #[test]
    fn luminance_domes_require_a_hard_clipped_core() {
        let grid = Grid {
            width: 2,
            height: 1,
            value: vec![[0.96; 3], [1.0; 3]],
            floor: vec![[0.96; 3], [1.0; 3]],
            confidence: vec![[0.85; 3], [1.0; 3]],
            trust: vec![[0.15_f32.powi(2); 3], [0.0; 3]],
            valid: vec![[false; 3], [false; 3]],
        };
        let region = Region {
            cells: vec![0, 1],
            boundary: vec![0, 1],
        };

        let cores =
            fully_clipped_components(&grid, &region, &mut RegionScratch::new(grid.value.len()));
        assert_eq!(cores.len(), 1);
        assert_eq!(cores[0].cells, vec![1]);
    }

    #[test]
    fn unfitted_partial_cell_has_no_neighbour_imported_luminance_support() {
        let value = vec![[0.20, 0.30, 0.40], [0.30, 0.45, 0.60], [0.40, 0.60, 0.80]];
        let grid = Grid {
            width: 3,
            height: 1,
            floor: value.clone(),
            value: value.clone(),
            confidence: vec![[0.0; 3], [0.0; 3], [0.80, 0.0, 0.0]],
            trust: vec![[1.0; 3], [1.0; 3], [0.04, 1.0, 1.0]],
            valid: vec![[true; 3], [true; 3], [false, true, true]],
        };
        let region = Region {
            cells: vec![2],
            boundary: vec![2],
        };
        let raw_support = continuous_luminance_support(&grid, &region);
        assert!(
            raw_support[0] > 0.9,
            "fixture must expose the ungated support"
        );
        assert!(best_fits(&grid, &region)[0].is_none());

        let (_, support, ..) = harmonic_prediction(&grid, value.clone(), &[region], [1.0; 3]);
        assert_eq!(support[2][0], 0.0);
    }

    #[test]
    fn two_guide_fit_rejects_collinear_predictors() {
        let width = 32;
        let mut valid = vec![[true; 3]; width];
        for item in &mut valid[16..] {
            item[2] = false;
        }
        let value: Vec<[f32; 3]> = (0..width)
            .map(|x| {
                let intensity = 0.1 + x as f32 * 0.025;
                [intensity, intensity * 2.0, intensity * 0.7]
            })
            .collect();
        let grid = Grid {
            width,
            height: 1,
            floor: value.clone(),
            value,
            confidence: valid
                .iter()
                .map(|channels| channels.map(|v| if v { 0.0 } else { 1.0 }))
                .collect(),
            trust: valid
                .iter()
                .map(|channels| channels.map(|v| if v { 1.0 } else { 0.0 }))
                .collect(),
            valid,
        };
        let region = Region {
            cells: (16..width).collect(),
            boundary: vec![16],
        };
        assert!(fit_two_guides(&grid, &region, 2).is_none());
        assert!(fit_line(&grid, &region, 2, 0).is_some());
    }

    #[test]
    fn affine_data_term_abstains_beyond_measured_guide_leverage() {
        let line = LineFit {
            guides: [0, 0],
            slopes: [3.0, 0.0],
            guide_min: [0.1, 0.0],
            guide_max: [0.3, 0.0],
            guide_count: 1,
            intercept: 0.05,
            r2: 1.0,
            trusted_mass: 1.0,
        };
        let current = [[2.0, 0.0, 0.0]];
        let supported_max = 0.3 + MAX_AFFINE_EXTRAPOLATION_SPANS * (0.3 - 0.1);
        let valid = [true, false, false];
        assert_eq!(
            fit_data(line, &current, 0, valid != [false; 3]),
            0.05 + 3.0 * supported_max
        );
        assert_eq!(fit_data(line, &current, 0, false), 6.05);
        assert_eq!(
            soft_extrapolation_limit(current[0][0], 0.1, 0.3),
            supported_max
        );
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
        // No explicit map: the stage has to engage across its whole operating
        // range on its own evidence. An all-ones map here would make any
        // confidence gating inside the stage untestable.
        let corrected = apply_sensor_knee(
            &mut captured,
            &before_knee.clone(),
            None,
            width,
            height,
            &rggb(),
        );
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
            ReconstructionOptions {
                white_balance: [1.0; 3],
                strength: 1.0,
                method: HighlightMethod::Harmonic,
                clipped_fraction_floor: 0.0,
            },
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
