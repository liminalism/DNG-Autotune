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
/// [`Default`] is the same unattended image policy the command line uses —
/// [`RunOptions::AUTO_PROFILE_VERSION`], named rather than spelled out here so
/// the two cannot drift. The file/output-management members of [`RunOptions`]
/// are intentionally absent: this API writes no image files.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub pixel_format: PixelFormat,
    pub preset: Preset,
    pub exposure_bias_ev: f32,
    pub max_samples: usize,
    pub local_white_balance: f32,
    pub local_tone: f32,
    /// HDR-like edge-aware single-frame local tone, 0 to 1. 0 is off and
    /// byte-identical. Mutually exclusive with `local_tone`.
    pub hdr: f32,
    /// `None` selects the automatic embedded-preview policy; `Some(0.0)`
    /// disables preview guidance.
    pub preview_exposure: Option<f32>,
    pub raw_color_path: RawColorPath,
    pub working_space: WorkingSpace,
    pub sub_black: SubBlack,
    pub saturation_scale: f32,
    /// Multiplier on the tone curve's highlight exponent alone; 1.0 is the
    /// preset as tuned. See `analyze::derive_params`.
    pub highlight_contrast: f32,
    pub highlight_color_ratio_exponent: f32,
    pub chroma_denoise: f32,
    pub luma_denoise: f32,
    /// Automatic night tone map scale, 0 to 1. 1.0 applies the edge-aware local
    /// tone operator to detected low-light frames; 0 disables it. Ignored when
    /// `hdr` or `local_tone` is set explicitly.
    pub night_tone: f32,
    pub sharpen: f32,
    pub demosaic: DemosaicMethod,
    pub hot_pixels: f32,
    pub highlight_reconstruction: f32,
    pub highlight_method: crate::raw_highlight::HighlightMethod,
    /// Fraction of clipped CFA sites below which the spatial highlight solver
    /// is declined and the frame develops on the post-demosaic `Current`
    /// estimator. This is what keeps an unclipped frame off the ~8 s / ~1.05 GB
    /// path. `0.0` always solves. See
    /// [`crate::raw_highlight::DEFAULT_SPATIAL_CLIPPED_FLOOR`].
    pub spatial_highlight_floor: f32,
    pub full_dng_color: bool,
    pub lens_correction: crate::lens::LensCorrectionMode,
    pub hue_sat_map: Option<std::sync::Arc<crate::huesatmap::HueSatMap>>,
    pub hue_sat_map_strength: f32,
    /// Build the EXIF payload and ICC profile on [`RenderedRgb16Image`], the
    /// same bytes the CLI writers embed. `false` leaves both `None` and skips
    /// the one extra metadata parse of the source file; it never changes pixels.
    /// Mirrors the CLI's `--no-metadata`, inverted.
    pub metadata: bool,
    /// Collect observational scene evidence without changing returned pixels.
    pub semantic: bool,
    /// Experimental dense-mask sky highlight luminance compression, 0 to 1.
    /// Requires `semantic`; zero is byte-identical to the observational path.
    pub semantic_sky_highlights: f32,
    /// Experimental mask-gated removal of measured sky magenta, 0 to 1.
    /// Does not infer a white point or neutralize the blue/yellow axis.
    pub semantic_sky_chroma: f32,
    /// Run the YuNet face detector under `semantic`. Off; see
    /// [`crate::scene::FACE_DETECTION_DEFAULT`].
    pub semantic_faces: bool,
    /// Directory containing prepared scene_image ONNX graphs.
    pub semantic_model_dir: std::path::PathBuf,
    /// Run the C5 illuminant estimator observationally. It changes no returned
    /// pixel; the estimate is reported and nothing else.
    pub illuminant: bool,
    /// Run the CamSDD scene classifier observationally and report the fused
    /// `scene_lighting` verdict. It changes no returned pixel.
    pub scene_classify: bool,
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
            hdr: options.hdr,
            preview_exposure: options.preview_exposure,
            raw_color_path: options.raw_color_path,
            working_space: options.working_space,
            sub_black: options.sub_black,
            saturation_scale: options.saturation_scale,
            highlight_contrast: options.highlight_contrast,
            highlight_color_ratio_exponent: options.highlight_color_ratio_exponent,
            chroma_denoise: options.chroma_denoise,
            luma_denoise: options.luma_denoise,
            night_tone: options.night_tone,
            sharpen: options.sharpen,
            demosaic: options.demosaic,
            hot_pixels: options.hot_pixels,
            highlight_reconstruction: options.highlight_reconstruction,
            highlight_method: options.highlight_method,
            spatial_highlight_floor: options.spatial_highlight_floor,
            full_dng_color: options.full_dng_color,
            lens_correction: options.lens_correction,
            hue_sat_map: options.hue_sat_map.clone(),
            hue_sat_map_strength: options.hue_sat_map_strength,
            metadata: options.write_metadata,
            semantic: options.semantic,
            semantic_sky_highlights: options.semantic_sky_highlights,
            semantic_sky_chroma: options.semantic_sky_chroma,
            semantic_faces: options.semantic_faces,
            semantic_model_dir: options.semantic_model_dir.clone(),
            illuminant: options.illuminant,
            scene_classify: options.scene_classify,
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
            self.hdr.is_finite() && (0.0..=1.0).contains(&self.hdr),
            "hdr must be between 0 and 1"
        );
        ensure!(
            !(self.hdr > 0.0 && self.local_tone > 0.0),
            "hdr and local_tone are two local-tone operators; set only one"
        );
        ensure!(
            self.semantic_sky_highlights.is_finite()
                && (0.0..=1.0).contains(&self.semantic_sky_highlights),
            "semantic_sky_highlights must be between 0 and 1"
        );
        ensure!(
            self.semantic_sky_highlights == 0.0 || self.semantic,
            "semantic_sky_highlights requires semantic inference"
        );
        ensure!(
            self.semantic_sky_chroma.is_finite() && (0.0..=1.0).contains(&self.semantic_sky_chroma),
            "semantic_sky_chroma must be between 0 and 1"
        );
        ensure!(
            self.semantic_sky_chroma == 0.0 || self.semantic,
            "semantic_sky_chroma requires semantic inference"
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
            self.highlight_contrast.is_finite() && (0.25..=4.0).contains(&self.highlight_contrast),
            "highlight_contrast must be between 0.25 and 4"
        );
        ensure!(
            self.highlight_color_ratio_exponent.is_finite()
                && (0.0..=1.0).contains(&self.highlight_color_ratio_exponent),
            "highlight_color_ratio_exponent must be between 0 and 1"
        );
        ensure!(
            self.chroma_denoise.is_finite() && (0.0..=2.0).contains(&self.chroma_denoise),
            "chroma_denoise must be between 0 and 2"
        );
        ensure!(
            self.luma_denoise.is_finite() && (0.0..=2.0).contains(&self.luma_denoise),
            "luma_denoise must be between 0 and 2"
        );
        ensure!(
            self.night_tone.is_finite() && (0.0..=1.0).contains(&self.night_tone),
            "night_tone must be between 0 and 1"
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
        ensure!(
            !self.highlight_method.is_spatial() || self.highlight_reconstruction == 1.0,
            "spatial highlight methods require highlight_reconstruction 1"
        );
        ensure!(
            self.spatial_highlight_floor.is_finite()
                && (0.0..=1.0).contains(&self.spatial_highlight_floor),
            "spatial_highlight_floor must be between 0 and 1"
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
    pub hdr: Option<crate::localtone::HdrReport>,
    pub chroma_denoise: Option<crate::chroma::ChromaDenoiseReport>,
    pub luma_denoise: Option<crate::luma::LumaDenoiseReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene: Option<crate::scene::SceneEvidence>,
    /// Observational illuminant estimate, when `illuminant` asked for one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub illuminant: Option<crate::illuminant::IlluminantEvidence>,
    /// Fused scene-lighting verdict, when `scene_classify` asked for one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_lighting: Option<crate::scene::SceneLightingReport>,
    /// Versioned, render-neutral guidance for a downstream encoder.
    pub encoder_hints: crate::encoder_hints::EncoderProfileHints,
    pub sharpen: Option<crate::sharpen::SharpenReport>,
    pub color: ColorReport,
    pub guidance_mode: GuidanceMode,
    pub analysis: AnalysisStats,
    pub parameters: ToneParams,
    pub measured: crate::metrics::OutputStats,
}

impl RenderReport {
    /// Borrow a dense semantic confidence plane selected by the encoder-hints
    /// contract. This keeps encoder integration independent of scene-model
    /// internals while avoiding confidence bytes in JSON sidecars.
    pub fn encoder_confidence_mask(&self, kind: crate::scene::RegionKind) -> Option<&[u8]> {
        self.scene
            .as_ref()
            .and_then(|scene| self.encoder_hints.confidence_mask(scene, kind))
    }

    /// Sample an encoder ROI hint directly in rendered pixel coordinates.
    pub fn encoder_confidence_at(
        &self,
        kind: crate::scene::RegionKind,
        x: usize,
        y: usize,
    ) -> Option<f32> {
        self.scene
            .as_ref()
            .and_then(|scene| self.encoder_hints.confidence_at_source(scene, kind, x, y))
    }

    /// Resample selected semantic evidence to an encoder's native AQ grid.
    ///
    /// Pass `(8, 8)` for JPEG XL's current atom grid, or `(16, 16)` / `(32, 32)`
    /// for bpg-rs HEVC quantization groups. Values remain normalized semantic
    /// confidence; the receiving encoder chooses its own quantizer response.
    pub fn encoder_spatial_aq_map(
        &self,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<crate::encoder_hints::SpatialAqMap, crate::encoder_hints::SpatialAqError> {
        let scene = self
            .scene
            .as_ref()
            .ok_or(crate::encoder_hints::SpatialAqError::SceneEvidenceUnavailable)?;
        self.encoder_hints
            .spatial_aq_map(scene, cell_width, cell_height)
    }
}

/// Self-describing packed image ready to hand to another crate.
#[derive(Debug, Clone)]
pub struct RenderedImage {
    pub width: u32,
    pub height: u32,
    pub row_stride: usize,
    pub pixel_format: PixelFormat,
    pub data: Vec<u8>,
    /// See [`RenderedRgb16Image::color_space`].
    pub color_space: crate::metadata::OutputColorSpace,
    /// See [`RenderedRgb16Image::exif`].
    pub exif: Option<Vec<u8>>,
    /// See [`RenderedRgb16Image::icc`].
    pub icc: Option<Vec<u8>>,
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

/// Self-describing native-endian 16-bit image for direct in-process use.
///
/// Unlike [`RenderedImage`], this keeps the renderer's native `Vec<u16>`
/// allocation intact. Consumers such as high-bit-depth image encoders can
/// borrow `data` directly without an endian packing pass or an intermediate
/// image file.
///
/// `color_space`, `exif` and `icc` are the cross-repo handoff contract: they
/// carry everything an encoder needs to write a faithful archive file without
/// re-opening the source RAW, and without assuming a colour space. All three
/// are additive — a consumer that reads only `width`/`height`/`data` behaves
/// exactly as it did before they existed.
#[derive(Debug, Clone)]
pub struct RenderedRgb16Image {
    pub width: u32,
    pub height: u32,
    /// Number of `u16` samples between adjacent rows.
    pub row_stride: usize,
    pub data: Vec<u16>,
    /// The colour space `data` is encoded in. Signal this to the encoder rather
    /// than assuming sRGB; see [`crate::metadata::OutputColorSpace`], which also
    /// explains why this is *not* simply `--working-space`.
    pub color_space: crate::metadata::OutputColorSpace,
    /// EXIF as a standalone TIFF structure, with every internal offset relative
    /// to the start of this buffer.
    ///
    /// **This is the bare TIFF blob, with no `Exif\0\0` header and no JPEG
    /// `APP1` framing.** It is byte-identical to what
    /// [`crate::metadata::SourceMetadata::exif_payload`] hands the CLI writers,
    /// i.e. exactly what goes into a PNG `eXIf` chunk verbatim, into a JPEG
    /// `APP1` segment after the six-byte header, and into a JPEG XL encoder's
    /// `with_exif`. Orientation is already normalized to `1` and the dimensions
    /// are the rendered ones, because the pixels are upright.
    ///
    /// `None` when `RenderOptions::metadata` is false, or when the source
    /// carried nothing worth writing.
    pub exif: Option<Vec<u8>>,
    /// The ICC profile for `color_space`. Generated, v2.1 matrix-shaper, and
    /// byte-identical to the profile the CLI embeds. `None` when
    /// `RenderOptions::metadata` is false.
    pub icc: Option<Vec<u8>>,
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
        color_space: rendered.color_space,
        exif: rendered.exif,
        icc: rendered.icc,
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
    let decoded_preview = (preview_strength > 0.0 || options.semantic)
        .then(|| crate::preview::read_preview_rgb(path))
        .flatten();
    let preview_semantic_eligible = decoded_preview
        .as_ref()
        .is_some_and(crate::preview::DecodedPreview::semantic_eligible);
    let preview = (preview_strength > 0.0)
        .then(|| {
            decoded_preview
                .as_ref()
                .and_then(crate::preview::measure_preview_oracle)
        })
        .flatten();
    drop(decoded_preview);

    let mut raw = rawler::decode_file(path)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    let camera = camera_metadata(&raw);
    let source_orientation = raw.orientation;
    let correction = crate::apply_corrections(path, &mut raw)
        .with_context(|| format!("failed to prepare {}", path.display()))?;
    let noise = crate::noise::estimate(&raw);
    let noise_floor = crate::noiseprofile::frame_floor(noise.as_ref());
    let shot = crate::shotinfo::read(path);

    let (mut linear, color, mut highlight_uncertainty, illuminant_proxy) =
        match options.raw_color_path {
            RawColorPath::Owned => crate::color::develop(
                &raw,
                path,
                DevelopOptions {
                    working_space: options.working_space,
                    sub_black: options.sub_black,
                    hot_pixels: options.hot_pixels,
                    highlight_reconstruction: options.highlight_reconstruction,
                    highlight_method: options.highlight_method,
                    spatial_highlight_floor: options.spatial_highlight_floor,
                    demosaic: options.demosaic,
                    snr10_ev: noise_floor.as_ref().map(|floor| floor.snr10_ev),
                    full_dng_color: options.full_dng_color,
                    lens_correction: options.lens_correction,
                    dump_stages: None,
                    illuminant_proxy: options.illuminant,
                    hue_sat_map: options.hue_sat_map.clone(),
                    hue_sat_map_strength: options.hue_sat_map_strength,
                },
            )?,
            RawColorPath::Rawler => (develop_rawler(&raw)?, ColorReport::rawler(&raw), None, None),
        };

    // Observational: the estimate is read off the pre-white-balance proxy and
    // reported. Nothing below this line consults it.
    let illuminant = illuminant_proxy
        .as_ref()
        .map(|proxy| crate::illuminant::observe(proxy, &raw, &options.semantic_model_dir));
    drop(illuminant_proxy);
    drop(raw);

    if correction.baseline_exposure_ev.abs() > 0.001 {
        let gain = correction.baseline_exposure_ev.exp2();
        linear
            .pixels
            .iter_mut()
            .for_each(|pixel| pixel.iter_mut().for_each(|channel| *channel *= gain));
    }
    let pre_orient_dims = (linear.width, linear.height);
    let mut linear = crate::orientation::apply_orientation(linear, source_orientation);
    if let Some(map) = highlight_uncertainty.take() {
        let (_, _, oriented) = crate::orientation::orient_data(
            pre_orient_dims.0,
            pre_orient_dims.1,
            map,
            source_orientation,
        );
        highlight_uncertainty = Some(oriented);
    }
    let chroma_denoise = crate::chroma::apply(
        &mut linear,
        noise_floor.as_ref().map(|floor| floor.snr10_ev),
        options.chroma_denoise,
    );
    let working_to_display = options
        .working_space
        .to_display()
        .filter(|_| options.raw_color_path == RawColorPath::Owned);
    // One shared perception hook for API and CLI at the same pipeline
    // boundary. Only the explicit sky experiment retains a render input, and
    // it is applied after analysis so the global controller cannot consume it.
    let (scene, semantic_sky_highlights, semantic_sky_chroma) = if options.semantic
        || options.scene_classify
    {
        let (proxy, mut evidence) = crate::scene::observe(
            &linear,
            working_to_display.as_ref(),
            highlight_uncertainty.as_deref(),
            noise_floor.as_ref().map(|floor| floor.snr10_ev),
            preview_semantic_eligible,
            &options.semantic_model_dir,
            crate::scene::PerceptionTasks {
                segment: options.semantic,
                faces: options.semantic_faces,
                classify: options.scene_classify,
            },
        )?;
        let sky_map = if options.semantic_sky_highlights > 0.0 {
            let map = crate::scene::build_sky_highlight_map(
                &linear,
                working_to_display.as_ref(),
                highlight_uncertainty.as_deref(),
                &proxy,
                &evidence,
                options.semantic_sky_highlights,
            )?;
            let report = map.report().clone();
            if report.affected_pixels > 0 {
                evidence.policy_adjustments.push(format!(
                    "{}: dense sky mask compressed {} pixel(s) by up to {:.3} EV at strength {:.2}",
                    report.version,
                    report.affected_pixels,
                    -report.correction_min_ev,
                    report.strength,
                ));
            }
            evidence.sky_highlight_adjustment = Some(report);
            Some(map)
        } else {
            None
        };
        let sky_chroma_map = if options.semantic_sky_chroma > 0.0 {
            let map = crate::scene::build_sky_chroma_map(
                &linear,
                working_to_display.as_ref(),
                highlight_uncertainty.as_deref(),
                &proxy,
                &evidence,
                options.semantic_sky_chroma,
            )?;
            let report = map.report().clone();
            if report.affected_pixels > 0 {
                evidence.policy_adjustments.push(format!(
                    "{}: dense sky mask reduced measured positive Oklab a on {} pixel(s) at strength {:.2}",
                    report.version, report.affected_pixels, report.strength,
                ));
            }
            evidence.sky_chroma_adjustment = Some(report);
            Some(map)
        } else {
            None
        };
        (Some(evidence), sky_map, sky_chroma_map)
    } else {
        (None, None, None)
    };
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
            snr10_ev: noise_floor.as_ref().map(|floor| floor.snr10_ev),
            iso: shot.as_ref().and_then(|info| info.iso),
            exposure_time: shot.as_ref().and_then(|info| info.exposure_time),
            f_number: shot.as_ref().and_then(|info| info.f_number),
            preview: preview.as_ref(),
            preview_strength,
            highlight_contrast: options.highlight_contrast,
        },
    )?;
    parameters.saturation *= options.saturation_scale;
    parameters.highlight_color_ratio_exponent = options.highlight_color_ratio_exponent;
    let encoder_hints =
        crate::encoder_hints::EncoderProfileHints::derive(&analysis, scene.as_ref());
    let scene_lighting = if options.scene_classify {
        scene
            .as_ref()
            .and_then(|evidence| evidence.classification.as_ref())
            .map(|classification| {
                crate::scene::fuse_lighting(
                    classification,
                    &analysis,
                    illuminant.as_ref().and_then(|evidence| evidence.cct_k),
                )
            })
    } else {
        None
    };
    let guidance_mode = if preview.is_some() && preview_strength > 0.0 {
        GuidanceMode::PreviewGuided
    } else {
        GuidanceMode::Independent
    };

    // Luminance noise reduction after analysis (exposure byte-identical) and
    // before the tone map and render (so lifted shadows are not grainy).
    let luma_denoise = crate::luma::apply(
        &mut linear,
        noise_floor.as_ref().map(|floor| floor.snr10_ev),
        options.luma_denoise,
    );
    if let Some(map) = &semantic_sky_highlights {
        map.apply(&mut linear)?;
    }
    if let Some(map) = &semantic_sky_chroma {
        map.apply(&mut linear)?;
    }

    // Automatic night tone map, driven by the low-light score, unless the caller
    // asked for HDR or local tone explicitly.
    let night_tone_strength = if options.hdr == 0.0 && options.local_tone == 0.0 {
        (crate::localtone::automatic_night_strength(analysis.low_light_score) * options.night_tone)
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    let hdr_strength = if options.hdr > 0.0 {
        options.hdr
    } else {
        night_tone_strength
    };

    let local_tone = if options.local_tone > 0.0 {
        Some(crate::localtone::build(
            &linear,
            options.local_tone,
            noise_floor.as_ref().map(|floor| floor.snr1_ev),
            highlight_uncertainty.as_deref(),
        )?)
    } else {
        None
    };
    let local_tone_report = local_tone.as_ref().map(|map| map.report().clone());
    let hdr = if hdr_strength > 0.0 {
        Some(crate::localtone::build_hdr(
            &linear,
            hdr_strength,
            noise_floor.as_ref().map(|floor| floor.snr1_ev),
            highlight_uncertainty.as_deref(),
        )?)
    } else {
        None
    };
    let hdr_report = hdr.as_ref().map(|map| map.report().clone());
    let correction_field: Option<&dyn crate::localtone::CorrectionField> = match (&hdr, &local_tone)
    {
        (Some(map), _) => Some(map),
        (None, Some(map)) => Some(map),
        (None, None) => None,
    };
    let mut rendered = crate::tone::render(
        &linear,
        &parameters,
        correction_field,
        highlight_uncertainty.as_deref(),
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

    // The same builder the CLI writers use, at the same output dimensions, so a
    // consumer that embeds this blob produces the EXIF `raw-autotune --format
    // png` would have written. Best effort: a source whose metadata will not
    // serialize must not fail a render that already succeeded.
    let color_space =
        crate::metadata::OutputColorSpace::of_render(options.working_space, options.raw_color_path);
    let (exif, icc) = if options.metadata {
        // Not gated on `SourceMetadata::is_empty`, matching `pipeline.rs`: a
        // file whose EXIF cannot be parsed still gets Software, Orientation,
        // ColorSpace and the output dimensions, which is what a library needs
        // least wrongly.
        let source = crate::metadata::SourceMetadata::read(path);
        let exif = source.exif_payload(width, height).ok();
        (exif, Some(color_space.icc_profile().to_vec()))
    } else {
        (None, None)
    };

    Ok(RenderedRgb16Image {
        width,
        height,
        row_stride,
        data,
        color_space,
        exif,
        icc,
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
            hdr: hdr_report,
            chroma_denoise,
            luma_denoise,
            scene,
            illuminant,
            scene_lighting,
            encoder_hints,
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
        assert_eq!(api.highlight_contrast, batch.highlight_contrast);
        assert_eq!(api.highlight_reconstruction, batch.highlight_reconstruction);
        assert_eq!(api.highlight_method, batch.highlight_method);
        assert_eq!(api.full_dng_color, batch.full_dng_color);
        assert_eq!(api.lens_correction, batch.lens_correction);
        assert!(api.hue_sat_map.is_none());
        assert_eq!(api.hue_sat_map_strength, 0.0);
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
