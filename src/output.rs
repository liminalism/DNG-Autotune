use crate::metadata::SourceMetadata;
use crate::noiseprofile::NoiseProfile;
use crate::tone::Rgb16Image;
use crate::types::{BatchSummary, JpegSettings, OutputFormat, Sidecar};
use anyhow::{Context, Result};
use image::{DynamicImage, ImageFormat};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

/// Write the rendered image with no metadata at all.
///
/// Byte-for-byte the pre-0.1.16 writer: the `image`-crate encoders on their
/// default settings, no EXIF, no ICC. Kept as its own entry point so the
/// "features off" arm of the regression gate is a single obvious call rather
/// than an argument someone has to notice.
pub fn save_image(
    path: &Path,
    image: Rgb16Image,
    format: OutputFormat,
    jpeg_quality: u8,
) -> Result<()> {
    save_image_with_metadata(
        path,
        image,
        format,
        &JpegSettings::from_quality(jpeg_quality),
        None,
    )
}

/// Write the rendered image, optionally with EXIF copied from the source RAW and
/// an embedded sRGB ICC profile.
///
/// `None` is identical to [`save_image`]. `Some(_)` adds EXIF and ICC and, for
/// TIFF, switches to the container [`crate::metadata::write_tiff`] builds,
/// because `image`'s TIFF encoder can embed an ICC profile but has no way to
/// write an EXIF IFD.
pub fn save_image_with_metadata(
    path: &Path,
    image: Rgb16Image,
    format: OutputFormat,
    jpeg: &JpegSettings,
    metadata: Option<&SourceMetadata>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory {}", parent.display()))?;
    }

    match (format, metadata) {
        (OutputFormat::Tiff, Some(metadata)) => {
            crate::metadata::write_tiff(path, &image, metadata)?
        }
        (OutputFormat::Tiff, None) => DynamicImage::ImageRgb16(image)
            .save_with_format(path, ImageFormat::Tiff)
            .with_context(|| format!("failed to write TIFF {}", path.display()))?,
        (OutputFormat::Png, Some(metadata)) => {
            crate::metadata::write_png(path, &image, Some(metadata))?
        }
        (OutputFormat::Png, None) => DynamicImage::ImageRgb16(image)
            .save_with_format(path, ImageFormat::Png)
            .with_context(|| format!("failed to write PNG {}", path.display()))?,
        (OutputFormat::Jpeg, metadata) => {
            let width = image.width();
            let height = image.height();
            // Reduction kept here, unchanged, so a JPEG written with metadata has
            // pixel-for-pixel the same entropy-coded data as one without.
            let rgb8: Vec<u8> = image
                .into_raw()
                .into_iter()
                .map(|value| ((value as u32 + 128) / 257) as u8)
                .collect();
            crate::metadata::write_jpeg(path, &rgb8, width, height, jpeg, metadata)?;
        }
    }

    Ok(())
}

pub fn save_sidecar(path: &Path, sidecar: &Sidecar) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory {}", parent.display()))?;
    }

    let file = File::create(path)
        .with_context(|| format!("failed to create sidecar {}", path.display()))?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, sidecar)
        .with_context(|| format!("failed to write sidecar {}", path.display()))
}

pub fn save_summary(path: &Path, summary: &BatchSummary) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create summary directory {}", parent.display()))?;
    }

    let file = File::create(path)
        .with_context(|| format!("failed to create summary {}", path.display()))?;
    serde_json::to_writer_pretty(BufWriter::new(file), summary)
        .with_context(|| format!("failed to write summary {}", path.display()))
}

pub fn save_noise_profile(path: &Path, profile: &NoiseProfile) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create profile directory {}", parent.display()))?;
    }

    let file = File::create(path)
        .with_context(|| format!("failed to create profile {}", path.display()))?;
    serde_json::to_writer_pretty(BufWriter::new(file), profile)
        .with_context(|| format!("failed to write profile {}", path.display()))
}
