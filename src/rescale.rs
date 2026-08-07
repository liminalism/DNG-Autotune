//! Sensor sample normalization, the demosaic ROI, and the default crop.
//!
//! # Why this module exists
//!
//! The owned colour path (see [`crate::color`]) used to rent four steps from
//! Rawler's `RawDevelop`: `Rescale`, `Demosaic`, `CropActiveArea` and
//! `CropDefault`. Only the demosaic is worth renting. This module owns the other
//! three, and the reason is one line of Rawler:
//!
//! ```text
//! let clip = |v: f32| if v.is_sign_negative() { 0.0 } else { v };
//! ```
//!
//! — `rawler::imgop::raw::correct_blacklevel_cfa`, applied to every sample
//! before demosaic. A sensor's noise floor is a *distribution* centred on the
//! black level, so roughly half of the samples in a genuinely black region sit
//! below it. Clipping those to zero rectifies the distribution: the mean of the
//! survivors is above black, so true black becomes unreachable and the shadow
//! floor is a positive pedestal with no noise in it. That is the defect this
//! module exists to remove, and it is the *only* one this corpus can
//! demonstrate.
//!
//! # The other four defects, and their honest status
//!
//! `imgop/raw.rs:161-186` has four more problems. They are real, they are closed
//! here, and **no file in this project's corpus trips any of them** — so nothing
//! in this module's comments, or in the CHANGELOG, may claim a measured fix:
//!
//! | | Defect | Why it is inert here |
//! |---|---|---|
//! | b | The repeat dim is hardcoded 2x2; `BlackLevel::{width,height,cpp}` is ignored | every real file records a 2x2 repeat (or 1x1) |
//! | c | The black-level pattern is not anchored at `ActiveArea`, though the CFA is | every real `ActiveArea` origin is `(0,0)` |
//! | d | `BlackLevel::as_bayer_array()` broadcasts element `[0]` unless the stored length is exactly 4, silently | the A7C and ProShot store exactly 4, and all four are equal |
//! | e | `par_chunks_exact_mut(width * 2)` plus `chunks_exact_mut(2)` leaves the last row unnormalized when the height is odd, and the last column when the width is odd — raw DN survives into develop | every real frame has even dimensions |
//!
//! Because the corpus cannot demonstrate them, [`RescaleReport::notes`] records
//! a line whenever a file trips the *precondition* behind one of them. That way
//! the "no real file is affected" claim degrades loudly on the next camera
//! instead of silently.
//!
//! # What this module does not claim
//!
//! An earlier draft of this work claimed Rawler mis-applies per-phase black
//! levels on GBRG sensors, on the theory that the four stored levels are keyed
//! by colour. **That is false.** DNG `BlackLevel` is *positional*:
//! `rawler-0.7.2/src/dng/writer.rs:247` shifts the black level by the
//! `ActiveArea` origin exactly as `writer.rs:284` shifts the CFA, so Rawler's
//! position indexing is correct. The ProShot DNGs are GBRG and record four
//! *equal* levels (`25625/100` = 256.25) with a `(0, 0)` origin, so the question
//! is inert for this corpus regardless. The test
//! `rawler_compat_and_clip_agree_on_gbrg_with_four_distinct_levels` pins the
//! finding: with the positional reading, the corrected code and the compatibility
//! code agree on GBRG even when all four levels differ.
//!
//! # Invariants
//!
//! - **[`normalize`] takes `&RawImage` and never `&mut`.** [`crate::noise`] reads
//!   `raw.data` as original integer DN and has a test pinning the fitted model to
//!   exact `f32` equality, so a normalizer that scaled the samples in place would
//!   silently move `snr10_ev` — and with it the chroma denoiser and the
//!   sharpener. `normalize_does_not_mutate_the_raw_image` asserts it.
//! - **[`SubBlack::RawlerCompat`] is bit-identical to `RawImage::apply_scaling`**,
//!   not merely close. Two orderings carry that: `max = white - black` is
//!   computed once and then *divided* by (a reciprocal multiply differs in the
//!   last bit), and the sub-black test is `is_sign_negative()` rather than
//!   `< 0.0` (they differ on `-0.0`). It is what makes the corpus diff between
//!   the old rented path and this one a test of the wiring rather than of the
//!   arithmetic.
//! - Rows are distributed to threads in chunks of a fixed size, so the work split
//!   is a property of the data and not of `--jobs`. Every output sample depends
//!   only on its own input sample and its own coordinates, which is what makes
//!   `parallel_normalization_equals_the_sequential_reference` a meaningful check
//!   of the chunk-offset arithmetic rather than a tautology.

use anyhow::{Result, bail, ensure};
use clap::ValueEnum;
use rawler::imgop::{Dim2, Point, Rect};
use rawler::rawimage::RawPhotometricInterpretation;
use rawler::{RawImage, RawImageData};
use rayon::prelude::*;
use serde::Serialize;

/// Rows handed to one parallel task.
///
/// Fixed by the data rather than by the thread count on purpose: determinism is
/// a product property here (`CLAUDE.md`), and a chunk size derived from
/// `rayon::current_num_threads()` would make the *shape* of the computation
/// depend on the machine even though — for this particular elementwise kernel —
/// the values would not.
const ROWS_PER_CHUNK: usize = 64;

/// What to do with a sample that sits below its own black level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubBlack {
    /// Reproduce `RawImage::apply_scaling` bit for bit, including all four of
    /// the shortcuts (b)-(e) described in the module documentation. This is the
    /// control arm: with it selected, the owned path's output must not move
    /// relative to the releases that rented `Rescale` from Rawler.
    RawlerCompat,
    /// Clip sub-black samples to zero, as Rawler does, but index the black level
    /// correctly and normalize every sample. Isolates defects (b)-(e) from
    /// defect (a): a diff against [`SubBlack::RawlerCompat`] on a real file is a
    /// *finding*, not a regression.
    Clip,
    /// Keep sub-black samples negative. The change under test: it is what lets a
    /// genuinely black region develop to black instead of to a positive pedestal.
    Preserve,
}

impl SubBlack {
    /// The spelling the command line uses.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RawlerCompat => "rawler-compat",
            Self::Clip => "clip",
            Self::Preserve => "preserve",
        }
    }

    /// Whether this policy rectifies the noise floor.
    const fn clips(self) -> bool {
        matches!(self, Self::RawlerCompat | Self::Clip)
    }
}

/// Normalized samples, in the layout the next stage needs.
///
/// `Mosaic` is one sample per pixel and still needs a demosaic; the two `Linear`
/// arms are already interleaved per pixel and do not.
pub enum NormalizedSamples {
    /// CFA (or monochrome) data: `width * height` samples, undemosaiced.
    Mosaic(Vec<f32>),
    /// `LinearRaw` data with three components per pixel.
    Linear3(Vec<[f32; 3]>),
    /// `LinearRaw` data with four components per pixel, e.g. an RGBE sensor.
    Linear4(Vec<[f32; 4]>),
}

/// Sample counts rather than samples: a `Debug` that printed a 50 megapixel
/// buffer would be unusable in a test failure message, which is the only place
/// this is ever formatted.
impl std::fmt::Debug for NormalizedSamples {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (arm, count) = match self {
            Self::Mosaic(data) => ("Mosaic", data.len()),
            Self::Linear3(data) => ("Linear3", data.len()),
            Self::Linear4(data) => ("Linear4", data.len()),
        };
        write!(formatter, "{arm}({count} pixels)")
    }
}

/// Per-sample clip confidence in 0.0..=1.0, smoothstep around white.
///
/// Computed in normalized space (0 is black, 1 is the per-channel white
/// level) so it is already per-channel white_level aware.  Values near
/// white are near 1, well-exposed are 0.  Preserved through hot-pixel
/// correction and demosaiced alongside the samples so highlight reconstruction
/// does not have to infer clipping from a demosaiced value that has been
/// averaged below the threshold.
#[derive(Debug)]
pub enum ClipConfidence {
    Mosaic(Vec<f32>),
    Linear3(Vec<[f32; 3]>),
    Linear4(Vec<[f32; 4]>),
}

#[inline]
fn smoothstep_clip(x: f32) -> f32 {
    const T0: f32 = 0.92;
    const T1: f32 = 0.985;
    if x <= T0 { return 0.0; }
    if x >= T1 { return 1.0; }
    let t = ((x - T0) / (T1 - T0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The result of [`normalize`]: samples in `0.0..=1.0` for an in-range sensor,
/// below zero where the noise floor dips under black and the policy kept it, and
/// above one where a sample exceeds the white level.
#[derive(Debug)]
pub struct Normalized {
    pub samples: NormalizedSamples,
    pub width: usize,
    pub height: usize,
    pub clip_confidence: ClipConfidence,
    pub report: RescaleReport,
}

/// What the rescale did, recorded in the sidecar.
///
/// This is the field that makes the difference between the three
/// [`SubBlack`] policies measurable on real files rather than only in unit
/// tests: `sub_black_fraction` says how much of the frame the clip was acting
/// on, and `min_normalized` says how far below black the sensor actually went.
#[derive(Debug, Clone, Serialize)]
pub struct RescaleReport {
    pub policy: SubBlack,
    /// Black levels as applied, in repeat order `[(y * width + x) * cpp + c]`.
    pub black_levels: Vec<f32>,
    /// White levels as applied, same layout.
    pub white_levels: Vec<f32>,
    /// Repeat dimensions actually honoured — Rawler assumes 2x2 unconditionally.
    pub repeat_width: usize,
    pub repeat_height: usize,
    pub repeat_cpp: usize,
    /// CFA colour name at each repeat position, in the same order as
    /// `black_levels`, read at the `ActiveArea` origin the pattern is anchored
    /// at. Empty for `LinearRaw`, which has no CFA. Present so a survey can
    /// answer "which level went to which colour?" mechanically instead of by
    /// reasoning about a pattern name.
    pub repeat_colors: Vec<String>,
    /// Fraction of samples below their own black level.
    ///
    /// A property of the *file*, not of the policy: it is always measured with
    /// the corrected indexing, so the same frame reports the same fraction under
    /// all three policies and the number is comparable across them.
    pub sub_black_fraction: f32,
    /// Smallest normalized sample, after the policy.
    pub min_normalized: f32,
    /// Largest normalized sample. Above 1.0 whenever the sensor delivered
    /// samples above the recorded white level, which is normal.
    pub max_normalized: f32,
    /// One line per precondition violation. Empty on every file in this
    /// project's corpus; see the module documentation for why that is recorded
    /// rather than assumed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A sample container this module can read without copying the whole frame.
///
/// `f32::from(u16)` is exact and is precisely what `RawImageData::as_f32` does
/// (`imgop::convert_to_f32_unscaled`), so reading through this trait rather than
/// materializing `as_f32()` first saves a full-frame allocation — 200 MB on a
/// 50 MP Expert RAW — without changing a single bit.
trait Sample: Copy + Send + Sync {
    fn widen(self) -> f32;
}

impl Sample for u16 {
    #[inline(always)]
    fn widen(self) -> f32 {
        f32::from(self)
    }
}

impl Sample for f32 {
    #[inline(always)]
    fn widen(self) -> f32 {
        self
    }
}

/// The black/white levels in the layout the file actually recorded, anchored
/// where DNG says they are anchored.
struct Levels {
    /// Black level per repeat position and component.
    black: Vec<f32>,
    /// `white - black` per position, computed once so the kernel divides by a
    /// value rather than multiplying by a reciprocal.
    max: Vec<f32>,
    width: usize,
    height: usize,
    cpp: usize,
    /// Repeat column the frame's column 0 lands on, given the `ActiveArea`
    /// origin the pattern is anchored at.
    start_x: usize,
    /// The origin itself, kept for the row calculation.
    origin: Point,
}

impl Levels {
    /// Index of the first component of the repeat position row `y` starts at.
    #[inline(always)]
    fn row_base(&self, y: usize) -> usize {
        // Modular subtraction of the anchor: the pattern's (0, 0) sits on the
        // ActiveArea's top-left corner, and rows above it — which are still
        // normalized, because Rawler normalizes the whole frame and then crops —
        // continue the pattern backwards.
        let row = (y + self.height - self.origin.y % self.height) % self.height;
        row * self.width * self.cpp
    }
}

/// Normalize `raw`'s samples to `0.0..=1.0` against its black and white levels.
///
/// Never mutates `raw`; see the module documentation for why that is load-bearing.
pub fn normalize(raw: &RawImage, policy: SubBlack) -> Result<Normalized> {
    let (width, height, cpp) = (raw.width, raw.height, raw.cpp);
    ensure!(
        width > 0 && height > 0 && cpp > 0,
        "the file reports an empty sample grid ({width}x{height}, cpp {cpp}), so there is \
         nothing to normalize"
    );
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(cpp))
        .ok_or_else(|| anyhow::anyhow!("{width}x{height} with cpp {cpp} overflows a usize"))?;
    let stored = match &raw.data {
        RawImageData::Integer(data) => data.len(),
        RawImageData::Float(data) => data.len(),
    };
    ensure!(
        stored == expected,
        "the decoded buffer holds {stored} samples but {width}x{height} with cpp {cpp} needs \
         {expected}"
    );

    let mut notes = Vec::new();
    let levels = read_levels(raw, &mut notes)?;

    let (samples, tally) = match (&raw.photometric, policy) {
        (RawPhotometricInterpretation::BlackIsZero, _) => {
            // Unreachable through the pipeline: `levels::prepare_levels` rejects
            // this photometric first, because Rawler's own `apply_scaling` is a
            // `todo!()` for it (`rawimage.rs:510`) and would abort the process.
            bail!(
                "this file reports BlackIsZero photometric data with {cpp} component(s) per \
                 pixel, which no colour path in this program can develop"
            );
        }
        (RawPhotometricInterpretation::Cfa(_), _) => {
            ensure!(
                cpp == 1,
                "the file reports a CFA photometric with {cpp} components per pixel; a colour \
                 filter array delivers exactly one sample per photosite"
            );
            match (&raw.data, policy) {
                (RawImageData::Integer(data), SubBlack::RawlerCompat) => {
                    compat_mosaic(data, width, height, raw, &levels)?
                }
                (RawImageData::Float(data), SubBlack::RawlerCompat) => {
                    compat_mosaic(data, width, height, raw, &levels)?
                }
                (RawImageData::Integer(data), _) => {
                    let out = mosaic(data, width, height, &levels, policy.clips());
                    let tally = tally_mosaic(data, &out, width, &levels);
                    (NormalizedSamples::Mosaic(out), tally)
                }
                (RawImageData::Float(data), _) => {
                    let out = mosaic(data, width, height, &levels, policy.clips());
                    let tally = tally_mosaic(data, &out, width, &levels);
                    (NormalizedSamples::Mosaic(out), tally)
                }
            }
        }
        (RawPhotometricInterpretation::LinearRaw, _) => match cpp {
            // A single-component LinearRaw frame is monochrome: no demosaic
            // happens, so the mosaic buffer *is* the developed plane.
            1 => match &raw.data {
                RawImageData::Integer(data) => linear_arm::<u16, 1>(data, raw, &levels, policy)?,
                RawImageData::Float(data) => linear_arm::<f32, 1>(data, raw, &levels, policy)?,
            },
            3 => match &raw.data {
                RawImageData::Integer(data) => linear_arm::<u16, 3>(data, raw, &levels, policy)?,
                RawImageData::Float(data) => linear_arm::<f32, 3>(data, raw, &levels, policy)?,
            },
            4 => match &raw.data {
                RawImageData::Integer(data) => linear_arm::<u16, 4>(data, raw, &levels, policy)?,
                RawImageData::Float(data) => linear_arm::<f32, 4>(data, raw, &levels, policy)?,
            },
            other => bail!(
                "the file reports linear raw data with {other} components per pixel; only 1, 3 \
                 and 4 are defined"
            ),
        },
    };

    let count = expected as f64;
    let report = RescaleReport {
        policy,
        black_levels: levels.black.clone(),
        white_levels: levels
            .black
            .iter()
            .zip(levels.max.iter())
            .map(|(black, max)| black + max)
            .collect(),
        repeat_width: levels.width,
        repeat_height: levels.height,
        repeat_cpp: levels.cpp,
        repeat_colors: repeat_colors(raw, &levels),
        sub_black_fraction: (tally.sub_black as f64 / count) as f32,
        min_normalized: tally.min,
        max_normalized: tally.max,
        notes,
    };

    let clip_confidence = match &samples {
        NormalizedSamples::Mosaic(data) => {
            ClipConfidence::Mosaic(data.iter().map(|&x| smoothstep_clip(x)).collect())
        }
        NormalizedSamples::Linear3(data) => ClipConfidence::Linear3(
            data.iter()
                .map(|px| [smoothstep_clip(px[0]), smoothstep_clip(px[1]), smoothstep_clip(px[2])])
                .collect(),
        ),
        NormalizedSamples::Linear4(data) => ClipConfidence::Linear4(
            data.iter()
                .map(|px| {
                    [
                        smoothstep_clip(px[0]),
                        smoothstep_clip(px[1]),
                        smoothstep_clip(px[2]),
                        smoothstep_clip(px[3]),
                    ]
                })
                .collect(),
        ),
    };

    Ok(Normalized {
        samples,
        width,
        height,
        clip_confidence,
        report,
    })
}

/// Which [`NormalizedSamples`] arm an `N`-component buffer belongs in.
///
/// A trait rather than a `match N` inside a generic function, and the reason is
/// memory rather than taste: the three- and four-component arms hand the vector
/// straight over, where a `match` would have had to rebuild it element by element
/// — a second 600 MB allocation on a 50 MP frame, which is the entire saving this
/// phase exists to deliver.
trait LinearArm: Sized {
    fn wrap(out: Vec<Self>) -> NormalizedSamples;
}

impl LinearArm for [f32; 1] {
    fn wrap(out: Vec<Self>) -> NormalizedSamples {
        // One component per pixel is monochrome: no demosaic follows, so the
        // flat buffer is the developed plane. This is the only arm that has to
        // repack, and it is the arm no corpus file takes.
        NormalizedSamples::Mosaic(out.into_iter().map(|pixel| pixel[0]).collect())
    }
}

impl LinearArm for [f32; 3] {
    fn wrap(out: Vec<Self>) -> NormalizedSamples {
        NormalizedSamples::Linear3(out)
    }
}

impl LinearArm for [f32; 4] {
    fn wrap(out: Vec<Self>) -> NormalizedSamples {
        NormalizedSamples::Linear4(out)
    }
}

/// One `LinearRaw` component count, for both storage types and all three policies.
fn linear_arm<T: Sample, const N: usize>(
    data: &[T],
    raw: &RawImage,
    levels: &Levels,
    policy: SubBlack,
) -> Result<(NormalizedSamples, Tally)>
where
    [f32; N]: LinearArm,
{
    let (width, height) = (raw.width, raw.height);
    let out = if policy == SubBlack::RawlerCompat {
        compat_linear::<T, N>(data, width, height, raw)?
    } else {
        linear::<T, N>(data, width, height, levels, policy.clips())
    };
    let tally = tally_linear::<T, N>(data, &out, width, levels);
    Ok((<[f32; N]>::wrap(out), tally))
}

/// Read the file's levels into the layout DNG defines, or explain why they are
/// unusable.
///
/// Rawler emits `inf` or `NaN` here — `whitelevel - blacklevel` is computed with
/// no check at all — and the failure then surfaces several stages later as "too
/// few valid pixels for analysis", which points at the analyser rather than at
/// the file. Naming the offending values at the point of discovery is the whole
/// improvement.
fn read_levels(raw: &RawImage, notes: &mut Vec<String>) -> Result<Levels> {
    let cpp = raw.cpp;
    let (mut width, mut height, mut bcpp) = (
        raw.blacklevel.width.max(1),
        raw.blacklevel.height.max(1),
        raw.blacklevel.cpp.max(1),
    );

    let mut black = raw.blacklevel.as_vec();
    let declared = width * height * bcpp;
    if black.len() != declared {
        // Defect (d)'s precondition: `as_bayer_array` would silently broadcast
        // element [0] here. There is no honest reading of a pattern whose
        // dimensions disagree with its own sample count, so fall back to a
        // single level and say so.
        notes.push(format!(
            "black level records {} sample(s) but declares a {width}x{height} repeat with cpp \
             {bcpp} ({declared} samples); treating it as one level per component",
            black.len()
        ));
        let first = black.first().copied().unwrap_or(0.0);
        black = vec![first; cpp];
        width = 1;
        height = 1;
        bcpp = cpp;
    }

    ensure!(
        bcpp == cpp,
        "the black level describes {bcpp} component(s) per repeat position but the image has \
         {cpp}; the pattern cannot be indexed"
    );

    if matches!(raw.photometric, RawPhotometricInterpretation::Cfa(_))
        && black.len() != 4
        && black.len() != 1
    {
        // Rawler reaches this through `as_bayer_array`, which broadcasts [0]
        // and drops the rest. Nothing in this corpus does, but the day something
        // does, the two policies will disagree and this note says why.
        notes.push(format!(
            "CFA black level stores {} samples, so Rawler's as_bayer_array would broadcast the \
             first one and ignore the rest",
            black.len()
        ));
    }

    let white = raw.whitelevel.as_vec();
    let positions = width * height * bcpp;
    // Anything other than one level, or one per component, is a shape the
    // byte-identity claim was never checked against: `WhiteLevel::as_bayer_array`
    // broadcasts element [0] unless the length is exactly 4, so a length of 2, 3
    // or 6 on CFA data silently loses levels. Noted for every such shape, even
    // the ones this module reads correctly, because the note exists to say "the
    // corpus has not seen this" rather than "this is wrong".
    if white.len() != 1 && white.len() != cpp {
        notes.push(format!(
            "white level stores {} value(s), which is neither 1 nor the {cpp} component(s) the \
             image has",
            white.len()
        ));
    }
    let white = if white.len() == 1 {
        vec![white[0]; positions]
    } else if white.len() == cpp {
        // One level per component, repeated across every repeat position.
        (0..positions).map(|index| white[index % bcpp]).collect()
    } else if white.len() == positions {
        // A level per repeat position, which is what `as_bayer_array` reads when
        // the length happens to be 4.
        white
    } else {
        let first = white.first().copied().unwrap_or(u16::MAX as f32);
        vec![first; positions]
    };

    let mut max = Vec::with_capacity(positions);
    for (index, (black, white)) in black.iter().zip(white.iter()).enumerate() {
        let range = white - black;
        if !range.is_finite() || range <= 0.0 {
            bail!(
                "repeat position {index} has an unusable level range: white {white} minus black \
                 {black} is {range}, so normalizing would divide by zero or by a non-finite value"
            );
        }
        max.push(range);
    }
    ensure!(
        max.len() == positions,
        "the file records {} black and {} white level(s), which cannot be paired",
        black.len(),
        white.len()
    );

    let origin = raw.active_area.map(|area| area.p).unwrap_or_default();
    if origin != Point::zero() {
        // Defect (c)'s precondition. Rawler shifts the CFA by this origin
        // (`decoders/dng.rs`) but not the black level (`rawimage.rs`), so on such
        // a file the two policies disagree — which is the finding, not a bug in
        // this module.
        notes.push(format!(
            "ActiveArea origin is ({}, {}), so the black-level repeat is anchored away from the \
             frame origin; Rawler anchors it at the frame origin instead",
            origin.x, origin.y
        ));
    }

    if !raw.width.is_multiple_of(2) || !raw.height.is_multiple_of(2) {
        // Defect (e)'s precondition, and the sharpest of the four: Rawler leaves
        // those samples at raw DN, thousands of times brighter than a normalized
        // sample, and reports success.
        notes.push(format!(
            "frame is {}x{}, and Rawler's two-row/two-column stride leaves the trailing odd \
             row/column unnormalized",
            raw.width, raw.height
        ));
    }

    Ok(Levels {
        start_x: (width - origin.x % width) % width,
        black,
        max,
        width,
        height,
        cpp: bcpp,
        origin,
    })
}

/// CFA colour name per repeat position, anchored the same way the levels are.
fn repeat_colors(raw: &RawImage, levels: &Levels) -> Vec<String> {
    let RawPhotometricInterpretation::Cfa(config) = &raw.photometric else {
        return Vec::new();
    };
    let mut colors = Vec::with_capacity(levels.width * levels.height * levels.cpp);
    for y in 0..levels.height {
        for x in 0..levels.width {
            // The decoder has already shifted the CFA into full-frame
            // coordinates, so the colour of repeat position (x, y) is read at
            // the ActiveArea origin plus that offset.
            let name = format!(
                "{:?}",
                config
                    .cfa
                    .cfa_color_at(levels.origin.y + y, levels.origin.x + x)
            );
            for _ in 0..levels.cpp {
                colors.push(name.clone());
            }
        }
    }
    colors
}

/// Running minimum, maximum and sub-black count.
#[derive(Debug, Clone, Copy)]
struct Tally {
    sub_black: u64,
    min: f32,
    max: f32,
}

impl Default for Tally {
    fn default() -> Self {
        Self {
            sub_black: 0,
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
        }
    }
}

impl Tally {
    #[inline(always)]
    fn observe(&mut self, value: f32) {
        if value < self.min {
            self.min = value;
        }
        if value > self.max {
            self.max = value;
        }
    }

    /// Fold per-chunk tallies in index order.
    ///
    /// `min`, `max` and integer addition are all associative, so this is
    /// order-independent — but folding in a fixed order costs nothing and means
    /// the reported numbers cannot depend on which thread finished first even if
    /// a future field is added that *is* order-sensitive.
    fn combine(parts: Vec<Self>) -> Self {
        let mut total = Self::default();
        for part in parts {
            total.sub_black += part.sub_black;
            if part.min < total.min {
                total.min = part.min;
            }
            if part.max > total.max {
                total.max = part.max;
            }
        }
        if total.min > total.max {
            // An empty frame cannot happen — `normalize` rejects it — but a
            // report saying `inf`/`-inf` would be worse than one saying zero.
            total.min = 0.0;
            total.max = 0.0;
        }
        total
    }
}

/// Normalize one sample. The order of operations is the bit-identity contract:
/// subtract, test the sign, then divide by a precomputed range.
#[inline(always)]
fn apply(value: f32, black: f32, max: f32, clip: bool) -> f32 {
    let shifted = value - black;
    // `is_sign_negative()` rather than `< 0.0`, matching Rawler: the two differ
    // on `-0.0`, and a policy that disagreed there would break bit-identity on
    // float-storage DNGs.
    if clip && shifted.is_sign_negative() {
        0.0 / max
    } else {
        shifted / max
    }
}

/// Corrected CFA/monochrome normalization: real repeat dimensions, anchored at
/// the `ActiveArea` origin, every sample touched.
fn mosaic<T: Sample>(
    source: &[T],
    width: usize,
    height: usize,
    levels: &Levels,
    clip: bool,
) -> Vec<f32> {
    let mut out = vec![0.0_f32; width * height];
    out.par_chunks_mut(width * ROWS_PER_CHUNK)
        .enumerate()
        .for_each(|(chunk, block)| {
            let first_row = chunk * ROWS_PER_CHUNK;
            for (offset, line) in block.chunks_mut(width).enumerate() {
                let row = first_row + offset;
                let base = levels.row_base(row);
                let mut column = levels.start_x;
                let src = &source[row * width..row * width + width];
                for (slot, sample) in line.iter_mut().zip(src.iter()) {
                    let index = base + column * levels.cpp;
                    *slot = apply(sample.widen(), levels.black[index], levels.max[index], clip);
                    column += 1;
                    if column == levels.width {
                        column = 0;
                    }
                }
            }
        });
    out
}

/// Corrected `LinearRaw` normalization.
fn linear<T: Sample, const N: usize>(
    source: &[T],
    width: usize,
    height: usize,
    levels: &Levels,
    clip: bool,
) -> Vec<[f32; N]> {
    debug_assert_eq!(levels.cpp, N);
    let mut out = vec![[0.0_f32; N]; width * height];
    out.par_chunks_mut(width * ROWS_PER_CHUNK)
        .enumerate()
        .for_each(|(chunk, block)| {
            let first_row = chunk * ROWS_PER_CHUNK;
            for (offset, line) in block.chunks_mut(width).enumerate() {
                let row = first_row + offset;
                let base = levels.row_base(row);
                let mut column = levels.start_x;
                let start = row * width * N;
                let src = &source[start..start + width * N];
                for (x, pixel) in line.iter_mut().enumerate() {
                    let index = base + column * levels.cpp;
                    for (component, slot) in pixel.iter_mut().enumerate() {
                        *slot = apply(
                            src[x * N + component].widen(),
                            levels.black[index + component],
                            levels.max[index + component],
                            clip,
                        );
                    }
                    column += 1;
                    if column == levels.width {
                        column = 0;
                    }
                }
            }
        });
    out
}

/// Bit-identical port of `apply_scaling` on CFA data.
///
/// Reproduces every shortcut in `imgop::raw::correct_blacklevel_cfa`
/// deliberately: the four levels come from `as_bayer_array` (so a stored length
/// other than 4 broadcasts element `[0]`), the 2x2 phase is measured from the
/// frame origin rather than from the `ActiveArea`, and the trailing odd row and
/// column are left at raw DN. That last one is the reason the loop is written
/// with explicit `height / 2` and `width / 2` bounds rather than over all
/// samples: the skip is the behaviour under test, not an oversight to be tidied.
fn compat_mosaic<T: Sample>(
    source: &[T],
    width: usize,
    height: usize,
    raw: &RawImage,
    levels: &Levels,
) -> Result<(NormalizedSamples, Tally)> {
    let black = raw.blacklevel.as_bayer_array();
    let white = raw.whitelevel.as_bayer_array();
    let mut max = [0.0_f32; 4];
    for phase in 0..4 {
        // Same order as Rawler: `max = white - black`, once, then divide.
        max[phase] = white[phase] - black[phase];
        if !max[phase].is_finite() || max[phase] <= 0.0 {
            bail!(
                "Bayer phase {phase} has an unusable level range: white {} minus black {} is {}",
                white[phase],
                black[phase],
                max[phase]
            );
        }
    }

    let mut out: Vec<f32> = Vec::with_capacity(width * height);
    out.par_extend(source.par_iter().map(|sample| sample.widen()));

    if height >= 2 && width >= 2 {
        out.par_chunks_exact_mut(width * 2).for_each(|block| {
            for column in 0..width / 2 {
                let (left, right) = (column * 2, column * 2 + 1);
                block[left] = apply(block[left], black[0], max[0], true);
                block[right] = apply(block[right], black[1], max[1], true);
                block[width + left] = apply(block[width + left], black[2], max[2], true);
                block[width + right] = apply(block[width + right], black[3], max[3], true);
            }
        });
    }

    let tally = tally_mosaic(source, &out, width, levels);
    Ok((NormalizedSamples::Mosaic(out), tally))
}

/// Bit-identical port of `apply_scaling` on `LinearRaw` data.
///
/// `imgop::raw::correct_blacklevel` walks the interleaved samples in
/// `blacklevel.len()`-sized chunks, so the component a level applies to is the
/// sample's index modulo that length — which is the pixel's component only when
/// the two happen to agree. `levels::prepare_levels` makes them agree before any
/// real file gets here; the general form is kept so this stays a faithful port
/// rather than a port of the case we happen to hit.
fn compat_linear<T: Sample, const N: usize>(
    source: &[T],
    width: usize,
    height: usize,
    raw: &RawImage,
) -> Result<Vec<[f32; N]>> {
    let black = raw.blacklevel.as_vec();
    let white = raw.whitelevel.as_vec();
    if black.len() != white.len() {
        // Rawler panics here (`correct_blacklevel`'s final match arm).
        bail!(
            "the file records {} black level(s) and {} white level(s); Rawler's linear-raw \
             rescale requires the same count of each",
            black.len(),
            white.len()
        );
    }
    let stride = black.len();
    ensure!(stride > 0, "the file records no black level at all");

    let mut max = Vec::with_capacity(stride);
    for (index, (black, white)) in black.iter().zip(white.iter()).enumerate() {
        let range = white - black;
        if !range.is_finite() || range <= 0.0 {
            bail!(
                "linear level {index} has an unusable range: white {white} minus black {black} \
                 is {range}"
            );
        }
        max.push(range);
    }

    // `chunks_exact_mut(stride)` drops a trailing partial chunk, leaving those
    // samples at raw DN.
    let limit = (source.len() / stride) * stride;
    let mut out = vec![[0.0_f32; N]; width * height];
    out.par_chunks_mut(width * ROWS_PER_CHUNK)
        .enumerate()
        .for_each(|(chunk, block)| {
            let first_row = chunk * ROWS_PER_CHUNK;
            for (offset, line) in block.chunks_mut(width).enumerate() {
                let start = (first_row + offset) * width * N;
                for (x, pixel) in line.iter_mut().enumerate() {
                    for (component, slot) in pixel.iter_mut().enumerate() {
                        let flat = start + x * N + component;
                        let value = source[flat].widen();
                        *slot = if flat < limit {
                            let level = if stride == N {
                                component
                            } else {
                                flat % stride
                            };
                            apply(value, black[level], max[level], true)
                        } else {
                            value
                        };
                    }
                }
            }
        });
    Ok(out)
}

/// Measure a normalized mosaic against the corrected levels.
fn tally_mosaic<T: Sample>(source: &[T], out: &[f32], width: usize, levels: &Levels) -> Tally {
    let parts: Vec<Tally> = out
        .par_chunks(width * ROWS_PER_CHUNK)
        .enumerate()
        .map(|(chunk, block)| {
            let first_row = chunk * ROWS_PER_CHUNK;
            let mut tally = Tally::default();
            for (offset, line) in block.chunks(width).enumerate() {
                let row = first_row + offset;
                let base = levels.row_base(row);
                let mut column = levels.start_x;
                let src = &source[row * width..row * width + width];
                for (value, sample) in line.iter().zip(src.iter()) {
                    let index = base + column * levels.cpp;
                    if (sample.widen() - levels.black[index]).is_sign_negative() {
                        tally.sub_black += 1;
                    }
                    tally.observe(*value);
                    column += 1;
                    if column == levels.width {
                        column = 0;
                    }
                }
            }
            tally
        })
        .collect();
    Tally::combine(parts)
}

/// Measure a normalized linear buffer against the corrected levels.
fn tally_linear<T: Sample, const N: usize>(
    source: &[T],
    out: &[[f32; N]],
    width: usize,
    levels: &Levels,
) -> Tally {
    let parts: Vec<Tally> = out
        .par_chunks(width * ROWS_PER_CHUNK)
        .enumerate()
        .map(|(chunk, block)| {
            let first_row = chunk * ROWS_PER_CHUNK;
            let mut tally = Tally::default();
            for (offset, line) in block.chunks(width).enumerate() {
                let row = first_row + offset;
                let base = levels.row_base(row);
                let mut column = levels.start_x;
                let start = row * width * N;
                for (x, pixel) in line.iter().enumerate() {
                    let index = base + column * levels.cpp;
                    for (component, value) in pixel.iter().enumerate() {
                        let sample = source[start + x * N + component].widen();
                        if (sample - levels.black[index + component]).is_sign_negative() {
                            tally.sub_black += 1;
                        }
                        tally.observe(*value);
                    }
                    column += 1;
                    if column == levels.width {
                        column = 0;
                    }
                }
            }
            tally
        })
        .collect();
    Tally::combine(parts)
}

/// The region of the frame the demosaic runs over.
///
/// This is all `ProcessingStep::CropActiveArea` ever meant: `develop.rs:140`
/// uses it to pick the demosaic ROI and nothing else. The returned rectangle is
/// in full-frame coordinates, which is what `Demosaic::demosaic` expects — the
/// DNG decoder has already shifted the CFA to match, so `color_at(y, x)` is
/// correct with unshifted coordinates.
pub fn demosaic_roi(raw: &RawImage) -> Rect {
    raw.active_area
        .unwrap_or_else(|| Rect::new(Point::zero(), Dim2::new(raw.width, raw.height)))
}

/// The `DefaultCrop` rectangle, expressed in the coordinates of the buffer that
/// was actually produced, or `None` when no copy is needed.
///
/// `roi` is the region of the source frame the produced buffer covers: the
/// demosaic ROI for CFA data, and the whole frame for `LinearRaw`, which is not
/// demosaiced at all. That distinction is a divergence from Rawler, on purpose.
/// `develop.rs:206` adapts the crop whenever both `Demosaic` and
/// `CropActiveArea` appear in the step list — even for `LinearRaw`, where the
/// demosaic (and therefore the ROI) was skipped, so the crop is shifted by an
/// origin that was never applied. It is inert on this corpus, where every
/// `ActiveArea` origin is `(0, 0)`, and it is a bug worth not reproducing.
///
/// `Rect::adapt` is deliberately not used: it carries four `assert!`s and would
/// abort the process on a file whose crop sits outside its active area. A batch
/// developer has to survive one bad input.
pub fn default_crop(raw: &RawImage, produced: Dim2, roi: Rect) -> Result<Option<Rect>> {
    let Some(crop) = raw.crop_area.or(raw.active_area) else {
        return Ok(None);
    };

    // The superpixel half-scale branch (`develop.rs:211`) is provably dead on
    // every path this program runs. `develop_intermediate` only ever selects
    // `PPGDemosaic` or `Bilinear4Channel`, both of which return a buffer the
    // size of the ROI; the only demosaic that halves the dimensions is
    // `Superpixel3Channel`/`Superpixel4Channel`
    // (`imgop/sensor/bayer/superpixel.rs:72`, `Color2D::new_with(out, roi.d.w >> 1,
    // roi.d.h >> 1)`), which neither Rawler's step list nor this module ever
    // reaches. Omitted rather than ported, with the invariant asserted.
    debug_assert_ne!(
        produced.w,
        roi.d.w / 2,
        "the produced buffer is half the ROI width, which only a superpixel demosaic does; the \
         omitted half-scale crop branch would have been needed"
    );

    ensure!(
        crop.p.x >= roi.p.x && crop.p.y >= roi.p.y,
        "the file's default crop starts at ({}, {}), above or left of the region that was \
         developed (({}, {})), so it cannot be applied",
        crop.p.x,
        crop.p.y,
        roi.p.x,
        roi.p.y
    );
    let placed = Rect::new(Point::new(crop.p.x - roi.p.x, crop.p.y - roi.p.y), crop.d);
    ensure!(
        placed.p.x + placed.d.w <= produced.w && placed.p.y + placed.d.h <= produced.h,
        "the file's default crop {placed:?} does not fit inside the {}x{} buffer that was \
         developed",
        produced.w,
        produced.h
    );

    // Rawler skips the copy when the dimensions already match. Widened to also
    // require a zero origin: a crop with the same size but a non-zero offset is
    // a real crop, and skipping it would silently develop the wrong pixels.
    if placed.d == produced && placed.p == Point::zero() {
        return Ok(None);
    }
    Ok(Some(placed))
}

/// Crop a row-major pixel buffer in place.
///
/// `Color2D::crop` allocates a second buffer, which on a 50 MP frame means
/// 600 MB of `[f32; 3]` alive twice at the moment of the copy — and the crop is
/// almost always a few pixels off each edge, so nearly all of that is a copy of
/// itself. Every destination row starts at or before its source row, so the move
/// can go forward through one buffer instead. Same bytes, same order, half the
/// high-water mark; `crop_in_place_matches_rawlers_color2d_crop` pins the
/// equivalence.
pub fn crop_in_place<T: Copy>(pixels: &mut Vec<T>, width: usize, crop: Rect) {
    debug_assert!(crop.p.y + crop.d.h <= pixels.len() / width.max(1));
    debug_assert!(crop.p.x + crop.d.w <= width);
    for row in 0..crop.d.h {
        let source = (crop.p.y + row) * width + crop.p.x;
        let destination = row * crop.d.w;
        debug_assert!(destination <= source);
        pixels.copy_within(source..source + crop.d.w, destination);
    }
    pixels.truncate(crop.d.w * crop.d.h);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawler::Orientation;
    use rawler::cfa::PlaneColor;
    use rawler::decoders::Camera;
    use rawler::formats::tiff::Rational;
    use rawler::rawimage::{BlackLevel, CFAConfig, WhiteLevel};
    use rawler::{CFA, RawImageData};
    use std::collections::HashMap;

    /// Parameterized `RawImage` builder.
    ///
    /// Shaped after `noise.rs`'s `synthetic_bayer_raw`, but parameterized over
    /// everything this module reads — pattern, repeat dimensions, active area and
    /// storage type — because the whole point of the tests below is to vary
    /// exactly those. Fields this module never touches carry inert placeholders.
    struct Builder {
        width: usize,
        height: usize,
        cpp: usize,
        photometric: RawPhotometricInterpretation,
        blacklevel: BlackLevel,
        whitelevel: WhiteLevel,
        active_area: Option<Rect>,
        crop_area: Option<Rect>,
        data: RawImageData,
    }

    impl Builder {
        /// A CFA frame with the given pattern and a 2x2 black-level repeat.
        fn cfa(pattern: &str, width: usize, height: usize, black: [f32; 4], white: u32) -> Self {
            let cfa = CFA::new(pattern);
            let colors = PlaneColor::new("RGB");
            Self {
                width,
                height,
                cpp: 1,
                photometric: RawPhotometricInterpretation::Cfa(CFAConfig::new(&cfa, &colors)),
                blacklevel: rational_levels(&black, 2, 2, 1),
                whitelevel: WhiteLevel::new(vec![white]),
                active_area: Some(Rect::new(Point::zero(), Dim2::new(width, height))),
                crop_area: None,
                data: RawImageData::Integer(ramp(width * height)),
            }
        }

        /// A `LinearRaw` frame with `cpp` components and one level per component.
        fn linear(cpp: usize, width: usize, height: usize, black: &[f32], white: u32) -> Self {
            Self {
                width,
                height,
                cpp,
                photometric: RawPhotometricInterpretation::LinearRaw,
                blacklevel: rational_levels(black, 1, 1, cpp),
                whitelevel: WhiteLevel::new(vec![white; cpp]),
                active_area: Some(Rect::new(Point::zero(), Dim2::new(width, height))),
                crop_area: None,
                data: RawImageData::Integer(ramp(width * height * cpp)),
            }
        }

        fn black(mut self, levels: &[f32], width: usize, height: usize, cpp: usize) -> Self {
            self.blacklevel = rational_levels(levels, width, height, cpp);
            self
        }

        fn active(mut self, area: Option<Rect>) -> Self {
            self.active_area = area;
            self
        }

        fn crop(mut self, area: Option<Rect>) -> Self {
            self.crop_area = area;
            self
        }

        fn data(mut self, data: RawImageData) -> Self {
            self.data = data;
            self
        }

        fn build(self) -> RawImage {
            RawImage {
                camera: Camera::default(),
                make: String::new(),
                model: String::new(),
                clean_make: String::new(),
                clean_model: String::new(),
                width: self.width,
                height: self.height,
                cpp: self.cpp,
                bps: 16,
                wb_coeffs: [1.0, 1.0, 1.0, 1.0],
                whitelevel: self.whitelevel,
                blacklevel: self.blacklevel,
                xyz_to_cam: [[0.0; 3]; 4],
                photometric: self.photometric,
                active_area: self.active_area,
                crop_area: self.crop_area,
                blackareas: Vec::new(),
                orientation: Orientation::Normal,
                data: self.data,
                color_matrix: HashMap::new(),
                dng_tags: HashMap::new(),
            }
        }
    }

    /// Build a `BlackLevel` without going through `BlackLevel::new`, which
    /// asserts the sample count — the malformed-layout tests need to construct
    /// exactly what it rejects.
    fn rational_levels(levels: &[f32], width: usize, height: usize, cpp: usize) -> BlackLevel {
        BlackLevel {
            levels: levels
                .iter()
                .map(|value| Rational::new((value * 100.0).round() as u32, 100))
                .collect(),
            width,
            height,
            cpp,
        }
    }

    /// Samples that straddle black: a low ramp so a black level of a few hundred
    /// leaves plenty of sub-black samples, deterministic so a bitwise comparison
    /// means something.
    fn ramp(count: usize) -> Vec<u16> {
        (0..count).map(|index| (index % 1024) as u16).collect()
    }

    /// Samples spread across a band of black levels, so that pairing a sample
    /// with the wrong level changes its value instead of clipping every sample to
    /// zero — which is what an all-sub-black frame would do, hiding exactly the
    /// divergence the level-indexing tests exist to find.
    fn straddle(count: usize, low: u16, span: u16) -> Vec<u16> {
        (0..count)
            .map(|index| low + (index as u16).wrapping_mul(37) % span)
            .collect()
    }

    fn bits(values: &[f32]) -> Vec<u32> {
        values.iter().map(|value| value.to_bits()).collect()
    }

    /// What Rawler itself would have produced, through its own public API.
    fn rawler_reference(raw: &RawImage) -> Vec<f32> {
        let mut clone = raw.clone();
        clone
            .apply_scaling()
            .expect("apply_scaling accepts these levels");
        clone.data.as_f32().to_vec()
    }

    fn flat(samples: &NormalizedSamples) -> Vec<f32> {
        match samples {
            NormalizedSamples::Mosaic(data) => data.clone(),
            NormalizedSamples::Linear3(data) => data.iter().flatten().copied().collect(),
            NormalizedSamples::Linear4(data) => data.iter().flatten().copied().collect(),
        }
    }

    fn normalized(raw: &RawImage, policy: SubBlack) -> Normalized {
        normalize(raw, policy).expect("these levels are usable")
    }

    /// The load-bearing claim of the whole phase: with `RawlerCompat` selected,
    /// this module is `RawImage::apply_scaling` to the last bit. Compared against
    /// Rawler's own shipped implementation rather than a transcription of it, and
    /// bitwise rather than with a tolerance — `to_bits()` so `+0.0` and `-0.0`
    /// cannot pass for each other, which is exactly the distinction the
    /// `is_sign_negative()` clip turns on.
    #[test]
    fn rawler_compat_is_bitwise_identical_on_rggb_with_four_distinct_levels() {
        let raw = Builder::cfa("RGGB", 8, 6, [200.0, 220.0, 240.0, 260.0], 4095).build();
        let ours = normalized(&raw, SubBlack::RawlerCompat);
        assert_eq!(bits(&flat(&ours.samples)), bits(&rawler_reference(&raw)));
    }

    /// The same on GBRG, with four different levels — the case an earlier draft
    /// claimed Rawler got wrong. `Clip` agreeing with `RawlerCompat` here is the
    /// evidence that it does not: `BlackLevel` is positional, so reading it
    /// positionally (which is what `Clip` does, anchored at the ActiveArea) and
    /// reading it by Bayer phase from the frame origin (which is what Rawler
    /// does) are the same thing whenever the origin is `(0, 0)`.
    #[test]
    fn rawler_compat_and_clip_agree_on_gbrg_with_four_distinct_levels() {
        let raw = Builder::cfa("GBRG", 8, 6, [200.0, 220.0, 240.0, 260.0], 4095).build();
        let compat = normalized(&raw, SubBlack::RawlerCompat);
        assert_eq!(bits(&flat(&compat.samples)), bits(&rawler_reference(&raw)));

        let clip = normalized(&raw, SubBlack::Clip);
        assert_eq!(
            bits(&flat(&clip.samples)),
            bits(&flat(&compat.samples)),
            "positional black levels make Rawler's phase indexing correct on GBRG; if this \
             fails, the 'no GBRG bug' finding is wrong"
        );
        assert_eq!(
            clip.report.repeat_colors,
            vec!["GREEN", "BLUE", "RED", "GREEN"],
            "the report has to name which level went to which colour"
        );
    }

    #[test]
    fn rawler_compat_is_bitwise_identical_on_linear_raw_cpp3() {
        let raw = Builder::linear(3, 5, 4, &[100.0, 150.0, 200.0], 4095).build();
        let ours = normalized(&raw, SubBlack::RawlerCompat);
        assert!(matches!(ours.samples, NormalizedSamples::Linear3(_)));
        assert_eq!(bits(&flat(&ours.samples)), bits(&rawler_reference(&raw)));
    }

    /// cpp 1 exercises `correct_blacklevel`'s `CH == 1` arm, which is the only
    /// one that touches every sample with no chunking at all.
    #[test]
    fn rawler_compat_is_bitwise_identical_on_linear_raw_cpp1() {
        let raw = Builder::linear(1, 7, 5, &[64.0], 1023).build();
        let ours = normalized(&raw, SubBlack::RawlerCompat);
        assert!(matches!(ours.samples, NormalizedSamples::Mosaic(_)));
        assert_eq!(bits(&flat(&ours.samples)), bits(&rawler_reference(&raw)));
    }

    /// ProShot's real black level is `25625/100`. A `Rational` read that rounded
    /// or that went through an integer would land on 256 and shift the whole
    /// shadow range by a quarter of a DN, so the exactness is worth pinning.
    #[test]
    fn the_proshot_fractional_black_level_reads_as_exactly_256_25() {
        let raw = Builder::cfa("GBRG", 8, 6, [0.0; 4], 4095)
            .black(&[256.25; 4], 2, 2, 1)
            .build();
        let report = normalized(&raw, SubBlack::Clip).report;
        assert_eq!(report.black_levels, vec![256.25_f32; 4]);
        assert_eq!(Rational::new(25625, 100).as_f32(), 256.25);
    }

    /// Defect (e), made visible. Rawler's two-row stride skips the last row of an
    /// odd-height frame and its two-column stride skips the last column of an
    /// odd-width one, leaving raw DN — thousands of times too bright — in the
    /// developed buffer. `Clip` normalizes them, so the two must differ, and the
    /// note must say so.
    #[test]
    fn clip_diverges_from_rawler_compat_on_odd_dimensions() {
        let raw = Builder::cfa("RGGB", 7, 5, [200.0; 4], 4095).build();
        let compat = normalized(&raw, SubBlack::RawlerCompat);
        let clip = normalized(&raw, SubBlack::Clip);
        let compat_samples = flat(&compat.samples);
        let clip_samples = flat(&clip.samples);

        assert_eq!(bits(&compat_samples), bits(&rawler_reference(&raw)));
        assert_ne!(bits(&clip_samples), bits(&compat_samples));

        // Every sample in this frame is below black, so a normalized buffer is
        // all zeros. The bottom-right sample is in both the skipped row and the
        // skipped column, and Rawler leaves it at raw DN — sample 34 of a ramp.
        assert_eq!(
            compat_samples[7 * 5 - 1],
            34.0,
            "Rawler's two-row/two-column stride should leave this sample at raw DN"
        );
        assert_eq!(clip_samples[7 * 5 - 1], 0.0);
        assert!(
            compat.report.max_normalized > 1.0,
            "raw DN surviving into the developed buffer is what makes defect (e) loud: {:?}",
            compat.report
        );
        assert_eq!(
            clip.report.max_normalized, 0.0,
            "every sample should be normalized under Clip: {:?}",
            clip.report
        );
        assert!(
            clip.report
                .notes
                .iter()
                .any(|note| note.contains("7x5") && note.contains("unnormalized")),
            "{:?}",
            clip.report.notes
        );
    }

    /// Defect (d), made visible: a two-element CFA black level. `as_bayer_array`
    /// broadcasts element `[0]` to all four phases and drops the second level
    /// entirely.
    #[test]
    fn clip_diverges_from_rawler_compat_on_a_two_element_black_level() {
        // Samples between the two levels: Rawler broadcasts 100 and calls them
        // all positive, while the real 2x1 pattern puts 300 on every odd column
        // and takes those below black.
        let raw = Builder::cfa("RGGB", 8, 6, [0.0; 4], 4095)
            .black(&[100.0, 300.0], 2, 1, 1)
            .data(RawImageData::Integer(straddle(48, 180, 120)))
            .build();
        let compat = normalized(&raw, SubBlack::RawlerCompat);
        let clip = normalized(&raw, SubBlack::Clip);

        assert_eq!(bits(&flat(&compat.samples)), bits(&rawler_reference(&raw)));
        assert_ne!(
            bits(&flat(&clip.samples)),
            bits(&flat(&compat.samples)),
            "Clip honours the 2x1 repeat; Rawler broadcasts the first level"
        );
        assert_eq!(clip.report.repeat_width, 2);
        assert_eq!(clip.report.repeat_height, 1);
        assert!(
            clip.report
                .notes
                .iter()
                .any(|note| note.contains("as_bayer_array")),
            "{:?}",
            clip.report.notes
        );
    }

    /// Defect (c), made visible: with a non-zero `ActiveArea` origin the CFA is
    /// shifted by the decoder but the black level is not, so Rawler pairs each
    /// level with the wrong sensor colour.
    #[test]
    fn clip_diverges_from_rawler_compat_on_a_non_zero_active_area_origin() {
        // The origin is odd in both axes, so anchoring the pattern at the active
        // area swaps level 0 with 3 and 1 with 2. Samples spread across the four
        // levels so the swap changes values rather than clipping them all.
        let raw = Builder::cfa("RGGB", 8, 6, [200.0, 220.0, 240.0, 260.0], 4095)
            .active(Some(Rect::new(Point::new(1, 1), Dim2::new(6, 4))))
            .data(RawImageData::Integer(straddle(48, 180, 120)))
            .build();
        let compat = normalized(&raw, SubBlack::RawlerCompat);
        let clip = normalized(&raw, SubBlack::Clip);

        assert_eq!(bits(&flat(&compat.samples)), bits(&rawler_reference(&raw)));
        assert_ne!(bits(&flat(&clip.samples)), bits(&flat(&compat.samples)));
        assert!(
            clip.report
                .notes
                .iter()
                .any(|note| note.contains("ActiveArea origin is (1, 1)")),
            "{:?}",
            clip.report.notes
        );
    }

    /// Defect (a): the change under test. A sample one DN below black must come
    /// out negative under `Preserve` and at exactly `+0.0` under the two clipping
    /// policies — `+0.0` and not merely "zero", because the sign of zero is what
    /// distinguishes a clipped sample from a preserved `-0.0` one.
    #[test]
    fn preserve_keeps_a_negative_where_rawler_compat_yields_positive_zero() {
        let raw = Builder::cfa("RGGB", 8, 6, [200.0; 4], 4095)
            .data(RawImageData::Integer(vec![199; 48]))
            .build();

        let compat = normalized(&raw, SubBlack::RawlerCompat);
        let preserve = normalized(&raw, SubBlack::Preserve);
        let compat_samples = flat(&compat.samples);
        let preserve_samples = flat(&preserve.samples);

        assert_eq!(compat_samples[0].to_bits(), 0.0_f32.to_bits());
        assert!(compat_samples[0].is_sign_positive());
        assert!(
            preserve_samples[0] < 0.0,
            "a sub-black sample must survive as a negative under Preserve, got {}",
            preserve_samples[0]
        );
        assert_eq!(
            preserve_samples[0],
            -1.0 / (4095.0 - 200.0),
            "one DN below black, normalized"
        );

        // The fraction is a property of the file, so all three policies report it
        // identically even though only one of them keeps the values.
        assert_eq!(compat.report.sub_black_fraction, 1.0);
        assert_eq!(preserve.report.sub_black_fraction, 1.0);
        assert_eq!(compat.report.min_normalized, 0.0);
        assert!(preserve.report.min_normalized < 0.0);
    }

    /// `noise::estimate` reads `raw.data` as original integer DN and a test pins
    /// its fitted model to exact `f32` equality, so a normalizer that touched the
    /// image would move `snr10_ev` and everything downstream of it.
    #[test]
    fn normalize_does_not_mutate_the_raw_image() {
        let raw = Builder::cfa("RGGB", 8, 6, [200.0, 220.0, 240.0, 260.0], 4095).build();
        let before_data = raw.data.as_f32().to_vec();
        let before_black = raw.blacklevel.clone();
        let before_white = raw.whitelevel.clone();

        for policy in [SubBlack::RawlerCompat, SubBlack::Clip, SubBlack::Preserve] {
            let _ = normalized(&raw, policy);
        }

        assert_eq!(bits(&raw.data.as_f32()), bits(&before_data));
        assert_eq!(raw.blacklevel, before_black);
        assert_eq!(raw.whitelevel, before_white);
    }

    /// The parallel kernel computes each sample's repeat position from a chunk
    /// index plus an offset, which is the one place a threading mistake could
    /// change values. Checked against a sequential implementation that walks the
    /// frame in plain order with no chunking at all.
    #[test]
    fn parallel_normalization_equals_the_sequential_reference() {
        // Deliberately not a multiple of ROWS_PER_CHUNK, and a 3x2 repeat so the
        // pattern phase does not line up with the chunk boundary either.
        let width = 13;
        let height = 3 * ROWS_PER_CHUNK + 5;
        let levels: Vec<f32> = (0..6).map(|index| 100.0 + index as f32 * 40.0).collect();
        let raw = Builder::cfa("RGGB", width, height, [0.0; 4], 4095)
            .black(&levels, 3, 2, 1)
            .active(Some(Rect::new(Point::new(2, 1), Dim2::new(9, 8))))
            .build();

        for policy in [SubBlack::Clip, SubBlack::Preserve] {
            let ours = flat(&normalized(&raw, policy).samples);
            let mut expected = Vec::with_capacity(width * height);
            let source = raw.data.as_f32();
            for y in 0..height {
                for x in 0..width {
                    // Anchored at the active-area origin (2, 1) with a 3x2
                    // repeat, written out longhand.
                    let row = (y + 2 * 6 - 1) % 2;
                    let column = (x + 3 * 6 - 2) % 3;
                    let index = row * 3 + column;
                    let shifted = source[y * width + x] - levels[index];
                    let shifted = if policy.clips() && shifted.is_sign_negative() {
                        0.0
                    } else {
                        shifted
                    };
                    expected.push(shifted / (4095.0 - levels[index]));
                }
            }
            assert_eq!(bits(&ours), bits(&expected), "{policy:?}");
        }
    }

    /// Rawler divides by `white - black` with no check, so a file recording
    /// `white <= black` develops to `inf` or `NaN` and fails several stages later
    /// as "too few valid pixels for analysis". Naming the values here is the
    /// improvement.
    #[test]
    fn an_unusable_level_range_is_a_named_error_rather_than_an_infinity() {
        let raw = Builder::cfa("RGGB", 8, 6, [4095.0; 4], 4095).build();
        for policy in [SubBlack::RawlerCompat, SubBlack::Clip, SubBlack::Preserve] {
            let message = normalize(&raw, policy)
                .expect_err("a zero-width level range must be refused")
                .to_string();
            assert!(
                message.contains("4095") && message.contains("unusable"),
                "{message}"
            );
        }
    }

    /// A black level describing a different number of components from the image
    /// cannot be indexed at all, and guessing would put a red level on a green
    /// photosite.
    #[test]
    fn a_black_level_with_the_wrong_component_count_is_refused() {
        let raw = Builder::linear(3, 4, 4, &[0.0; 3], 4095)
            .black(&[10.0, 20.0, 30.0, 40.0], 1, 1, 4)
            .build();
        let message = normalize(&raw, SubBlack::Clip)
            .expect_err("cpp 4 levels on a cpp 3 image must be refused")
            .to_string();
        assert!(
            message.contains("4 component(s)") && message.contains("the image has 3"),
            "{message}"
        );
    }

    /// The three real geometries in this project's corpus, from
    /// `cargo run --example probe-levels`.
    #[test]
    fn the_three_real_geometries_produce_the_right_roi_and_crop() {
        // Sony A7C: 6048x4024 sensor, active area the whole frame, DefaultCrop
        // inset by 12 pixels on each side.
        let a7c = Builder::cfa("RGGB", 8, 6, [512.0; 4], 15360)
            .active(Some(Rect::new(Point::zero(), Dim2::new(6048, 4024))))
            .crop(Some(Rect::new(Point::new(12, 12), Dim2::new(6000, 4000))))
            .build();
        assert_eq!(
            demosaic_roi(&a7c),
            Rect::new(Point::zero(), Dim2::new(6048, 4024))
        );
        assert_eq!(
            default_crop(&a7c, Dim2::new(6048, 4024), demosaic_roi(&a7c)).unwrap(),
            Some(Rect::new(Point::new(12, 12), Dim2::new(6000, 4000)))
        );

        // ProShot: 4080x3060, DefaultCrop inset by 8.
        let proshot = Builder::cfa("GBRG", 8, 6, [256.25; 4], 4095)
            .active(Some(Rect::new(Point::zero(), Dim2::new(4080, 3060))))
            .crop(Some(Rect::new(Point::new(8, 8), Dim2::new(4064, 3044))))
            .build();
        assert_eq!(
            default_crop(&proshot, Dim2::new(4080, 3060), demosaic_roi(&proshot)).unwrap(),
            Some(Rect::new(Point::new(8, 8), Dim2::new(4064, 3044)))
        );

        // Samsung Expert RAW: crop == active == the whole frame, so there is
        // nothing to copy and the answer must be `None` rather than a no-op rect.
        let samsung = Builder::linear(3, 8, 6, &[0.0; 3], 65535)
            .active(Some(Rect::new(Point::zero(), Dim2::new(5712, 4284))))
            .crop(Some(Rect::new(Point::zero(), Dim2::new(5712, 4284))))
            .build();
        assert_eq!(
            default_crop(&samsung, Dim2::new(5712, 4284), demosaic_roi(&samsung)).unwrap(),
            None
        );
    }

    /// The no-op skip requires a zero origin as well as matching dimensions,
    /// where Rawler tests only the dimensions (`develop.rs:216`).
    ///
    /// The widened condition turns out to be *unreachable* rather than merely
    /// unused, and that is the useful finding: given the bounds check, a crop
    /// whose size already fills the buffer cannot also be offset inside it. So
    /// Rawler's narrower test is not a live bug — it is guarded by an assertion
    /// in `Color2D::crop` that would have aborted the batch instead. Keeping the
    /// origin in the condition means the skip stays correct if the bounds check
    /// is ever relaxed, and this test records why no divergence can be
    /// demonstrated.
    #[test]
    fn a_same_size_crop_at_a_non_zero_origin_cannot_fit_and_is_refused() {
        let raw = Builder::cfa("RGGB", 8, 6, [0.0; 4], 4095)
            .active(Some(Rect::new(Point::zero(), Dim2::new(8, 6))))
            .crop(Some(Rect::new(Point::new(1, 1), Dim2::new(8, 6))))
            .build();
        assert!(default_crop(&raw, Dim2::new(8, 6), demosaic_roi(&raw)).is_err());

        // A genuinely smaller offset crop is returned, in buffer coordinates.
        let inset = Builder::cfa("RGGB", 10, 10, [0.0; 4], 4095)
            .active(Some(Rect::new(Point::zero(), Dim2::new(10, 10))))
            .crop(Some(Rect::new(Point::new(1, 1), Dim2::new(8, 6))))
            .build();
        assert_eq!(
            default_crop(&inset, Dim2::new(10, 10), demosaic_roi(&inset)).unwrap(),
            Some(Rect::new(Point::new(1, 1), Dim2::new(8, 6)))
        );
    }

    /// A crop outside the developed buffer is a malformed file, and
    /// `Color2D::crop`'s assertions would abort the whole batch on it.
    #[test]
    fn a_crop_outside_the_developed_buffer_is_refused_rather_than_asserted() {
        let raw = Builder::cfa("RGGB", 8, 6, [0.0; 4], 4095)
            .active(Some(Rect::new(Point::zero(), Dim2::new(8, 6))))
            .crop(Some(Rect::new(Point::new(4, 4), Dim2::new(8, 6))))
            .build();
        assert!(default_crop(&raw, Dim2::new(8, 6), demosaic_roi(&raw)).is_err());

        let above = Builder::cfa("RGGB", 8, 6, [0.0; 4], 4095)
            .active(Some(Rect::new(Point::new(4, 4), Dim2::new(4, 2))))
            .crop(Some(Rect::new(Point::zero(), Dim2::new(4, 2))))
            .build();
        assert!(default_crop(&above, Dim2::new(4, 2), demosaic_roi(&above)).is_err());
    }

    /// The in-place crop is the memory win, so it has to be the same bytes
    /// Rawler's allocating crop would have produced.
    #[test]
    fn crop_in_place_matches_rawlers_color2d_crop() {
        use rawler::pixarray::Color2D;

        let (width, height) = (11, 9);
        let pixels: Vec<[f32; 3]> = (0..width * height)
            .map(|index| [index as f32, index as f32 + 0.5, -(index as f32)])
            .collect();
        let crop = Rect::new(Point::new(3, 2), Dim2::new(6, 5));

        let theirs = Color2D::new_with(pixels.clone(), width, height)
            .crop(crop)
            .into_inner();
        let mut ours = pixels;
        crop_in_place(&mut ours, width, crop);

        assert_eq!(ours.len(), 30);
        assert_eq!(ours, theirs);
    }

    /// The two remaining tripwires, which have no matching policy divergence and
    /// so would otherwise go untested: a black level whose declared repeat
    /// dimensions disagree with its own sample count, and a white-level count
    /// that is neither one nor one per component.
    ///
    /// Both are shapes the corpus has never produced. The notes are the whole
    /// mechanism by which "no real file trips defects (b)-(e)" stays checkable
    /// instead of becoming folklore, so they are worth a test of their own.
    #[test]
    fn the_remaining_preconditions_each_emit_a_note() {
        let mismatched = Builder::cfa("RGGB", 8, 6, [0.0; 4], 4095)
            // Declares a 2x2 repeat but stores three levels.
            .black(&[100.0, 200.0, 300.0], 2, 2, 1)
            .build();
        let notes = normalized(&mismatched, SubBlack::Clip).report.notes;
        assert!(
            notes
                .iter()
                .any(|note| note.contains("declares a 2x2 repeat") && note.contains("3 sample(s)")),
            "{notes:?}"
        );

        let mut odd_white = Builder::cfa("RGGB", 8, 6, [200.0; 4], 4095).build();
        odd_white.whitelevel = WhiteLevel::new(vec![4095, 4095]);
        let notes = normalized(&odd_white, SubBlack::Clip).report.notes;
        assert!(
            notes
                .iter()
                .any(|note| note.contains("white level stores 2 value(s)")),
            "{notes:?}"
        );
    }

    /// Float-storage DNGs exist (`RawImageData::Float`), and they are the only
    /// way a `-0.0` sample can reach the clip — which is the case
    /// `is_sign_negative()` and `< 0.0` disagree about.
    #[test]
    fn a_negative_zero_sample_is_clipped_by_sign_not_by_comparison() {
        let raw = Builder::cfa("RGGB", 8, 6, [0.0; 4], 4095)
            .data(RawImageData::Float(vec![-0.0; 48]))
            .build();
        let compat = flat(&normalized(&raw, SubBlack::RawlerCompat).samples);
        let preserve = flat(&normalized(&raw, SubBlack::Preserve).samples);

        assert_eq!(bits(&compat), bits(&rawler_reference(&raw)));
        assert!(
            compat[0].is_sign_positive(),
            "the clip turns -0.0 into +0.0, which is why it must be written with \
             is_sign_negative()"
        );
        assert!(
            preserve[0].is_sign_negative(),
            "Preserve must leave -0.0 alone, got {}",
            preserve[0]
        );
    }
}
