//! The camera's own rendering of a capture, read from the file's embedded preview.
//!
//! Every RAW file carries a JPEG (or, occasionally, uncompressed RGB) preview
//! that the camera produced from the same exposure. That is the vendor's opinion
//! about how the scene should look, and for scenes where the controller's
//! "aim the median at middle grey" rule is wrong, it is a far better target.
//!
//! The motivating case is `expertraw.dng`: the controller renders it with its
//! median at -0.28 EV, while Samsung's own preview of the same capture sits at
//! -2.93 EV. It is a night scene, and the controller renders it as day.
//!
//! # Finding the preview without being fooled
//!
//! Scanning the file for `FF D8` markers does not work. Samsung DNGs append a
//! quarter-resolution single-channel gain map after the preview's end-of-image
//! marker, inside the same strip, and a naive scan finds it and reports a flat
//! grey frame. The previews also embed a 512x384 EXIF thumbnail inside their own
//! APP1 segment, so the *first* end-of-image marker in the strip belongs to that
//! thumbnail rather than to the preview. Truncating there destroys the image.
//!
//! So previews are located through the TIFF directory instead, and auxiliary
//! images are rejected by requiring three samples per pixel. The whole declared
//! span is handed to the decoder, which stops at the primary image on its own.

use crate::analyze::luminance;
use crate::redecode::read_range;
use crate::tone::srgb_decode;
use crate::types::MID_GRAY;
use image::ImageFormat;
use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, IFD};
use serde::Serialize;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

// TIFF/EXIF tags.
const TAG_IMAGE_WIDTH: u16 = 256;
const TAG_IMAGE_LENGTH: u16 = 257;
const TAG_COMPRESSION: u16 = 259;
const TAG_PHOTOMETRIC: u16 = 262;
const TAG_STRIP_OFFSETS: u16 = 273;
const TAG_SAMPLES_PER_PIXEL: u16 = 277;
const TAG_STRIP_BYTE_COUNTS: u16 = 279;
const TAG_JPEG_OFFSET: u16 = 513;
const TAG_JPEG_LENGTH: u16 = 514;
const TAG_SUB_IFDS: u16 = 330;

const PHOTOMETRIC_RGB: u32 = 2;
const PHOTOMETRIC_YCBCR: u32 = 6;
const PHOTOMETRIC_CFA: u32 = 32803;
const PHOTOMETRIC_LINEAR_RAW: u32 = 34892;

const COMPRESSION_NONE: u32 = 1;
const COMPRESSION_OLD_JPEG: u32 = 6;
const COMPRESSION_JPEG: u32 = 7;

/// Smallest preview worth measuring, in pixels.
///
/// An absolute floor rather than a fraction of the raw: one test file offers only
/// a 256x191 thumbnail (48 896 pixels), which is 6% of its raw dimensions but is
/// still the only rendering that camera provides. A fractional rule would reject
/// it for no benefit.
const MIN_PREVIEW_PIXELS: usize = 30_000;
/// Largest preview accepted, as a guard against a corrupt directory asking for a
/// huge allocation.
const MAX_PREVIEW_PIXELS: usize = 80_000_000;
/// A preview flatter than this carries no usable exposure information.
const MIN_PREVIEW_RANGE_EV: f32 = 0.5;
/// Reject when this much of the frame is pinned at black or white.
const MAX_DEGENERATE_FRACTION: f32 = 0.60;
/// Pixels sampled when measuring, matching the analyzer's own budget.
const TARGET_SAMPLES: usize = 250_000;

/// Where the preview's bytes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewSource {
    /// `JPEGInterchangeFormat` / `...Length`, as Sony ARW uses.
    JpegInterchange,
    /// A single JPEG strip, as Samsung DNG uses.
    JpegStrip,
    /// Uncompressed RGB8 strips.
    UncompressedStrips,
}

/// The camera's rendering, reduced to the statistics the controller needs.
#[derive(Debug, Clone, Serialize)]
pub struct PreviewOracle {
    pub width: u32,
    pub height: u32,
    pub source: PreviewSource,
    /// Subject brightness in display EV relative to middle grey, measured with
    /// the same centre weighting the analyzer applies to the raw.
    pub subject_display_ev: f32,
    pub p05_display_ev: f32,
    pub p50_display_ev: f32,
    pub p95_display_ev: f32,
    pub sampled_pixels: usize,
}

/// A preview candidate found in the directory, before its bytes are read.
struct Candidate {
    /// Declared dimensions. Sony's preview IFD carries a JPEG offset but no
    /// `ImageWidth`/`ImageLength` at all, so size is only known after decoding.
    width: Option<usize>,
    height: Option<usize>,
    offset: u64,
    length: usize,
    source: PreviewSource,
    /// Every strip, for the uncompressed case.
    strips: Vec<(u64, usize)>,
}

impl Candidate {
    /// Declared pixel count, or 0 when the directory does not say.
    fn area(&self) -> usize {
        match (self.width, self.height) {
            (Some(width), Some(height)) => width.saturating_mul(height),
            _ => 0,
        }
    }
}

fn entry_u32(ifd: &IFD, tag: u16) -> Option<u32> {
    ifd.get_entry(tag).map(|entry| entry.value.force_u32(0))
}

fn entry_usize(ifd: &IFD, tag: u16) -> Option<usize> {
    ifd.get_entry(tag).map(|entry| entry.value.force_usize(0))
}

/// Decide whether an IFD could hold a colour preview.
fn candidate_from(ifd: &IFD) -> Option<Candidate> {
    // Auxiliary images — Samsung's gain and depth maps — are single-channel.
    // This one check rejects every one of them.
    if let Some(samples) = entry_u32(ifd, TAG_SAMPLES_PER_PIXEL)
        && samples != 3
    {
        return None;
    }

    let photometric = entry_u32(ifd, TAG_PHOTOMETRIC);
    let compression = entry_u32(ifd, TAG_COMPRESSION).unwrap_or(0);

    match photometric {
        Some(PHOTOMETRIC_CFA | PHOTOMETRIC_LINEAR_RAW) => return None,
        Some(PHOTOMETRIC_RGB | PHOTOMETRIC_YCBCR) => {}
        // Sony's preview IFD carries no photometric tag at all; it is identified
        // by being JPEG-compressed with an interchange offset.
        None => {
            if !matches!(compression, COMPRESSION_OLD_JPEG | COMPRESSION_JPEG)
                || ifd.get_entry(TAG_JPEG_OFFSET).is_none()
            {
                return None;
            }
        }
        Some(_) => return None,
    }

    let width = entry_usize(ifd, TAG_IMAGE_WIDTH);
    let height = entry_usize(ifd, TAG_IMAGE_LENGTH);

    if let (Some(offset), Some(length)) = (
        entry_usize(ifd, TAG_JPEG_OFFSET),
        entry_usize(ifd, TAG_JPEG_LENGTH),
    ) {
        return Some(Candidate {
            width,
            height,
            offset: offset as u64,
            length,
            source: PreviewSource::JpegInterchange,
            strips: Vec::new(),
        });
    }

    let offsets = ifd.get_entry(TAG_STRIP_OFFSETS)?;
    let counts = ifd.get_entry(TAG_STRIP_BYTE_COUNTS)?;
    let strips: Vec<(u64, usize)> = (0..offsets.count() as usize)
        .map(|index| {
            (
                offsets.value.force_usize(index) as u64,
                counts.value.force_usize(index),
            )
        })
        .collect();
    let first = *strips.first()?;

    let source = match compression {
        COMPRESSION_OLD_JPEG | COMPRESSION_JPEG => PreviewSource::JpegStrip,
        // Assembling raw strips needs the dimensions up front.
        COMPRESSION_NONE if width.is_some() && height.is_some() => {
            PreviewSource::UncompressedStrips
        }
        _ => return None,
    };

    Some(Candidate {
        width,
        height,
        offset: first.0,
        length: first.1,
        source,
        strips,
    })
}

/// Reduce a decoded RGB8 preview to display-EV statistics.
fn measure(rgb: &image::RgbImage) -> Option<PreviewOracle> {
    let (width, height) = (rgb.width() as usize, rgb.height() as usize);
    let total = width.saturating_mul(height);
    if total == 0 {
        return None;
    }

    // Same stride rule the analyzer uses, so the two measurements are directly
    // comparable rather than merely similar.
    let stride = (((total as f64 / TARGET_SAMPLES as f64).sqrt()).ceil() as usize).max(1);

    // Same centre-fifth window as `analyze`. Brightness is rotation-invariant and
    // the window is defined by fractional margins, so orientation can be ignored.
    let centre_x0 = width / 5;
    let centre_x1 = width - centre_x0;
    let centre_y0 = height / 5;
    let centre_y1 = height - centre_y0;

    let mut values = Vec::with_capacity(total / (stride * stride) + 1);
    let mut centre = Vec::new();
    let mut degenerate = 0usize;

    for y in (0..height).step_by(stride) {
        for x in (0..width).step_by(stride) {
            let pixel = rgb.get_pixel(x as u32, y as u32).0;
            if pixel.iter().all(|c| *c == 0) || pixel.iter().all(|c| *c == 255) {
                degenerate += 1;
            }

            let linear = [
                srgb_decode(pixel[0] as f32 / 255.0),
                srgb_decode(pixel[1] as f32 / 255.0),
                srgb_decode(pixel[2] as f32 / 255.0),
            ];
            let y_linear = luminance(linear);
            if !y_linear.is_finite() || y_linear <= 1.0e-8 {
                continue;
            }
            let ev = (y_linear / MID_GRAY).log2();
            if !ev.is_finite() {
                continue;
            }

            values.push(ev);
            if x >= centre_x0 && x < centre_x1 && y >= centre_y0 && y < centre_y1 {
                centre.push(ev);
            }
        }
    }

    if values.len() < 64 {
        return None;
    }
    if degenerate as f32 / values.len() as f32 > MAX_DEGENERATE_FRACTION {
        return None;
    }

    values.sort_unstable_by(f32::total_cmp);
    centre.sort_unstable_by(f32::total_cmp);

    let quantile = |sorted: &[f32], q: f32| -> f32 {
        let position = q * (sorted.len() - 1) as f32;
        let lower = position.floor() as usize;
        let upper = position.ceil() as usize;
        let fraction = position - lower as f32;
        sorted[lower] * (1.0 - fraction) + sorted[upper] * fraction
    };

    let p05 = quantile(&values, 0.05);
    let p50 = quantile(&values, 0.50);
    let p95 = quantile(&values, 0.95);
    if p95 - p05 < MIN_PREVIEW_RANGE_EV {
        return None;
    }

    let centre_median = if centre.is_empty() {
        p50
    } else {
        quantile(&centre, 0.50)
    };

    // The controller compares `target - subject_ev`, so the oracle has to be
    // measured with the *same* estimator. A plain median against a centre-weighted
    // subject would systematically double-count a centred bright subject.
    Some(PreviewOracle {
        width: rgb.width(),
        height: rgb.height(),
        source: PreviewSource::JpegStrip, // replaced by the caller
        subject_display_ev: 0.60 * centre_median + 0.40 * p50,
        p05_display_ev: p05,
        p50_display_ev: p50,
        p95_display_ev: p95,
        sampled_pixels: values.len(),
    })
}

/// Read and measure the embedded preview, or `None` when there is not a usable one.
///
/// Best effort throughout: a file that develops must never fail because its
/// preview is missing, auxiliary, or corrupt.
pub fn read(path: &Path) -> Option<PreviewOracle> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let tiff = GenericTiffReader::new(&mut reader, 0, 0, None, &[TAG_SUB_IFDS]).ok()?;

    // The `TiffReader` trait method, not `root_ifd().find_ifds_with_tag(...)`.
    // `root_ifd()` is only the first IFD of the chain, which misses both the
    // chained IFD1 and Sony's preview IFD.
    let mut candidates: Vec<Candidate> = tiff
        .find_ifds_with_filter(|ifd| candidate_from(ifd).is_some())
        .into_iter()
        .filter_map(candidate_from)
        .filter(|candidate| {
            // Undeclared size (area 0) is checked after decoding instead.
            let declared_ok = candidate.area() == 0
                || (MIN_PREVIEW_PIXELS..=MAX_PREVIEW_PIXELS).contains(&candidate.area());
            declared_ok && candidate.length > 0
        })
        .collect();

    // Sort explicitly. `find_ifds_with_filter` walks `sub_ifds()`, a `HashMap`
    // whose iteration order varies per process, so relying on the order it
    // returns would make output non-deterministic.
    //
    // Declared area first; where it is unknown (Sony), compressed byte length
    // separates the preview from the thumbnail decisively — 706 KB against 7 KB.
    candidates.sort_by(|a, b| {
        b.area()
            .cmp(&a.area())
            .then(b.length.cmp(&a.length))
            .then(a.offset.cmp(&b.offset))
    });

    let candidate = candidates.first()?;

    let rgb = match candidate.source {
        PreviewSource::UncompressedStrips => {
            let (width, height) = (candidate.width?, candidate.height?);
            let expected = width.checked_mul(height)?.checked_mul(3)?;
            let mut bytes = Vec::with_capacity(expected);
            for (offset, length) in &candidate.strips {
                bytes.extend_from_slice(&read_range(&mut reader, *offset, *length).ok()?);
            }
            bytes.truncate(expected);
            if bytes.len() != expected {
                return None;
            }
            image::RgbImage::from_raw(width as u32, height as u32, bytes)?
        }
        _ => {
            // Hand over the whole declared span. A conforming decoder stops at
            // the primary image and ignores anything appended after it.
            let bytes = read_range(&mut reader, candidate.offset, candidate.length).ok()?;
            let decoded = image::load_from_memory_with_format(&bytes, ImageFormat::Jpeg).ok()?;
            if decoded.color().channel_count() < 3 {
                return None;
            }
            decoded.into_rgb8()
        }
    };

    // Where the directory declared a size, the decode must match it: a mismatch
    // means the bytes were not the image the directory described.
    if let (Some(width), Some(height)) = (candidate.width, candidate.height)
        && (rgb.width() as usize != width || rgb.height() as usize != height)
    {
        return None;
    }

    // Undeclared sizes are bounds-checked here instead.
    let decoded_area = (rgb.width() as usize).saturating_mul(rgb.height() as usize);
    if !(MIN_PREVIEW_PIXELS..=MAX_PREVIEW_PIXELS).contains(&decoded_area) {
        return None;
    }

    let mut oracle = measure(&rgb)?;
    oracle.source = candidate.source;
    Some(oracle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn flat(value: u8, width: u32, height: u32) -> RgbImage {
        RgbImage::from_pixel(width, height, Rgb([value, value, value]))
    }

    /// A preview whose subject sits at middle grey must report 0 EV, so the
    /// oracle agrees with the controller when the vendor agrees with it.
    #[test]
    fn middle_grey_preview_measures_zero_ev() {
        // sRGB encoding of linear 0.18.
        let encoded = (crate::tone::srgb_encode(MID_GRAY) * 255.0).round() as u8;
        let mut image = flat(encoded, 256, 256);
        // Add range so the degenerate-flatness guard does not reject it.
        for x in 0..40u32 {
            for y in 0..256u32 {
                image.put_pixel(x, y, Rgb([250, 250, 250]));
                image.put_pixel(255 - x, y, Rgb([8, 8, 8]));
            }
        }
        let oracle = measure(&image).unwrap();
        assert!(
            oracle.subject_display_ev.abs() < 0.05,
            "middle grey measured {} EV",
            oracle.subject_display_ev
        );
    }

    /// A darker vendor rendering must read as a lower EV; this is the whole
    /// signal the feature acts on.
    #[test]
    fn darker_previews_measure_lower() {
        let build = |level: u8| {
            let mut image = flat(level, 128, 128);
            for x in 0..20u32 {
                for y in 0..128u32 {
                    image.put_pixel(x, y, Rgb([level.saturating_add(60); 3]));
                    image.put_pixel(127 - x, y, Rgb([level.saturating_sub(40); 3]));
                }
            }
            measure(&image).unwrap().subject_display_ev
        };
        assert!(build(60) < build(190));
    }

    #[test]
    fn a_flat_preview_is_rejected() {
        assert!(measure(&flat(128, 128, 128)).is_none());
    }

    #[test]
    fn an_all_black_preview_is_rejected() {
        assert!(measure(&flat(0, 128, 128)).is_none());
    }

    #[test]
    fn missing_files_do_not_panic() {
        assert!(read(Path::new("no-such-file.ARW")).is_none());
    }
}
