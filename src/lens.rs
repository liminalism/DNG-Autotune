//! Metadata-driven lens correction.
//!
//! DNG 1.3 added `OpcodeList3`, a big-endian list of operations that run just
//! after demosaic. This module implements the two operations that carry the
//! ordinary photographic lens model:
//!
//! - `WarpRectilinear` (opcode 1): radial distortion, tangential distortion,
//!   and per-plane lateral chromatic aberration.
//! - `FixVignetteRadial` (opcode 3): a radial gain polynomial.
//!
//! No make/model guesses are made. A file without a usable opcode list is left
//! untouched, which is the only safe general policy for interchangeable lenses
//! and phones whose computational camera may already have corrected the raw.

use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{Entry, GenericTiffReader, Value};
use rayon::prelude::*;
use serde::Serialize;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

const TAG_OPCODE_LIST_3: u16 = 51022;
const OPCODE_WARP_RECTILINEAR: u32 = 1;
const OPCODE_FIX_VIGNETTE_RADIAL: u32 = 3;
const MAX_SUPPORTED_DNG_VERSION: u32 = 0x0107_0100;
const FLAG_OPTIONAL: u32 = 1;
const MAX_REASONABLE_GAIN: f64 = 16.0;

#[derive(Debug, Clone)]
struct WarpRectilinear {
    coefficients: Vec<[f64; 6]>,
    center: [f64; 2],
}

#[derive(Debug, Clone)]
struct FixVignetteRadial {
    coefficients: [f64; 5],
    center: [f64; 2],
}

#[derive(Debug, Clone)]
enum Operation {
    Warp(WarpRectilinear),
    Vignette(FixVignetteRadial),
}

/// Coordinate system of a developed buffer within the raw IFD's image bounds.
///
/// Bayer development normally produces only `ActiveArea`. Keeping its origin
/// here lets the DNG polynomial remain in the full raw image's coordinates,
/// then `DefaultCrop` can be applied after the opcode as the specification
/// requires.
#[derive(Debug, Clone, Copy)]
pub struct ImageGeometry {
    pub full_width: usize,
    pub full_height: usize,
    pub origin_x: usize,
    pub origin_y: usize,
}

/// Machine-readable account of a DNG lens correction.
#[derive(Debug, Clone, Serialize)]
pub struct LensCorrectionReport {
    pub source: &'static str,
    pub opcodes_declared: usize,
    pub opcodes_applied: usize,
    pub rectilinear_warps: usize,
    pub radial_vignette_corrections: usize,
    pub optional_opcodes_skipped: usize,
    pub required_opcodes_unsupported: usize,
    pub clipped_channels: usize,
    pub max_displacement_pixels: f32,
    pub max_gain: f32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Parsed `OpcodeList3` plus its accumulating report.
pub struct LensCorrection {
    operations: Vec<Operation>,
    report: LensCorrectionReport,
}

impl LensCorrection {
    /// Read a DNG `OpcodeList3`. Returns `None` when the file carries no list.
    ///
    /// Malformed or unsupported required opcodes return a report-only value:
    /// `apply_*` then leaves the pixels untouched while the sidecar explains
    /// why trusting only part of the list would have been unsafe.
    pub fn read(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let mut reader = BufReader::new(file);
        let tiff = GenericTiffReader::new(&mut reader, 0, 0, None, &[]).ok()?;
        let entry = find_entry(&tiff, TAG_OPCODE_LIST_3)?;
        let bytes = undefined_bytes(entry)?;
        Some(parse(bytes))
    }

    pub fn apply_three(
        &mut self,
        pixels: &mut Vec<[f32; 3]>,
        width: usize,
        height: usize,
        geometry: ImageGeometry,
    ) {
        if !self.can_apply() || pixels.len() != width.saturating_mul(height) {
            return;
        }
        for operation in &self.operations {
            match operation {
                Operation::Warp(warp)
                    if warp.coefficients.len() == 1 || warp.coefficients.len() == 3 =>
                {
                    apply_warp::<3>(pixels, width, height, geometry, warp, &mut self.report);
                }
                Operation::Warp(warp) => self.report.notes.push(format!(
                    "WarpRectilinear has {} coefficient sets for a three-plane image; skipped",
                    warp.coefficients.len()
                )),
                Operation::Vignette(vignette) => {
                    apply_vignette::<3>(
                        pixels,
                        width,
                        height,
                        geometry,
                        vignette,
                        &mut self.report,
                    );
                }
            }
        }
    }

    pub fn apply_four(
        &mut self,
        pixels: &mut Vec<[f32; 4]>,
        width: usize,
        height: usize,
        geometry: ImageGeometry,
    ) {
        if !self.can_apply() || pixels.len() != width.saturating_mul(height) {
            return;
        }
        for operation in &self.operations {
            match operation {
                Operation::Warp(warp)
                    if warp.coefficients.len() == 1 || warp.coefficients.len() == 4 =>
                {
                    apply_warp::<4>(pixels, width, height, geometry, warp, &mut self.report);
                }
                Operation::Warp(warp) => self.report.notes.push(format!(
                    "WarpRectilinear has {} coefficient sets for a four-plane image; skipped",
                    warp.coefficients.len()
                )),
                Operation::Vignette(vignette) => {
                    apply_vignette::<4>(
                        pixels,
                        width,
                        height,
                        geometry,
                        vignette,
                        &mut self.report,
                    );
                }
            }
        }
    }

    pub fn into_report(self) -> LensCorrectionReport {
        self.report
    }

    fn can_apply(&self) -> bool {
        self.report.required_opcodes_unsupported == 0
    }
}

fn find_entry(tiff: &GenericTiffReader, tag: u16) -> Option<&Entry> {
    if let Some(entry) = tiff.get_entry(tag) {
        return Some(entry);
    }
    tiff.find_ifds_with_tag(tag)
        .into_iter()
        .min_by_key(|ifd| ifd.offset)
        .and_then(|ifd| ifd.get_entry(tag))
}

fn undefined_bytes(entry: &Entry) -> Option<&[u8]> {
    match &entry.value {
        Value::Undefined(bytes) | Value::Byte(bytes) => Some(bytes),
        _ => None,
    }
}

fn parse(bytes: &[u8]) -> LensCorrection {
    let mut report = LensCorrectionReport {
        source: "dng_opcode_list3",
        opcodes_declared: 0,
        opcodes_applied: 0,
        rectilinear_warps: 0,
        radial_vignette_corrections: 0,
        optional_opcodes_skipped: 0,
        required_opcodes_unsupported: 0,
        clipped_channels: 0,
        max_displacement_pixels: 0.0,
        max_gain: 1.0,
        notes: Vec::new(),
    };
    let mut cursor = Cursor::new(bytes);
    let Some(count) = cursor.u32() else {
        report.required_opcodes_unsupported = 1;
        report
            .notes
            .push("truncated opcode-list header".to_string());
        return LensCorrection {
            operations: Vec::new(),
            report,
        };
    };
    report.opcodes_declared = count as usize;
    let mut operations = Vec::new();

    for index in 0..count {
        let Some(id) = cursor.u32() else {
            malformed(&mut report, index, "truncated opcode ID");
            break;
        };
        let Some(version) = cursor.u32() else {
            malformed(&mut report, index, "truncated opcode version");
            break;
        };
        let Some(flags) = cursor.u32() else {
            malformed(&mut report, index, "truncated opcode flags");
            break;
        };
        let Some(parameter_bytes) = cursor.u32() else {
            malformed(&mut report, index, "truncated parameter length");
            break;
        };
        let Some(parameters) = cursor.take(parameter_bytes as usize) else {
            malformed(&mut report, index, "truncated parameter area");
            break;
        };
        let optional = flags & FLAG_OPTIONAL != 0;

        if version > MAX_SUPPORTED_DNG_VERSION {
            unsupported(
                &mut report,
                optional,
                format!("opcode {id} requires newer DNG version {version:#010x}"),
            );
            continue;
        }

        let operation = match id {
            OPCODE_WARP_RECTILINEAR => parse_warp(parameters).map(Operation::Warp),
            OPCODE_FIX_VIGNETTE_RADIAL => parse_vignette(parameters).map(Operation::Vignette),
            _ => {
                unsupported(
                    &mut report,
                    optional,
                    format!("opcode {id} is not implemented"),
                );
                continue;
            }
        };

        match operation {
            Some(operation) => operations.push(operation),
            None => unsupported(
                &mut report,
                optional,
                format!("opcode {id} has invalid parameters"),
            ),
        }
    }

    if cursor.remaining() != 0 {
        report.notes.push(format!(
            "{} trailing byte(s) after the declared opcode list",
            cursor.remaining()
        ));
    }
    if report.required_opcodes_unsupported != 0 {
        operations.clear();
        report
            .notes
            .push("entire lens correction skipped because a required opcode was unusable".into());
    }
    LensCorrection { operations, report }
}

fn malformed(report: &mut LensCorrectionReport, index: u32, message: &str) {
    report.required_opcodes_unsupported += 1;
    report
        .notes
        .push(format!("opcode {}: {message}", index + 1));
}

fn unsupported(report: &mut LensCorrectionReport, optional: bool, note: String) {
    if optional {
        report.optional_opcodes_skipped += 1;
        report.notes.push(format!("optional {note}; skipped"));
    } else {
        report.required_opcodes_unsupported += 1;
        report.notes.push(format!("required {note}"));
    }
}

fn parse_warp(bytes: &[u8]) -> Option<WarpRectilinear> {
    let mut cursor = Cursor::new(bytes);
    let count = cursor.u32()? as usize;
    if count == 0 || count > 4 || bytes.len() != 4 + count * 48 + 16 {
        return None;
    }
    let mut coefficients = Vec::with_capacity(count);
    for _ in 0..count {
        let set = [
            cursor.f64()?,
            cursor.f64()?,
            cursor.f64()?,
            cursor.f64()?,
            cursor.f64()?,
            cursor.f64()?,
        ];
        if !set.iter().all(|value| value.is_finite()) || !radial_is_monotonic(&set) {
            return None;
        }
        coefficients.push(set);
    }
    let center = [cursor.f64()?, cursor.f64()?];
    if !valid_center(center) || cursor.remaining() != 0 {
        return None;
    }
    Some(WarpRectilinear {
        coefficients,
        center,
    })
}

fn parse_vignette(bytes: &[u8]) -> Option<FixVignetteRadial> {
    if bytes.len() != 56 {
        return None;
    }
    let mut cursor = Cursor::new(bytes);
    let coefficients = [
        cursor.f64()?,
        cursor.f64()?,
        cursor.f64()?,
        cursor.f64()?,
        cursor.f64()?,
    ];
    let center = [cursor.f64()?, cursor.f64()?];
    if !coefficients.iter().all(|value| value.is_finite())
        || !valid_center(center)
        || cursor.remaining() != 0
    {
        return None;
    }
    for index in 0..=256 {
        let r2 = (index as f64 / 256.0).powi(2);
        let gain = vignette_gain(coefficients, r2);
        if !gain.is_finite() || !(0.0..=MAX_REASONABLE_GAIN).contains(&gain) {
            return None;
        }
    }
    Some(FixVignetteRadial {
        coefficients,
        center,
    })
}

fn valid_center(center: [f64; 2]) -> bool {
    center
        .iter()
        .all(|value| value.is_finite() && (-1.0..=2.0).contains(value))
}

fn radial_is_monotonic(set: &[f64; 6]) -> bool {
    (0..=256).all(|index| {
        let r2 = (index as f64 / 256.0).powi(2);
        let derivative =
            set[0] + 3.0 * set[1] * r2 + 5.0 * set[2] * r2.powi(2) + 7.0 * set[3] * r2.powi(3);
        derivative.is_finite() && derivative > 0.0
    })
}

fn apply_warp<const C: usize>(
    pixels: &mut Vec<[f32; C]>,
    width: usize,
    height: usize,
    geometry: ImageGeometry,
    warp: &WarpRectilinear,
    report: &mut LensCorrectionReport,
) {
    let Some(frame) = Frame::new(geometry).map(|frame| frame.with_center(warp.center, geometry))
    else {
        report
            .notes
            .push("invalid image geometry for WarpRectilinear; skipped".into());
        return;
    };
    let source = pixels.as_slice();
    let max_displacement = std::sync::atomic::AtomicU32::new(0);
    let clipped = std::sync::atomic::AtomicUsize::new(0);
    let output: Vec<[f32; C]> = (0..source.len())
        .into_par_iter()
        .map(|index| {
            let x = index % width;
            let y = index / width;
            let global_x = (geometry.origin_x + x) as f64;
            let global_y = (geometry.origin_y + y) as f64;
            let mut out = [0.0; C];
            for (channel, output_channel) in out.iter_mut().enumerate() {
                let set = if warp.coefficients.len() == 1 {
                    warp.coefficients[0]
                } else {
                    warp.coefficients[channel]
                };
                let (source_x, source_y) = warp_point(global_x, global_y, frame, set);
                atomic_max_f32(
                    &max_displacement,
                    ((source_x - global_x).powi(2) + (source_y - global_y).powi(2)).sqrt() as f32,
                );
                let local_x = source_x - geometry.origin_x as f64;
                let local_y = source_y - geometry.origin_y as f64;
                let value = bicubic_channel(source, width, height, local_x, local_y, channel);
                let bounded = value.clamp(0.0, 1.0);
                if bounded.to_bits() != value.to_bits() {
                    clipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                *output_channel = bounded;
            }
            out
        })
        .collect();
    *pixels = output;
    report.opcodes_applied += 1;
    report.rectilinear_warps += 1;
    report.clipped_channels += clipped.load(std::sync::atomic::Ordering::Relaxed);
    report.max_displacement_pixels = report.max_displacement_pixels.max(f32::from_bits(
        max_displacement.load(std::sync::atomic::Ordering::Relaxed),
    ));
}

fn apply_vignette<const C: usize>(
    pixels: &mut [[f32; C]],
    width: usize,
    _height: usize,
    geometry: ImageGeometry,
    vignette: &FixVignetteRadial,
    report: &mut LensCorrectionReport,
) {
    let Some(frame) =
        Frame::new(geometry).map(|frame| frame.with_center(vignette.center, geometry))
    else {
        report
            .notes
            .push("invalid image geometry for FixVignetteRadial; skipped".into());
        return;
    };
    let clipped = std::sync::atomic::AtomicUsize::new(0);
    let max_gain = std::sync::atomic::AtomicU32::new(1.0_f32.to_bits());
    pixels
        .par_iter_mut()
        .enumerate()
        .for_each(|(index, pixel)| {
            let x = (geometry.origin_x + index % width) as f64;
            let y = (geometry.origin_y + index / width) as f64;
            let dx = (x - frame.cx) / frame.m;
            let dy = (y - frame.cy) / frame.m;
            let gain = vignette_gain(vignette.coefficients, dx * dx + dy * dy);
            atomic_max_f32(&max_gain, gain as f32);
            for value in pixel {
                let corrected = *value * gain as f32;
                let bounded = corrected.clamp(0.0, 1.0);
                if bounded.to_bits() != corrected.to_bits() {
                    clipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                *value = bounded;
            }
        });
    report.opcodes_applied += 1;
    report.radial_vignette_corrections += 1;
    report.clipped_channels += clipped.load(std::sync::atomic::Ordering::Relaxed);
    report.max_gain = report.max_gain.max(f32::from_bits(
        max_gain.load(std::sync::atomic::Ordering::Relaxed),
    ));
}

#[derive(Clone, Copy)]
struct Frame {
    cx: f64,
    cy: f64,
    m: f64,
}

impl Frame {
    fn new(geometry: ImageGeometry) -> Option<Self> {
        if geometry.full_width < 2 || geometry.full_height < 2 {
            return None;
        }
        let x1 = (geometry.full_width - 1) as f64;
        let y1 = (geometry.full_height - 1) as f64;
        // The caller supplies the opcode center separately in `with_center`.
        Some(Self {
            cx: x1,
            cy: y1,
            m: 1.0,
        })
    }

    fn with_center(self, center: [f64; 2], geometry: ImageGeometry) -> Self {
        let x1 = (geometry.full_width - 1) as f64;
        let y1 = (geometry.full_height - 1) as f64;
        let cx = center[0] * x1;
        let cy = center[1] * y1;
        let mx = cx.max((x1 - cx).abs());
        let my = cy.max((y1 - cy).abs());
        Self {
            cx,
            cy,
            m: mx.hypot(my).max(f64::EPSILON),
        }
    }
}

fn warp_point(x: f64, y: f64, frame: Frame, set: [f64; 6]) -> (f64, f64) {
    let dx = (x - frame.cx) / frame.m;
    let dy = (y - frame.cy) / frame.m;
    let r2 = dx * dx + dy * dy;
    let radial = set[0] + set[1] * r2 + set[2] * r2.powi(2) + set[3] * r2.powi(3);
    let tx = set[4] * (2.0 * dx * dy) + set[5] * (r2 + 2.0 * dx * dx);
    let ty = set[5] * (2.0 * dx * dy) + set[4] * (r2 + 2.0 * dy * dy);
    (
        frame.cx + frame.m * (radial * dx + tx),
        frame.cy + frame.m * (radial * dy + ty),
    )
}

fn vignette_gain(coefficients: [f64; 5], r2: f64) -> f64 {
    1.0 + r2
        * (coefficients[0]
            + r2 * (coefficients[1]
                + r2 * (coefficients[2] + r2 * (coefficients[3] + r2 * coefficients[4]))))
}

fn bicubic_channel<const C: usize>(
    pixels: &[[f32; C]],
    width: usize,
    height: usize,
    x: f64,
    y: f64,
    channel: usize,
) -> f32 {
    let x0 = x.floor() as isize;
    let y0 = y.floor() as isize;
    let tx = (x - x0 as f64) as f32;
    let ty = (y - y0 as f64) as f32;
    let mut rows = [0.0; 4];
    for (row_index, row) in rows.iter_mut().enumerate() {
        let sy = (y0 + row_index as isize - 1).clamp(0, height as isize - 1) as usize;
        let mut values = [0.0; 4];
        for (column_index, value) in values.iter_mut().enumerate() {
            let sx = (x0 + column_index as isize - 1).clamp(0, width as isize - 1) as usize;
            *value = pixels[sy * width + sx][channel];
        }
        *row = cubic(values, tx);
    }
    cubic(rows, ty)
}

#[inline]
fn cubic(p: [f32; 4], t: f32) -> f32 {
    // Catmull-Rom spline: interpolating, deterministic, and the cubic kernel
    // recommended (without prescribing a particular spline) by DNG 1.7.1.
    0.5 * ((2.0 * p[1])
        + (-p[0] + p[2]) * t
        + (2.0 * p[0] - 5.0 * p[1] + 4.0 * p[2] - p[3]) * t * t
        + (-p[0] + 3.0 * p[1] - 3.0 * p[2] + p[3]) * t * t * t)
}

fn atomic_max_f32(target: &std::sync::atomic::AtomicU32, value: f32) {
    if !value.is_finite() || value < 0.0 {
        return;
    }
    let mut current = target.load(std::sync::atomic::Ordering::Relaxed);
    while value > f32::from_bits(current) {
        match target.compare_exchange_weak(
            current,
            value.to_bits(),
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn u32(&mut self) -> Option<u32> {
        let bytes: [u8; 4] = self.take(4)?.try_into().ok()?;
        Some(u32::from_be_bytes(bytes))
    }

    fn f64(&mut self) -> Option<f64> {
        let bytes: [u8; 8] = self.take(8)?.try_into().ok()?;
        Some(f64::from_be_bytes(bytes))
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(count)?;
        let result = self.bytes.get(self.offset..end)?;
        self.offset = end;
        Some(result)
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend(value.to_be_bytes());
    }

    fn push_f64(bytes: &mut Vec<u8>, value: f64) {
        bytes.extend(value.to_be_bytes());
    }

    fn opcode(id: u32, flags: u32, parameters: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_u32(&mut bytes, id);
        push_u32(&mut bytes, 0x0103_0000);
        push_u32(&mut bytes, flags);
        push_u32(&mut bytes, parameters.len() as u32);
        bytes.extend(parameters);
        bytes
    }

    fn list(opcodes: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_u32(&mut bytes, opcodes.len() as u32);
        for opcode in opcodes {
            bytes.extend(opcode);
        }
        bytes
    }

    fn identity_warp() -> Vec<u8> {
        let mut parameters = Vec::new();
        push_u32(&mut parameters, 1);
        for value in [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5] {
            push_f64(&mut parameters, value);
        }
        opcode(OPCODE_WARP_RECTILINEAR, 0, &parameters)
    }

    #[test]
    fn parses_big_endian_identity_warp() {
        let correction = parse(&list(&[identity_warp()]));
        assert_eq!(correction.operations.len(), 1);
        assert_eq!(correction.report.opcodes_declared, 1);
        assert_eq!(correction.report.required_opcodes_unsupported, 0);
    }

    #[test]
    fn optional_unknown_opcode_is_skipped() {
        let correction = parse(&list(&[opcode(99, FLAG_OPTIONAL, &[]), identity_warp()]));
        assert_eq!(correction.operations.len(), 1);
        assert_eq!(correction.report.optional_opcodes_skipped, 1);
        assert_eq!(correction.report.required_opcodes_unsupported, 0);
    }

    #[test]
    fn required_unknown_opcode_disables_the_whole_list() {
        let correction = parse(&list(&[identity_warp(), opcode(99, 0, &[])]));
        assert!(correction.operations.is_empty());
        assert_eq!(correction.report.required_opcodes_unsupported, 1);
    }

    #[test]
    fn identity_warp_is_pixel_identical() {
        let mut correction = parse(&list(&[identity_warp()]));
        let mut pixels: Vec<[f32; 3]> = (0..36)
            .map(|index| {
                let value = index as f32 / 100.0;
                [value, value * 0.5, value * 0.25]
            })
            .collect();
        let original = pixels.clone();
        correction.apply_three(
            &mut pixels,
            6,
            6,
            ImageGeometry {
                full_width: 6,
                full_height: 6,
                origin_x: 0,
                origin_y: 0,
            },
        );
        assert_eq!(pixels, original);
        assert_eq!(correction.report.rectilinear_warps, 1);
    }

    #[test]
    fn radial_vignette_lifts_corners_and_preserves_center() {
        let mut parameters = Vec::new();
        for value in [1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5] {
            push_f64(&mut parameters, value);
        }
        let mut correction = parse(&list(&[opcode(OPCODE_FIX_VIGNETTE_RADIAL, 0, &parameters)]));
        let mut pixels = vec![[0.25; 3]; 25];
        correction.apply_three(
            &mut pixels,
            5,
            5,
            ImageGeometry {
                full_width: 5,
                full_height: 5,
                origin_x: 0,
                origin_y: 0,
            },
        );
        assert!((pixels[12][0] - 0.25).abs() < 1.0e-6);
        assert!(pixels[0][0] > 0.49);
    }
}
