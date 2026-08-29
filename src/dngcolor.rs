//! Full DNG colour science: ForwardMatrix, CameraCalibration, AnalogBalance and
//! dual-illuminant interpolation, composed per the Adobe DNG 1.7 specification.
//!
//! # What the owned path does without this, and why it is not enough
//!
//! [`crate::color`]'s milestone-1 transform is Rawler's own shortcut: pick one
//! `ColorMatrix` (XYZ→camera), row-normalize `working·xyz_to_cam` so camera
//! neutral lands on working neutral, and invert. That is a faithful reproduction
//! of Rawler *minus the clipping*, which is exactly what milestone 1 wanted — one
//! variable at a time. But it discards four things a DNG actually carries:
//!
//! - **`ForwardMatrix`** maps the reference-camera neutral straight to the D50
//!   PCS white, so it encodes the manufacturer's rendering intent far better than
//!   a row-normalized `ColorMatrix` inverse. Modern phone DNGs (Samsung Expert
//!   RAW, most Camera2 apps) ship it.
//! - **`CameraCalibration`** and **`AnalogBalance`** correct the individual
//!   sensor unit against the reference camera the matrices were fitted to.
//! - **Dual-illuminant interpolation.** A DNG carries two matrices, at two
//!   calibration illuminants, and the colour depends on blending them by the
//!   scene's correlated colour temperature. The shortcut uses one and ignores
//!   the other — `docs/STATUS.md` flags this as the reason a file with two
//!   matrices and no D65 one is not even developed deterministically on Rawler's
//!   own path.
//!
//! # The composition (Adobe DNG 1.7.1, "Mapping Camera Color Space to CIE XYZ")
//!
//! With `CM` = ColorMatrix, `CC` = CameraCalibration, `AB` = AnalogBalance
//! (diagonal), `FM` = ForwardMatrix, and `n` = AsShotNeutral (the camera-space
//! neutral under the scene light):
//!
//! ```text
//! reference_neutral = Inverse(AB · CC) · n
//! D                 = Diagonal(1 / reference_neutral)          (so D·ref = 1)
//! camera → XYZ(D50) = FM · D · Inverse(AB · CC)
//! ```
//!
//! By construction `FM`'s columns sum to the D50 white, so this maps `n` exactly
//! onto the D50 PCS white — i.e. the white balance is *baked into the matrix* and
//! must **not** be applied again separately (unlike the milestone-1 path, which
//! multiplies by `wb_coeffs`). From D50 the pixel is Bradford-adapted to D65 and
//! taken into the working space:
//!
//! ```text
//! camera → working = Inverse(working→XYZ_D65) · Bradford(D50→D65) · camera→XYZ(D50)
//! ```
//!
//! Two calibration illuminants are blended by reciprocal colour temperature
//! (DNG's rule), and the scene temperature itself comes from the neutral: the
//! white point depends on `AB · CC · CM`, which depends on the temperature, so
//! the loop is iterated to a fixed point. `AsShotWhiteXY` is accepted too; in
//! that case the white point is already known and no inverse solve is needed.
//!
//! # Status and scope
//!
//! **Off by default** (`--dng-color`), owned path only, and it activates only on
//! files that actually carry a `ForwardMatrix`; anything else falls back to the
//! milestone-1 transform unchanged, so the default output does not move. When a
//! file has a single calibration illuminant the interpolation collapses to that
//! matrix and the result is the honest full-calibration transform for that one
//! light.
//!
//! zenraw's `src/dng_render.rs` (surveyed 2026-07-31) was read for sequencing —
//! its Bradford adaptation and reciprocal-temperature interpolation are the same
//! shape as here — but it has no `ForwardMatrix`-in-path and no
//! `CameraCalibration`, so the composition above is implemented from the DNG
//! specification rather than taken from it.

use crate::color::WorkingSpace;
use rawler::RawImage;
use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{Entry, GenericTiffReader, Value};
use rawler::imgop::xyz::Illuminant;
use serde::Serialize;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Row-major 3x3 in `f64`; the colour composition is done in double precision
/// and only the final camera→working matrix is narrowed to `f32`.
type Mat3 = [[f64; 3]; 3];
type Vec3 = [f64; 3];
type Mat43 = [[f64; 3]; 4];
type Mat34 = [[f64; 4]; 3];
type Mat44 = [[f64; 4]; 4];
type Vec4 = [f64; 4];

// --- DNG tag ids used here (Adobe DNG 1.7.1, chapter 4). ---
const TAG_COLOR_MATRIX_1: u16 = 50721;
const TAG_COLOR_MATRIX_2: u16 = 50722;
const TAG_CAMERA_CALIBRATION_1: u16 = 50723;
const TAG_CAMERA_CALIBRATION_2: u16 = 50724;
const TAG_REDUCTION_MATRIX_1: u16 = 50725;
const TAG_REDUCTION_MATRIX_2: u16 = 50726;
const TAG_ANALOG_BALANCE: u16 = 50727;
const TAG_AS_SHOT_NEUTRAL: u16 = 50728;
const TAG_AS_SHOT_WHITE_XY: u16 = 50729;
const TAG_CALIBRATION_ILLUMINANT_1: u16 = 50778;
const TAG_CALIBRATION_ILLUMINANT_2: u16 = 50779;
const TAG_CAMERA_CALIBRATION_SIGNATURE: u16 = 50931;
const TAG_PROFILE_CALIBRATION_SIGNATURE: u16 = 50932;
const TAG_FORWARD_MATRIX_1: u16 = 50964;
const TAG_FORWARD_MATRIX_2: u16 = 50965;
const TAG_CALIBRATION_ILLUMINANT_3: u16 = 52529;
const TAG_CAMERA_CALIBRATION_3: u16 = 52530;
const TAG_COLOR_MATRIX_3: u16 = 52531;
const TAG_FORWARD_MATRIX_3: u16 = 52532;
const TAG_ILLUMINANT_DATA_1: u16 = 52533;
const TAG_ILLUMINANT_DATA_2: u16 = 52534;
const TAG_ILLUMINANT_DATA_3: u16 = 52535;
const TAG_REDUCTION_MATRIX_3: u16 = 52538;

/// The Bradford cone-response matrix, the chromatic adaptation transform the ICC
/// and DNG worlds both use in practice.
const BRADFORD: Mat3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// D50 and D65 white points as CIE xy (the DNG PCS white is D50).
const D50_XY: (f64, f64) = (0.34567, 0.35850);
const D65_XY: (f64, f64) = (0.31270, 0.32900);

fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn mat_vec(m: &Mat3, v: &Vec3) -> Vec3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn mat_invert(m: &Mat3) -> Option<Mat3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    let mut out = [[0.0; 3]; 3];
    out[0][0] = (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det;
    out[0][1] = (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det;
    out[0][2] = (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det;
    out[1][0] = (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det;
    out[1][1] = (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det;
    out[1][2] = (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det;
    out[2][0] = (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det;
    out[2][1] = (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det;
    out[2][2] = (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det;
    Some(out)
}

fn diag(v: &Vec3) -> Mat3 {
    [[v[0], 0.0, 0.0], [0.0, v[1], 0.0], [0.0, 0.0, v[2]]]
}

const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

fn lerp_mat(a: &Mat3, b: &Mat3, g: f64) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = g * a[i][j] + (1.0 - g) * b[i][j];
        }
    }
    out
}

fn blend_mat(a: &Mat3, b: &Mat3, c: &Mat3, weights: [f64; 3]) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = weights[0] * a[i][j] + weights[1] * b[i][j] + weights[2] * c[i][j];
        }
    }
    out
}

fn xy_to_xyz(x: f64, y: f64) -> Vec3 {
    [x / y, 1.0, (1.0 - x - y) / y]
}

fn xyz_to_xy(xyz: &Vec3) -> (f64, f64) {
    let sum = xyz[0] + xyz[1] + xyz[2];
    (xyz[0] / sum, xyz[1] / sum)
}

fn valid_xy(xy: (f64, f64)) -> bool {
    xy.0.is_finite() && xy.1.is_finite() && xy.0 > 0.0 && xy.1 > 0.0 && xy.0 + xy.1 < 1.0
}

/// Bradford chromatic adaptation from one white point to another.
fn bradford(from_xy: (f64, f64), to_xy: (f64, f64)) -> Mat3 {
    let s = mat_vec(&BRADFORD, &xy_to_xyz(from_xy.0, from_xy.1));
    let d = mat_vec(&BRADFORD, &xy_to_xyz(to_xy.0, to_xy.1));
    let ratio = diag(&[d[0] / s[0], d[1] / s[1], d[2] / s[2]]);
    let brad_inv = mat_invert(&BRADFORD).expect("Bradford is invertible");
    mat_mul(&brad_inv, &mat_mul(&ratio, &BRADFORD))
}

/// Robertson isotemperature lines in CIE 1960 UCS `(u, v)`: reciprocal
/// megakelvin, black-body-locus u/v, and isotemperature-line slope.
///
/// The table is the standard Wyszecki & Stiles data used for Robertson CCT,
/// spanning 1,667 K to infinity. Unlike a cubic approximation in xy, this
/// remains well behaved when the selected white has tint away from the locus.
const TEMPERATURE_LINES: [[f64; 4]; 31] = [
    [0.0, 0.18006, 0.26352, -0.24341],
    [10.0, 0.18066, 0.26589, -0.25479],
    [20.0, 0.18133, 0.26846, -0.26876],
    [30.0, 0.18208, 0.27119, -0.28539],
    [40.0, 0.18293, 0.27407, -0.30470],
    [50.0, 0.18388, 0.27709, -0.32675],
    [60.0, 0.18494, 0.28021, -0.35156],
    [70.0, 0.18611, 0.28342, -0.37915],
    [80.0, 0.18740, 0.28668, -0.40955],
    [90.0, 0.18880, 0.28997, -0.44278],
    [100.0, 0.19032, 0.29326, -0.47888],
    [125.0, 0.19462, 0.30141, -0.58204],
    [150.0, 0.19962, 0.30921, -0.70471],
    [175.0, 0.20525, 0.31647, -0.84901],
    [200.0, 0.21142, 0.32312, -1.0182],
    [225.0, 0.21807, 0.32909, -1.2168],
    [250.0, 0.22511, 0.33439, -1.4512],
    [275.0, 0.23247, 0.33904, -1.7298],
    [300.0, 0.24010, 0.34308, -2.0637],
    [325.0, 0.24702, 0.34655, -2.4681],
    [350.0, 0.25591, 0.34951, -2.9641],
    [375.0, 0.26400, 0.35200, -3.5814],
    [400.0, 0.27218, 0.35407, -4.3633],
    [425.0, 0.28039, 0.35577, -5.3762],
    [450.0, 0.28863, 0.35714, -6.7262],
    [475.0, 0.29685, 0.35823, -8.5955],
    [500.0, 0.30505, 0.35907, -11.324],
    [525.0, 0.31320, 0.35968, -15.628],
    [550.0, 0.32129, 0.36011, -23.325],
    [575.0, 0.32931, 0.36038, -40.770],
    [600.0, 0.33724, 0.36051, -116.45],
];

/// CIE xy chromaticity of an XYZ triplet, or `None` when it falls outside the
/// diagram.
///
/// Crate-visible so [`crate::illuminant`] can place an estimated illuminant on
/// the same diagram this module's own white points live on, rather than growing
/// a second copy of the conversion and its validity rule.
pub(crate) fn chromaticity(xyz: &[f64; 3]) -> Option<(f64, f64)> {
    let xy = xyz_to_xy(xyz);
    valid_xy(xy).then_some(xy)
}

/// Correlated colour temperature from CIE xy via Robertson's method.
fn correlated_temperature(x: f64, y: f64) -> Option<f64> {
    temperature_and_duv(x, y).map(|(temperature, _)| temperature)
}

/// Robertson's method, also returning the signed distance from the Planckian
/// locus in CIE 1960 `uv` — the usual `Duv` tint measure, positive above the
/// locus (green) and negative below it (magenta).
///
/// The locus point is interpolated linearly between the two bracketing
/// isotemperature lines, using the same fraction the temperature interpolation
/// uses. That is an approximation of a curve by a chord over at most 25
/// reciprocal-megakelvin, which is well inside the precision this is reported
/// at; it is not accurate enough to drive a render, and nothing does.
pub(crate) fn temperature_and_duv(x: f64, y: f64) -> Option<(f64, f64)> {
    let denominator = 1.5 - x + 6.0 * y;
    if !denominator.is_finite() || denominator.abs() < 1e-12 {
        return None;
    }
    let u = 2.0 * x / denominator;
    let v = 3.0 * y / denominator;
    let mut previous_distance = 0.0;

    for index in 1..TEMPERATURE_LINES.len() {
        let [reciprocal, line_u, line_v, slope] = TEMPERATURE_LINES[index];
        let length = (1.0 + slope * slope).sqrt();
        let direction_u = 1.0 / length;
        let direction_v = slope / length;
        let distance = -(u - line_u) * direction_v + (v - line_v) * direction_u;

        if distance <= 0.0 || index == TEMPERATURE_LINES.len() - 1 {
            let distance = (-distance.min(0.0)).max(0.0);
            let fraction = if index == 1 {
                0.0
            } else {
                distance / (previous_distance + distance)
            };
            let previous = TEMPERATURE_LINES[index - 1];
            let interpolated = previous[0] * fraction + reciprocal * (1.0 - fraction);
            if !interpolated.is_finite() || interpolated <= 0.0 {
                return None;
            }
            let locus_u = previous[1] * fraction + line_u * (1.0 - fraction);
            let locus_v = previous[2] * fraction + line_v * (1.0 - fraction);
            let offset_u = u - locus_u;
            let offset_v = v - locus_v;
            let duv = offset_v.signum() * (offset_u * offset_u + offset_v * offset_v).sqrt();
            return Some((1.0e6 / interpolated, duv));
        }
        previous_distance = distance;
    }
    None
}

/// Nominal correlated colour temperature for a DNG calibration illuminant.
///
/// The DNG spec interpolates by reciprocal temperature, so all that is needed is
/// a representative temperature per `LightSource` code. Daylight-class sources
/// collapse to their standard values; the ones a phone or camera actually writes
/// (StandardA/Tungsten, D65) are the ones that matter.
fn illuminant_temp(illuminant: Illuminant) -> f64 {
    match illuminant {
        Illuminant::A | Illuminant::Tungsten => 2850.0,
        Illuminant::IsoStudioTungsten => 3200.0,
        Illuminant::D50 => 5000.0,
        Illuminant::D55
        | Illuminant::Daylight
        | Illuminant::FineWeather
        | Illuminant::Flash
        | Illuminant::B => 5500.0,
        Illuminant::D65 | Illuminant::CloudyWeather | Illuminant::C => 6500.0,
        Illuminant::D75 | Illuminant::Shade => 7500.0,
        Illuminant::DaylightFluorescent => 6400.0,
        Illuminant::DaylightWhiteFluorescent => 5050.0,
        Illuminant::Fluorescent | Illuminant::CoolWhiteFluorescent => 4150.0,
        Illuminant::WhiteFluorescent => 3525.0,
        Illuminant::Unknown => 0.0,
    }
}

/// Weight toward illuminant 1, interpolated by reciprocal temperature (mireds)
/// and clamped to `[0, 1]`, per the DNG specification.
fn dual_weight(temp: f64, temp1: f64, temp2: f64) -> f64 {
    if !temp.is_finite()
        || temp <= 0.0
        || !temp1.is_finite()
        || temp1 <= 0.0
        || !temp2.is_finite()
        || temp2 <= 0.0
        || (temp1 - temp2).abs() < 1e-6
    {
        return 1.0;
    }
    let inv = 1.0 / temp;
    let inv1 = 1.0 / temp1;
    let inv2 = 1.0 / temp2;
    let g = (inv - inv2) / (inv1 - inv2);
    g.clamp(0.0, 1.0)
}

fn cct_to_xy(temp: f64) -> Option<(f64, f64)> {
    if !temp.is_finite() || !(1667.0..=25_000.0).contains(&temp) {
        return None;
    }
    let x = if temp <= 4000.0 {
        -0.2661239e9 / temp.powi(3) - 0.2343580e6 / temp.powi(2) + 0.8776956e3 / temp + 0.179910
    } else {
        -3.0258469e9 / temp.powi(3) + 2.1070379e6 / temp.powi(2) + 0.2226347e3 / temp + 0.240390
    };
    let y = if temp <= 2222.0 {
        -1.1063814 * x.powi(3) - 1.34811020 * x.powi(2) + 2.18555832 * x - 0.20219683
    } else if temp <= 4000.0 {
        -0.9549476 * x.powi(3) - 1.37418593 * x.powi(2) + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x.powi(3) - 5.87338670 * x.powi(2) + 3.75112997 * x - 0.37001483
    };
    valid_xy((x, y)).then_some((x, y))
}

fn triple_weights(point: (f64, f64), whites: [(f64, f64); 3]) -> [f64; 3] {
    let [(x1, y1), (x2, y2), (x3, y3)] = whites;
    let denominator = (y2 - y3) * (x1 - x3) + (x3 - x2) * (y1 - y3);
    if !denominator.is_finite() || denominator.abs() < 1.0e-12 {
        return [1.0, 0.0, 0.0];
    }
    let mut weights = [
        ((y2 - y3) * (point.0 - x3) + (x3 - x2) * (point.1 - y3)) / denominator,
        ((y3 - y1) * (point.0 - x3) + (x1 - x3) * (point.1 - y3)) / denominator,
        0.0,
    ];
    weights[2] = 1.0 - weights[0] - weights[1];
    // Outside the calibration triangle, use the closest point on its convex
    // hull. Clamping and renormalizing is deterministic and agrees at every
    // vertex and edge.
    for weight in &mut weights {
        *weight = weight.clamp(0.0, 1.0);
    }
    let sum: f64 = weights.iter().sum();
    if sum <= 1.0e-12 {
        [1.0, 0.0, 0.0]
    } else {
        weights.map(|weight| weight / sum)
    }
}

/// Scale a camera neutral to the DNG SDK convention: its largest coordinate is
/// one. The neutral's scale is otherwise arbitrary, but leaving it arbitrary
/// would make the composed matrix change exposure.
fn normalize_neutral(mut neutral: Vec3) -> Option<Vec3> {
    if neutral.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return None;
    }
    let max = neutral.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !max.is_finite() || max <= 0.0 {
        return None;
    }
    for value in &mut neutral {
        *value /= max;
    }
    Some(neutral)
}

/// Normalize a ForwardMatrix so that it maps camera `[1, 1, 1]` to the D50 PCS
/// white exactly, as required by the DNG definition.
fn normalize_forward_matrix(mut matrix: Mat3) -> Option<Mat3> {
    let pcs = xy_to_xyz(D50_XY.0, D50_XY.1);
    for row in 0..3 {
        let sum: f64 = matrix[row].iter().sum();
        if !sum.is_finite() || sum.abs() < 1e-12 {
            return None;
        }
        let scale = pcs[row] / sum;
        for value in &mut matrix[row] {
            *value *= scale;
        }
    }
    Some(matrix)
}

/// The DNG calibration data this module needs, read from a decoded file.
struct DngData {
    cm1: Mat3,
    cm2: Option<Mat3>,
    cm3: Option<Mat3>,
    cc1: Mat3,
    cc2: Mat3,
    cc3: Mat3,
    fm1: Option<Mat3>,
    fm2: Option<Mat3>,
    fm3: Option<Mat3>,
    analog_balance: Vec3,
    camera_neutral: Vec3,
    white_balance_source: &'static str,
    used_camera_calibration: bool,
    illuminants: Vec<CalibrationIlluminant>,
}

/// What the full DNG path did, recorded in the colour report.
#[derive(Debug, Clone, Serialize)]
pub struct DngColorReport {
    /// Set when the calibration came from a standalone DCP rather than from the
    /// picture's own tags, naming that profile. `None` is the ordinary case: a
    /// DNG describing itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_source: Option<String>,
    /// The illuminants the calibration sets were fitted at.
    pub illuminant1: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub illuminant2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub illuminant3: Option<String>,
    /// Scene correlated colour temperature estimated from the neutral, in kelvin.
    pub estimated_cct: f32,
    /// Interpolation weight toward illuminant 1 (`1.0` when single-illuminant).
    pub weight_illuminant1: f32,
    pub interpolation_weights: Vec<f32>,
    pub calibration_count: usize,
    pub camera_channels: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub custom_illuminants: Vec<String>,
    /// Which mutually exclusive DNG white-balance tag supplied the scene white.
    pub white_balance_source: &'static str,
    /// Whether the per-unit `CameraCalibration` matrices were used. The DNG
    /// signature rule replaces them with identity matrices on a mismatch.
    pub used_camera_calibration: bool,
    /// Whether a `ForwardMatrix` drove the conversion. False means the
    /// ColorMatrix route (and ReductionMatrix for four channels) was used.
    pub used_forward_matrix: bool,
    pub used_reduction_matrix: bool,
}

/// Find a tag deterministically. DNG profile tags normally live in IFD 0, so a
/// chained top-level IFD has precedence. The offset sort is the fallback for a
/// writer that put them in a SubIFD; Rawler stores SubIFDs in a `HashMap`, whose
/// traversal order cannot be allowed to choose the colour profile.
fn find_entry(tiff: &GenericTiffReader, tag: u16) -> Option<&Entry> {
    if let Some(entry) = tiff.get_entry(tag) {
        return Some(entry);
    }
    tiff.find_ifds_with_tag(tag)
        .into_iter()
        .min_by_key(|ifd| ifd.offset)
        .and_then(|ifd| ifd.get_entry(tag))
}

/// Read an exact 9-element matrix tag as a row-major [`Mat3`].
///
/// The DNG profile tags (`ForwardMatrix`, `CameraCalibration`, `AnalogBalance`,
/// `AsShotNeutral`, `CalibrationIlluminant`) live in the file's IFD, not in
/// `RawImage::dng_tags` — that field is only ever populated by a caller's
/// override, never by the decoder. So they are read straight from the TIFF
/// directory. This path deliberately accepts only three-colour DNGs; treating
/// the first nine values of a 3-by-4 matrix as 3-by-3 would silently corrupt an
/// RGBE file.
fn read_matrix(tiff: &GenericTiffReader, tag: u16) -> Option<Mat3> {
    let value = &find_entry(tiff, tag)?.value;
    if value.count() != 9 {
        return None;
    }
    let mut m = [[0.0f64; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = value.force_f32(i * 3 + j) as f64;
            if !cell.is_finite() {
                return None;
            }
        }
    }
    Some(m)
}

fn read_matrix43(tiff: &GenericTiffReader, tag: u16) -> Option<Mat43> {
    let value = &find_entry(tiff, tag)?.value;
    if value.count() != 12 {
        return None;
    }
    let mut matrix = [[0.0; 3]; 4];
    for (index, cell) in matrix.iter_mut().flatten().enumerate() {
        *cell = value.force_f32(index) as f64;
        if !cell.is_finite() {
            return None;
        }
    }
    Some(matrix)
}

fn read_matrix34(tiff: &GenericTiffReader, tag: u16) -> Option<Mat34> {
    let value = &find_entry(tiff, tag)?.value;
    if value.count() != 12 {
        return None;
    }
    let mut matrix = [[0.0; 4]; 3];
    for (index, cell) in matrix.iter_mut().flatten().enumerate() {
        *cell = value.force_f32(index) as f64;
        if !cell.is_finite() {
            return None;
        }
    }
    Some(matrix)
}

fn read_matrix44(tiff: &GenericTiffReader, tag: u16) -> Option<Mat44> {
    let value = &find_entry(tiff, tag)?.value;
    if value.count() != 16 {
        return None;
    }
    let mut matrix = [[0.0; 4]; 4];
    for (index, cell) in matrix.iter_mut().flatten().enumerate() {
        *cell = value.force_f32(index) as f64;
        if !cell.is_finite() {
            return None;
        }
    }
    Some(matrix)
}

/// Read a 3-element vector tag from an IFD.
fn read_vec3(tiff: &GenericTiffReader, tag: u16) -> Option<Vec3> {
    let value = &find_entry(tiff, tag)?.value;
    if value.count() != 3 {
        return None;
    }
    let vector = [
        value.force_f32(0) as f64,
        value.force_f32(1) as f64,
        value.force_f32(2) as f64,
    ];
    vector.iter().all(|v| v.is_finite()).then_some(vector)
}

fn read_vec4(tiff: &GenericTiffReader, tag: u16) -> Option<Vec4> {
    let value = &find_entry(tiff, tag)?.value;
    if value.count() != 4 {
        return None;
    }
    let mut vector = [0.0; 4];
    for (index, cell) in vector.iter_mut().enumerate() {
        *cell = value.force_f32(index) as f64;
    }
    vector
        .iter()
        .all(|value| value.is_finite())
        .then_some(vector)
}

fn read_xy(tiff: &GenericTiffReader, tag: u16) -> Option<(f64, f64)> {
    let value = &find_entry(tiff, tag)?.value;
    if value.count() != 2 {
        return None;
    }
    let xy = (value.force_f32(0) as f64, value.force_f32(1) as f64);
    valid_xy(xy).then_some(xy)
}

fn read_illuminant(tiff: &GenericTiffReader, tag: u16) -> Option<Illuminant> {
    let value = &find_entry(tiff, tag)?.value;
    if value.count() != 1 {
        return None;
    }
    Illuminant::try_from(value.force_u16(0)).ok()
}

fn undefined_bytes(tiff: &GenericTiffReader, tag: u16) -> Option<&[u8]> {
    match &find_entry(tiff, tag)?.value {
        Value::Undefined(bytes) | Value::Byte(bytes) => Some(bytes),
        _ => None,
    }
}

fn read_u16(bytes: &[u8], offset: usize, little: bool) -> Option<u16> {
    let raw: [u8; 2] = bytes.get(offset..offset + 2)?.try_into().ok()?;
    Some(if little {
        u16::from_le_bytes(raw)
    } else {
        u16::from_be_bytes(raw)
    })
}

fn read_u32(bytes: &[u8], offset: usize, little: bool) -> Option<u32> {
    let raw: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(if little {
        u32::from_le_bytes(raw)
    } else {
        u32::from_be_bytes(raw)
    })
}

fn read_rational(bytes: &[u8], offset: usize, little: bool) -> Option<f64> {
    let numerator = read_u32(bytes, offset, little)?;
    let denominator = read_u32(bytes, offset + 4, little)?;
    (denominator != 0).then_some(numerator as f64 / denominator as f64)
}

// Smooth analytic approximation of the CIE 1931 2-degree colour matching
// functions, evaluated at one-nanometre intervals for custom spectral
// illuminants. This avoids reducing an SPD profile to a nominal CCT.
fn cie_xyz_bar(wavelength: f64) -> [f64; 3] {
    fn gaussian(wavelength: f64, mean: f64, left: f64, right: f64) -> f64 {
        let scale = if wavelength < mean { left } else { right };
        (-0.5 * ((wavelength - mean) * scale).powi(2)).exp()
    }
    [
        1.056 * gaussian(wavelength, 599.8, 0.0264, 0.0323)
            + 0.362 * gaussian(wavelength, 442.0, 0.0624, 0.0374)
            - 0.065 * gaussian(wavelength, 501.1, 0.0490, 0.0382),
        0.821 * gaussian(wavelength, 568.8, 0.0213, 0.0247)
            + 0.286 * gaussian(wavelength, 530.9, 0.0613, 0.0322),
        1.217 * gaussian(wavelength, 437.0, 0.0845, 0.0278)
            + 0.681 * gaussian(wavelength, 459.0, 0.0385, 0.0725),
    ]
}

fn parse_illuminant_data_endian(bytes: &[u8], little: bool) -> Option<((f64, f64), &'static str)> {
    match read_u16(bytes, 0, little)? {
        0 => {
            let xy = (
                read_rational(bytes, 2, little)?,
                read_rational(bytes, 10, little)?,
            );
            valid_xy(xy).then_some((xy, "custom_xy"))
        }
        1 => {
            let count = read_u32(bytes, 2, little)? as usize;
            if !(2..=4096).contains(&count) {
                return None;
            }
            let minimum = read_rational(bytes, 6, little)?;
            let spacing = read_rational(bytes, 14, little)?;
            if !minimum.is_finite() || !spacing.is_finite() || spacing <= 0.0 {
                return None;
            }
            let values: Vec<f64> = (0..count)
                .map(|i| read_rational(bytes, 22 + i * 8, little))
                .collect::<Option<_>>()?;
            if values
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
            {
                return None;
            }
            let maximum = minimum + spacing * (count - 1) as f64;
            let sample = |wavelength: f64| {
                if wavelength <= minimum {
                    return values[0];
                }
                if wavelength >= maximum {
                    return values[count - 1];
                }
                let position = (wavelength - minimum) / spacing;
                let lower = position.floor() as usize;
                let fraction = position - lower as f64;
                values[lower] * (1.0 - fraction) + values[lower + 1] * fraction
            };
            let mut xyz = [0.0_f64; 3];
            for wavelength in 360..=830 {
                let power = sample(wavelength as f64);
                let observer = cie_xyz_bar(wavelength as f64);
                for axis in 0..3 {
                    xyz[axis] += power * observer[axis];
                }
            }
            let xy = xyz_to_xy(&xyz);
            valid_xy(xy).then_some((xy, "custom_spectrum"))
        }
        _ => None,
    }
}

fn read_illuminant_data(tiff: &GenericTiffReader, tag: u16) -> Option<((f64, f64), &'static str)> {
    let bytes = undefined_bytes(tiff, tag)?;
    parse_illuminant_data_endian(bytes, true).or_else(|| parse_illuminant_data_endian(bytes, false))
}

#[derive(Clone)]
struct CalibrationIlluminant {
    label: String,
    xy: (f64, f64),
    temp: f64,
    custom_kind: Option<&'static str>,
}

fn calibration_illuminant(
    tiff: &GenericTiffReader,
    illuminant_tag: u16,
    data_tag: u16,
) -> Option<CalibrationIlluminant> {
    let source = read_illuminant(tiff, illuminant_tag).unwrap_or(Illuminant::Unknown);
    if source == Illuminant::Unknown {
        let (xy, kind) = read_illuminant_data(tiff, data_tag)?;
        return Some(CalibrationIlluminant {
            label: "Other".to_string(),
            xy,
            temp: correlated_temperature(xy.0, xy.1)?,
            custom_kind: Some(kind),
        });
    }
    let temp = illuminant_temp(source);
    let xy = cct_to_xy(temp)?;
    Some(CalibrationIlluminant {
        label: format!("{source:?}"),
        xy,
        temp,
        custom_kind: None,
    })
}

fn read_signature(tiff: &GenericTiffReader, tag: u16) -> Option<Vec<u8>> {
    let Some(entry) = find_entry(tiff, tag) else {
        // The specified default is the empty string, so two absent signatures
        // match. An entry with the wrong TIFF type is not the default: it is
        // malformed and must not authorize CameraCalibration.
        return Some(Vec::new());
    };
    let bytes = match &entry.value {
        Value::Ascii(value) => value.as_bytes().as_slice(),
        Value::Byte(value) => value.as_slice(),
        _ => return None,
    };
    let end = bytes
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |i| i + 1);
    Some(bytes[..end].to_vec())
}

/// Gather the DNG calibration data, or `None` when the profile is incomplete or
/// malformed. ForwardMatrix is optional in the DNG model; the no-forward path
/// is composed from ColorMatrix plus Bradford adaptation.
fn gather(raw: &RawImage, tiff: &GenericTiffReader) -> Option<DngData> {
    let mut data = gather_profile(tiff)?;
    attach_neutral(&mut data, raw, Some(tiff))?;
    Some(data)
}

/// Read the calibration half of a DNG profile: the matrices, the illuminants
/// they were fitted at, and the per-unit calibration the signature rule allows.
/// The scene white is not part of it -- see `attach_neutral`.
fn gather_profile(tiff: &GenericTiffReader) -> Option<DngData> {
    let cm1 = read_matrix(tiff, TAG_COLOR_MATRIX_1)?;
    let tagged_fm1 = read_matrix(tiff, TAG_FORWARD_MATRIX_1).and_then(normalize_forward_matrix);
    let tagged_fm2 = read_matrix(tiff, TAG_FORWARD_MATRIX_2).and_then(normalize_forward_matrix);
    let tagged_fm3 = read_matrix(tiff, TAG_FORWARD_MATRIX_3).and_then(normalize_forward_matrix);

    let illuminant1 =
        calibration_illuminant(tiff, TAG_CALIBRATION_ILLUMINANT_1, TAG_ILLUMINANT_DATA_1)
            .unwrap_or(CalibrationIlluminant {
                label: "Unknown".to_string(),
                xy: D50_XY,
                temp: 0.0,
                custom_kind: None,
            });
    let candidate2 =
        calibration_illuminant(tiff, TAG_CALIBRATION_ILLUMINANT_2, TAG_ILLUMINANT_DATA_2);
    let cm2_tag = read_matrix(tiff, TAG_COLOR_MATRIX_2);
    let dual = matches!((&candidate2, cm2_tag), (Some(second), Some(_))
        if second.temp > 0.0
            && illuminant1.temp > 0.0
            && second.xy != illuminant1.xy);
    let cm2 = dual.then_some(cm2_tag).flatten();

    let triple_declared = find_entry(tiff, TAG_CALIBRATION_ILLUMINANT_3).is_some()
        || find_entry(tiff, TAG_COLOR_MATRIX_3).is_some();
    let candidate3 =
        calibration_illuminant(tiff, TAG_CALIBRATION_ILLUMINANT_3, TAG_ILLUMINANT_DATA_3);
    let cm3_tag = read_matrix(tiff, TAG_COLOR_MATRIX_3);
    let triple = if triple_declared {
        let second = candidate2.as_ref()?;
        let third = candidate3.as_ref()?;
        if !dual
            || cm3_tag.is_none()
            || third.xy == illuminant1.xy
            || third.xy == second.xy
            || illuminant1.xy == second.xy
        {
            return None;
        }
        let fm_count = [tagged_fm1, tagged_fm2, tagged_fm3]
            .iter()
            .filter(|matrix| matrix.is_some())
            .count();
        if fm_count != 0 && fm_count != 3 {
            return None;
        }
        true
    } else {
        false
    };
    let cm3 = triple.then_some(cm3_tag).flatten();

    // In a one/dual profile the SDK permits either numbered ForwardMatrix to
    // stand alone. A triple profile requires all three or none.
    let fm1 = if triple {
        tagged_fm1
    } else {
        tagged_fm1.or(tagged_fm2)
    };
    let fm2 = if dual && tagged_fm1.is_some() {
        tagged_fm2
    } else {
        None
    };
    let fm3 = triple.then_some(tagged_fm3).flatten();

    let cc1_tag = read_matrix(tiff, TAG_CAMERA_CALIBRATION_1);
    let cc2_tag = read_matrix(tiff, TAG_CAMERA_CALIBRATION_2);
    let cc3_tag = read_matrix(tiff, TAG_CAMERA_CALIBRATION_3);
    let signatures_match = matches!(
        (
            read_signature(tiff, TAG_CAMERA_CALIBRATION_SIGNATURE),
            read_signature(tiff, TAG_PROFILE_CALIBRATION_SIGNATURE)
        ),
        (Some(camera), Some(profile)) if camera == profile
    );
    let used_camera_calibration =
        signatures_match && (cc1_tag.is_some() || cc2_tag.is_some() || cc3_tag.is_some());
    let (cc1, cc2, cc3) = if used_camera_calibration {
        let cc1 = cc1_tag.unwrap_or(IDENTITY);
        let cc2 = if dual { cc2_tag.unwrap_or(cc1) } else { cc1 };
        let cc3 = if triple { cc3_tag.unwrap_or(cc1) } else { cc1 };
        (cc1, cc2, cc3)
    } else {
        (IDENTITY, IDENTITY, IDENTITY)
    };
    let analog_balance = read_vec3(tiff, TAG_ANALOG_BALANCE).unwrap_or([1.0, 1.0, 1.0]);
    if analog_balance.iter().any(|v| *v <= 0.0) {
        return None;
    }

    let mut illuminants = vec![illuminant1];
    if dual {
        illuminants.push(candidate2?);
    }
    if triple {
        illuminants.push(candidate3?);
    }

    // ReductionMatrix is legal only when ColorPlanes is greater than three.
    if [
        TAG_REDUCTION_MATRIX_1,
        TAG_REDUCTION_MATRIX_2,
        TAG_REDUCTION_MATRIX_3,
    ]
    .into_iter()
    .any(|tag| find_entry(tiff, tag).is_some())
    {
        return None;
    }

    Some(DngData {
        cm1,
        cm2,
        cm3,
        cc1,
        cc2,
        cc3,
        fm1,
        fm2,
        fm3,
        analog_balance,
        // A placeholder until `attach_neutral` supplies the scene white. No
        // caller may read it before then: every routine that consumes
        // `camera_neutral` is reached through `attach_neutral`'s callers.
        camera_neutral: [1.0; 3],
        white_balance_source: "as_shot_neutral",
        used_camera_calibration,
        illuminants,
    })
}

/// Fill in the scene white from the file the picture came out of.
///
/// Split from `gather_profile` because the two halves need not come from the
/// same file. A DNG carries its calibration and its scene white together, but a
/// standalone DCP is only the calibration half: there is no picture in it and so
/// no white to read, and the neutral has to come from the RAW being developed.
fn attach_neutral(
    data: &mut DngData,
    raw: &RawImage,
    tiff: Option<&GenericTiffReader>,
) -> Option<()> {
    // AsShotNeutral and AsShotWhiteXY are mutually exclusive in DNG. The
    // decoder-derived reciprocal is a last-resort compatibility fallback, and
    // the only route available when the profile came from a separate file.
    if let Some(neutral) = tiff
        .and_then(|tiff| read_vec3(tiff, TAG_AS_SHOT_NEUTRAL))
        .and_then(normalize_neutral)
    {
        data.camera_neutral = neutral;
    } else if let Some(xy) = tiff.and_then(|tiff| read_xy(tiff, TAG_AS_SHOT_WHITE_XY)) {
        let weights = weights_for_xy(data, xy);
        data.camera_neutral = normalize_neutral(mat_vec(
            &xyz_to_camera(data, weights),
            &xy_to_xyz(xy.0, xy.1),
        ))?;
        data.white_balance_source = "as_shot_white_xy";
    } else {
        data.camera_neutral = {
            let wb = raw.wb_coeffs;
            let neutral = (wb[..3].iter().all(|v| v.is_finite() && *v > 0.0)).then_some([
                1.0 / wb[0] as f64,
                1.0 / wb[1] as f64,
                1.0 / wb[2] as f64,
            ])?;
            normalize_neutral(neutral)?
        };
        data.white_balance_source = "decoder_white_balance";
    }
    Some(())
}

fn weights_for_xy(data: &DngData, xy: (f64, f64)) -> [f64; 3] {
    if data.cm3.is_some() {
        return triple_weights(
            xy,
            [
                data.illuminants[0].xy,
                data.illuminants[1].xy,
                data.illuminants[2].xy,
            ],
        );
    }
    if data.cm2.is_some() {
        let first = data.illuminants[0].temp;
        let second = data.illuminants[1].temp;
        let g = correlated_temperature(xy.0, xy.1)
            .map(|temp| dual_weight(temp, first, second))
            .unwrap_or(1.0);
        return [g, 1.0 - g, 0.0];
    }
    [1.0, 0.0, 0.0]
}

/// The interpolated `AB · CC · CM` transform specified by DNG for translating
/// between the selected white xy and individual-camera neutral coordinates.
fn xyz_to_camera(data: &DngData, weights: [f64; 3]) -> Mat3 {
    let cm = if let (Some(cm2), Some(cm3)) = (data.cm2, data.cm3) {
        blend_mat(&data.cm1, &cm2, &cm3, weights)
    } else if let Some(cm2) = data.cm2 {
        lerp_mat(&data.cm1, &cm2, weights[0])
    } else {
        data.cm1
    };
    let cc = if data.cm3.is_some() {
        blend_mat(&data.cc1, &data.cc2, &data.cc3, weights)
    } else if data.cm2.is_some() {
        lerp_mat(&data.cc1, &data.cc2, weights[0])
    } else {
        data.cc1
    };
    mat_mul(&diag(&data.analog_balance), &mat_mul(&cc, &cm))
}

/// Solve for the scene white and dual-illuminant blend weight.
///
/// DNG calls for iteration until xy converges. Thirty passes and the
/// two-cycle average are the conservative reference behaviour; normal profiles
/// converge in a handful.
fn solve_white(data: &DngData) -> Option<((f64, f64), [f64; 3], f64)> {
    let mut last = D50_XY;
    for pass in 0..30 {
        let weights = weights_for_xy(data, last);
        let inverse = mat_invert(&xyz_to_camera(data, weights))?;
        let next = xyz_to_xy(&mat_vec(&inverse, &data.camera_neutral));
        if !valid_xy(next) {
            return None;
        }
        if (next.0 - last.0).abs() + (next.1 - last.1).abs() < 1e-7 {
            let cct = correlated_temperature(next.0, next.1)?;
            return Some((next, weights_for_xy(data, next), cct));
        }
        if pass == 29 {
            let averaged = ((last.0 + next.0) * 0.5, (last.1 + next.1) * 0.5);
            let cct = correlated_temperature(averaged.0, averaged.1)?;
            return Some((averaged, weights_for_xy(data, averaged), cct));
        }
        last = next;
    }
    None
}

/// Compose camera → XYZ(D50) from the interpolated calibration data.
fn camera_to_xyz_d50(data: &DngData, weights: [f64; 3], white_xy: (f64, f64)) -> Option<Mat3> {
    let cc = if data.cm3.is_some() {
        blend_mat(&data.cc1, &data.cc2, &data.cc3, weights)
    } else if data.cm2.is_some() {
        lerp_mat(&data.cc1, &data.cc2, weights[0])
    } else {
        data.cc1
    };
    let fm = match (data.fm1, data.fm2, data.fm3) {
        (Some(fm1), Some(fm2), Some(fm3)) => Some(blend_mat(&fm1, &fm2, &fm3, weights)),
        (Some(fm1), Some(fm2), None) => Some(lerp_mat(&fm1, &fm2, weights[0])),
        (Some(fm1), None, None) => Some(fm1),
        _ => None,
    };
    if fm.is_none() {
        let camera_to_xyz = mat_invert(&xyz_to_camera(data, weights))?;
        return Some(mat_mul(&bradford(white_xy, D50_XY), &camera_to_xyz));
    }
    let fm = fm?;
    let ab = diag(&data.analog_balance);
    let ab_cc = mat_mul(&ab, &cc);
    let ab_cc_inv = mat_invert(&ab_cc)?;
    let reference_neutral = mat_vec(&ab_cc_inv, &data.camera_neutral);
    if reference_neutral
        .iter()
        .any(|v| !v.is_finite() || *v <= 1e-9)
    {
        return None;
    }
    let d = diag(&[
        1.0 / reference_neutral[0],
        1.0 / reference_neutral[1],
        1.0 / reference_neutral[2],
    ]);
    Some(mat_mul(&fm, &mat_mul(&d, &ab_cc_inv)))
}

/// The composed camera → working-space matrix, plus a report, or `None` when the
/// file does not carry enough DNG calibration data for the full path.
pub fn camera_to_working(
    raw: &RawImage,
    path: &Path,
    working_space: WorkingSpace,
) -> Option<([[f32; 3]; 3], DngColorReport)> {
    // The profile tags live in the file's TIFF directory; open it once and read
    // the root IFD, exactly as `crate::metadata` does for EXIF.
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let tiff = GenericTiffReader::new(&mut reader, 0, 0, None, &[]).ok()?;
    let data = gather(raw, &tiff)?;
    compose(data, working_space)
}

/// The same composition, but with the calibration read from a standalone DCP
/// rather than from the picture's own tags.
///
/// This is what makes a bundled profile a *calibration* instead of a look. A
/// DCP's `ProfileHueSatMapData` encodes the residual left by that profile's own
/// `ForwardMatrix`; applied after some other matrix it is a hue rotation of
/// unknown provenance. Reading both halves from the same DCP puts the table back
/// with the matrix it was fitted against, and the CCT that
/// [`solve_white`] returns on the way is the one the table's two calibration
/// illuminants must be interpolated at -- which is otherwise unavailable for any
/// file that is not itself a DNG.
///
/// The scene white still comes from the RAW: a profile has no picture in it.
pub fn camera_to_working_from_profile(
    raw: &RawImage,
    raw_path: &Path,
    profile_bytes: &[u8],
    working_space: WorkingSpace,
) -> Option<([[f32; 3]; 3], DngColorReport)> {
    // Bytes rather than a path: the bundled profile is `include_bytes!`d into
    // the binary, so `--preset vivid` must not need the source tree at runtime.
    let profile_tiff = GenericTiffReader::new_with_buffer(profile_bytes, 0, 0, None).ok()?;
    let mut data = gather_profile(&profile_tiff)?;

    // A DNG still knows its own scene white better than the decoder's
    // reciprocal does, so prefer the picture's tags when it has them; an ARW
    // has none and falls through to `wb_coeffs`.
    let raw_file = File::open(raw_path).ok();
    let mut raw_reader = raw_file.map(BufReader::new);
    let raw_tiff = raw_reader
        .as_mut()
        .and_then(|reader| GenericTiffReader::new(reader, 0, 0, None, &[]).ok());
    attach_neutral(&mut data, raw, raw_tiff.as_ref())?;
    compose(data, working_space)
}

/// Solve the scene white and compose camera → working RGB from gathered data.
fn compose(data: DngData, working_space: WorkingSpace) -> Option<([[f32; 3]; 3], DngColorReport)> {
    let (white_xy, weights, cct) = solve_white(&data)?;
    let cam_to_xyz_d50 = camera_to_xyz_d50(&data, weights, white_xy)?;

    // XYZ(D50) → XYZ(D65) → working RGB.
    let d50_to_d65 = bradford(D50_XY, D65_XY);
    let working_to_xyz_d65: Mat3 = {
        let m = working_space.to_xyz_d65();
        [
            [m[0][0] as f64, m[0][1] as f64, m[0][2] as f64],
            [m[1][0] as f64, m[1][1] as f64, m[1][2] as f64],
            [m[2][0] as f64, m[2][1] as f64, m[2][2] as f64],
        ]
    };
    let xyz_d65_to_working = mat_invert(&working_to_xyz_d65)?;
    let cam_to_working = mat_mul(&xyz_d65_to_working, &mat_mul(&d50_to_d65, &cam_to_xyz_d50));

    let mut out = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = cam_to_working[i][j] as f32;
            if !out[i][j].is_finite() {
                return None;
            }
        }
    }

    let report = DngColorReport {
        // Overwritten by the caller on the profile route; the file route leaves
        // it unset, which is what says the calibration is the file's own.
        profile_source: None,
        illuminant1: data.illuminants[0].label.clone(),
        illuminant2: data.illuminants.get(1).map(|value| value.label.clone()),
        illuminant3: data.illuminants.get(2).map(|value| value.label.clone()),
        estimated_cct: cct as f32,
        weight_illuminant1: weights[0] as f32,
        interpolation_weights: weights[..data.illuminants.len()]
            .iter()
            .map(|weight| *weight as f32)
            .collect(),
        calibration_count: data.illuminants.len(),
        camera_channels: 3,
        custom_illuminants: data
            .illuminants
            .iter()
            .filter_map(|value| {
                value
                    .custom_kind
                    .map(|kind| format!("{}:{kind}", value.label))
            })
            .collect(),
        white_balance_source: data.white_balance_source,
        used_camera_calibration: data.used_camera_calibration,
        used_forward_matrix: data.fm1.is_some(),
        // DNG permits ReductionMatrix only when the camera has more than three
        // colour planes. A three-channel profile never applies one.
        used_reduction_matrix: false,
    };
    Some((out, report))
}

/// Matrix returned to the colour stage. DNG permits three or four camera
/// channels while the working/output side is always RGB.
pub enum DngMatrix {
    Three([[f32; 3]; 3]),
    Four([[f32; 4]; 3]),
}

fn invert44(matrix: &Mat44) -> Option<Mat44> {
    let mut augmented = [[0.0_f64; 8]; 4];
    for row in 0..4 {
        augmented[row][..4].copy_from_slice(&matrix[row]);
        augmented[row][4 + row] = 1.0;
    }
    for column in 0..4 {
        let pivot = (column..4).max_by(|left, right| {
            augmented[*left][column]
                .abs()
                .total_cmp(&augmented[*right][column].abs())
        })?;
        if augmented[pivot][column].abs() < 1.0e-12 {
            return None;
        }
        augmented.swap(column, pivot);
        let divisor = augmented[column][column];
        for cell in &mut augmented[column] {
            *cell /= divisor;
        }
        for row in 0..4 {
            if row == column {
                continue;
            }
            let factor = augmented[row][column];
            for j in 0..8 {
                augmented[row][j] -= factor * augmented[column][j];
            }
        }
    }
    let mut inverse = [[0.0; 4]; 4];
    for row in 0..4 {
        inverse[row].copy_from_slice(&augmented[row][4..]);
    }
    inverse
        .iter()
        .flatten()
        .all(|value| value.is_finite())
        .then_some(inverse)
}

fn mat44_mul43(left: &Mat44, right: &Mat43) -> Mat43 {
    let mut output = [[0.0; 3]; 4];
    for row in 0..4 {
        for column in 0..3 {
            output[row][column] = (0..4).map(|k| left[row][k] * right[k][column]).sum();
        }
    }
    output
}

fn mat34_mul43(left: &Mat34, right: &Mat43) -> Mat3 {
    let mut output = [[0.0; 3]; 3];
    for row in 0..3 {
        for column in 0..3 {
            output[row][column] = (0..4).map(|k| left[row][k] * right[k][column]).sum();
        }
    }
    output
}

fn mat3_mul34(left: &Mat3, right: &Mat34) -> Mat34 {
    let mut output = [[0.0; 4]; 3];
    for row in 0..3 {
        for column in 0..4 {
            output[row][column] = (0..3).map(|k| left[row][k] * right[k][column]).sum();
        }
    }
    output
}

fn mat34_mul44(left: &Mat34, right: &Mat44) -> Mat34 {
    let mut output = [[0.0; 4]; 3];
    for row in 0..3 {
        for column in 0..4 {
            output[row][column] = (0..4).map(|k| left[row][k] * right[k][column]).sum();
        }
    }
    output
}

fn mat44_mul44(left: &Mat44, right: &Mat44) -> Mat44 {
    let mut output = [[0.0; 4]; 4];
    for row in 0..4 {
        for column in 0..4 {
            output[row][column] = (0..4).map(|k| left[row][k] * right[k][column]).sum();
        }
    }
    output
}

fn mat43_vec3(matrix: &Mat43, vector: &Vec3) -> Vec4 {
    let mut output = [0.0; 4];
    for row in 0..4 {
        output[row] = (0..3)
            .map(|column| matrix[row][column] * vector[column])
            .sum();
    }
    output
}

fn mat44_vec4(matrix: &Mat44, vector: &Vec4) -> Vec4 {
    let mut output = [0.0; 4];
    for row in 0..4 {
        output[row] = (0..4)
            .map(|column| matrix[row][column] * vector[column])
            .sum();
    }
    output
}

fn blend43(a: &Mat43, b: &Mat43, c: &Mat43, weights: [f64; 3]) -> Mat43 {
    let mut output = [[0.0; 3]; 4];
    for row in 0..4 {
        for column in 0..3 {
            output[row][column] = weights[0] * a[row][column]
                + weights[1] * b[row][column]
                + weights[2] * c[row][column];
        }
    }
    output
}

fn blend34(a: &Mat34, b: &Mat34, c: &Mat34, weights: [f64; 3]) -> Mat34 {
    let mut output = [[0.0; 4]; 3];
    for row in 0..3 {
        for column in 0..4 {
            output[row][column] = weights[0] * a[row][column]
                + weights[1] * b[row][column]
                + weights[2] * c[row][column];
        }
    }
    output
}

fn blend44(a: &Mat44, b: &Mat44, c: &Mat44, weights: [f64; 3]) -> Mat44 {
    let mut output = [[0.0; 4]; 4];
    for row in 0..4 {
        for column in 0..4 {
            output[row][column] = weights[0] * a[row][column]
                + weights[1] * b[row][column]
                + weights[2] * c[row][column];
        }
    }
    output
}

fn normalize_forward34(mut matrix: Mat34) -> Option<Mat34> {
    let pcs = xy_to_xyz(D50_XY.0, D50_XY.1);
    for row in 0..3 {
        let sum: f64 = matrix[row].iter().sum();
        if !sum.is_finite() || sum.abs() < 1.0e-12 {
            return None;
        }
        for value in &mut matrix[row] {
            *value *= pcs[row] / sum;
        }
    }
    Some(matrix)
}

fn normalize_neutral4(mut vector: Vec4) -> Option<Vec4> {
    if vector
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return None;
    }
    let maximum = vector.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    for value in &mut vector {
        *value /= maximum;
    }
    Some(vector)
}

struct DngData4 {
    cm: [Mat43; 3],
    cc: [Mat44; 3],
    fm: [Option<Mat34>; 3],
    rm: [Option<Mat34>; 3],
    analog: Vec4,
    neutral: Vec4,
    illuminants: Vec<CalibrationIlluminant>,
    white_balance_source: &'static str,
    used_camera_calibration: bool,
}

fn identity44() -> Mat44 {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn weights4(data: &DngData4, xy: (f64, f64)) -> [f64; 3] {
    match data.illuminants.len() {
        3 => triple_weights(
            xy,
            [
                data.illuminants[0].xy,
                data.illuminants[1].xy,
                data.illuminants[2].xy,
            ],
        ),
        2 => {
            let temperature =
                correlated_temperature(xy.0, xy.1).unwrap_or(data.illuminants[0].temp);
            let first = dual_weight(
                temperature,
                data.illuminants[0].temp,
                data.illuminants[1].temp,
            );
            [first, 1.0 - first, 0.0]
        }
        _ => [1.0, 0.0, 0.0],
    }
}

fn xyz_to_camera4(data: &DngData4, weights: [f64; 3]) -> Mat43 {
    let cm = blend43(&data.cm[0], &data.cm[1], &data.cm[2], weights);
    let cc = blend44(&data.cc[0], &data.cc[1], &data.cc[2], weights);
    let mut ab = [[0.0; 4]; 4];
    for (index, row) in ab.iter_mut().enumerate() {
        row[index] = data.analog[index];
    }
    mat44_mul43(&ab, &mat44_mul43(&cc, &cm))
}

fn pseudo_inverse43(matrix: &Mat43) -> Option<Mat34> {
    let mut mt_m = [[0.0; 3]; 3];
    for (row, output_row) in mt_m.iter_mut().enumerate() {
        for (column, output) in output_row.iter_mut().enumerate() {
            *output = (0..4).map(|k| matrix[k][row] * matrix[k][column]).sum();
        }
    }
    let inverse = mat_invert(&mt_m)?;
    let mut result = [[0.0; 4]; 3];
    for (row, output_row) in result.iter_mut().enumerate() {
        for (column, output) in output_row.iter_mut().enumerate() {
            *output = (0..3).map(|k| inverse[row][k] * matrix[column][k]).sum();
        }
    }
    Some(result)
}

fn gather4(raw: &RawImage, tiff: &GenericTiffReader) -> Option<DngData4> {
    let cm1 = read_matrix43(tiff, TAG_COLOR_MATRIX_1)?;
    let cm2 = read_matrix43(tiff, TAG_COLOR_MATRIX_2);
    let cm3 = read_matrix43(tiff, TAG_COLOR_MATRIX_3);
    let first = calibration_illuminant(tiff, TAG_CALIBRATION_ILLUMINANT_1, TAG_ILLUMINANT_DATA_1)?;
    let second = calibration_illuminant(tiff, TAG_CALIBRATION_ILLUMINANT_2, TAG_ILLUMINANT_DATA_2);
    let third = calibration_illuminant(tiff, TAG_CALIBRATION_ILLUMINANT_3, TAG_ILLUMINANT_DATA_3);
    let count = if cm3.is_some() || third.is_some() {
        if cm2.is_none() || cm3.is_none() || second.is_none() || third.is_none() {
            return None;
        }
        3
    } else if cm2.is_some() && second.is_some() {
        2
    } else {
        1
    };
    let mut illuminants = vec![first];
    if count >= 2 {
        illuminants.push(second?);
    }
    if count == 3 {
        illuminants.push(third?);
        if illuminants[0].xy == illuminants[1].xy
            || illuminants[0].xy == illuminants[2].xy
            || illuminants[1].xy == illuminants[2].xy
        {
            return None;
        }
    }
    let cm = [cm1, cm2.unwrap_or(cm1), cm3.unwrap_or(cm1)];

    let tagged_fm = [
        read_matrix34(tiff, TAG_FORWARD_MATRIX_1).and_then(normalize_forward34),
        read_matrix34(tiff, TAG_FORWARD_MATRIX_2).and_then(normalize_forward34),
        read_matrix34(tiff, TAG_FORWARD_MATRIX_3).and_then(normalize_forward34),
    ];
    if count == 3 && tagged_fm.iter().filter(|value| value.is_some()).count() % 3 != 0 {
        return None;
    }
    let fm = if count < 3 && tagged_fm[0].is_none() && tagged_fm[1].is_some() {
        [tagged_fm[1], None, None]
    } else {
        tagged_fm
    };
    let rm = [
        read_matrix34(tiff, TAG_REDUCTION_MATRIX_1),
        read_matrix34(tiff, TAG_REDUCTION_MATRIX_2),
        read_matrix34(tiff, TAG_REDUCTION_MATRIX_3),
    ];
    if count == 3 && rm.iter().filter(|value| value.is_some()).count() % 3 != 0 {
        return None;
    }

    let signatures_match = matches!(
        (
            read_signature(tiff, TAG_CAMERA_CALIBRATION_SIGNATURE),
            read_signature(tiff, TAG_PROFILE_CALIBRATION_SIGNATURE)
        ),
        (Some(camera), Some(profile)) if camera == profile
    );
    let tagged_cc = [
        read_matrix44(tiff, TAG_CAMERA_CALIBRATION_1),
        read_matrix44(tiff, TAG_CAMERA_CALIBRATION_2),
        read_matrix44(tiff, TAG_CAMERA_CALIBRATION_3),
    ];
    let used_camera_calibration =
        signatures_match && tagged_cc.iter().any(|matrix| matrix.is_some());
    let identity = identity44();
    let cc = if used_camera_calibration {
        [
            tagged_cc[0].unwrap_or(identity),
            tagged_cc[1].or(tagged_cc[0]).unwrap_or(identity),
            tagged_cc[2].or(tagged_cc[0]).unwrap_or(identity),
        ]
    } else {
        [identity; 3]
    };
    let analog = read_vec4(tiff, TAG_ANALOG_BALANCE).unwrap_or([1.0; 4]);
    if analog.iter().any(|value| *value <= 0.0) {
        return None;
    }

    let mut data = DngData4 {
        cm,
        cc,
        fm,
        rm,
        analog,
        neutral: [1.0; 4],
        illuminants,
        white_balance_source: "as_shot_neutral",
        used_camera_calibration,
    };
    if let Some(neutral) = read_vec4(tiff, TAG_AS_SHOT_NEUTRAL).and_then(normalize_neutral4) {
        data.neutral = neutral;
    } else if let Some(xy) = read_xy(tiff, TAG_AS_SHOT_WHITE_XY) {
        data.neutral = normalize_neutral4(mat43_vec3(
            &xyz_to_camera4(&data, weights4(&data, xy)),
            &xy_to_xyz(xy.0, xy.1),
        ))?;
        data.white_balance_source = "as_shot_white_xy";
    } else {
        data.neutral = normalize_neutral4(raw.wb_coeffs.map(|value| 1.0 / value as f64))?;
        data.white_balance_source = "decoder_white_balance";
    }
    Some(data)
}

fn solve_white4(data: &DngData4) -> Option<((f64, f64), [f64; 3], f64)> {
    let mut last = D50_XY;
    for pass in 0..30 {
        let weights = weights4(data, last);
        let inverse = pseudo_inverse43(&xyz_to_camera4(data, weights))?;
        let xyz = [
            (0..4).map(|k| inverse[0][k] * data.neutral[k]).sum(),
            (0..4).map(|k| inverse[1][k] * data.neutral[k]).sum(),
            (0..4).map(|k| inverse[2][k] * data.neutral[k]).sum(),
        ];
        let next = xyz_to_xy(&xyz);
        if !valid_xy(next) {
            return None;
        }
        if (next.0 - last.0).abs() + (next.1 - last.1).abs() < 1.0e-7 || pass == 29 {
            let chosen = if pass == 29 {
                ((last.0 + next.0) * 0.5, (last.1 + next.1) * 0.5)
            } else {
                next
            };
            return Some((
                chosen,
                weights4(data, chosen),
                correlated_temperature(chosen.0, chosen.1)?,
            ));
        }
        last = next;
    }
    None
}

fn camera_to_xyz4(
    data: &DngData4,
    weights: [f64; 3],
    white_xy: (f64, f64),
) -> Option<(Mat34, bool)> {
    let cc = blend44(&data.cc[0], &data.cc[1], &data.cc[2], weights);
    let mut ab = [[0.0; 4]; 4];
    for (index, row) in ab.iter_mut().enumerate() {
        row[index] = data.analog[index];
    }
    let ab_cc = {
        let mut output = [[0.0; 4]; 4];
        for row in 0..4 {
            for column in 0..4 {
                output[row][column] = (0..4).map(|k| ab[row][k] * cc[k][column]).sum();
            }
        }
        output
    };
    if data.fm[0].is_some() {
        let fm = blend34(
            &data.fm[0]?,
            &data.fm[1].unwrap_or(data.fm[0]?),
            &data.fm[2].unwrap_or(data.fm[0]?),
            weights,
        );
        let inverse = invert44(&ab_cc)?;
        let reference = mat44_vec4(&inverse, &data.neutral);
        if reference
            .iter()
            .any(|value| !value.is_finite() || *value <= 1.0e-9)
        {
            return None;
        }
        let mut diagonal = [[0.0; 4]; 4];
        for index in 0..4 {
            diagonal[index][index] = 1.0 / reference[index];
        }
        return Some((mat34_mul44(&fm, &mat44_mul44(&diagonal, &inverse)), false));
    }

    let xyz_to_camera = xyz_to_camera4(data, weights);
    let camera_to_xyz = if data.rm[0].is_some() {
        let rm = blend34(
            &data.rm[0]?,
            &data.rm[1].unwrap_or(data.rm[0]?),
            &data.rm[2].unwrap_or(data.rm[0]?),
            weights,
        );
        mat3_mul34(&mat_invert(&mat34_mul43(&rm, &xyz_to_camera))?, &rm)
    } else {
        pseudo_inverse43(&xyz_to_camera)?
    };
    Some((
        mat3_mul34(&bradford(white_xy, D50_XY), &camera_to_xyz),
        data.rm[0].is_some(),
    ))
}

fn camera_to_working4(
    raw: &RawImage,
    tiff: &GenericTiffReader,
    working_space: WorkingSpace,
) -> Option<(Mat34, DngColorReport)> {
    let data = gather4(raw, tiff)?;
    let (white_xy, weights, cct) = solve_white4(&data)?;
    let (camera_to_xyz, used_reduction_matrix) = camera_to_xyz4(&data, weights, white_xy)?;
    let working_to_xyz = working_space.to_xyz_d65().map(|row| row.map(f64::from));
    let xyz_to_working = mat_invert(&working_to_xyz)?;
    let camera_to_working = mat3_mul34(
        &mat_mul(&xyz_to_working, &bradford(D50_XY, D65_XY)),
        &camera_to_xyz,
    );
    let report = DngColorReport {
        // A four-channel profile is always the file's own; the DCP route is
        // three-channel by construction.
        profile_source: None,
        illuminant1: data.illuminants[0].label.clone(),
        illuminant2: data.illuminants.get(1).map(|value| value.label.clone()),
        illuminant3: data.illuminants.get(2).map(|value| value.label.clone()),
        estimated_cct: cct as f32,
        weight_illuminant1: weights[0] as f32,
        interpolation_weights: weights[..data.illuminants.len()]
            .iter()
            .map(|weight| *weight as f32)
            .collect(),
        calibration_count: data.illuminants.len(),
        camera_channels: 4,
        custom_illuminants: data
            .illuminants
            .iter()
            .filter_map(|value| {
                value
                    .custom_kind
                    .map(|kind| format!("{}:{kind}", value.label))
            })
            .collect(),
        white_balance_source: data.white_balance_source,
        used_camera_calibration: data.used_camera_calibration,
        used_forward_matrix: data.fm[0].is_some(),
        used_reduction_matrix,
    };
    Some((camera_to_working, report))
}

/// Compose whichever DNG camera-channel dimensionality the profile declares.
pub fn camera_to_working_any(
    raw: &RawImage,
    path: &Path,
    working_space: WorkingSpace,
) -> Option<(DngMatrix, DngColorReport)> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let tiff = GenericTiffReader::new(&mut reader, 0, 0, None, &[]).ok()?;
    match find_entry(&tiff, TAG_COLOR_MATRIX_1)?.value.count() {
        9 => camera_to_working(raw, path, working_space)
            .map(|(matrix, report)| (DngMatrix::Three(matrix), report)),
        12 => {
            let (matrix, report) = camera_to_working4(raw, &tiff, working_space)?;
            let mut narrowed = [[0.0_f32; 4]; 3];
            for row in 0..3 {
                for column in 0..4 {
                    narrowed[row][column] = matrix[row][column] as f32;
                    if !narrowed[row][column].is_finite() {
                        return None;
                    }
                }
            }
            Some((DngMatrix::Four(narrowed), report))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn bradford_maps_source_white_to_destination_white() {
        let m = bradford(D65_XY, D50_XY);
        let d65 = xy_to_xyz(D65_XY.0, D65_XY.1);
        let mapped = mat_vec(&m, &d65);
        let (x, y) = xyz_to_xy(&mapped);
        assert!(approx(x, D50_XY.0, 1e-4), "x {x}");
        assert!(approx(y, D50_XY.1, 1e-4), "y {y}");
    }

    #[test]
    fn dual_weight_is_one_at_illuminant1_and_zero_at_illuminant2() {
        // Illuminant1 = StandardA (2856), illuminant2 = D65 (6504).
        assert!(approx(dual_weight(2856.0, 2856.0, 6504.0), 1.0, 1e-9));
        assert!(approx(dual_weight(6504.0, 2856.0, 6504.0), 0.0, 1e-9));
        // Numbered tag order is not temperature order. The result must still
        // weight the numbered matrices correctly when illuminant 1 is hotter.
        assert!(approx(dual_weight(6504.0, 6504.0, 2856.0), 1.0, 1e-9));
        assert!(approx(dual_weight(2856.0, 6504.0, 2856.0), 0.0, 1e-9));
        let mid = dual_weight(4000.0, 2856.0, 6504.0);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn robertson_cct_is_near_the_standard_temperatures() {
        assert!(approx(
            correlated_temperature(D65_XY.0, D65_XY.1).unwrap(),
            6504.0,
            120.0
        ));
        assert!(approx(
            correlated_temperature(0.4476, 0.4074).unwrap(),
            2856.0,
            120.0
        )); // Illuminant A
    }

    #[test]
    fn forward_matrix_is_normalized_to_the_d50_pcs_white() {
        let unnormalized = [[0.4, 0.2, 0.1], [0.1, 0.6, 0.2], [0.0, 0.1, 0.5]];
        let normalized = normalize_forward_matrix(unnormalized).unwrap();
        let mapped = mat_vec(&normalized, &[1.0, 1.0, 1.0]);
        let d50 = xy_to_xyz(D50_XY.0, D50_XY.1);
        for axis in 0..3 {
            assert!(approx(mapped[axis], d50[axis], 1e-12));
        }
    }

    #[test]
    fn neutral_scale_does_not_change_the_composed_exposure() {
        let a = normalize_neutral([0.31, 0.5, 0.39]).unwrap();
        let b = normalize_neutral([0.62, 1.0, 0.78]).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn triple_weights_reproduce_vertices_and_remain_normalized() {
        let whites = [(0.45, 0.40), (0.31, 0.33), (0.36, 0.30)];
        for (index, white) in whites.into_iter().enumerate() {
            let weights = triple_weights(white, whites);
            for (weight_index, weight) in weights.into_iter().enumerate() {
                let expected = if weight_index == index { 1.0 } else { 0.0 };
                assert!(approx(weight, expected, 1.0e-12), "{weights:?}");
            }
        }
        let weights = triple_weights((0.36, 0.34), whites);
        assert!(weights.iter().all(|weight| (0.0..=1.0).contains(weight)));
        assert!(approx(weights.iter().sum(), 1.0, 1.0e-12));
    }

    #[test]
    fn custom_xy_illuminant_data_obeys_the_dng_binary_layout() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&3127_u32.to_le_bytes());
        bytes.extend_from_slice(&10_000_u32.to_le_bytes());
        bytes.extend_from_slice(&3290_u32.to_le_bytes());
        bytes.extend_from_slice(&10_000_u32.to_le_bytes());

        let (xy, kind) = parse_illuminant_data_endian(&bytes, true).unwrap();
        assert_eq!(kind, "custom_xy");
        assert!(approx(xy.0, D65_XY.0, 1.0e-12));
        assert!(approx(xy.1, D65_XY.1, 1.0e-12));
    }

    #[test]
    fn four_channel_pseudoinverse_is_a_left_inverse() {
        let matrix: Mat43 = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.2, 0.3, 0.4],
        ];
        let product = mat34_mul43(&pseudo_inverse43(&matrix).unwrap(), &matrix);
        for (row, values) in product.iter().enumerate() {
            for (column, value) in values.iter().enumerate() {
                let expected = if row == column { 1.0 } else { 0.0 };
                assert!(approx(*value, expected, 1.0e-12), "{product:?}");
            }
        }
    }

    #[test]
    fn four_channel_forward_matrix_maps_the_as_shot_neutral_to_d50() {
        let fm = normalize_forward34([
            [0.4, 0.2, 0.2, 0.1],
            [0.1, 0.5, 0.2, 0.1],
            [0.1, 0.1, 0.3, 0.4],
        ])
        .unwrap();
        let data = DngData4 {
            cm: [[
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.2, 0.3, 0.5],
            ]; 3],
            cc: [identity44(); 3],
            fm: [Some(fm), None, None],
            rm: [None; 3],
            analog: [1.0; 4],
            neutral: [0.5, 1.0, 0.8, 0.7],
            illuminants: vec![CalibrationIlluminant {
                label: "D65".into(),
                xy: D65_XY,
                temp: 6504.0,
                custom_kind: None,
            }],
            white_balance_source: "as_shot_neutral",
            used_camera_calibration: false,
        };
        let (matrix, used_reduction) = camera_to_xyz4(&data, [1.0, 0.0, 0.0], D65_XY).unwrap();
        assert!(!used_reduction);
        let xyz = [
            (0..4).map(|k| matrix[0][k] * data.neutral[k]).sum(),
            (0..4).map(|k| matrix[1][k] * data.neutral[k]).sum(),
            (0..4).map(|k| matrix[2][k] * data.neutral[k]).sum(),
        ];
        let xy = xyz_to_xy(&xyz);
        assert!(approx(xy.0, D50_XY.0, 1.0e-10), "{xy:?}");
        assert!(approx(xy.1, D50_XY.1, 1.0e-10), "{xy:?}");
    }

    #[test]
    fn four_channel_reduction_matrix_path_maps_white_to_d50() {
        let cm: Mat43 = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.2, 0.3, 0.5],
        ];
        let reduction: Mat34 = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let neutral = normalize_neutral4(mat43_vec3(&cm, &xy_to_xyz(D65_XY.0, D65_XY.1))).unwrap();
        let data = DngData4 {
            cm: [cm; 3],
            cc: [identity44(); 3],
            fm: [None; 3],
            rm: [Some(reduction), None, None],
            analog: [1.0; 4],
            neutral,
            illuminants: vec![CalibrationIlluminant {
                label: "D65".into(),
                xy: D65_XY,
                temp: 6504.0,
                custom_kind: None,
            }],
            white_balance_source: "as_shot_neutral",
            used_camera_calibration: false,
        };
        let (matrix, used_reduction) = camera_to_xyz4(&data, [1.0, 0.0, 0.0], D65_XY).unwrap();
        assert!(used_reduction);
        let xyz = [
            (0..4).map(|k| matrix[0][k] * data.neutral[k]).sum(),
            (0..4).map(|k| matrix[1][k] * data.neutral[k]).sum(),
            (0..4).map(|k| matrix[2][k] * data.neutral[k]).sum(),
        ];
        let xy = xyz_to_xy(&xyz);
        assert!(approx(xy.0, D50_XY.0, 1.0e-10), "{xy:?}");
        assert!(approx(xy.1, D50_XY.1, 1.0e-10), "{xy:?}");
    }

    /// The load-bearing invariant: the composed transform maps the as-shot
    /// neutral onto the working-space neutral (equal RGB). This catches almost
    /// any error in the FM / D / AB·CC composition, because getting the white
    /// balance baked in wrong shows up here immediately.
    #[test]
    fn as_shot_neutral_maps_to_working_neutral() {
        // A plausible single-illuminant DNG: identity-ish ColorMatrix at D65,
        // a ForwardMatrix that is the working→XYZ(D50) of sRGB (so reference
        // neutral -> D50 white), identity calibration and analog balance, and a
        // warm-ish neutral.
        let srgb_to_xyz_d65: Mat3 = [
            [0.4124564, 0.3575761, 0.1804375],
            [0.2126729, 0.7151522, 0.0721750],
            [0.0193339, 0.1191920, 0.9503041],
        ];
        // ForwardMatrix maps reference-neutral [1,1,1] to D50 white: build it as
        // sRGB(D65)→XYZ then Bradford D65→D50 so its columns sum to D50 white.
        let fm = mat_mul(&bradford(D65_XY, D50_XY), &srgb_to_xyz_d65);
        // ColorMatrix (XYZ→camera) = inverse of camera→XYZ. Take camera==sRGB(D65)
        // for the test so the neutral is unambiguous.
        let cm = mat_invert(&srgb_to_xyz_d65).unwrap();
        let neutral = [0.62, 1.0, 0.78];

        let data = DngData {
            cm1: cm,
            cm2: None,
            cm3: None,
            cc1: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            cc2: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            cc3: IDENTITY,
            fm1: Some(fm),
            fm2: None,
            fm3: None,
            analog_balance: [1.0, 1.0, 1.0],
            camera_neutral: neutral,
            white_balance_source: "as_shot_neutral",
            used_camera_calibration: false,
            illuminants: vec![CalibrationIlluminant {
                label: "D65".into(),
                xy: D65_XY,
                temp: 6504.0,
                custom_kind: None,
            }],
        };
        let (white_xy, weights, _) = solve_white(&data).unwrap();
        let cam_to_xyz_d50 = camera_to_xyz_d50(&data, weights, white_xy).unwrap();
        // camera neutral -> XYZ(D50) must be the D50 white chromaticity.
        let xyz = mat_vec(&cam_to_xyz_d50, &neutral);
        let (x, y) = xyz_to_xy(&xyz);
        assert!(approx(x, D50_XY.0, 2e-3), "white x {x}");
        assert!(approx(y, D50_XY.1, 2e-3), "white y {y}");

        // And into sRGB(D65) working space the neutral is equal-channel.
        let d50_to_d65 = bradford(D50_XY, D65_XY);
        let xyz_to_srgb = mat_invert(&srgb_to_xyz_d65).unwrap();
        let cam_to_working = mat_mul(&xyz_to_srgb, &mat_mul(&d50_to_d65, &cam_to_xyz_d50));
        let rgb = mat_vec(&cam_to_working, &neutral);
        assert!(approx(rgb[0], rgb[1], 2e-3), "r vs g: {rgb:?}");
        assert!(approx(rgb[1], rgb[2], 2e-3), "g vs b: {rgb:?}");
    }

    #[test]
    fn white_solve_uses_analog_balance_and_camera_calibration() {
        let selected_xy = (0.38, 0.36);
        let cc = [[1.08, 0.02, 0.0], [0.01, 0.94, 0.01], [0.0, 0.03, 1.04]];
        let analog = [1.1, 0.9, 1.05];
        let mut data = DngData {
            cm1: IDENTITY,
            cm2: None,
            cm3: None,
            cc1: cc,
            cc2: cc,
            cc3: cc,
            fm1: Some(normalize_forward_matrix(IDENTITY).unwrap()),
            fm2: None,
            fm3: None,
            analog_balance: analog,
            camera_neutral: [1.0; 3],
            white_balance_source: "as_shot_neutral",
            used_camera_calibration: true,
            illuminants: vec![CalibrationIlluminant {
                label: "D50".into(),
                xy: D50_XY,
                temp: 5000.0,
                custom_kind: None,
            }],
        };
        data.camera_neutral = normalize_neutral(mat_vec(
            &xyz_to_camera(&data, [1.0, 0.0, 0.0]),
            &xy_to_xyz(selected_xy.0, selected_xy.1),
        ))
        .unwrap();

        let (solved, _, _) = solve_white(&data).unwrap();
        assert!(approx(solved.0, selected_xy.0, 1e-7), "{solved:?}");
        assert!(approx(solved.1, selected_xy.1, 1e-7), "{solved:?}");
    }

    /// Exercises the actual TIFF-tag route on the optional local corpus. The
    /// RAW files are intentionally not distributable, so a source checkout
    /// without them skips rather than weakening the synthetic unit tests.
    #[test]
    fn real_proshot_profile_uses_both_calibrations_and_maps_white() {
        let path = Path::new("raw/raw_old/files_2026-07-27_16-48-01/proshot.dng");
        if !path.exists() {
            eprintln!("skipping real_proshot_profile_uses_both_calibrations_and_maps_white");
            return;
        }
        let raw = rawler::decode_file(path).expect("ProShot DNG decodes");
        let (matrix, report) =
            camera_to_working(&raw, path, WorkingSpace::Srgb).expect("full DNG profile composes");

        assert_eq!(report.illuminant1, "D65");
        assert_eq!(report.illuminant2.as_deref(), Some("A"));
        assert_eq!(report.white_balance_source, "as_shot_neutral");
        assert!(report.used_camera_calibration);
        assert!(report.weight_illuminant1 > 0.0 && report.weight_illuminant1 < 1.0);

        let neutral = normalize_neutral([
            1.0 / raw.wb_coeffs[0] as f64,
            1.0 / raw.wb_coeffs[1] as f64,
            1.0 / raw.wb_coeffs[2] as f64,
        ])
        .unwrap();
        let working = [
            matrix[0][0] as f64 * neutral[0]
                + matrix[0][1] as f64 * neutral[1]
                + matrix[0][2] as f64 * neutral[2],
            matrix[1][0] as f64 * neutral[0]
                + matrix[1][1] as f64 * neutral[1]
                + matrix[1][2] as f64 * neutral[2],
            matrix[2][0] as f64 * neutral[0]
                + matrix[2][1] as f64 * neutral[1]
                + matrix[2][2] as f64 * neutral[2],
        ];
        assert!(approx(working[0], 1.0, 5e-4), "{working:?}");
        assert!(approx(working[1], 1.0, 5e-4), "{working:?}");
        assert!(approx(working[2], 1.0, 5e-4), "{working:?}");
    }

    /// The DCP route on an ARW, which is the case the whole change exists for:
    /// a Sony file carries no DNG calibration tag at all, so before this the
    /// profile's second illuminant could never be reached.
    #[test]
    fn a_standalone_profile_calibrates_a_file_that_has_no_dng_tags_of_its_own() {
        let path = Path::new("raw/arw_better/_DSC1236.ARW");
        let profile = Path::new("profiles/SONY_ILCE-7C.dcp");
        if !path.exists() || !profile.exists() {
            eprintln!("skipping a_standalone_profile_calibrates_a_file_that_has_no_dng_tags");
            return;
        }
        let raw = rawler::decode_file(path).expect("A7C ARW decodes");
        let bytes = std::fs::read(profile).expect("profile reads");

        // The file route finds nothing: an ARW has no DNG ColorMatrix1.
        assert!(
            camera_to_working_any(&raw, path, WorkingSpace::Srgb).is_none(),
            "an ARW must not resolve a DNG profile of its own"
        );

        let (matrix, report) =
            camera_to_working_from_profile(&raw, path, &bytes, WorkingSpace::Srgb)
                .expect("the standalone profile composes");

        // Both of the profile's calibrations are in play, at a weight strictly
        // between them. That interpolation is the thing an ARW never had.
        assert_eq!(report.illuminant1, "A");
        assert_eq!(report.illuminant2.as_deref(), Some("D65"));
        assert!(
            report.weight_illuminant1 > 0.0 && report.weight_illuminant1 < 1.0,
            "weights {:?} are not an interpolation",
            report.interpolation_weights
        );
        assert!(
            report.used_forward_matrix,
            "the profile has a ForwardMatrix"
        );
        // A profile has no picture in it, so the white comes from the RAW.
        assert_eq!(report.white_balance_source, "decoder_white_balance");
        assert!(
            (2000.0..=20000.0).contains(&report.estimated_cct),
            "CCT {} is not a daylight-ish scene",
            report.estimated_cct
        );

        // The scene neutral must land equal-channel in the working space, the
        // same invariant the file route is held to above.
        let neutral = normalize_neutral([
            1.0 / raw.wb_coeffs[0] as f64,
            1.0 / raw.wb_coeffs[1] as f64,
            1.0 / raw.wb_coeffs[2] as f64,
        ])
        .unwrap();
        let working: Vec<f64> = (0..3)
            .map(|row| {
                (0..3)
                    .map(|column| matrix[row][column] as f64 * neutral[column])
                    .sum()
            })
            .collect();
        assert!(approx(working[0], working[1], 5e-4), "{working:?}");
        assert!(approx(working[1], working[2], 5e-4), "{working:?}");
    }
}
