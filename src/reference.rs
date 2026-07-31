//! The camera's own JPEG, shipped alongside the RAW, used as a yardstick.
//!
//! `docs/PLAN.md` sets the bar for this program as "never clearly worse than
//! the camera's own JPEG, usually at least as good". That is only a testable
//! claim if the camera's JPEG is on disk next to the RAW and both are measured
//! the same way, which is why the plan asks for RAW+JPEG pairs while the corpus
//! is being gathered.
//!
//! This module is measurement only. Nothing here feeds the render: the
//! reference is read, reduced to a handful of numbers, and reported. The
//! embedded preview in [`crate::preview`] is the one vendor rendering that
//! *does* steer a decision, and it is deliberately a separate path — a
//! reference JPEG is an answer sheet, and grading against an answer sheet the
//! renderer has already read would prove nothing.
//!
//! # What is comparable, and when
//!
//! Two of the numbers are available without rendering anything, so they work
//! under `--dry-run` over a whole corpus:
//!
//! - `center_weighted_key_display_ev`, measured with exactly the estimator
//!   [`crate::preview`] uses, against the display EV this program's curve will
//!   place the same statistic at. That difference is the exposure error.
//!
//! That field was called `subject_display_ev` until schema 7, which was a
//! promise the program does not keep: it is `0.60 * centre median + 0.40 * frame
//! median`, a centre-weighted key measurement with no subject detection of any
//! kind behind it. The name mattered because it invited reading a large delta as
//! "the subject is misplaced" when the honest reading is "the centre of the frame
//! is brighter or darker than the camera made it", which on a backlit or
//! off-centre composition are different claims.
//!
//! The rest — colourfulness, saturation, blown highlights, crushed shadows —
//! need our render to exist, so they appear only in a full run.
//!
//! # Aggregate statistics, and one per-pixel comparison
//!
//! Everything here was aggregate-only until 0.1.18: two images reduced to scalars
//! and the scalars differenced. That is cheap and geometry-independent, but it
//! cannot see a *hue* difference, because a frame's mean hue is dominated by
//! whatever colour covers most of it. The 0.1.17 colour-path A/B ran into exactly
//! that wall — the scorecard had no axis that could reward keeping an out-of-gamut
//! colour's hue, which is what the owned colour path is for.
//!
//! So one per-pixel comparison now exists, on a deliberately small canonical grid:
//! see [`Canonical`], [`downsample`] and [`ReferenceReport::compare_pixels`]. It
//! is confined to a hue and chroma comparison and it refuses to run when the two
//! renderings are different crops, which is also how the review's
//! "canonically resize and align paired outputs" item is satisfied.

use crate::metrics::OutputStats;
use crate::tone::map_ev;
use crate::types::ToneParams;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Extensions tried, in order, when pairing a RAW with a rendering.
const REFERENCE_EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "JPG", "JPEG"];

/// Where to look for the camera's rendering of a RAW.
#[derive(Debug, Clone)]
pub enum ReferenceSource {
    /// Do not look. The default: most runs are not corpus work.
    Disabled,
    /// Alongside the RAW itself, which is how a camera writes RAW+JPEG.
    Sibling,
    /// In one directory, for corpora whose renderings were collected separately.
    Directory(PathBuf),
}

impl ReferenceSource {
    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    /// The camera JPEG paired with `raw`, if one exists.
    ///
    /// Pairing is by file stem, which is what every camera and every phone
    /// camera app does. Extensions are tried in a fixed order so that a
    /// directory holding both `name.jpg` and `name.JPEG` resolves the same way
    /// on every run and every filesystem.
    pub fn locate(&self, raw: &Path) -> Option<PathBuf> {
        let directory = match self {
            Self::Disabled => return None,
            Self::Sibling => raw.parent()?.to_path_buf(),
            Self::Directory(directory) => directory.clone(),
        };
        let stem = raw.file_stem()?;

        REFERENCE_EXTENSIONS
            .iter()
            .map(|extension| directory.join(stem).with_extension(extension))
            .find(|candidate| candidate.is_file())
    }
}

/// The camera's rendering, measured.
#[derive(Debug, Clone, Serialize)]
pub struct ReferenceReport {
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// Centre-weighted key brightness in display EV relative to middle grey,
    /// measured with the same weighting [`crate::preview`] and
    /// [`crate::analyze`] use.
    pub center_weighted_key_display_ev: f32,
    pub p05_display_ev: f32,
    pub p50_display_ev: f32,
    pub p95_display_ev: f32,
    pub measured: OutputStats,
    /// This program's rendering minus the camera's. Positive means brighter,
    /// more colourful, or more clipped than the camera.
    pub delta: ReferenceDelta,
    /// A few megabytes of the camera's rendering, kept so a per-pixel comparison
    /// is possible later in the pipeline.
    ///
    /// This is the *only* thing retained from the decoded reference. The full
    /// buffer is dropped when [`read`] returns, deliberately: a 50-megapixel phone
    /// JPEG decodes to 150 MB and `read` is called before the RAW decode precisely
    /// so that buffer and the developer's own peak never coexist. A canonical grid
    /// at [`CANONICAL_LONG_EDGE`] is about 2 MB, which buys the comparison back
    /// without reopening that problem.
    #[serde(skip)]
    pub canonical: Option<Canonical>,
}

/// Ours minus the camera's, for each measure the two share.
///
/// Every field but the first is `None` under `--dry-run`, where our own
/// rendering was never produced.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ReferenceDelta {
    /// Where our curve places the centre-weighted key, minus where the camera
    /// placed it. Available without rendering, because the controller's target
    /// and the tone curve are both known before a single pixel is mapped.
    pub center_weighted_key_display_ev: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colourfulness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_saturation: Option<f32>,
    /// Ours divided by the camera's, which is the form the chroma path is
    /// tuned in: 1.0 means the two renderings are equally saturated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturation_ratio: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_level: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub near_white_fraction: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crushed_fraction: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub luminance_entropy: Option<f32>,
    /// Ours divided by the camera's, over highlights only. See
    /// [`crate::metrics::OutputStats::mean_saturation_highlight`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlight_saturation_ratio: Option<f32>,
    /// Per-pixel hue comparison against the camera, on a canonical grid. `None`
    /// when the two renderings are not comparable geometrically — see
    /// [`HueComparison`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hue: Option<HueComparison>,
}

/// Per-pixel hue agreement with the camera's rendering.
///
/// The axis `docs/REVIEW-2026-07-30.md`'s A/B was missing. The four standing axes
/// — highlights kept, shadows kept, tonal detail, local detail — cannot see a hue
/// shift, so the owned colour path's whole purpose was invisible to the scorecard
/// and `crushed_fraction` could only punish it. This measures the thing directly.
///
/// Read it knowing what it is and is not. The camera JPEG is a *reference, not
/// ground truth* — `docs/PLAN.md` says so under that heading — and its hue is a
/// vendor's opinion, so a
/// small delta means "renders colour like Sony does", not "correct". The
/// reference-free proof lives in `crate::color`'s tests, which push synthetic
/// camera RGB of known XYZ through both colour paths and check the hue angle
/// against the analytic answer. This statistic exists to show that effect at
/// corpus scale, on real scenes, where a unit test cannot reach.
#[derive(Debug, Clone, Serialize)]
pub struct HueComparison {
    /// Median absolute hue difference, in degrees.
    pub median_degrees: f32,
    /// 90th percentile, which is where a gamut-mapping failure shows up: hue
    /// errors are concentrated in the out-of-gamut minority, so a median can stay
    /// flat while the tail moves a long way.
    pub p90_degrees: f32,
    /// Largest absolute hue difference anywhere on the canonical grid.
    pub max_degrees: f32,
    /// Mean absolute hue difference.
    ///
    /// Reported alongside the median because on this axis they answer different
    /// questions and the median is the less informative one. The clip only acts on
    /// out-of-gamut pixels, a small minority of most frames, so a large hue
    /// improvement on that minority barely moves the median while moving the mean.
    /// The 0.1.18 baseline showed exactly that shape: median hue change between the
    /// two colour paths was 0.0000 degrees while the best frame improved by 26.7.
    pub mean_degrees: f32,
    /// Pixels chromatic enough in *both* renderings for a hue to be meaningful.
    /// Read it before the percentiles: a near-neutral frame legitimately has
    /// almost nothing to compare.
    pub comparable_pixels: usize,
    /// Our mean Oklab chroma divided by the camera's, over those pixels. A
    /// perceptually-founded companion to `saturation_ratio`, which is computed in
    /// display-code space where equal steps are not equal colour differences.
    pub chroma_ratio: f32,
    /// The canonical grid both renderings were resampled onto.
    pub grid_width: usize,
    pub grid_height: usize,
}

/// Long edge of the canonical comparison grid.
///
/// Small on purpose. A hue statistic wants the *scene's* colour, not the sensor's
/// noise, and box-averaging down to this size suppresses per-pixel noise while
/// leaving every real colour region intact. It also keeps the retained thumbnail
/// at a few megabytes, which is what makes a per-pixel comparison possible at all
/// given that `read` runs before the RAW decode specifically so the two full-size
/// buffers never coexist.
const CANONICAL_LONG_EDGE: usize = 512;

/// How far two aspect ratios may differ before a per-pixel comparison is refused.
///
/// The review's comparison-harness item notes that Sony pairs are native
/// resolution and same-geometry while Samsung's differ, and asks for Samsung to be
/// excluded until canonical resizing exists. This *is* that resizing, so the
/// exclusion becomes a measured guard rather than a manual rule: resampling two
/// different crops onto one grid would compare different scenes and report the
/// disagreement as a hue error.
const MAX_ASPECT_MISMATCH: f32 = 0.01;

/// A small, canonically-sized copy of a rendering, in linear light.
///
/// Not serialized: these are pixels, and a sidecar is a report. `#[serde(skip)]`
/// rather than a separate side channel so the buffer travels with the report that
/// owns it and cannot be forgotten about.
#[derive(Debug, Clone)]
pub struct Canonical {
    pub width: usize,
    pub height: usize,
    /// Linear-light RGB, one entry per grid pixel.
    pub pixels: Vec<[f32; 3]>,
}

/// Box-average a rendering down onto the canonical grid.
///
/// Plain box averaging rather than a windowed filter: it is exact, has no filter
/// parameter to argue about, and is deterministic regardless of thread count. The
/// source box for an output pixel is a half-open range, so every source pixel
/// contributes to exactly one output pixel and none is skipped or double-counted.
///
/// `linear_at` must return **linear-light** values. Averaging sRGB-encoded values
/// would darken every gradient, and while that error would apply equally to both
/// renderings and so largely cancel in a delta, it would also shift hue — which is
/// the one thing this grid exists to measure.
pub fn downsample(
    width: usize,
    height: usize,
    linear_at: impl Fn(usize, usize) -> [f32; 3],
) -> Option<Canonical> {
    if width == 0 || height == 0 {
        return None;
    }

    let long = width.max(height);
    let (grid_width, grid_height) = if long <= CANONICAL_LONG_EDGE {
        (width, height)
    } else {
        let scale = CANONICAL_LONG_EDGE as f64 / long as f64;
        (
            ((width as f64 * scale).round() as usize).max(1),
            ((height as f64 * scale).round() as usize).max(1),
        )
    };

    downsample_onto(width, height, grid_width, grid_height, linear_at)
}

/// Box-average onto a grid somebody else already chose.
///
/// The second image of a pair must land on the *same* grid as the first, not on
/// an independently rounded one. Deriving both grids separately looked right and
/// was wrong: two renderings whose aspect ratios agree to well within a percent
/// still round to grids a pixel apart, and the first version of this comparison
/// silently declined 35 of 106 corpus pairs for that reason alone — a rounding
/// artifact wearing the costume of a geometry mismatch. The aspect check in
/// [`ReferenceReport::compare_pixels`] is what guards against a real crop
/// difference; this function exists so rounding cannot masquerade as one.
pub fn downsample_onto(
    width: usize,
    height: usize,
    grid_width: usize,
    grid_height: usize,
    linear_at: impl Fn(usize, usize) -> [f32; 3],
) -> Option<Canonical> {
    if width == 0 || height == 0 || grid_width == 0 || grid_height == 0 {
        return None;
    }

    let mut pixels = Vec::with_capacity(grid_width * grid_height);
    for grid_y in 0..grid_height {
        let y0 = grid_y * height / grid_height;
        let y1 = (((grid_y + 1) * height) / grid_height)
            .max(y0 + 1)
            .min(height);
        for grid_x in 0..grid_width {
            let x0 = grid_x * width / grid_width;
            let x1 = (((grid_x + 1) * width) / grid_width).max(x0 + 1).min(width);

            let mut sum = [0.0_f64; 3];
            let mut count = 0_u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let pixel = linear_at(x, y);
                    for channel in 0..3 {
                        sum[channel] += f64::from(pixel[channel]);
                    }
                    count += 1;
                }
            }
            let divisor = f64::from(count.max(1));
            pixels.push([
                (sum[0] / divisor) as f32,
                (sum[1] / divisor) as f32,
                (sum[2] / divisor) as f32,
            ]);
        }
    }

    Some(Canonical {
        width: grid_width,
        height: grid_height,
        pixels,
    })
}

/// Canonical grid of an sRGB-encoded 8-bit rendering, linearized first.
fn canonical_from_rgb8(image: &image::RgbImage) -> Option<Canonical> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let raw = image.as_raw();
    downsample(width, height, |x, y| {
        let index = (y * width + x) * 3;
        [
            crate::tone::srgb_decode(f32::from(raw[index]) / 255.0),
            crate::tone::srgb_decode(f32::from(raw[index + 1]) / 255.0),
            crate::tone::srgb_decode(f32::from(raw[index + 2]) / 255.0),
        ]
    })
}

/// Our own 16-bit rendering, box-averaged onto a grid the reference already fixed.
fn canonical_from_render(
    image: &crate::tone::Rgb16Image,
    grid_width: usize,
    grid_height: usize,
) -> Option<Canonical> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let raw = image.as_raw();
    let scale = 1.0 / f32::from(u16::MAX);
    downsample_onto(width, height, grid_width, grid_height, |x, y| {
        let index = (y * width + x) * 3;
        [
            crate::tone::srgb_decode(f32::from(raw[index]) * scale),
            crate::tone::srgb_decode(f32::from(raw[index + 1]) * scale),
            crate::tone::srgb_decode(f32::from(raw[index + 2]) * scale),
        ]
    })
}

/// Compare two canonical grids pixel for pixel in Oklab.
fn compare_hue(ours: &Canonical, theirs: &Canonical) -> Option<HueComparison> {
    if ours.width != theirs.width || ours.height != theirs.height {
        return None;
    }

    let mut differences: Vec<f32> = Vec::new();
    let mut our_chroma = 0.0_f64;
    let mut their_chroma = 0.0_f64;

    for (our_pixel, their_pixel) in ours.pixels.iter().zip(theirs.pixels.iter()) {
        let a = crate::oklab::from_linear_srgb(*our_pixel);
        let b = crate::oklab::from_linear_srgb(*their_pixel);
        if let Some(difference) = crate::oklab::hue_difference(a, b) {
            differences.push(difference.to_degrees());
            our_chroma += f64::from(a.chroma());
            their_chroma += f64::from(b.chroma());
        }
    }

    if differences.is_empty() {
        return None;
    }
    differences.sort_by(f32::total_cmp);
    let at = |fraction: f32| {
        let index = ((differences.len() - 1) as f32 * fraction).round() as usize;
        differences[index]
    };

    let mean_degrees =
        (differences.iter().map(|d| f64::from(*d)).sum::<f64>() / differences.len() as f64) as f32;

    Some(HueComparison {
        median_degrees: at(0.50),
        p90_degrees: at(0.90),
        max_degrees: differences[differences.len() - 1],
        mean_degrees,
        comparable_pixels: differences.len(),
        chroma_ratio: if their_chroma > 1.0e-9 {
            (our_chroma / their_chroma) as f32
        } else {
            f32::NAN
        },
        grid_width: ours.width,
        grid_height: ours.height,
    })
}

/// Read and measure the camera's rendering.
///
/// Best effort, like the preview reader: a corrupt or unreadable reference is
/// reported as absent, never as a failure of the file it was paired with.
pub fn read(path: &Path) -> Option<ReferenceReport> {
    let rgb = image::open(path).ok()?.into_rgb8();
    let display = crate::preview::display_ev(&rgb)?;

    Some(ReferenceReport {
        path: path.to_string_lossy().into_owned(),
        width: rgb.width(),
        height: rgb.height(),
        center_weighted_key_display_ev: display.center_weighted_key_ev,
        p05_display_ev: display.p05_ev,
        p50_display_ev: display.p50_ev,
        p95_display_ev: display.p95_ev,
        measured: OutputStats::measure_rgb8(&rgb),
        delta: ReferenceDelta::default(),
        canonical: canonical_from_rgb8(&rgb),
    })
}

/// The display EV this program's curve will place the centre-weighted key at.
///
/// `target_median_ev` is the *curve-input* EV the controller aimed that
/// statistic at, so it has to go through the curve before it can be compared
/// with a measurement of somebody else's finished rendering. This is the forward
/// direction of the inversion the preview oracle performs.
pub fn predicted_center_weighted_key_display_ev(target_median_ev: f32, params: &ToneParams) -> f32 {
    map_ev(target_median_ev, params)
}

impl ReferenceReport {
    /// Fill in the difference against our own analysis, and against our own
    /// rendering when one was produced.
    pub fn compare(&mut self, predicted_key_display_ev: f32, ours: Option<&OutputStats>) {
        self.delta = ReferenceDelta {
            center_weighted_key_display_ev: predicted_key_display_ev
                - self.center_weighted_key_display_ev,
            ..Default::default()
        };

        let Some(ours) = ours else {
            return;
        };
        let reference = &self.measured;

        self.delta.colourfulness = Some(ours.colourfulness - reference.colourfulness);
        self.delta.mean_saturation = Some(ours.mean_saturation - reference.mean_saturation);
        self.delta.saturation_ratio = (reference.mean_saturation > 1.0e-6)
            .then(|| ours.mean_saturation / reference.mean_saturation);
        self.delta.mean_level = Some(ours.mean_level - reference.mean_level);
        self.delta.near_white_fraction =
            Some(ours.near_white_fraction - reference.near_white_fraction);
        self.delta.crushed_fraction = Some(ours.crushed_fraction - reference.crushed_fraction);
        self.delta.luminance_entropy = Some(ours.luminance_entropy - reference.luminance_entropy);
        self.delta.highlight_saturation_ratio = (reference.mean_saturation_highlight > 1.0e-6
            && reference.highlight_pixels > 0
            && ours.highlight_pixels > 0)
            .then(|| ours.mean_saturation_highlight / reference.mean_saturation_highlight);
    }

    /// Compare our finished rendering with the camera's, pixel for pixel, on a
    /// canonical grid.
    ///
    /// Separate from [`compare`](Self::compare) because it needs the rendered
    /// buffer rather than its statistics, and because it can legitimately decline:
    /// the two renderings must describe the same framing before a per-pixel
    /// comparison means anything. Declining is recorded as `delta.hue == None`
    /// with the reason logged, not as a zero.
    pub fn compare_pixels(&mut self, ours: &crate::tone::Rgb16Image) -> Result<(), &'static str> {
        let Some(theirs) = &self.canonical else {
            return Err("the camera's rendering produced no canonical grid");
        };

        // Aspect first, on the *original* dimensions: the canonical grids are
        // rounded to whole pixels, so comparing them would hide a real crop
        // difference behind the rounding.
        let ours_aspect = ours.width() as f32 / ours.height() as f32;
        let theirs_aspect = self.width as f32 / self.height as f32;
        if !ours_aspect.is_finite()
            || !theirs_aspect.is_finite()
            || (ours_aspect - theirs_aspect).abs() / theirs_aspect > MAX_ASPECT_MISMATCH
        {
            return Err("the two renderings are different crops of the scene");
        }

        let Some(mine) = canonical_from_render(ours, theirs.width, theirs.height) else {
            return Err("our rendering produced no canonical grid");
        };

        match compare_hue(&mine, theirs) {
            Some(comparison) => {
                self.delta.hue = Some(comparison);
                Ok(())
            }
            None => Err("neither rendering has enough chromatic pixels to compare"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// Box averaging must be exact and must cover every source pixel once.
    #[test]
    fn downsample_averages_every_source_pixel_exactly_once() {
        // 4x2 of known values, onto a 2x1 grid: each output is the mean of a 2x2.
        let values = [[1.0_f32, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]];
        let grid =
            downsample_onto(4, 2, 2, 1, |x, y| [values[y][x]; 3]).expect("a well-formed grid");
        assert_eq!((grid.width, grid.height), (2, 1));
        // (1+2+5+6)/4 = 3.5 and (3+4+7+8)/4 = 5.5
        assert!(
            (grid.pixels[0][0] - 3.5).abs() < 1.0e-6,
            "{:?}",
            grid.pixels
        );
        assert!(
            (grid.pixels[1][0] - 5.5).abs() < 1.0e-6,
            "{:?}",
            grid.pixels
        );
    }

    /// An image already at or under the canonical size is passed through, not
    /// resampled — otherwise a small reference would be blurred against nothing.
    #[test]
    fn downsample_leaves_a_small_image_alone() {
        let grid = downsample(3, 2, |x, y| [(x + y) as f32; 3]).expect("valid");
        assert_eq!((grid.width, grid.height), (3, 2));
        assert_eq!(grid.pixels[0][0], 0.0);
        assert_eq!(grid.pixels[5][0], 3.0);
    }

    /// The bug this function exists to prevent. Two renderings whose aspect ratios
    /// agree can still round to grids a pixel apart, and the first version of the
    /// hue comparison declined 35 of 106 corpus pairs for exactly that reason —
    /// reporting a rounding artifact as a geometry mismatch. Resampling the second
    /// image onto the first's grid makes the sizes agree by construction.
    #[test]
    fn independently_rounded_grids_can_disagree_but_onto_cannot() {
        // 4000x3000 and 4000x2996 differ by 0.13% in aspect — far inside the
        // tolerance a real crop difference has to clear — yet the short edge
        // rounds to 384 and 383 respectively.
        let a = downsample(4000, 3000, |_, _| [0.5; 3]).expect("valid");
        let b = downsample(4000, 2996, |_, _| [0.5; 3]).expect("valid");
        let aspect_mismatch = ((4000.0 / 3000.0_f32) - (4000.0 / 2996.0)).abs() / (4000.0 / 2996.0);
        assert!(
            aspect_mismatch < MAX_ASPECT_MISMATCH,
            "the two shapes must pass the aspect guard for this test to mean anything, \
             mismatch was {aspect_mismatch}"
        );
        assert_ne!(
            (a.width, a.height),
            (b.width, b.height),
            "expected independent rounding to disagree"
        );

        let onto = downsample_onto(4000, 2996, a.width, a.height, |_, _| [0.5; 3]).expect("valid");
        assert_eq!((onto.width, onto.height), (a.width, a.height));
    }

    /// Two identical renderings must report no hue difference at all, or every
    /// number this axis produces is suspect.
    #[test]
    fn identical_renderings_have_no_hue_difference() {
        let colour = |x: usize, y: usize| {
            [
                0.1 + 0.005 * x as f32,
                0.4 - 0.003 * y as f32,
                0.2 + 0.004 * (x + y) as f32,
            ]
        };
        let grid = downsample(32, 24, colour).expect("valid");
        let comparison = compare_hue(&grid, &grid).expect("a chromatic image");
        assert!(comparison.median_degrees < 1.0e-3, "{comparison:?}");
        assert!(comparison.mean_degrees < 1.0e-3, "{comparison:?}");
        assert!(comparison.max_degrees < 1.0e-3, "{comparison:?}");
        assert!(
            (comparison.chroma_ratio - 1.0).abs() < 1.0e-4,
            "{comparison:?}"
        );
        assert_eq!(comparison.comparable_pixels, 32 * 24);
    }

    /// A hue rotation must be detected at roughly its true size, so the numbers
    /// mean degrees rather than merely "more" and "less".
    #[test]
    fn a_channel_swap_is_reported_as_a_large_hue_difference() {
        let ours = downsample(16, 16, |_, _| [0.5, 0.2, 0.1]).expect("valid");
        let theirs = downsample(16, 16, |_, _| [0.1, 0.2, 0.5]).expect("valid");
        let comparison = compare_hue(&ours, &theirs).expect("chromatic");
        assert!(
            comparison.median_degrees > 60.0,
            "swapping red and blue should be a big hue move, got {}",
            comparison.median_degrees
        );
    }

    /// A neutral pair has no hue to compare, and must say so rather than
    /// reporting perfect agreement.
    #[test]
    fn a_neutral_pair_declines_rather_than_claiming_agreement() {
        let grey = downsample(16, 16, |_, _| [0.4; 3]).expect("valid");
        assert!(compare_hue(&grey, &grey).is_none());
    }

    fn write_jpeg(path: &Path, image: &RgbImage) {
        image
            .save_with_format(path, image::ImageFormat::Jpeg)
            .unwrap();
    }

    fn temporary_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("raw-autotune-reference-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn textured(level: u8) -> RgbImage {
        RgbImage::from_fn(128, 96, |x, y| {
            let wobble = ((x % 8) as i16 - (y % 5) as i16) * 6;
            let channel = |base: i16| (base + wobble).clamp(0, 255) as u8;
            Rgb([
                channel(level as i16 + 30),
                channel(level as i16),
                channel(level as i16 - 25),
            ])
        })
    }

    #[test]
    fn a_disabled_source_never_pairs() {
        let directory = temporary_directory("disabled");
        let raw = directory.join("frame.dng");
        write_jpeg(&directory.join("frame.jpg"), &textured(120));
        assert!(ReferenceSource::Disabled.locate(&raw).is_none());
    }

    #[test]
    fn a_sibling_jpeg_is_found_by_stem() {
        let directory = temporary_directory("sibling");
        let raw = directory.join("frame.dng");
        let jpeg = directory.join("frame.jpg");
        write_jpeg(&jpeg, &textured(120));
        assert_eq!(ReferenceSource::Sibling.locate(&raw), Some(jpeg));
    }

    #[test]
    fn an_unpaired_raw_reports_no_reference() {
        let directory = temporary_directory("unpaired");
        write_jpeg(&directory.join("other.jpg"), &textured(120));
        assert!(
            ReferenceSource::Sibling
                .locate(&directory.join("frame.dng"))
                .is_none()
        );
    }

    #[test]
    fn a_separate_directory_is_searched_instead_of_the_siblings() {
        let raws = temporary_directory("split-raw");
        let renderings = temporary_directory("split-jpeg");
        let raw = raws.join("frame.dng");
        let jpeg = renderings.join("frame.jpg");
        write_jpeg(&jpeg, &textured(120));

        assert_eq!(
            ReferenceSource::Directory(renderings).locate(&raw),
            Some(jpeg)
        );
        assert!(ReferenceSource::Sibling.locate(&raw).is_none());
    }

    /// A brighter reference must read as a higher subject EV, since that
    /// difference is the whole exposure signal.
    #[test]
    fn a_brighter_reference_measures_higher() {
        let directory = temporary_directory("brightness");
        let dark = directory.join("dark.jpg");
        let bright = directory.join("bright.jpg");
        write_jpeg(&dark, &textured(70));
        write_jpeg(&bright, &textured(190));

        let dark = read(&dark).unwrap();
        let bright = read(&bright).unwrap();
        assert!(bright.center_weighted_key_display_ev > dark.center_weighted_key_display_ev + 1.0);
    }

    #[test]
    fn an_unreadable_reference_is_absent_rather_than_fatal() {
        assert!(read(Path::new("no-such-file.jpg")).is_none());
    }

    /// Under `--dry-run` only the exposure difference is knowable; claiming a
    /// colour difference we never measured would be worse than reporting none.
    #[test]
    fn without_a_render_only_the_exposure_delta_is_reported() {
        let directory = temporary_directory("dry-run");
        let path = directory.join("frame.jpg");
        write_jpeg(&path, &textured(120));

        let mut report = read(&path).unwrap();
        report.compare(report.center_weighted_key_display_ev + 0.75, None);

        assert!((report.delta.center_weighted_key_display_ev - 0.75).abs() < 1.0e-4);
        assert!(report.delta.colourfulness.is_none());
        assert!(report.delta.saturation_ratio.is_none());
    }

    /// The ratio is the form the chroma path is tuned in, so it has to be 1.0
    /// when a rendering is compared with itself.
    #[test]
    fn comparing_a_rendering_with_itself_gives_a_unit_saturation_ratio() {
        let directory = temporary_directory("identity");
        let path = directory.join("frame.jpg");
        write_jpeg(&path, &textured(120));

        let mut report = read(&path).unwrap();
        let ours = report.measured.clone();
        report.compare(report.center_weighted_key_display_ev, Some(&ours));

        assert!((report.delta.saturation_ratio.unwrap() - 1.0).abs() < 1.0e-5);
        assert_eq!(report.delta.colourfulness, Some(0.0));
        assert_eq!(report.delta.mean_level, Some(0.0));
    }
}
