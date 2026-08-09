//! Batch RAW developer with a deterministic automatic tone controller.
//!
//! The crate is exposed as a library so the `probe-*` examples exercise the
//! same decode path the binary does. That matters: several supported files are
//! only decoded correctly because of the repairs in [`redecode`], and a debug
//! tool that called Rawler directly would report on data the program never
//! actually uses.

pub mod analyze;
pub mod api;
pub mod chroma;
pub mod cli;
pub mod color;
pub mod demosaic;
pub mod dngcolor;
pub mod files;
pub mod highlight;
pub mod hotpixels;
pub mod lens;
pub mod levels;
pub mod ljpeg;
pub mod localtone;
pub mod memory;
pub mod metadata;
pub mod metrics;
pub mod noise;
pub mod noiseprofile;
pub mod oklab;
pub mod orientation;
pub mod output;
pub mod pipeline;
pub mod preview;
pub mod raw_highlight;
pub mod redecode;
pub mod reference;
pub mod rescale;
pub mod sharpen;
pub mod shotinfo;
pub mod synthetic;
pub mod tone;
pub mod types;
pub mod whitebalance;

use anyhow::Result;
use rawler::RawImage;
use std::path::Path;

/// Apply every correction the develop path depends on to an already-decoded image.
///
/// Kept separate from [`decode_corrected`] so the renderer can record the file's
/// original metadata before anything is reconciled, without decoding twice.
pub fn apply_corrections(path: &Path, raw: &mut RawImage) -> Result<redecode::FileReport> {
    let mut report = redecode::inspect_and_repair(path, raw)?;
    redecode::ensure_developable(raw)?;
    report.notes.extend(levels::prepare_levels(raw)?);
    Ok(report)
}

/// Decode a RAW file and apply every correction the develop path depends on.
///
/// This is what the debug examples use, so that what a probe reports is what
/// the renderer sees.
pub fn decode_corrected(path: &Path) -> Result<(RawImage, redecode::FileReport)> {
    let mut raw = rawler::decode_file(path)?;
    let report = apply_corrections(path, &mut raw)?;
    Ok((raw, report))
}
