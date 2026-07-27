//! Inspect a RAW file's container and repair data Rawler decodes incorrectly.
//!
//! Rawler 0.7.2 mis-decodes lossless JPEG streams that use restart intervals
//! (see [`crate::ljpeg`]). It does not fail — it returns a smooth ramp — so the
//! only way to avoid silently developing garbage is to detect the case and
//! decode the entropy data ourselves.
//!
//! Detection is narrow on purpose: the raw IFD must be lossless-JPEG
//! compressed *and* its first stream must declare a restart interval. Files
//! Rawler handles correctly are left completely untouched.
//!
//! This module also reads `BaselineExposure`, which Rawler does not expose.
//! Both jobs need the file's TIFF structure, so they share one parse, and chunk
//! bytes are read on demand rather than slurping the file: a 50 MP DNG runs to
//! 140 MB and most files need no repair at all.

use crate::ljpeg;
use anyhow::{Context, Result, bail, ensure};
use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, IFD, Value};
use rawler::{RawImage, RawImageData};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

// TIFF/DNG tag ids.
const TAG_PHOTOMETRIC: u16 = 262;
const TAG_COMPRESSION: u16 = 259;
const TAG_IMAGE_WIDTH: u16 = 256;
const TAG_IMAGE_LENGTH: u16 = 257;
const TAG_STRIP_OFFSETS: u16 = 273;
const TAG_ROWS_PER_STRIP: u16 = 278;
const TAG_STRIP_BYTE_COUNTS: u16 = 279;
const TAG_TILE_WIDTH: u16 = 322;
const TAG_TILE_LENGTH: u16 = 323;
const TAG_TILE_OFFSETS: u16 = 324;
const TAG_TILE_BYTE_COUNTS: u16 = 325;
const TAG_SUB_IFDS: u16 = 330;
const TAG_BASELINE_EXPOSURE: u16 = 50730;

const PHOTOMETRIC_CFA: u32 = 32803;
const PHOTOMETRIC_LINEAR_RAW: u32 = 34892;
/// TIFF compression 7: "modern" JPEG, which for a raw IFD means lossless JPEG.
const COMPRESSION_JPEG: u32 = 7;

/// What inspecting a file's container told us.
#[derive(Debug, Clone, Default)]
pub struct FileReport {
    /// Corrections applied, for the sidecar and the console.
    pub notes: Vec<String>,
    /// DNG `BaselineExposure` in EV, or 0.0 when the file records none.
    pub baseline_exposure_ev: f32,
}

/// Where one compressed chunk belongs in the destination sample buffer.
struct Chunk {
    offset: u64,
    length: usize,
    row_origin: usize,
    sample_origin: usize,
}

fn entry_usize(ifd: &IFD, tag: u16, index: usize) -> Option<usize> {
    ifd.get_entry(tag)
        .map(|entry| entry.value.force_usize(index))
}

/// Find the IFD holding the raw image data.
fn raw_ifd(tiff: &GenericTiffReader) -> Option<&IFD> {
    tiff.root_ifd()
        .find_ifds_with_tag(TAG_PHOTOMETRIC)
        .into_iter()
        .find(|ifd| {
            let photometric = ifd
                .get_entry(TAG_PHOTOMETRIC)
                .map(|entry| entry.value.force_u32(0))
                .unwrap_or(0);
            matches!(photometric, PHOTOMETRIC_CFA | PHOTOMETRIC_LINEAR_RAW)
        })
}

/// Read `BaselineExposure`, a signed rational in EV.
///
/// The DNG spec defines it as an offset applied to the scene-linear data before
/// the default rendering; Samsung's Expert RAW records +3 EV, and without it
/// those files develop several stops too dark.
fn baseline_exposure(tiff: &GenericTiffReader, raw: Option<&IFD>) -> f32 {
    let read = |ifd: &IFD| -> Option<f32> {
        match &ifd.get_entry(TAG_BASELINE_EXPOSURE)?.value {
            Value::SRational(values) => {
                let value = values.first()?;
                if value.d == 0 {
                    return None;
                }
                Some(value.n as f32 / value.d as f32)
            }
            _ => None,
        }
    };

    raw.and_then(read)
        .or_else(|| read(tiff.root_ifd()))
        .filter(|value| value.is_finite() && value.abs() <= 8.0)
        .unwrap_or(0.0)
}

/// Build the chunk list for a stripped or tiled raw IFD.
fn chunks(ifd: &IFD, samples_per_pixel: usize) -> Result<Vec<Chunk>> {
    let image_length =
        entry_usize(ifd, TAG_IMAGE_LENGTH, 0).context("raw IFD has no ImageLength")?;

    if let (Some(offsets), Some(counts)) = (
        ifd.get_entry(TAG_TILE_OFFSETS),
        ifd.get_entry(TAG_TILE_BYTE_COUNTS),
    ) {
        let image_width =
            entry_usize(ifd, TAG_IMAGE_WIDTH, 0).context("raw IFD has no ImageWidth")?;
        let tile_width =
            entry_usize(ifd, TAG_TILE_WIDTH, 0).context("tiled raw IFD has no TileWidth")?;
        let tile_length =
            entry_usize(ifd, TAG_TILE_LENGTH, 0).context("tiled raw IFD has no TileLength")?;
        ensure!(
            tile_width > 0 && tile_length > 0,
            "raw IFD declares a zero-sized tile"
        );

        let across = image_width.div_ceil(tile_width);
        let down = image_length.div_ceil(tile_length);
        let count = offsets.count() as usize;
        ensure!(
            count == across * down,
            "raw IFD declares {count} tiles but its geometry needs {}",
            across * down
        );

        return Ok((0..count)
            .map(|index| Chunk {
                offset: offsets.value.force_usize(index) as u64,
                length: counts.value.force_usize(index),
                row_origin: (index / across) * tile_length,
                sample_origin: (index % across) * tile_width * samples_per_pixel,
            })
            .collect());
    }

    let offsets = ifd
        .get_entry(TAG_STRIP_OFFSETS)
        .context("raw IFD has neither tile nor strip offsets")?;
    let counts = ifd
        .get_entry(TAG_STRIP_BYTE_COUNTS)
        .context("raw IFD has strip offsets but no byte counts")?;
    let rows_per_strip = entry_usize(ifd, TAG_ROWS_PER_STRIP, 0).unwrap_or(image_length);
    ensure!(rows_per_strip > 0, "raw IFD declares zero rows per strip");

    let count = offsets.count() as usize;
    Ok((0..count)
        .map(|index| Chunk {
            offset: offsets.value.force_usize(index) as u64,
            length: counts.value.force_usize(index),
            row_origin: index * rows_per_strip,
            sample_origin: 0,
        })
        .collect())
}

/// Read a byte span. Shared with `preview`, which locates JPEG previews the
/// same way this module locates raw strips.
pub(crate) fn read_range(
    reader: &mut BufReader<File>,
    offset: u64,
    length: usize,
) -> Result<Vec<u8>> {
    reader.seek(SeekFrom::Start(offset))?;
    let mut buffer = vec![0u8; length];
    reader.read_exact(&mut buffer)?;
    Ok(buffer)
}

/// Inspect `path`'s container, repairing `raw`'s samples if Rawler got them wrong.
pub fn inspect_and_repair(path: &Path, raw: &mut RawImage) -> Result<FileReport> {
    let mut report = FileReport::default();

    let Ok(file) = File::open(path) else {
        // Inspection is best effort: never fail a file we could otherwise develop.
        return Ok(report);
    };
    let mut reader = BufReader::new(file);

    let Ok(tiff) = GenericTiffReader::new(&mut reader, 0, 0, None, &[TAG_SUB_IFDS]) else {
        return Ok(report);
    };

    let ifd = raw_ifd(&tiff);
    report.baseline_exposure_ev = baseline_exposure(&tiff, ifd);

    let Some(ifd) = ifd else {
        return Ok(report);
    };
    let compression = ifd
        .get_entry(TAG_COMPRESSION)
        .map(|entry| entry.value.force_u32(0))
        .unwrap_or(0);
    if compression != COMPRESSION_JPEG {
        return Ok(report);
    }

    let chunks = chunks(ifd, raw.cpp)?;
    let Some(first) = chunks.first() else {
        return Ok(report);
    };

    // Only the marker segments matter for the decision, so read a bounded
    // prefix rather than the whole (possibly 100 MB+) chunk.
    let probe_length = first.length.min(4096);
    let Ok(header) = read_range(&mut reader, first.offset, probe_length) else {
        return Ok(report);
    };
    if !ljpeg::uses_restart_intervals(&header) {
        // Rawler decodes restart-free lossless JPEG correctly; leave it alone.
        return Ok(report);
    }

    let stride = raw
        .width
        .checked_mul(raw.cpp)
        .context("raw image dimensions overflow")?;
    let total = stride
        .checked_mul(raw.height)
        .context("raw image dimensions overflow")?;
    ensure!(total > 0, "raw image has no samples to decode");

    let mut samples = vec![0u16; total];
    let mut restart_interval = 0usize;

    for (index, chunk) in chunks.iter().enumerate() {
        let bytes = read_range(&mut reader, chunk.offset, chunk.length)
            .with_context(|| format!("failed to read lossless JPEG chunk {index}"))?;

        let info = ljpeg::decode_into(
            &bytes,
            &mut samples,
            stride,
            chunk.row_origin,
            chunk.sample_origin,
        )
        .with_context(|| format!("failed to decode lossless JPEG chunk {index}"))?;

        ensure!(
            info.components == raw.cpp || chunks.len() > 1 || info.samples_per_line() == stride,
            "lossless JPEG chunk {index} yields {} samples per line, but the image needs {stride}",
            info.samples_per_line()
        );
        restart_interval = info.restart_interval;
    }

    report.notes.push(format!(
        "re-decoded lossless JPEG with restart interval {restart_interval} ({} chunk(s)); \
         Rawler 0.7.2 ignores restart markers and would have produced a corrupt image",
        chunks.len()
    ));
    raw.data = RawImageData::Integer(samples);

    Ok(report)
}

/// Report why a raw image cannot be developed, for callers that want to fail
/// early rather than render nonsense.
pub fn ensure_developable(raw: &RawImage) -> Result<()> {
    if let RawImageData::Integer(samples) = &raw.data
        && samples.is_empty()
    {
        bail!("decoded raw image contains no samples");
    }
    Ok(())
}
