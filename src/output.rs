use crate::noiseprofile::NoiseProfile;
use crate::tone::Rgb16Image;
use crate::types::{BatchSummary, OutputFormat, Sidecar};
use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ExtendedColorType, ImageFormat};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

pub fn save_image(
    path: &Path,
    image: Rgb16Image,
    format: OutputFormat,
    jpeg_quality: u8,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory {}", parent.display()))?;
    }

    match format {
        OutputFormat::Tiff => DynamicImage::ImageRgb16(image)
            .save_with_format(path, ImageFormat::Tiff)
            .with_context(|| format!("failed to write TIFF {}", path.display()))?,
        OutputFormat::Png => DynamicImage::ImageRgb16(image)
            .save_with_format(path, ImageFormat::Png)
            .with_context(|| format!("failed to write PNG {}", path.display()))?,
        OutputFormat::Jpeg => {
            let width = image.width();
            let height = image.height();
            let rgb8: Vec<u8> = image
                .into_raw()
                .into_iter()
                .map(|value| ((value as u32 + 128) / 257) as u8)
                .collect();

            let file = File::create(path)
                .with_context(|| format!("failed to create JPEG {}", path.display()))?;
            let writer = BufWriter::new(file);
            let mut encoder = JpegEncoder::new_with_quality(writer, jpeg_quality);
            encoder
                .encode(&rgb8, width, height, ExtendedColorType::Rgb8)
                .with_context(|| format!("failed to encode JPEG {}", path.display()))?;
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
