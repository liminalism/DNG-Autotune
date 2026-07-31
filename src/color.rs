//! The owned colour path: camera RGB to working-space scene-linear, unclipped.
//!
//! # Why this module exists
//!
//! Until 0.1.17 every stage of this program ran downstream of Rawler's
//! `ProcessingStep::Calibrate`, which is not just a colour conversion. Its
//! per-pixel tail (`rawler::imgop::raw::clip_euclidean_norm_avg`) does two
//! destructive things before the data ever reaches us:
//!
//! 1. **Negative channels are clipped unconditionally.** A colour outside the
//!    display gamut converts to a negative channel; that is normal and
//!    recoverable. Clipping it is a hue shift, applied silently.
//! 2. **Any pixel with a channel above 1.0 is replaced** by the average of its
//!    max-normalized colour and its Euclidean norm. After as-shot white
//!    balance — where a Sony A7C multiplies red by about 2.3 and blue by about
//!    1.6 against green at 1.0 — a large share of highlights is above 1.0. So
//!    this is not an edge case: it is the entire highlight range of most
//!    frames, rewritten by a formula with no photographic motivation, before
//!    any of this crate's own highlight handling gets a look.
//!
//!    Despite the name, this second step is not a clip and does not bound its
//!    output: `[2.3, 1.0, 1.6]` comes out as `[1.359, 1.076, 1.207]`, still
//!    above the white point. What it actually removes is the ratio between the
//!    channels — the spread collapses from 1.30 to 0.28. The magnitude mostly
//!    survives and the *colour* is what is spent, which is why blown skies come
//!    out of the Rawler path grey rather than merely bright.
//!
//! That is why `docs/PLAN.md`'s highlight-reconstruction item could not work as
//! written, and why the tuned colour constants in `analyze.rs` are suspect: they
//! were fitted against a signal that had already been squashed. See
//! `docs/REVIEW-2026-07-30.md`.
//!
//! # What the owned path does instead
//!
//! Exactly the same conversion, with nothing thrown away:
//!
//! ```text
//! camera RGB  --(as-shot wb)-->  camera neutral  --(cam_to_working)-->  scene linear
//! ```
//!
//! `cam_to_working` is composed the same way Rawler composes its own — row-
//! normalize `xyz_to_cam * working_to_xyz`, then take the pseudo-inverse — so
//! at `WorkingSpace::Srgb` the owned path is Rawler's result *minus the
//! clipping*, and an A/B diff isolates precisely what the clipping was costing.
//! That was the point of doing it this way round rather than jumping straight to
//! full DNG colour science: one variable at a time.
//!
//! Negatives survive. Values above 1.0 survive. No gamut mapping happens here at
//! all — `tone::compress_gamut` is where out-of-range values are resolved, at the
//! end, once, against the pixel's own rendered luminance.
//!
//! **That hand-off is not yet correct, and the A/B found it.** `compress_gamut`
//! anchors on `luminance(rgb).clamp(0.0, 1.0)`, and `luminance` is a *signed* sum
//! — so a pixel whose luminance is negative anchors at 0, which makes the
//! compressor's scale `0 / |min|` = 0 and zeroes every channel including the
//! positive ones. Rawler's per-channel clip kept those, so on that cohort this
//! module is currently *more* destructive than the clip it replaces. See
//! [`ClipCost::negative_luminance_fraction`], the test
//! `a_negative_luminance_pixel_renders_to_pure_black_but_a_clipped_one_does_not`,
//! and `docs/STATUS.md` for the fix that is queued. It is why `rawler` is still
//! the default.
//!
//! # The optional full-DNG path
//!
//! [`crate::dngcolor`] adds `ForwardMatrix`, `CameraCalibration`,
//! `AnalogBalance`, reciprocal-temperature dual-illuminant interpolation and
//! Bradford adaptation under `--dng-color`. It is opt-in while its colour change
//! is measured against the paired corpus. Without that flag, or on a file with
//! no usable `ForwardMatrix`, the row-normalized milestone-1 transform here is
//! unchanged.
//!
//! One loss used to be *upstream* of this module:
//! `rawler::imgop::raw::correct_blacklevel*` clips sub-black sensor noise to zero
//! during `Rescale`, before demosaic. Since 0.1.18 that step is this program's
//! own — see [`crate::rescale`], which also owns the demosaic ROI and the default
//! crop, leaving Rawler's `PPGDemosaic` as the only piece still rented. The clip
//! is still applied by default: `--sub-black` selects the policy and defaults to
//! bit-for-bit Rawler compatibility, so owning the step moved no output. Turning
//! it off is a separate, measured change.
//!
//! Note what that does *not* mean. It is tempting to conclude that the negatives
//! this module produces are therefore all genuine out-of-gamut scene colour. They
//! are not: the clip happens per channel in *camera* space, so shadow noise
//! sitting at zero in one channel goes negative in the working space the moment it
//! passes through a matrix with negative off-diagonals — which the A7C's has. On
//! this corpus the frames with the largest negative populations are the *noisiest*
//! ones (`snr10_ev` between +1.7 and +2.0 EV), not the most colourful. So a fix
//! that treats every negative as a colour to be gamut-mapped would spend its
//! effort turning black speckle grey. The per-frame noise model in
//! [`crate::noise`] is what tells the two apart.

use crate::demosaic::{DemosaicMethod, DemosaicReport};
use crate::rescale::{self, NormalizedSamples, RescaleReport, SubBlack};
use crate::types::{CameraRgb, Image, SceneLinear};
use anyhow::{Context, Result, bail, ensure};
use clap::ValueEnum;
use rawler::RawImage;
use rawler::imgop::sensor::bayer::{Demosaic, bilinear::Bilinear4Channel, ppg::PPGDemosaic};
use rawler::imgop::xyz::Illuminant;
use rawler::imgop::{Dim2, Point, Rect};
use rawler::pixarray::PixF32;
use rawler::rawimage::RawPhotometricInterpretation;
use rayon::prelude::*;
use serde::Serialize;

/// Row-major 3x3.
pub type Matrix3 = [[f32; 3]; 3];

/// Colour conversion the develop path uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RawColorPath {
    /// Rawler's `Calibrate`, clipping and all. Kept as the control arm of the
    /// A/B and for reproducing pre-0.1.18 output; no longer the default.
    Rawler,
    /// This module: same matrix composition, nothing clipped. The default since
    /// 0.1.18, on the evidence of the paired scorecard.
    Owned,
}

impl RawColorPath {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rawler => "rawler",
            Self::Owned => "owned",
        }
    }
}

/// Linear RGB space the owned path converts camera RGB into.
///
/// Only the owned path honours this; Rawler's `Calibrate` is hardcoded to sRGB
/// primaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkingSpace {
    /// sRGB primaries, D65. The default, and what makes the owned path a
    /// controlled comparison against Rawler rather than two changes at once.
    Srgb,
    /// ITU-R BT.2020 primaries, D65. Wide enough to hold essentially every
    /// real camera colour, so the intermediate stages (analysis, chroma
    /// denoising, local white balance) see far fewer out-of-gamut channels.
    Rec2020,
}

impl WorkingSpace {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::Rec2020 => "rec2020",
        }
    }

    /// Working-space linear RGB to CIE XYZ, both D65-referred.
    ///
    /// sRGB values are Lindbloom's; BT.2020's are the published matrix for
    /// primaries R(0.708, 0.292) G(0.170, 0.797) B(0.131, 0.046) at D65.
    #[allow(clippy::excessive_precision)]
    pub const fn to_xyz_d65(self) -> Matrix3 {
        match self {
            Self::Srgb => [
                [0.4124564, 0.3575761, 0.1804375],
                [0.2126729, 0.7151522, 0.0721750],
                [0.0193339, 0.1191920, 0.9503041],
            ],
            Self::Rec2020 => [
                [0.6369580, 0.1446169, 0.1688810],
                [0.2627002, 0.6779981, 0.0593017],
                [0.0000000, 0.0280727, 1.0609851],
            ],
        }
    }

    /// Working-space linear RGB to display (sRGB primaries) linear RGB, or
    /// `None` when the two are the same space.
    ///
    /// `None` rather than an identity matrix on purpose: it is what lets
    /// `tone::render` take a code path with no extra arithmetic at all, so the
    /// default configuration is byte-identical to the releases before this one.
    pub fn to_display(self) -> Option<Matrix3> {
        match self {
            Self::Srgb => None,
            other => {
                let xyz_to_srgb = invert3(Self::Srgb.to_xyz_d65())
                    .expect("the sRGB primary matrix is invertible");
                Some(multiply3(&xyz_to_srgb, &other.to_xyz_d65()))
            }
        }
    }
}

/// Multiply a 3x3 by a 3x3.
fn multiply3(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    let mut result = [[0.0_f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            result[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    result
}

/// Multiply an `A`x3 by a 3x3.
fn multiply_rows<const A: usize>(a: &[[f32; 3]; A], b: &Matrix3) -> [[f32; 3]; A] {
    let mut result = [[0.0_f32; 3]; A];
    for i in 0..A {
        for j in 0..3 {
            result[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    result
}

/// Scale every row so it sums to 1, leaving all-zero rows alone.
///
/// This is the step that carries the white-balance guarantee: with each camera
/// channel's response to working-space white normalized to 1, the inverse maps
/// camera neutral onto working-space neutral, so a patch the as-shot
/// coefficients made neutral stays neutral. It is also the only chromatic
/// adaptation in the milestone-1 path.
fn normalize_rows<const A: usize>(matrix: [[f32; 3]; A]) -> [[f32; 3]; A] {
    let mut result = [[0.0_f32; 3]; A];
    for row in 0..A {
        let sum: f32 = matrix[row].iter().sum();
        if sum != 0.0 {
            for column in 0..3 {
                result[row][column] = matrix[row][column] / sum;
            }
        }
    }
    result
}

/// Analytic inverse of a 3x3, or `None` when it is singular.
///
/// Crate-visible so `tone`'s tests can express a colour in the working space and
/// check it renders the same as the sRGB original, rather than carrying a second
/// hand-rolled inversion.
pub(crate) fn invert3(m: Matrix3) -> Option<Matrix3> {
    /// The two indices other than `skip`, in order.
    const fn others(skip: usize) -> [usize; 2] {
        match skip {
            0 => [1, 2],
            1 => [0, 2],
            _ => [0, 1],
        }
    }

    let cofactor = |r: usize, c: usize| {
        let [r0, r1] = others(r);
        let [c0, c1] = others(c);
        let minor = m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
        if (r + c) % 2 == 0 { minor } else { -minor }
    };

    let determinant: f32 = (0..3).map(|j| m[0][j] * cofactor(0, j)).sum();
    if !determinant.is_finite() || determinant.abs() < 1.0e-12 {
        return None;
    }

    let mut inverse = [[0.0_f32; 3]; 3];
    for (i, row) in inverse.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            // Adjugate is the transpose of the cofactor matrix.
            *cell = cofactor(j, i) / determinant;
        }
    }
    Some(inverse)
}

/// Moore-Penrose pseudo-inverse of an `N`x3 matrix: `(AᵀA)⁻¹Aᵀ`, shape 3x`N`.
///
/// For `N == 3` this is the plain inverse. For `N == 4` — a camera with an
/// emerald channel — rows beyond the ones the file actually filled are zero and
/// contribute nothing to `AᵀA`, so a three-component matrix gives the same
/// answer whether it is stored 3x3 or padded to 4x3.
fn pseudo_inverse<const N: usize>(matrix: [[f32; 3]; N]) -> Option<[[f32; N]; 3]> {
    let mut gram = [[0.0_f32; 3]; 3];
    for (i, row) in gram.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..N).map(|k| matrix[k][i] * matrix[k][j]).sum();
        }
    }
    let inverse = invert3(gram)?;

    let mut result = [[0.0_f32; N]; 3];
    for (i, row) in result.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| inverse[i][k] * matrix[j][k]).sum();
        }
    }
    Some(result)
}

/// Pick the calibration matrix, deterministically.
///
/// `RawImage::color_matrix` is a `HashMap`, and Rawler's own `Calibrate` falls
/// back to `.iter().next()` when no D65 matrix exists — which is a different
/// matrix from one process to the next, since Rust randomizes `HashMap` order
/// per instance. Determinism is a product property here (`CLAUDE.md`), so the
/// fallback is an explicit sort instead: prefer D65, else the lowest illuminant
/// code, which is stable across runs and across machines.
///
/// Dual-illuminant interpolation — using both matrices and the as-shot white
/// point to blend them — is milestone 2. Today the second matrix is simply
/// unused, exactly as in Rawler.
fn choose_matrix(raw: &RawImage) -> Option<(Illuminant, &Vec<f32>)> {
    let usable = |entry: &(&Illuminant, &Vec<f32>)| {
        let length = entry.1.len();
        length % 3 == 0 && (3..=4).contains(&(length / 3))
    };

    if let Some(matrix) = raw.color_matrix.get(&Illuminant::D65).filter(|matrix| {
        let length = matrix.len();
        length % 3 == 0 && (3..=4).contains(&(length / 3))
    }) {
        return Some((Illuminant::D65, matrix));
    }

    raw.color_matrix
        .iter()
        .filter(usable)
        .min_by_key(|(illuminant, _)| u16::from(**illuminant))
        .map(|(illuminant, matrix)| (*illuminant, matrix))
}

/// Everything needed to take one camera-RGB pixel into the working space.
#[derive(Debug, Clone)]
pub struct ColorTransform {
    /// Camera channel to working-space RGB, 3 rows by up to 4 columns.
    cam_to_working: [[f32; 4]; 3],
    /// As-shot white balance, in the file's own RGBE channel order.
    white_balance: [f32; 4],
    /// How many camera channels the calibration matrix describes.
    channels: usize,
    illuminant: Illuminant,
    working_space: WorkingSpace,
}

impl ColorTransform {
    /// Compose the transform from a decoded file's calibration data.
    pub fn derive(raw: &RawImage, working_space: WorkingSpace) -> Result<Self> {
        let (illuminant, flat) = choose_matrix(raw).context(
            "the file carries no usable XYZ-to-camera calibration matrix, so the owned \
             colour path cannot convert it; re-run with --raw-color-path rawler",
        )?;
        let channels = flat.len() / 3;

        // Pad to four camera channels so the 3- and 4-colour cases share one
        // code path; unfilled rows stay zero and drop out of the pseudo-inverse.
        let mut xyz_to_cam = [[0.0_f32; 3]; 4];
        for channel in 0..channels {
            for axis in 0..3 {
                xyz_to_cam[channel][axis] = flat[channel * 3 + axis];
            }
        }

        let working_to_cam =
            normalize_rows(multiply_rows(&xyz_to_cam, &working_space.to_xyz_d65()));
        let cam_to_working = pseudo_inverse(working_to_cam).with_context(|| {
            format!(
                "the {illuminant:?} calibration matrix is singular, so it cannot be inverted; \
                 re-run with --raw-color-path rawler"
            )
        })?;

        // Same rule as Rawler: a NaN in the first coefficient means the file
        // recorded no as-shot white balance, and the honest answer is to apply
        // none rather than to guess one.
        let white_balance = if raw.wb_coeffs[0].is_nan() {
            [1.0; 4]
        } else {
            raw.wb_coeffs
                .map(|value| if value.is_finite() { value } else { 0.0 })
        };

        Ok(Self {
            cam_to_working,
            white_balance,
            channels,
            illuminant,
            working_space,
        })
    }

    /// Compose the full DNG transform (ForwardMatrix, CameraCalibration,
    /// AnalogBalance, dual-illuminant interpolation) for files that carry it.
    ///
    /// Returns `None` when the file lacks a `ForwardMatrix` or a usable
    /// `ColorMatrix`, so the caller falls back to [`ColorTransform::derive`]. The
    /// white balance is baked into the matrix here — the DNG neutral maps to the
    /// working white by construction — so `white_balance` is unity and
    /// [`convert3`](Self::convert3) applies the matrix alone.
    pub fn derive_dng(
        raw: &RawImage,
        path: &std::path::Path,
        working_space: WorkingSpace,
    ) -> Option<(Self, crate::dngcolor::DngColorReport)> {
        let mut cam_to_working = [[0.0_f32; 4]; 3];
        let (channels, report) =
            match crate::dngcolor::camera_to_working_any(raw, path, working_space)? {
                (crate::dngcolor::DngMatrix::Three(matrix), report) => {
                    for (row, coefficients) in matrix.iter().enumerate() {
                        cam_to_working[row][..3].copy_from_slice(coefficients);
                    }
                    (3, report)
                }
                (crate::dngcolor::DngMatrix::Four(matrix), report) => {
                    cam_to_working = matrix;
                    (4, report)
                }
            };
        let transform = Self {
            cam_to_working,
            white_balance: [1.0; 4],
            channels,
            // The scene illuminant is interpolated, not one of the file's
            // discrete calibration illuminants; the report carries the detail.
            illuminant: Illuminant::Unknown,
            working_space,
        };
        Some((transform, report))
    }

    /// Convert one three-channel camera pixel. Nothing is clipped.
    #[inline]
    pub fn convert3(&self, pixel: [f32; 3]) -> [f32; 3] {
        let m = &self.cam_to_working;
        let r = pixel[0] * self.white_balance[0];
        let g = pixel[1] * self.white_balance[1];
        let b = pixel[2] * self.white_balance[2];
        [
            m[0][0] * r + m[0][1] * g + m[0][2] * b,
            m[1][0] * r + m[1][1] * g + m[1][2] * b,
            m[2][0] * r + m[2][1] * g + m[2][2] * b,
        ]
    }

    /// Convert one four-channel (RGBE) camera pixel. Nothing is clipped.
    #[inline]
    pub fn convert4(&self, pixel: [f32; 4]) -> [f32; 3] {
        let m = &self.cam_to_working;
        let c: [f32; 4] = [
            pixel[0] * self.white_balance[0],
            pixel[1] * self.white_balance[1],
            pixel[2] * self.white_balance[2],
            pixel[3] * self.white_balance[3],
        ];
        [
            m[0][0] * c[0] + m[0][1] * c[1] + m[0][2] * c[2] + m[0][3] * c[3],
            m[1][0] * c[0] + m[1][1] * c[1] + m[1][2] * c[2] + m[1][3] * c[3],
            m[2][0] * c[0] + m[2][1] * c[1] + m[2][2] * c[2] + m[2][3] * c[3],
        ]
    }
}

/// What Rawler's `clip_euclidean_norm_avg` would have done to a pixel.
///
/// Reimplemented from the documented four-step description rather than lifted,
/// and used only to *measure* the cost of the clipping: `--raw-color-path owned`
/// reports how far each frame's pixels would have moved, so one run answers
/// "what was `Calibrate` costing?" without needing a paired before-and-after.
/// Worth knowing what this operator actually is, because the name misleads: it
/// does *not* bound its output to 1.0. On a white-balanced clipped highlight —
/// `[2.3, 1.0, 1.6]`, an ordinary A7C white point applied to a blown pixel — it
/// returns `[1.359, 1.076, 1.207]`, all three channels still above the white
/// point. What it removes is the *ratio* between the channels: the spread
/// collapses from 1.30 to 0.28, which is the highlight's colour, gone. So the
/// two losses are of different kinds. Negatives are destroyed outright; highlights
/// keep some magnitude but are desaturated toward grey by a formula with no
/// photographic motivation, before this crate's own hue-preserving roll-off
/// (`tone::render_pixel_local`'s `highlight_norm`) gets to see them.
///
/// The arithmetic is written in Rawler's exact order — `sum.sqrt() / sqrt(N)`
/// rather than the algebraically equal `(sum / N).sqrt()` — so the reported
/// delta is what Rawler would really have produced, to the last bit.
#[inline]
fn rawler_clip(pixel: [f32; 3]) -> [f32; 3] {
    let clipped = pixel.map(|value| if value < 0.0 { 0.0 } else { value });
    let maximum = clipped.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if maximum > 1.0 {
        let colour = clipped.map(|value| value / maximum);
        let norm = clipped
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt()
            / (3.0_f32).sqrt();
        colour.map(|value| (value + norm) / 2.0)
    } else {
        clipped
    }
}

/// How much of the frame Rawler's `Calibrate` would have rewritten, and by how
/// much. Present only on the owned path, where the unclipped values exist to
/// compare against.
#[derive(Debug, Clone, Serialize)]
pub struct ClipCost {
    /// Fraction of pixels with at least one channel below 0 — out-of-gamut
    /// colours, which Rawler clips to zero.
    pub negative_fraction: f32,
    /// Fraction of pixels whose *luminance* is at or below zero.
    ///
    /// This is the number that predicts the owned path's one real regression, and
    /// it is a much smaller population than `negative_fraction`. A negative
    /// channel alone is harmless: `tone::compress_gamut` anchors on the pixel's
    /// rendered luminance and lands the offending channel on exactly 0, keeping
    /// the rest of the colour. But `tone::render_pixel_local` clamps that anchor
    /// with `luminance(rgb).clamp(0.0, 1.0)`, so a pixel whose luminance is
    /// negative gets anchor 0 — and `compress_gamut`'s scale then evaluates to
    /// `0 / |min| = 0`, collapsing *every* channel to zero, including the positive
    /// ones. That is strictly more destructive than the per-channel clip Rawler
    /// applies, and it is why removing the clip made `crushed_fraction` worse
    /// rather than better. Measured here so the fix can be verified against a
    /// prediction instead of a hunch.
    pub negative_luminance_fraction: f32,
    /// Fraction of pixels with at least one channel above 1 — the highlight
    /// range Rawler replaces with its colour/norm average.
    pub above_one_fraction: f32,
    /// Fraction of pixels Rawler's clip would have moved at all.
    pub altered_fraction: f32,
    /// Mean absolute per-channel movement over the whole frame.
    pub mean_abs_delta: f32,
    /// Largest absolute per-channel movement anywhere in the frame.
    pub max_abs_delta: f32,
}

/// Which colour path ran, and on what. Recorded in the sidecar and the summary
/// so a survey can tell the two paths apart mechanically.
#[derive(Debug, Clone, Serialize)]
pub struct ColorReport {
    pub path: RawColorPath,
    pub working_space: WorkingSpace,
    /// Calibration illuminant actually used, e.g. `D65`.
    pub illuminant: String,
    /// Every illuminant the decoder exposed, sorted. The DNG report carries the
    /// exact one-, two-, or three-calibration interpolation details.
    pub illuminants_available: Vec<String>,
    /// As-shot white balance as applied, in the file's RGBE order.
    pub white_balance: [f32; 4],
    /// `cam_to_working`, row-major, 3 rows by `camera_channels` columns.
    pub cam_to_working: Vec<f32>,
    pub camera_channels: usize,
    /// Bayer interpolation selected for this frame. Absent for LinearRaw,
    /// monochrome, four-colour CFA and the Rawler colour path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub demosaic: Option<DemosaicReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_cost: Option<ClipCost>,
    /// What black/white normalization did, on the owned path only.
    ///
    /// `Option` with `skip_serializing_if` rather than a plain field: the Rawler
    /// path does not own the rescale, has nothing honest to report about it, and
    /// its sidecars have to stay byte-identical across this change so the default
    /// path's A/B baseline survives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rescale: Option<RescaleReport>,
    /// What hot/dead pixel suppression did, when it ran. Owned path only, and
    /// absent (via `skip_serializing_if`) when the correction was off, so a run
    /// with it off serializes exactly as it did before schema 10.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hot_pixels: Option<crate::hotpixels::HotPixelReport>,
    /// What clipped-highlight reconstruction did, when it ran. Owned path only,
    /// absent when off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlight_reconstruction: Option<crate::highlight::HighlightReport>,
    /// What the DNG matrix transform did. Owned path only; absent after a safe
    /// fallback to the decoder camera matrix or when explicitly disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dng_color: Option<crate::dngcolor::DngColorReport>,
    /// Standardized lens operations read from DNG `OpcodeList3`. Absent for
    /// non-DNG files, files without lens opcodes, and when explicitly disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens_correction: Option<crate::lens::LensCorrectionReport>,
}

impl ColorReport {
    /// What to record when Rawler's `Calibrate` did the conversion.
    ///
    /// The illuminant field is where this gets interesting. Rawler's rule is
    /// `find(D65).or_else(|| color_matrix.iter().next())`, and that fallback
    /// walks a `HashMap` — so on a file with two calibration matrices and no D65
    /// one, the matrix Rawler picks differs between processes. That is a
    /// determinism break in the shipped default path, not a hypothetical: it is
    /// the same `HashMap`-ordering trap `CLAUDE.md` already records for IFD
    /// lookups. Reporting it as `nondeterministic` rather than guessing means a
    /// single `--dry-run --summary` over the corpus answers whether any real file
    /// is affected. The owned path has no such case; see [`choose_matrix`].
    pub fn rawler(raw: &RawImage) -> Self {
        let mut illuminants_available: Vec<String> = raw
            .color_matrix
            .keys()
            .map(|illuminant| format!("{illuminant:?}"))
            .collect();
        illuminants_available.sort();

        let illuminant = if raw.color_matrix.contains_key(&Illuminant::D65) {
            "D65".to_string()
        } else if raw.color_matrix.len() > 1 {
            "nondeterministic".to_string()
        } else {
            illuminants_available
                .first()
                .cloned()
                .unwrap_or_else(|| "none".to_string())
        };

        Self {
            path: RawColorPath::Rawler,
            // Rawler's `Calibrate` is hardcoded to sRGB primaries, so
            // `--working-space` cannot apply here whatever it was set to.
            working_space: WorkingSpace::Srgb,
            illuminant,
            illuminants_available,
            white_balance: raw.wb_coeffs,
            // Rawler composes the matrix internally and never hands it back, so
            // there is nothing honest to report. The owned path reports its own.
            cam_to_working: Vec::new(),
            camera_channels: 3,
            demosaic: None,
            clip_cost: None,
            // Rawler's `Rescale` still normalizes on this path, so there is no
            // policy of ours to report.
            rescale: None,
            // Both corrections live inside the owned demosaic/convert path, which
            // this path does not run.
            hot_pixels: None,
            highlight_reconstruction: None,
            dng_color: None,
            lens_correction: None,
        }
    }
}

/// Smallest ROI `PPGDemosaic` can work in.
///
/// `interpolate_borders` handles three pixels in from each edge and the
/// interior passes read a five-wide neighbourhood, so a ROI narrower than this
/// indexes out of its own buffer. Rawler does not check; a malformed
/// `ActiveArea` would panic inside the demosaic and take the whole batch down
/// with it, since a panic there is not attributable to one file's data.
const MIN_DEMOSAIC_ROI: usize = 6;

/// Apply the file's `DefaultCrop` to a developed buffer, in place, and report the
/// dimensions that survive.
fn apply_default_crop<T: Copy>(
    raw: &RawImage,
    pixels: &mut Vec<T>,
    produced: Dim2,
    roi: Rect,
) -> Result<Dim2> {
    match rescale::default_crop(raw, produced, roi)? {
        Some(crop) => {
            rescale::crop_in_place(pixels, produced.w, crop);
            Ok(crop.d)
        }
        None => Ok(produced),
    }
}

/// Demosaic to camera RGB, without any colour conversion.
///
/// The returned image is in the sensor's own channel space: white balance has
/// not been applied and no matrix has been touched. Typing it
/// `Image<CameraRgb>` is what stops it reaching the analyser or the tone curve
/// by accident — both of those take `Image<SceneLinear>`.
///
/// The automatic RCD/AMaZE-class Bayer path is owned here. Rawler's PPG is kept
/// as an explicit diagnostic control; calling it directly still lets normalized
/// samples come from a borrowed `&RawImage` without `develop_intermediate`'s
/// clone-and-scale package.
pub fn demosaic_camera_rgb(
    raw: &RawImage,
    sub_black: SubBlack,
    hot_pixels: f32,
    demosaic: DemosaicMethod,
    snr10_ev: Option<f32>,
) -> Result<(
    CameraImage,
    RescaleReport,
    Option<crate::hotpixels::HotPixelReport>,
    Option<DemosaicReport>,
)> {
    demosaic_camera_rgb_with_lens(raw, sub_black, hot_pixels, demosaic, snr10_ev, None)
}

fn demosaic_camera_rgb_with_lens(
    raw: &RawImage,
    sub_black: SubBlack,
    hot_pixels: f32,
    demosaic: DemosaicMethod,
    snr10_ev: Option<f32>,
    mut lens: Option<&mut crate::lens::LensCorrection>,
) -> Result<(
    CameraImage,
    RescaleReport,
    Option<crate::hotpixels::HotPixelReport>,
    Option<DemosaicReport>,
)> {
    let normalized = rescale::normalize(raw, sub_black)?;
    let (width, height) = (normalized.width, normalized.height);
    // The whole frame, for the buffers that are never cropped to an active area.
    let full = Rect::new(Point::zero(), Dim2::new(width, height));

    // Hot/dead pixel suppression runs on the raw mosaic, before the demosaic
    // smears a stuck site across its neighbourhood. Only the CFA arm has a
    // mosaic to correct; `LinearRaw` frames were demosaiced in-camera and carry
    // no isolated single-site defects for this detector to find.
    let mut hot_pixel_report = None;
    let mut demosaic_report = None;

    let camera = match normalized.samples {
        NormalizedSamples::Mosaic(mut samples) => match &raw.photometric {
            RawPhotometricInterpretation::Cfa(config) => {
                let roi = rescale::demosaic_roi(raw);
                ensure!(
                    roi.p.x + roi.d.w <= width && roi.p.y + roi.d.h <= height,
                    "the file's active area {roi:?} does not fit inside its own {width}x{height} \
                     sample grid, so there is no region to demosaic"
                );
                ensure!(
                    roi.d.w >= MIN_DEMOSAIC_ROI && roi.d.h >= MIN_DEMOSAIC_ROI,
                    "the file's active area is {}x{}, too small for a demosaic to interpolate \
                     ({MIN_DEMOSAIC_ROI}x{MIN_DEMOSAIC_ROI} is the minimum)",
                    roi.d.w,
                    roi.d.h
                );

                if hot_pixels > 0.0 {
                    hot_pixel_report = Some(crate::hotpixels::correct_cfa(
                        &mut samples,
                        width,
                        height,
                        &config.cfa,
                        hot_pixels,
                    ));
                }

                let pixels = PixF32::new_with(samples, width, height);
                if config.cfa.is_rgb() {
                    let report = crate::demosaic::select(
                        demosaic,
                        pixels.pixels(),
                        width,
                        height,
                        &config.cfa,
                        snr10_ev,
                    );
                    let mut buffer = match report.resolved {
                        DemosaicMethod::Ppg => PPGDemosaic::new()
                            .demosaic(&pixels, &config.cfa, &config.colors, roi)
                            .into_inner(),
                        DemosaicMethod::Rcd | DemosaicMethod::Amaze => {
                            crate::demosaic::demosaic_bayer(
                                pixels.pixels(),
                                width,
                                height,
                                &config.cfa,
                                roi,
                                report.resolved,
                            )
                        }
                        DemosaicMethod::Auto => unreachable!("auto is resolved before demosaic"),
                    };
                    demosaic_report = Some(report);
                    let produced = roi.d;
                    // Free the mosaic before the crop: on a 24 megapixel frame
                    // that is 100 MB returned before the next allocation.
                    drop(pixels);
                    if let Some(correction) = lens.as_deref_mut() {
                        correction.apply_three(
                            &mut buffer,
                            produced.w,
                            produced.h,
                            crate::lens::ImageGeometry {
                                full_width: width,
                                full_height: height,
                                origin_x: roi.p.x,
                                origin_y: roi.p.y,
                            },
                        );
                    }
                    let dim = apply_default_crop(raw, &mut buffer, produced, roi)?;
                    CameraImage::Three(Image::new(dim.w, dim.h, buffer)?)
                } else if config.cfa.unique_colors() == 4 {
                    // An RGBE sensor. No target source in `docs/STATUS.md` has
                    // one, but the Rawler path handled it and dropping the case
                    // would have been a regression.
                    let demosaiced =
                        Bilinear4Channel::new().demosaic(&pixels, &config.cfa, &config.colors, roi);
                    let produced = Dim2::new(demosaiced.width, demosaiced.height);
                    let mut buffer = demosaiced.into_inner();
                    drop(pixels);
                    if let Some(correction) = lens.as_deref_mut() {
                        correction.apply_four(
                            &mut buffer,
                            produced.w,
                            produced.h,
                            crate::lens::ImageGeometry {
                                full_width: width,
                                full_height: height,
                                origin_x: roi.p.x,
                                origin_y: roi.p.y,
                            },
                        );
                    }
                    let dim = apply_default_crop(raw, &mut buffer, produced, roi)?;
                    CameraImage::Four {
                        width: dim.w,
                        height: dim.h,
                        pixels: buffer,
                    }
                } else {
                    bail!(
                        "the sensor's colour filter array '{}' has {} unique colours, which no \
                         demosaic in this program handles; re-run with --raw-color-path rawler",
                        config.cfa.name,
                        config.cfa.unique_colors()
                    );
                }
            }
            // A single-component frame that is not a CFA is monochrome: there is
            // nothing to interpolate, so replicating the channel is the whole
            // conversion. Rawler's path does the same thing one stage later.
            _ => {
                let mut buffer: Vec<[f32; 3]> =
                    samples.into_iter().map(|value| [value; 3]).collect();
                if let Some(correction) = lens.as_deref_mut() {
                    correction.apply_three(
                        &mut buffer,
                        full.d.w,
                        full.d.h,
                        crate::lens::ImageGeometry {
                            full_width: width,
                            full_height: height,
                            origin_x: 0,
                            origin_y: 0,
                        },
                    );
                }
                let dim = apply_default_crop(raw, &mut buffer, full.d, full)?;
                CameraImage::Three(Image::new(dim.w, dim.h, buffer)?)
            }
        },
        // `LinearRaw` is already interleaved, so no demosaic runs and the buffer
        // covers the whole frame — which is why `full`, not the active area, is
        // the coordinate system the crop is expressed in. See
        // `rescale::default_crop` for the Rawler bug that distinction avoids.
        NormalizedSamples::Linear3(mut pixels) => {
            if let Some(correction) = lens.as_deref_mut() {
                correction.apply_three(
                    &mut pixels,
                    full.d.w,
                    full.d.h,
                    crate::lens::ImageGeometry {
                        full_width: width,
                        full_height: height,
                        origin_x: 0,
                        origin_y: 0,
                    },
                );
            }
            let dim = apply_default_crop(raw, &mut pixels, full.d, full)?;
            CameraImage::Three(Image::new(dim.w, dim.h, pixels)?)
        }
        NormalizedSamples::Linear4(mut pixels) => {
            if let Some(correction) = lens {
                correction.apply_four(
                    &mut pixels,
                    full.d.w,
                    full.d.h,
                    crate::lens::ImageGeometry {
                        full_width: width,
                        full_height: height,
                        origin_x: 0,
                        origin_y: 0,
                    },
                );
            }
            let dim = apply_default_crop(raw, &mut pixels, full.d, full)?;
            CameraImage::Four {
                width: dim.w,
                height: dim.h,
                pixels,
            }
        }
    };

    Ok((camera, normalized.report, hot_pixel_report, demosaic_report))
}

/// Demosaiced sensor data, before any colour conversion.
///
/// The four-channel arm exists because Rawler only collapses an RGBE sensor to
/// three channels *inside* `Calibrate`, which the owned path does not run. No
/// target source in `docs/STATUS.md` uses a four-colour CFA, but dropping the
/// case would have been a regression against the Rawler path, which handles it.
pub enum CameraImage {
    Three(Image<CameraRgb>),
    Four {
        width: usize,
        height: usize,
        pixels: Vec<[f32; 4]>,
    },
}

/// How to develop a file through the owned colour path.
///
/// A struct rather than a third positional argument: `develop(raw, space,
/// sub_black)` reads as two interchangeable enums at the call site, and the next
/// milestone of the colour path (`ForwardMatrix`, dual-illuminant interpolation)
/// adds more.
#[derive(Debug, Clone, Copy)]
pub struct DevelopOptions {
    pub working_space: WorkingSpace,
    pub sub_black: SubBlack,
    /// Hot/dead pixel suppression strength, 0 to 1. 0 skips the correction
    /// entirely, keeping the mosaic byte-identical.
    pub hot_pixels: f32,
    /// Clipped-highlight reconstruction strength, 0 to 1. 0 skips it entirely.
    pub highlight_reconstruction: f32,
    pub demosaic: DemosaicMethod,
    pub snr10_ev: Option<f32>,
    /// Compose the DNG matrix transform when the profile is complete, using
    /// either its ForwardMatrix or ColorMatrix/ReductionMatrix route.
    pub full_dng_color: bool,
    /// Apply standardized DNG `OpcodeList3` lens corrections when present.
    pub lens_correction: bool,
}

/// Develop to scene-linear working-space RGB through the owned colour path.
pub fn develop(
    raw: &RawImage,
    path: &std::path::Path,
    options: DevelopOptions,
) -> Result<(Image<SceneLinear>, ColorReport)> {
    // The DNG matrix transform is tried first when asked for. Incomplete or
    // unsupported profiles fall back to the decoder camera matrix.
    let mut dng_color = None;
    let transform = if options.full_dng_color {
        match ColorTransform::derive_dng(raw, path, options.working_space) {
            Some((transform, report)) => {
                dng_color = Some(report);
                transform
            }
            None => ColorTransform::derive(raw, options.working_space)?,
        }
    } else {
        ColorTransform::derive(raw, options.working_space)?
    };
    let mut lens_correction = options
        .lens_correction
        .then(|| crate::lens::LensCorrection::read(path))
        .flatten();
    let (mut camera, rescale, hot_pixels, demosaic) = demosaic_camera_rgb_with_lens(
        raw,
        options.sub_black,
        options.hot_pixels,
        options.demosaic,
        options.snr10_ev,
        lens_correction.as_mut(),
    )?;

    // Clipped-highlight reconstruction runs on camera RGB, after the demosaic
    // and before white balance and the colour matrix: "was this channel at the
    // sensor's clip point?" is a question about the raw sample. Three-channel
    // only — the RGBE arm has no target source and no corpus behind it.
    let mut highlight_reconstruction = None;
    if options.highlight_reconstruction > 0.0
        && let CameraImage::Three(image) = &mut camera
    {
        let wb = [
            transform.white_balance[0],
            transform.white_balance[1],
            transform.white_balance[2],
        ];
        highlight_reconstruction = Some(crate::highlight::reconstruct(
            image,
            wb,
            options.highlight_reconstruction,
        ));
    }

    let image: Image<SceneLinear> = match camera {
        // In place: the camera-RGB buffer becomes the scene-linear one rather
        // than being collected into a second allocation of the same size. See
        // `Image::map_into`.
        CameraImage::Three(image) => {
            if transform.channels != 3 {
                bail!(
                    "the sensor demosaiced to three colour channels but its DNG calibration \
                     matrix describes {}; re-run with --raw-color-path rawler",
                    transform.channels
                );
            }
            image.map_into(|pixel| transform.convert3(pixel))
        }
        CameraImage::Four {
            width,
            height,
            pixels,
        } => {
            if transform.channels < 4 {
                bail!(
                    "the sensor demosaiced to four colour channels but the {:?} calibration \
                     matrix only describes {}; re-run with --raw-color-path rawler",
                    transform.illuminant,
                    transform.channels
                );
            }
            // Four channels in, three out, so this arm cannot reuse its input.
            let converted = pixels
                .par_iter()
                .map(|pixel| transform.convert4(*pixel))
                .collect();
            Image::<SceneLinear>::new(width, height, converted)?
        }
    };

    let clip_cost = measure_clip_cost(&image.pixels);

    let mut illuminants_available: Vec<String> = raw
        .color_matrix
        .keys()
        .map(|illuminant| format!("{illuminant:?}"))
        .collect();
    illuminants_available.sort();

    let report = ColorReport {
        path: RawColorPath::Owned,
        // From the transform, not the argument, so the report can only ever
        // describe the space the pixels were actually converted into.
        working_space: transform.working_space,
        illuminant: format!("{:?}", transform.illuminant),
        illuminants_available,
        white_balance: transform.white_balance,
        cam_to_working: transform
            .cam_to_working
            .iter()
            .flat_map(|row| row[..transform.channels].iter().copied())
            .collect(),
        camera_channels: transform.channels,
        demosaic,
        clip_cost: Some(clip_cost),
        rescale: Some(rescale),
        hot_pixels,
        highlight_reconstruction,
        dng_color,
        lens_correction: lens_correction.map(crate::lens::LensCorrection::into_report),
    };

    Ok((image, report))
}

/// Measure what Rawler's clip would have cost this frame.
///
/// Summed with a deterministic reduction — `par_iter().map().reduce()` over
/// disjoint chunks then a fixed-order fold would still float-drift with the
/// thread count, so the accumulation is sequential over a parallel-mapped
/// per-chunk result. In practice the chunking is fixed by `chunks`, not by the
/// scheduler, so the result does not depend on `--jobs`.
fn measure_clip_cost(pixels: &[[f32; 3]]) -> ClipCost {
    const CHUNK: usize = 65_536;

    #[derive(Default, Clone, Copy)]
    struct Tally {
        negative: u64,
        negative_luminance: u64,
        above_one: u64,
        altered: u64,
        sum_abs: f64,
        max_abs: f32,
    }

    let tallies: Vec<Tally> = pixels
        .par_chunks(CHUNK)
        .map(|chunk| {
            let mut tally = Tally::default();
            for pixel in chunk {
                let minimum = pixel.iter().copied().fold(f32::INFINITY, f32::min);
                let maximum = pixel.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                if minimum < 0.0 {
                    tally.negative += 1;
                    // Only worth evaluating for a pixel that has a negative
                    // channel at all; luminance is a positively weighted sum, so
                    // an all-non-negative pixel cannot have negative luminance.
                    if crate::analyze::luminance(*pixel) <= 0.0 {
                        tally.negative_luminance += 1;
                    }
                }
                if maximum > 1.0 {
                    tally.above_one += 1;
                }
                let clipped = rawler_clip(*pixel);
                let mut moved = false;
                for channel in 0..3 {
                    let delta = (pixel[channel] - clipped[channel]).abs();
                    if delta > 0.0 {
                        moved = true;
                    }
                    tally.sum_abs += delta as f64;
                    if delta > tally.max_abs {
                        tally.max_abs = delta;
                    }
                }
                if moved {
                    tally.altered += 1;
                }
            }
            tally
        })
        .collect();

    let mut total = Tally::default();
    for tally in tallies {
        total.negative += tally.negative;
        total.negative_luminance += tally.negative_luminance;
        total.above_one += tally.above_one;
        total.altered += tally.altered;
        total.sum_abs += tally.sum_abs;
        total.max_abs = total.max_abs.max(tally.max_abs);
    }

    let count = pixels.len().max(1) as f64;
    ClipCost {
        negative_fraction: (total.negative as f64 / count) as f32,
        negative_luminance_fraction: (total.negative_luminance as f64 / count) as f32,
        above_one_fraction: (total.above_one as f64 / count) as f32,
        altered_fraction: (total.altered as f64 / count) as f32,
        mean_abs_delta: (total.sum_abs / (count * 3.0)) as f32,
        max_abs_delta: total.max_abs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sony ILCE-7C, D65, from Rawler's own camera definition. Used as a
    /// realistic matrix rather than a synthetic one.
    const A7C_D65: [f32; 9] = [
        0.7374, -0.2389, -0.0551, -0.5435, 1.3162, 0.2519, -0.1006, 0.1795, 0.6552,
    ];

    fn padded(flat: &[f32; 9]) -> [[f32; 3]; 4] {
        let mut matrix = [[0.0_f32; 3]; 4];
        for channel in 0..3 {
            for axis in 0..3 {
                matrix[channel][axis] = flat[channel * 3 + axis];
            }
        }
        matrix
    }

    /// The load-bearing claim of milestone 1: the owned path's matrix is the
    /// *same* matrix Rawler computes. If this drifts, an A/B diff stops
    /// isolating the clipping and starts measuring two changes at once.
    ///
    /// Checked against Rawler's own public helpers, so it is a comparison
    /// against the shipped implementation rather than against a transcription
    /// of it.
    #[test]
    fn cam_to_working_matches_rawlers_composition_at_srgb() {
        use rawler::imgop::matrix::{multiply, normalize, pseudo_inverse as rawler_pseudo_inverse};
        use rawler::imgop::xyz::SRGB_TO_XYZ_D65;

        let xyz_to_cam = padded(&A7C_D65);

        let theirs = rawler_pseudo_inverse(normalize(multiply(&xyz_to_cam, &SRGB_TO_XYZ_D65)));
        let ours = pseudo_inverse(normalize_rows(multiply_rows(
            &xyz_to_cam,
            &WorkingSpace::Srgb.to_xyz_d65(),
        )))
        .expect("the A7C matrix is invertible");

        for row in 0..3 {
            for column in 0..3 {
                assert!(
                    (ours[row][column] - theirs[row][column]).abs() < 2.0e-5,
                    "cam_to_working[{row}][{column}]: ours {} vs rawler {}",
                    ours[row][column],
                    theirs[row][column]
                );
            }
        }
    }

    /// The white-balance guarantee the row-normalization exists to provide: a
    /// patch the as-shot coefficients made neutral must stay neutral, in every
    /// working space. This is what makes the composition correct rather than
    /// merely invertible.
    #[test]
    fn camera_neutral_maps_to_working_neutral() {
        for space in [WorkingSpace::Srgb, WorkingSpace::Rec2020] {
            let cam_to_working = pseudo_inverse(normalize_rows(multiply_rows(
                &padded(&A7C_D65),
                &space.to_xyz_d65(),
            )))
            .expect("invertible");
            for (channel, row) in cam_to_working.iter().enumerate() {
                let sum: f32 = row[..3].iter().sum();
                assert!(
                    (sum - 1.0).abs() < 1.0e-4,
                    "{space:?} row {channel} sums to {sum}, so camera neutral would tint"
                );
            }
        }
    }

    #[test]
    fn invert3_round_trips_and_rejects_singular_matrices() {
        let m = [[2.0, 0.0, 1.0], [1.0, 3.0, 2.0], [0.5, -1.0, 4.0]];
        let inverse = invert3(m).expect("non-singular");
        let product = multiply3(&m, &inverse);
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((product[i][j] - expected).abs() < 1.0e-5, "{product:?}");
            }
        }
        assert!(invert3([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [1.0, 1.0, 1.0]]).is_none());
    }

    /// `WorkingSpace::Srgb` must report `None`, not an identity matrix: the
    /// byte-identity guarantee for the default configuration rests on
    /// `tone::render` being able to skip the conversion entirely.
    #[test]
    fn srgb_needs_no_display_conversion_and_rec2020_does() {
        assert!(WorkingSpace::Srgb.to_display().is_none());
        let rec2020 = WorkingSpace::Rec2020
            .to_display()
            .expect("a real conversion");
        // BT.2020 is wider, so pulling it into sRGB must expand the primaries:
        // the leading diagonal exceeds 1 and the off-diagonals go negative.
        assert!(rec2020[0][0] > 1.4, "{rec2020:?}");
        assert!(rec2020[0][1] < 0.0 && rec2020[0][2] < 0.0, "{rec2020:?}");
        // White is white in both spaces, so every row still sums to 1 — but only
        // to about 2e-4, and not because of accumulated float error. The two
        // published matrices disagree slightly about where D65 is: summing each
        // one's columns gives Z = 1.0890578 for BT.2020 against 1.0888300 for
        // sRGB. A tolerance tight enough to call that a bug would be asserting
        // that the standards agree to more digits than they do.
        for row in rec2020 {
            let sum: f32 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1.0e-3, "row sums to {sum}");
        }
    }

    /// The reference-free proof that the owned path is more accurate, not merely
    /// different.
    ///
    /// Every other comparison in this project is against a camera JPEG, which is a
    /// *reference, not ground truth* — a vendor's opinion about colour. This test
    /// needs no reference at all. It runs the round trip
    ///
    /// ```text
    /// target linear sRGB -> XYZ -> camera RGB -> back to linear sRGB
    /// ```
    ///
    /// through a real camera matrix, so the analytically correct answer is the
    /// value it started from. Two things then follow:
    ///
    /// 1. the *colorimetric* composition recovers the target exactly, which
    ///    validates this module's matrix arithmetic end to end rather than one
    ///    function at a time;
    /// 2. inserting Rawler's `clip_euclidean_norm_avg` into that round trip moves
    ///    the answer, and the size of the move is what the clipping costs — in
    ///    degrees of hue, on colours whose correct hue is known.
    ///
    /// The colours are chosen to sit inside a camera's gamut but outside sRGB's,
    /// which is the population that matters: it is exactly where a negative channel
    /// appears and exactly where the clip acts.
    #[test]
    fn the_clip_shifts_hue_on_colours_whose_true_hue_is_known() {
        use crate::oklab;

        let space = WorkingSpace::Srgb;
        let srgb_to_xyz = space.to_xyz_d65();
        let xyz_to_cam = padded(&A7C_D65);

        // The colorimetric composition: no row normalization, so this is the
        // honest inverse rather than the dcraw-lineage "camera neutral maps to
        // display neutral" shortcut the shipped path uses.
        let cam_to_srgb = pseudo_inverse(multiply_rows(&xyz_to_cam, &srgb_to_xyz))
            .expect("the A7C matrix composes invertibly");

        let to_camera = |rgb: [f32; 3]| {
            let xyz = [
                srgb_to_xyz[0][0] * rgb[0]
                    + srgb_to_xyz[0][1] * rgb[1]
                    + srgb_to_xyz[0][2] * rgb[2],
                srgb_to_xyz[1][0] * rgb[0]
                    + srgb_to_xyz[1][1] * rgb[1]
                    + srgb_to_xyz[1][2] * rgb[2],
                srgb_to_xyz[2][0] * rgb[0]
                    + srgb_to_xyz[2][1] * rgb[1]
                    + srgb_to_xyz[2][2] * rgb[2],
            ];
            let mut camera = [0.0_f32; 3];
            for channel in 0..3 {
                camera[channel] = (0..3)
                    .map(|axis| xyz_to_cam[channel][axis] * xyz[axis])
                    .sum();
            }
            camera
        };
        let from_camera = |camera: [f32; 3]| {
            let mut rgb = [0.0_f32; 3];
            for channel in 0..3 {
                rgb[channel] = (0..3).map(|k| cam_to_srgb[channel][k] * camera[k]).sum();
            }
            rgb
        };

        // In-gamut controls first, then colours outside sRGB: a saturated green
        // and a saturated cyan of the sort a foliage or sky pixel produces.
        let in_gamut = [[0.20_f32, 0.35, 0.50], [0.60, 0.30, 0.10]];
        let out_of_gamut = [
            [0.05_f32, 0.90, -0.15],
            [-0.08, 0.55, 0.70],
            [0.95, -0.05, 0.20],
        ];

        for target in in_gamut.iter().chain(out_of_gamut.iter()) {
            let recovered = from_camera(to_camera(*target));
            for channel in 0..3 {
                assert!(
                    (recovered[channel] - target[channel]).abs() < 2.0e-3,
                    "the colorimetric round trip lost {target:?}, recovering {recovered:?}"
                );
            }
        }

        // In gamut, the clip is inert, so it costs no hue at all.
        for target in in_gamut {
            let owned = from_camera(to_camera(target));
            let clipped = rawler_clip(owned);
            let shift = oklab::hue_difference(
                oklab::from_linear_srgb(owned),
                oklab::from_linear_srgb(clipped),
            )
            .expect("a chromatic control colour");
            assert!(
                shift.to_degrees() < 0.01,
                "the clip moved an in-gamut colour {target:?} by {} degrees",
                shift.to_degrees()
            );
        }

        // Out of gamut, it moves the hue away from the known-correct answer, and
        // the owned path is the one that keeps it.
        for target in out_of_gamut {
            let truth = oklab::from_linear_srgb(target);
            let owned = from_camera(to_camera(target));
            let clipped = rawler_clip(owned);

            let owned_error = oklab::hue_difference(oklab::from_linear_srgb(owned), truth)
                .expect("a chromatic colour")
                .to_degrees();
            let clipped_error = oklab::hue_difference(oklab::from_linear_srgb(clipped), truth)
                .expect("a chromatic colour")
                .to_degrees();

            assert!(
                owned_error < 0.5,
                "the owned path should reproduce {target:?} almost exactly, off by {owned_error} degrees"
            );
            assert!(
                clipped_error > 2.0,
                "the clip should move {target:?} measurably, but it moved only {clipped_error} degrees"
            );
            assert!(
                clipped_error > owned_error * 10.0,
                "the clip's hue error ({clipped_error} degrees) should dwarf the owned path's \
                 ({owned_error} degrees) on {target:?}"
            );
        }
    }

    /// The measurement that justifies the whole module: the clip has to be a
    /// no-op on in-range pixels, and a large move on the ones that matter.
    ///
    /// It also pins what the operator is *not*. The first version of this test
    /// asserted the output was bounded by 1.0, and it failed — which was the
    /// useful result. `clip_euclidean_norm_avg` leaves a blown highlight above
    /// the white point and takes its colour instead. That is a worse failure than
    /// clipping would be, and it is the reason the owned path exists.
    #[test]
    fn rawler_clip_is_inert_in_range_and_destructive_outside_it() {
        let in_range = [0.2, 0.5, 0.9];
        assert_eq!(rawler_clip(in_range), in_range);

        // A white-balanced clipped highlight on a Sony A7C: green at the white
        // point, red and blue lifted past it by the as-shot coefficients.
        let highlight = [2.3, 1.0, 1.6];
        let clipped = rawler_clip(highlight);

        let spread = |pixel: [f32; 3]| {
            pixel.iter().copied().fold(f32::NEG_INFINITY, f32::max)
                - pixel.iter().copied().fold(f32::INFINITY, f32::min)
        };
        assert!(
            spread(clipped) < spread(highlight) / 4.0,
            "the highlight's colour should be mostly gone: spread {} -> {} ({clipped:?})",
            spread(highlight),
            spread(clipped)
        );
        // Not a clip: the magnitude survives, only the colour is spent.
        assert!(
            clipped.iter().any(|value| *value > 1.0),
            "expected the operator to leave the pixel above the white point, got {clipped:?}"
        );

        // An out-of-gamut colour: the negative channel is destroyed outright.
        assert_eq!(rawler_clip([0.4, 0.2, -0.15])[2], 0.0);
    }

    #[test]
    fn clip_cost_is_zero_on_an_in_range_frame() {
        let cost = measure_clip_cost(&[[0.1, 0.2, 0.3], [0.9, 0.5, 0.0]]);
        assert_eq!(cost.negative_fraction, 0.0);
        assert_eq!(cost.above_one_fraction, 0.0);
        assert_eq!(cost.altered_fraction, 0.0);
        assert_eq!(cost.mean_abs_delta, 0.0);
        assert_eq!(cost.max_abs_delta, 0.0);
    }

    /// A negative channel is not by itself the problem; a negative *luminance*
    /// is. The two populations differ by a lot, and conflating them is what led
    /// the first writeup of this module to describe the wrong mechanism.
    #[test]
    fn negative_luminance_is_a_much_smaller_population_than_negative_channels() {
        // Out of sRGB gamut but comfortably bright: green dominates luminance, so
        // the pixel renders as a saturated colour with its blue landed on 0.
        let bright_out_of_gamut = [0.05, 0.90, -0.10];
        // A near-black pixel whose red went negative far enough to outweigh the
        // rest. This is the cohort that collapses to pure black.
        let dark_negative_luminance = [-0.02, 0.004, 0.001];

        assert!(crate::analyze::luminance(bright_out_of_gamut) > 0.0);
        assert!(crate::analyze::luminance(dark_negative_luminance) <= 0.0);

        let cost = measure_clip_cost(&[
            bright_out_of_gamut,
            dark_negative_luminance,
            [0.2, 0.3, 0.4],
        ]);
        assert!(
            (cost.negative_fraction - 2.0 / 3.0).abs() < 1.0e-6,
            "{cost:?}"
        );
        assert!(
            (cost.negative_luminance_fraction - 1.0 / 3.0).abs() < 1.0e-6,
            "{cost:?}"
        );
    }

    /// The `crushed_fraction` regression, and the fix for it, pinned end to end
    /// through the real render.
    ///
    /// History, because it is the useful part. `tone::render_pixel_local` used to
    /// clamp its chroma anchor with `luminance(rgb).clamp(0.0, 1.0)`. `luminance`
    /// is a *signed* sum, so a negative-luminance pixel anchored at exactly 0, and
    /// `compress_gamut`'s scale became `anchor / (anchor - min)` = `0 / |min|` = 0
    /// — multiplying *every* channel by zero, including the positive ones. Rawler's
    /// per-channel clip keeps those, so on this cohort the owned path was strictly
    /// **more** destructive than the clip it replaced. That was the whole of the
    /// regression the 0.1.17 A/B measured, and it is the opposite of what removing
    /// a clip is supposed to do.
    ///
    /// 0.1.18 floors that anchor at `black_output_linear`, matching what
    /// `mapped_norm` was already clamped to. This test now asserts the fixed
    /// behaviour: the out-of-gamut channel still lands on zero, because it is
    /// genuinely outside the gamut, but the pixel keeps the colour it had.
    #[test]
    fn a_negative_luminance_pixel_keeps_its_positive_channels() {
        use crate::types::{Image, SceneLinear};

        let params = crate::analyze::analyze(
            &Image::<SceneLinear>::new(8, 8, vec![[0.18_f32; 3]; 64]).unwrap(),
            &crate::analyze::AnalysisInputs::new(1_000, crate::types::Preset::Auto, 0.0),
        )
        .expect("a flat mid-grey frame analyses")
        .1;

        let negative_luminance = [-0.02_f32, 0.004, 0.001];
        assert!(crate::analyze::luminance(negative_luminance) <= 0.0);

        let owned = crate::tone::render(
            &Image::<SceneLinear>::new(1, 1, vec![negative_luminance]).unwrap(),
            &params,
            None,
            None,
        )
        .into_raw();

        assert_ne!(
            owned,
            vec![0, 0, 0],
            "the anchor floor should stop this pixel collapsing to pure black; if it \
             is black again, the `black_output_linear` floor in \
             tone::render_pixel_local was lost and the 0.1.17 crushed regression is \
             back"
        );
        assert_eq!(
            owned[0], 0,
            "the genuinely out-of-gamut channel should still land on zero, since it \
             is outside the gamut; only the positive channels are being rescued"
        );
        assert!(
            owned[1] > 0 && owned[2] > 0,
            "the positive channels carry the pixel's remaining colour and must \
             survive, got {owned:?}"
        );

        // Rawler's own answer for the same pixel: the negative channel zeroed, the
        // rest intact. It must also not be black — that is what made the owned
        // path's old collapse a real regression rather than a scoring artefact.
        let clipped = crate::tone::render(
            &Image::<SceneLinear>::new(1, 1, vec![rawler_clip(negative_luminance)]).unwrap(),
            &params,
            None,
            None,
        );
        assert_ne!(clipped.into_raw(), vec![0, 0, 0]);
    }

    #[test]
    fn clip_cost_counts_both_causes_separately() {
        let cost = measure_clip_cost(&[
            [0.1, 0.2, 0.3],   // untouched
            [2.3, 1.0, 1.6],   // above one
            [0.4, 0.2, -0.15], // negative
            [3.0, -0.2, 0.5],  // both
        ]);
        assert!((cost.negative_fraction - 0.5).abs() < 1.0e-6);
        assert!((cost.above_one_fraction - 0.5).abs() < 1.0e-6);
        assert!((cost.altered_fraction - 0.75).abs() < 1.0e-6);
        assert!(cost.max_abs_delta > 1.0, "{cost:?}");
    }

    /// The measurement must not depend on how the frame happens to be chunked
    /// across threads, or a survey number would move with `--jobs`.
    #[test]
    fn clip_cost_is_independent_of_frame_length() {
        let pattern = [[2.3, 1.0, 1.6], [0.1, 0.2, 0.3], [0.4, 0.2, -0.15]];
        let short: Vec<[f32; 3]> = pattern.iter().copied().cycle().take(3).collect();
        let long: Vec<[f32; 3]> = pattern.iter().copied().cycle().take(300_000).collect();
        let a = measure_clip_cost(&short);
        let b = measure_clip_cost(&long);
        assert!((a.altered_fraction - b.altered_fraction).abs() < 1.0e-6);
        assert!((a.mean_abs_delta - b.mean_abs_delta).abs() < 1.0e-4);
        assert_eq!(a.max_abs_delta, b.max_abs_delta);
    }
}
