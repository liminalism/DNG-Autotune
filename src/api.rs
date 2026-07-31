//! In-memory integration API.
//!
//! [`render_file`] runs the same automatic sensor, colour, lens, analysis, tone,
//! and output-sharpening stages as the batch command, but performs no filesystem
//! output. The returned packed byte buffer can be moved directly through a Rust
//! channel or framed by the receiving crate for IPC/network transport.

use crate::color::{ColorReport, DevelopOptions, RawColorPath, WorkingSpace};
use crate::demosaic::DemosaicMethod;
use crate::rescale::SubBlack;
use crate::types::{AnalysisStats, CameraMetadata, GuidanceMode, Preset, RunOptions, ToneParams};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::path::Path;

/// Packed pixel representation returned to the receiving crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    /// Three bytes per pixel in R, G, B order, using the sRGB transfer function.
    Rgb8Srgb,
    /// Six bytes per pixel in R, G, B order. Each channel is an unsigned
    /// big-endian 16-bit value using the sRGB transfer function.
    Rgb16SrgbBe,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb8Srgb => 3,
            Self::Rgb16SrgbBe => 6,
        }
    }
}

/// Automatic render controls useful to an embedding crate.
///
/// [`Default`] is the same `archive-auto-v2` image policy used by the command
/// line. The file/output-management members of [`RunOptions`] are intentionally
/// absent: this API writes nothing.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub pixel_format: PixelFormat,
    pub preset: Preset,
    pub exposure_bias_ev: f32,
    pub max_samples: usize,
    pub local_white_balance: f32,
    pub local_tone: f32,
    /// `None` selects the automatic embedded-preview policy; `Some(0.0)`
    /// disables preview guidance.
    pub preview_exposure: Option<f32>,
    pub raw_color_path: RawColorPath,
    pub working_space: WorkingSpace,
    pub sub_black: SubBlack,
    pub saturation_scale: f32,
    pub chroma_denoise: f32,
    pub sharpen: f32,
    pub demosaic: DemosaicMethod,
    pub hot_pixels: f32,
    pub highlight_reconstruction: f32,
    pub full_dng_color: bool,
    pub lens_correction: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self::automatic()
    }
}

impl RenderOptions {
    /// Current unattended defaults, without inventing an output directory.
    pub fn automatic() -> Self {
        let options = RunOptions::automatic(std::path::PathBuf::new());
        Self::from_run_options(&options)
    }

    /// Reuse the image-affecting members of a batch configuration.
    pub fn from_run_options(options: &RunOptions) -> Self {
        Self {
            pixel_format: PixelFormat::Rgb8Srgb,
            preset: options.preset,
            exposure_bias_ev: options.exposure_bias_ev,
            max_samples: options.max_samples,
            local_white_balance: options.local_white_balance,
            local_tone: options.local_tone,
            preview_exposure: options.preview_exposure,
            raw_color_path: options.raw_color_path,
            working_space: options.working_space,
            sub_black: options.sub_black,
            saturation_scale: options.saturation_scale,
            chroma_denoise: options.chroma_denoise,
            sharpen: options.sharpen,
            demosaic: options.demosaic,
            hot_pixels: options.hot_pixels,
            highlight_reconstruction: options.highlight_reconstruction,
            full_dng_color: options.full_dng_color,
            lens_correction: options.lens_correction,
        }
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.exposure_bias_ev.is_finite(),
            "exposure_bias_ev must be finite"
        );
        ensure!(
            self.max_samples >= 1_000,
            "max_samples must be at least 1000"
        );
        ensure!(
            self.local_white_balance.is_finite() && (0.0..=1.0).contains(&self.local_white_balance),
            "local_white_balance must be between 0 and 1"
        );
        ensure!(
            self.local_tone.is_finite() && (0.0..=1.0).contains(&self.local_tone),
            "local_tone must be between 0 and 1"
        );
        ensure!(
            self.preview_exposure
                .is_none_or(|value| value.is_finite() && (0.0..=1.0).contains(&value)),
            "preview_exposure must be between 0 and 1"
        );
        ensure!(
            self.saturation_scale.is_finite() && (0.0..=4.0).contains(&self.saturation_scale),
            "saturation_scale must be between 0 and 4"
        );
        ensure!(
            self.chroma_denoise.is_finite() && (0.0..=2.0).contains(&self.chroma_denoise),
            "chroma_denoise must be between 0 and 2"
        );
        ensure!(
            self.sharpen.is_finite() && (0.0..=3.0).contains(&self.sharpen),
            "sharpen must be between 0 and 3"
        );
        ensure!(
            self.hot_pixels.is_finite() && (0.0..=1.0).contains(&self.hot_pixels),
            "hot_pixels must be between 0 and 1"
        );
        ensure!(
            self.highlight_reconstruction.is_finite()
                && (0.0..=1.0).contains(&self.highlight_reconstruction),
            "highlight_reconstruction must be between 0 and 1"
        );
        Ok(())
    }
}

/// Diagnostics returned alongside the pixels, without any JSON/file round trip.
#[derive(Debug, Clone, Serialize)]
pub struct RenderReport {
    pub automatic_profile_version: &'static str,
    pub camera: CameraMetadata,
    pub level_normalization: Vec<String>,
    pub baseline_exposure_ev: f32,
    pub noise: Option<crate::noise::NoiseEstimate>,
    pub noise_floor: Option<crate::noiseprofile::NoiseFloor>,
    pub shot: Option<crate::shotinfo::ShotInfo>,
    pub preview: Option<crate::preview::PreviewOracle>,
    pub local_white_balance: Option<crate::whitebalance::LocalWhiteBalance>,
    pub local_tone: Option<crate::localtone::LocalToneReport>,
    pub chroma_denoise: Option<crate::chroma::ChromaDenoiseReport>,
    pub sharpen: Option<crate::sharpen::SharpenReport>,
    pub color: ColorReport,
    pub guidance_mode: GuidanceMode,
    pub analysis: AnalysisStats,
    pub parameters: ToneParams,
    pub measured: crate::metrics::OutputStats,
}

/// Self-describing packed image ready to hand to another crate.
#[derive(Debug, Clone)]
pub struct RenderedImage {
    pub width: u32,
    pub height: u32,
    pub row_stride: usize,
    pub pixel_format: PixelFormat,
    pub data: Vec<u8>,
    pub report: RenderReport,
}

impl RenderedImage {
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }
}

/// Self-describing native-endian 16-bit sRGB image for direct in-process use.
///
/// Unlike [`RenderedImage`], this keeps the renderer's native `Vec<u16>`
/// allocation intact. Consumers such as high-bit-depth image encoders can
/// borrow `data` directly without an endian packing pass or an intermediate
/// image file.
#[derive(Debug, Clone)]
pub struct RenderedRgb16Image {
    pub width: u32,
    pub height: u32,
    /// Number of `u16` samples between adjacent rows.
    pub row_stride: usize,
    pub data: Vec<u16>,
    pub report: RenderReport,
}

/// Develop one RAW directly into a packed RGB buffer.
///
/// This function performs no output writes and retains no references to the
/// input. `RenderedImage` is `Send`, so ownership can be moved directly to a
/// receiving worker or transport task.
pub fn render_file(path: impl AsRef<Path>, options: &RenderOptions) -> Result<RenderedImage> {
    let rendered = render_file_rgb16(path, options)?;
    let width = rendered.width;
    let height = rendered.height;
    let data = pack(rendered.data, options.pixel_format);
    let row_stride = width as usize * options.pixel_format.bytes_per_pixel();

    Ok(RenderedImage {
        width,
        height,
        row_stride,
        pixel_format: options.pixel_format,
        data,
        report: rendered.report,
    })
}

/// Develop one RAW directly into native-endian packed 16-bit sRGB samples.
///
/// This is the no-extra-conversion integration path for an in-process encoder:
/// the `Vec<u16>` produced by the tone renderer is returned as-is. The
/// `pixel_format` member of [`RenderOptions`] only controls [`render_file`] and
/// is intentionally ignored here.
pub fn render_file_rgb16(
    path: impl AsRef<Path>,
    options: &RenderOptions,
) -> Result<RenderedRgb16Image> {
    options.validate()?;
    let path = path.as_ref();

    let preview_strength = options
        .preview_exposure
        .unwrap_or(crate::preview::AUTO_STRENGTH);
    let preview = (preview_strength > 0.0)
        .then(|| crate::preview::read(path))
        .flatten();

    let mut raw = rawler::decode_file(path)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    let camera = camera_metadata(&raw);
    let source_orientation = raw.orientation;
    let correction = crate::apply_corrections(path, &mut raw)
        .with_context(|| format!("failed to prepare {}", path.display()))?;
    let noise = crate::noise::estimate(&raw);
    let noise_floor = crate::noiseprofile::frame_floor(noise.as_ref());
    let shot = crate::shotinfo::read(path);

    let (mut linear, color) = match options.raw_color_path {
        RawColorPath::Owned => crate::color::develop(
            &raw,
            path,
            DevelopOptions {
                working_space: options.working_space,
                sub_black: options.sub_black,
                hot_pixels: options.hot_pixels,
                highlight_reconstruction: options.highlight_reconstruction,
                demosaic: options.demosaic,
                snr10_ev: noise_floor.as_ref().map(|floor| floor.snr10_ev),
                full_dng_color: options.full_dng_color,
                lens_correction: options.lens_correction,
            },
        )?,
        RawColorPath::Rawler => (develop_rawler(&raw)?, ColorReport::rawler(&raw)),
    };
    drop(raw);

    if correction.baseline_exposure_ev.abs() > 0.001 {
        let gain = correction.baseline_exposure_ev.exp2();
        linear
            .pixels
            .iter_mut()
            .for_each(|pixel| pixel.iter_mut().for_each(|channel| *channel *= gain));
    }
    let mut linear = crate::orientation::apply_orientation(linear, source_orientation);
    let chroma_denoise = crate::chroma::apply(
        &mut linear,
        noise_floor.as_ref().map(|floor| floor.snr10_ev),
        options.chroma_denoise,
    );
    let local_white_balance = (options.local_white_balance > 0.0)
        .then(|| crate::whitebalance::apply(&mut linear, options.local_white_balance))
        .flatten();

    let (analysis, mut parameters) = crate::analyze::analyze(
        &linear,
        &crate::analyze::AnalysisInputs {
            max_samples: options.max_samples,
            preset: options.preset,
            exposure_bias_ev: options.exposure_bias_ev,
            noise_floor_ev: noise_floor.as_ref().map(|floor| floor.snr1_ev),
            preview: preview.as_ref(),
            preview_strength,
        },
    )?;
    parameters.saturation *= options.saturation_scale;
    let guidance_mode = if preview.is_some() && preview_strength > 0.0 {
        GuidanceMode::PreviewGuided
    } else {
        GuidanceMode::Independent
    };

    let local_tone = if options.local_tone > 0.0 {
        Some(crate::localtone::build(
            &linear,
            options.local_tone,
            noise_floor.as_ref().map(|floor| floor.snr1_ev),
        )?)
    } else {
        None
    };
    let local_tone_report = local_tone.as_ref().map(|map| map.report().clone());
    let working_to_display = options
        .working_space
        .to_display()
        .filter(|_| options.raw_color_path == RawColorPath::Owned);
    let mut rendered = crate::tone::render(
        &linear,
        &parameters,
        local_tone.as_ref(),
        working_to_display.as_ref(),
    );
    let sharpen = crate::sharpen::apply(
        &mut rendered,
        noise_floor.as_ref().map(|floor| floor.snr10_ev),
        options.sharpen,
    );
    let measured = crate::metrics::OutputStats::measure(&rendered);
    let width = rendered.width();
    let height = rendered.height();
    let data = rendered.into_raw();
    let row_stride = width as usize * 3;

    Ok(RenderedRgb16Image {
        width,
        height,
        row_stride,
        data,
        report: RenderReport {
            automatic_profile_version: RunOptions::AUTO_PROFILE_VERSION,
            camera,
            level_normalization: correction.notes,
            baseline_exposure_ev: correction.baseline_exposure_ev,
            noise,
            noise_floor,
            shot,
            preview,
            local_white_balance,
            local_tone: local_tone_report,
            chroma_denoise,
            sharpen,
            color,
            guidance_mode,
            analysis,
            parameters,
            measured,
        },
    })
}

fn camera_metadata(raw: &rawler::RawImage) -> CameraMetadata {
    CameraMetadata {
        make: raw.make.clone(),
        model: raw.model.clone(),
        clean_make: raw.clean_make.clone(),
        clean_model: raw.clean_model.clone(),
        source_width: raw.width,
        source_height: raw.height,
        components_per_pixel: raw.cpp,
        bits_per_sample: raw.bps,
        orientation: format!("{:?}", raw.orientation),
        white_balance_coefficients: raw
            .wb_coeffs
            .map(|value| value.is_finite().then_some(value)),
        black_levels: raw
            .blacklevel
            .as_vec()
            .into_iter()
            .map(|value| value.is_finite().then_some(value))
            .collect(),
        white_levels: raw
            .whitelevel
            .as_vec()
            .into_iter()
            .map(|value| value.is_finite().then_some(value))
            .collect(),
    }
}

fn develop_rawler(raw: &rawler::RawImage) -> Result<crate::types::LinearImage> {
    use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};

    let developer = RawDevelop {
        steps: vec![
            ProcessingStep::Rescale,
            ProcessingStep::Demosaic,
            ProcessingStep::CropActiveArea,
            ProcessingStep::WhiteBalance,
            ProcessingStep::Calibrate,
            ProcessingStep::CropDefault,
        ],
    };
    match developer
        .develop_intermediate(raw)
        .context("Rawler failed during color development")?
    {
        Intermediate::ThreeColor(pixels) => {
            crate::types::LinearImage::new(pixels.width, pixels.height, pixels.into_inner())
        }
        Intermediate::Monochrome(pixels) => {
            let width = pixels.width;
            let height = pixels.height;
            crate::types::LinearImage::new(
                width,
                height,
                pixels
                    .into_inner()
                    .into_iter()
                    .map(|value| [value; 3])
                    .collect(),
            )
        }
        Intermediate::FourColor(_) => {
            anyhow::bail!("Rawler returned an unsupported four-channel developed image")
        }
    }
}

fn pack(channels: Vec<u16>, format: PixelFormat) -> Vec<u8> {
    match format {
        PixelFormat::Rgb8Srgb => channels
            .into_iter()
            .map(|value| ((value as u32 + 128) / 257) as u8)
            .collect(),
        PixelFormat::Rgb16SrgbBe => {
            let mut bytes = Vec::with_capacity(channels.len() * 2);
            for value in channels {
                bytes.extend(value.to_be_bytes());
            }
            bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_api_defaults_match_the_batch_profile() {
        let batch = RunOptions::automatic(std::path::PathBuf::from("ignored"));
        let api = RenderOptions::automatic();
        assert_eq!(api.preset, batch.preset);
        assert_eq!(api.raw_color_path, batch.raw_color_path);
        assert_eq!(api.demosaic, batch.demosaic);
        assert_eq!(api.hot_pixels, batch.hot_pixels);
        assert_eq!(api.highlight_reconstruction, batch.highlight_reconstruction);
        assert_eq!(api.full_dng_color, batch.full_dng_color);
        assert_eq!(api.lens_correction, batch.lens_correction);
    }

    #[test]
    fn packed_formats_have_documented_byte_order_and_rounding() {
        assert_eq!(
            pack(vec![0, 128, 257, 65_535], PixelFormat::Rgb8Srgb),
            vec![0, 0, 1, 255]
        );
        assert_eq!(
            pack(vec![0x1234, 0xabcd], PixelFormat::Rgb16SrgbBe),
            vec![0x12, 0x34, 0xab, 0xcd]
        );
    }
}
