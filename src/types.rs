use clap::ValueEnum;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::PathBuf;

pub const MID_GRAY: f32 = 0.18;
/// Schema shared by per-image sidecars and batch summaries.
///
/// 4 added `measured.mean_saturation` and the `reference` block.
/// 5 added the `chroma_denoise` block.
/// 6 added `chroma_denoise.guided` and the `sharpen` block.
/// 7 added the `color` block, `guidance_mode` and `controller_version`, and
///   renamed every `subject_*` field to `center_weighted_key_*`.
/// 8 added `measured.mean_saturation_highlight`/`highlight_pixels`, the
///   `reference.delta.hue` block and `highlight_saturation_ratio`, and
///   `color.clip_cost.negative_luminance_fraction`.
/// 9 added the `color.rescale` block, present only on the owned colour path,
///   which owns black/white normalization from 0.1.18 on.
/// 10 added `color.hot_pixels` and `color.highlight_reconstruction`, both present
///   only on the owned colour path and only when the corresponding correction
///   ran; a run with both off serializes identically to schema 9.
/// 11 added `color.dng_color`, present when the DNG matrix path was composed.
/// 12 adds pre-demosaic correction, highlight reconstruction, adaptive demosaic,
///   extended DNG calibration details, and the automatic profile identifier.
/// 13 is the 0.1.19 baseline.
/// 14 adds `color.highlight_reconstruction.near_white_pixels`, the count of
///   pixels with two or more channels at the sensor clip. Those pixels also
///   changed rendering in the same release — see `CHANGELOG.md`: they are now
///   anchored across all channels at full strength, which is what removes the
///   white-balance cast on blown skies. A frame with no such pixels serializes
///   the same values it did under 13 apart from the new field, and
///   `--highlight-reconstruction 0` is unchanged in both output and sidecar.
pub const REPORT_SCHEMA_VERSION: u32 = 14;

/// Name for the exposure controller's current behaviour, frozen at the colour
/// path's correctness boundary.
///
/// `docs/REVIEW-2026-07-30.md` adopts a freeze: no new exposure heuristics until
/// the colour core lands, because every constant tuned against the clipped path
/// may be a compensation for the clipping. Recording the label in every sidecar
/// is what makes a later comparison against a `v2` controller mechanical rather
/// than archaeological — the corpus grades already on disk say which controller
/// produced them.
pub const CONTROLLER_VERSION: &str = "v1";

/// Whether the exposure target came from this program's own analysis or from the
/// camera's rendering of the same frame.
///
/// The corpus work has to keep these apart. `docs/PLAN.md`, "borrow the
/// judgement, beat the rendering", sets out why:
/// the preview oracle borrows the camera's *judgement* about a scene, which is
/// free and caps nothing, but a scorecard that mixes guided and unguided frames
/// cannot say whether this program's own judgement is improving. So the mode is
/// recorded per file rather than inferred from whether a flag was passed —
/// `--preview-exposure` is automatic, so the answer differs file by file within
/// one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuidanceMode {
    /// The controller chose the exposure target with no reference to the
    /// camera's own rendering — either the file carries no usable preview, or
    /// the oracle was switched off.
    Independent,
    /// The camera's embedded preview supplied the target the subject was placed
    /// at.
    PreviewGuided,
}

impl GuidanceMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Independent => "independent",
            Self::PreviewGuided => "preview_guided",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    Neutral,
    Auto,
    Punchy,
}

impl Preset {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Auto => "auto",
            Self::Punchy => "punchy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Tiff,
    Png,
    Jpeg,
}

impl OutputFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Tiff => "tif",
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
}

/// Chroma subsampling for the JPEG encoder, exposing the three factors that
/// matter in practice plus an automatic choice.
///
/// The mapping onto `jpeg_encoder::SamplingFactor` lives in
/// [`crate::metadata`] so this enum carries no dependency on the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum JpegSubsampling {
    /// 4:4:4 below the quality threshold and 4:2:0 above it, matching what most
    /// encoders pick when left alone. The default.
    Auto,
    /// 4:4:4 — full chroma resolution, largest file, no colour bleed.
    #[value(name = "444")]
    S444,
    /// 4:2:2 — chroma halved horizontally.
    #[value(name = "422")]
    S422,
    /// 4:2:0 — chroma halved both ways, smallest file.
    #[value(name = "420")]
    S420,
}

impl JpegSubsampling {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::S444 => "4:4:4",
            Self::S422 => "4:2:2",
            Self::S420 => "4:2:0",
        }
    }
}

/// Everything the `jpeg-encoder` crate lets a caller tune, gathered so the
/// whole JPEG configuration travels as one value from the CLI to the writer.
#[derive(Debug, Clone, Copy)]
pub struct JpegSettings {
    /// Quality from 1 to 100.
    pub quality: u8,
    /// Emit a progressive JPEG rather than a baseline one. Smaller, but slower
    /// to encode and to decode.
    pub progressive: bool,
    /// Optimize the Huffman tables for this image. A few percent smaller at the
    /// cost of a second encoding pass.
    pub optimized_huffman: bool,
    /// Chroma subsampling.
    pub subsampling: JpegSubsampling,
}

impl JpegSettings {
    /// The settings a plain `--jpeg-quality` implies: baseline, unoptimized,
    /// automatic subsampling.
    pub const fn from_quality(quality: u8) -> Self {
        Self {
            quality,
            progressive: false,
            optimized_huffman: false,
            subsampling: JpegSubsampling::Auto,
        }
    }

    /// Archive-oriented automatic output: high quality, full chroma, baseline
    /// compatibility, and deterministic optimized Huffman tables.
    pub const fn archive() -> Self {
        Self {
            quality: 95,
            progressive: false,
            optimized_huffman: true,
            subsampling: JpegSubsampling::S444,
        }
    }
}

impl Default for JpegSettings {
    fn default() -> Self {
        Self::archive()
    }
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub output_dir: PathBuf,
    pub format: OutputFormat,
    pub preset: Preset,
    pub exposure_bias_ev: f32,
    /// How many files to hold in flight, or the request to work it out from
    /// available memory. Resolved once per run by [`crate::memory::plan`];
    /// nothing this affects changes what is rendered.
    pub jobs: crate::memory::JobCount,
    pub overwrite: bool,
    pub emit_baseline: bool,
    pub write_sidecar: bool,
    pub jpeg: JpegSettings,
    pub max_samples: usize,
    pub recursive: bool,
    pub dry_run: bool,
    pub local_white_balance: f32,
    pub local_tone: f32,
    /// `None` means the automatic decision: [`crate::preview::AUTO_STRENGTH`]
    /// on files whose embedded preview is a real rendering, and nothing at all
    /// on files that only carry a thumbnail, since `preview::read` rejects
    /// those. `Some` is the user overriding that.
    pub preview_exposure: Option<f32>,
    /// Which colour conversion to develop through.
    pub raw_color_path: crate::color::RawColorPath,
    /// Linear RGB space the owned colour path works in.
    pub working_space: crate::color::WorkingSpace,
    /// What the owned colour path does with sub-black sensor samples. Defaults
    /// to bit-for-bit Rawler compatibility, so owning the rescale step does not
    /// move any output until the change is measured on its own.
    pub sub_black: crate::rescale::SubBlack,
    /// Where to write per-stage scene-linear dumps, when asked for.
    pub dump_stages: Option<PathBuf>,
    /// Copy the source EXIF into the output and embed the sRGB ICC profile.
    /// On by default: `docs/PLAN.md` criterion 2 counts this as the product,
    /// not polish, because a photo library with no capture metadata sorts an
    /// archive by file modification date.
    pub write_metadata: bool,
    pub noise_scan: Option<PathBuf>,
    pub noise_profile: Option<PathBuf>,
    pub pool_noise: bool,
    pub summary_path: Option<PathBuf>,
    /// Where to find the camera's own JPEG of each capture, for measurement.
    pub reference: crate::reference::ReferenceSource,
    /// Multiplier on the preset's saturation; 1.0 is the preset as tuned.
    pub saturation_scale: f32,
    /// Multiplier on the tone curve's highlight exponent alone; 1.0 is the
    /// preset as tuned. Raises where highlights land without moving middle grey,
    /// the black point or the shadow branch. See `analyze::derive_params`.
    pub highlight_contrast: f32,
    /// Multiplier on the automatic chroma-denoise strength; 1.0 is automatic.
    pub chroma_denoise: f32,
    /// Multiplier on the automatic output-sharpening amount; 1.0 is automatic.
    pub sharpen: f32,
    /// Bayer demosaic policy. `Auto` resolves per frame from pre-demosaic
    /// statistics and is recorded in the colour report.
    pub demosaic: crate::demosaic::DemosaicMethod,
    /// Hot/dead pixel suppression strength on the CFA mosaic, 0 to 1. 0 is off
    /// and byte-identical; owned colour path only.
    pub hot_pixels: f32,
    /// Clipped-highlight reconstruction strength, 0 to 1. 0 is off and
    /// byte-identical; owned colour path only.
    pub highlight_reconstruction: f32,
    /// Use the DNG matrix model (ForwardMatrix/ColorMatrix, CameraCalibration,
    /// AnalogBalance, ReductionMatrix, and up to three illuminants) on files
    /// that carry it. On by default and owned colour path only; safely falls
    /// back to the decoder's camera matrix when a profile is incomplete.
    pub full_dng_color: bool,
    /// Apply standardized DNG post-demosaic lens opcodes when the file carries
    /// them. Unknown cameras and files without opcodes are left untouched.
    pub lens_correction: bool,
}

impl RunOptions {
    pub const AUTO_PROFILE_VERSION: &'static str = "archive-auto-v2";

    /// The unattended archive profile shared by the flag CLI and the minimal
    /// interactive front-end. Callers change only explicit user overrides.
    pub fn automatic(output_dir: PathBuf) -> Self {
        let summary_path = Some(output_dir.join("summary.json"));
        Self {
            output_dir,
            format: OutputFormat::Jpeg,
            preset: Preset::Auto,
            exposure_bias_ev: 0.0,
            jobs: crate::memory::JobCount::Auto,
            overwrite: false,
            emit_baseline: false,
            write_sidecar: false,
            jpeg: JpegSettings::archive(),
            max_samples: 250_000,
            recursive: true,
            dry_run: false,
            local_white_balance: 0.0,
            local_tone: 0.0,
            preview_exposure: None,
            raw_color_path: crate::color::RawColorPath::Owned,
            working_space: crate::color::WorkingSpace::Srgb,
            sub_black: crate::rescale::SubBlack::Preserve,
            dump_stages: None,
            write_metadata: true,
            noise_scan: None,
            noise_profile: None,
            pool_noise: false,
            summary_path,
            reference: crate::reference::ReferenceSource::Disabled,
            saturation_scale: 1.0,
            highlight_contrast: 1.0,
            chroma_denoise: 1.0,
            sharpen: 1.0,
            demosaic: crate::demosaic::DemosaicMethod::Auto,
            hot_pixels: 0.5,
            highlight_reconstruction: 0.75,
            full_dng_color: true,
            lens_correction: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InputJob {
    pub input: PathBuf,
    pub relative: PathBuf,
}

#[derive(Debug, Clone)]
pub struct OutputPaths {
    pub image: PathBuf,
    pub sidecar: PathBuf,
    pub baseline: PathBuf,
}

/// Sensor channel space: demosaiced, black/white-level normalized, but with no
/// white balance and no colour matrix applied. Not displayable and not
/// analysable — the numbers mean "how much light this photosite's filter let
/// through", which is camera-specific.
#[derive(Debug, Clone, Copy)]
pub struct CameraRgb;

/// Scene-referred linear RGB in the working space: white-balanced, matrixed,
/// unbounded above, and possibly negative where a colour falls outside the
/// working space's gamut. Everything from analysis to the tone curve's input
/// lives here.
#[derive(Debug, Clone, Copy)]
pub struct SceneLinear;

/// A pixel buffer tagged with the colour space its numbers are in.
///
/// The phantom parameter is the point: before this existed, "camera RGB",
/// "scene-linear sRGB" and "scene-linear working space" were all `Vec<[f32; 3]>`
/// and the stage order was enforced by comments. Handing the analyser
/// un-white-balanced sensor data would have compiled and produced a plausible-
/// looking wrong answer. Now it does not compile.
///
/// There is deliberately no `DisplayLinear` or `Encoded` marker: the render path
/// takes scene-linear straight to encoded `u16` inside `tone::render_pixel_local`
/// without ever materializing a display-linear buffer, and the encoded stage
/// already has a distinct type in `tone::Rgb16Image`. Markers with no
/// inhabitants would be decoration.
///
/// There is also deliberately no escape hatch — no `retag`, no way to relabel a
/// buffer without rebuilding it. One was written first and turned out to have no
/// caller, which is the useful result: every place the colour space changes also
/// changes the numbers, so a reinterpret-in-place would only ever have been a
/// way to skip a conversion by mistake. [`Image::map_into`] is that rule turned
/// into a signature rather than an exception to it: it changes the marker, and it
/// cannot be called without supplying the transform that changes the numbers.
#[derive(Debug, Clone)]
pub struct Image<S> {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[f32; 3]>,
    space: PhantomData<S>,
}

impl<S> Image<S> {
    pub fn new(width: usize, height: usize, pixels: Vec<[f32; 3]>) -> anyhow::Result<Self> {
        anyhow::ensure!(
            pixels.len() == width.saturating_mul(height),
            "linear image buffer has {} pixels, expected {}x{}={}",
            pixels.len(),
            width,
            height,
            width.saturating_mul(height)
        );
        Ok(Self {
            width,
            height,
            pixels,
            space: PhantomData,
        })
    }

    /// Rewrite every pixel and retag the buffer as a different colour space.
    ///
    /// In place, and that is the whole reason it exists. The owned colour path
    /// used to demosaic into one `Vec<[f32; 3]>` and then `collect()` the
    /// converted pixels into a second one — 600 MB each on a 50 megapixel Expert
    /// RAW, both alive at the same moment. Reusing the allocation removes one of
    /// them, which bears directly on the `--jobs 8` exhaustion recorded in
    /// `docs/STATUS.md`.
    ///
    /// Each pixel's result depends only on itself, so the parallel rewrite is
    /// deterministic regardless of how rayon splits it.
    pub fn map_into<T, F>(mut self, op: F) -> Image<T>
    where
        F: Fn([f32; 3]) -> [f32; 3] + Send + Sync,
    {
        self.pixels
            .par_iter_mut()
            .for_each(|pixel| *pixel = op(*pixel));
        Image {
            width: self.width,
            height: self.height,
            pixels: self.pixels,
            space: PhantomData,
        }
    }
}

/// What the analyser, the local operators and the tone curve all work on.
pub type LinearImage = Image<SceneLinear>;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TonalClass {
    Normal,
    HighKey,
    LowKey,
    Flat,
    HighDynamicRange,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalysisStats {
    pub sampled_pixels: usize,
    pub sample_stride: usize,
    pub p005_ev: f32,
    pub p05_ev: f32,
    pub p50_ev: f32,
    pub p95_ev: f32,
    pub p995_ev: f32,
    pub center_median_ev: f32,
    pub measured_dynamic_range_ev: f32,
    pub key_score: f32,
    /// Curve-input EV the controller aimed the subject at. Recorded because the
    /// preview oracle overrides exactly this quantity.
    pub target_median_ev: f32,
    pub tonal_class: TonalClass,
    pub near_black_fraction: f32,
    pub near_white_fraction: f32,
    pub mean_chroma: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToneParams {
    pub exposure_ev: f32,
    pub black_input_ev: f32,
    pub white_input_ev: f32,
    pub black_output_linear: f32,
    pub white_output_linear: f32,
    pub black_output_ev: f32,
    pub white_output_ev: f32,
    pub contrast: f32,
    pub shadow_power: f32,
    pub highlight_power: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub highlight_desaturation: f32,
    /// How far the tone curve is driven by the brightest channel rather than by
    /// luminance, in highlights. 0 is pure luminance; 1 fully protects the
    /// brightest channel from clipping. See `tone::render_pixel`.
    pub highlight_norm: f32,
    /// Scene EV below which the sensor delivers no usable signal, when a noise
    /// estimate was available. The black point is never placed below it.
    pub noise_floor_ev: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CameraMetadata {
    pub make: String,
    pub model: String,
    pub clean_make: String,
    pub clean_model: String,
    pub source_width: usize,
    pub source_height: usize,
    pub components_per_pixel: usize,
    pub bits_per_sample: usize,
    pub orientation: String,
    pub white_balance_coefficients: [Option<f32>; 4],
    pub black_levels: Vec<Option<f32>>,
    pub white_levels: Vec<Option<f32>>,
}

#[derive(Debug, Serialize)]
pub struct Sidecar {
    pub schema_version: u32,
    pub application: String,
    pub application_version: String,
    pub automatic_profile_version: String,
    pub input: String,
    pub output: Option<String>,
    pub preset: Preset,
    pub camera: CameraMetadata,
    pub developed_width: usize,
    pub developed_height: usize,
    /// Adjustments applied to the file's stored black/white levels before
    /// development. Empty means the levels were used exactly as recorded.
    pub level_normalization: Vec<String>,
    /// DNG `BaselineExposure` applied to the scene-linear data, in EV.
    pub baseline_exposure_ev: f32,
    /// Objective measurements of the rendered image.
    pub measured: Option<crate::metrics::OutputStats>,
    /// Fitted sensor noise model, when one could be estimated.
    pub noise: Option<crate::noise::NoiseEstimate>,
    /// The floor actually used, which may be pooled across the camera and ISO.
    pub noise_floor: Option<crate::noiseprofile::NoiseFloor>,
    /// Capture settings read from EXIF.
    pub shot: Option<crate::shotinfo::ShotInfo>,
    /// Local multi-illuminant white balance, when `--local-white-balance` asked
    /// for one.
    pub local_white_balance: Option<crate::whitebalance::LocalWhiteBalance>,
    /// The camera's own rendering, when it was read and used as the target.
    pub preview: Option<crate::preview::PreviewOracle>,
    /// Full-resolution local tone adaptation, when requested.
    pub local_tone: Option<crate::localtone::LocalToneReport>,
    /// Chroma noise reduction, when the frame was noisy enough to need it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chroma_denoise: Option<crate::chroma::ChromaDenoiseReport>,
    /// Output sharpening, when the frame was clean enough to take it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharpen: Option<crate::sharpen::SharpenReport>,
    /// The camera's own JPEG of this capture, measured and differenced against
    /// our own rendering. Present only when `--reference` asked for it and a
    /// paired file was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<crate::reference::ReferenceReport>,
    /// Which colour path developed this file, on what calibration data, and —
    /// on the owned path — what Rawler's clipping would have cost.
    pub color: crate::color::ColorReport,
    /// Whether this file's exposure target was the controller's own or the
    /// camera's. Recorded per file because the decision is automatic and so
    /// differs within a single run.
    pub guidance_mode: GuidanceMode,
    /// Frozen name for the controller that produced these parameters.
    pub controller_version: String,
    pub analysis: AnalysisStats,
    pub parameters: ToneParams,
    pub limitations: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_reports_percentiles() {
        let values: Vec<f32> = (0..=10).map(|v| v as f32).collect();
        let distribution = Distribution::from_samples(values).unwrap();
        assert_eq!(distribution.count, 11);
        assert_eq!(distribution.min, 0.0);
        assert_eq!(distribution.median, 5.0);
        assert_eq!(distribution.max, 10.0);
        assert_eq!(distribution.p10, 1.0);
        assert_eq!(distribution.p90, 9.0);
        assert!((distribution.mean - 5.0).abs() < 1.0e-6);
    }

    #[test]
    fn distribution_is_none_when_empty() {
        assert!(Distribution::from_samples(Vec::new()).is_none());
    }

    #[test]
    fn distribution_sorts_unordered_input() {
        let distribution = Distribution::from_samples(vec![3.0, -1.0, 2.0]).unwrap();
        assert_eq!(distribution.min, -1.0);
        assert_eq!(distribution.median, 2.0);
        assert_eq!(distribution.max, 3.0);
    }
}

#[derive(Debug)]
pub struct ProcessReport {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub camera: String,
    pub tonal_class: TonalClass,
    pub exposure_ev: f32,
    pub elapsed_ms: u128,
    pub skipped: bool,
    pub dry_run: bool,
    /// Full measurements, kept so `--summary` works in `--dry-run` too, where
    /// no sidecars are written. `None` for skipped files.
    pub analysis: Option<AnalysisStats>,
    pub parameters: Option<ToneParams>,
    pub baseline_exposure_ev: f32,
    /// Measured only when an image was actually rendered, so this is `None`
    /// under `--dry-run`.
    pub measured: Option<crate::metrics::OutputStats>,
    /// Fitted sensor noise model. Available under `--dry-run` too, since it is
    /// estimated from the raw samples before development.
    pub noise: Option<crate::noise::NoiseEstimate>,
    /// The floor actually used, pooled or per-frame.
    pub noise_floor: Option<crate::noiseprofile::NoiseFloor>,
    /// Capture settings read from EXIF.
    pub shot: Option<crate::shotinfo::ShotInfo>,
    /// The camera's own rendering, when it was read and used.
    pub preview: Option<crate::preview::PreviewOracle>,
    pub local_tone: Option<crate::localtone::LocalToneReport>,
    /// Chroma noise reduction, when one was applied.
    pub chroma_denoise: Option<crate::chroma::ChromaDenoiseReport>,
    /// Output sharpening, when one was applied.
    pub sharpen: Option<crate::sharpen::SharpenReport>,
    /// The camera's own JPEG of this capture, measured against ours.
    pub reference: Option<crate::reference::ReferenceReport>,
    /// Which colour path ran. `None` for a skipped file, which was never decoded.
    pub color: Option<crate::color::ColorReport>,
    /// `None` for a skipped file, where no exposure decision was taken.
    pub guidance_mode: Option<GuidanceMode>,
}

/// One row of the batch summary.
#[derive(Debug, Serialize)]
pub struct SummaryEntry {
    pub input: String,
    pub output: Option<String>,
    pub camera: String,
    pub status: &'static str,
    pub elapsed_ms: u128,
    pub baseline_exposure_ev: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis: Option<AnalysisStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<ToneParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured: Option<crate::metrics::OutputStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noise: Option<crate::noise::NoiseEstimate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noise_floor: Option<crate::noiseprofile::NoiseFloor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shot: Option<crate::shotinfo::ShotInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<crate::preview::PreviewOracle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_tone: Option<crate::localtone::LocalToneReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chroma_denoise: Option<crate::chroma::ChromaDenoiseReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharpen: Option<crate::sharpen::SharpenReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<crate::reference::ReferenceReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<crate::color::ColorReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance_mode: Option<GuidanceMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One half of the split scorecard: everything measured over the files that were
/// developed in a single guidance mode.
///
/// The split is the point. `docs/REVIEW-2026-07-30.md` adopts separating
/// independent-Auto from preview-guided evaluation, because a pooled median
/// cannot distinguish "the controller is getting better at judging scenes" from
/// "more files happened to carry a usable preview this run". Run the corpus twice
/// — once plainly, once with `--no-preview` — and the `independent` block is the
/// column that measures this program's own judgement.
#[derive(Debug, Default, Serialize)]
pub struct GuidanceScorecard {
    /// Files developed in this mode.
    pub files: usize,
    /// Of those, how many had a camera JPEG to be measured against.
    pub reference_pairs: usize,
    /// Ours minus the camera's, in display EV.
    pub center_weighted_key_ev_delta: Option<Distribution>,
    /// Ours divided by the camera's; 1.0 is a match.
    pub saturation_ratio: Option<Distribution>,
    /// The same ratio over highlights only, which is where Rawler's clipping
    /// desaturates and where the whole-frame figure is nearly blind.
    pub highlight_saturation_ratio: Option<Distribution>,
    pub colourfulness_delta: Option<Distribution>,
    pub mean_level_delta: Option<Distribution>,
    /// Distribution of the per-file median hue difference against the camera, in
    /// degrees. The axis the 0.1.17 A/B lacked.
    pub hue_median_degrees: Option<Distribution>,
    /// ...and of the per-file 90th percentile, which is where a gamut-mapping
    /// failure shows up: hue errors concentrate in the out-of-gamut minority, so a
    /// median can hold still while the tail moves a long way.
    pub hue_p90_degrees: Option<Distribution>,
    /// Per-file mean hue difference. See `HueComparison::mean_degrees` for why
    /// this matters more than the median on this axis.
    pub hue_mean_degrees: Option<Distribution>,
    /// How many of `reference_pairs` could actually be compared per pixel. A gap
    /// means renderings of different crops, which the comparison refuses.
    pub hue_pairs: usize,
}

/// Machine-readable report for a whole run, written by `--summary`.
///
/// This exists so a survey of a few hundred files does not mean parsing console
/// text: `--dry-run --summary out.json` measures a batch without writing images.
#[derive(Debug, Serialize)]
pub struct BatchSummary {
    pub schema_version: u32,
    pub application_version: String,
    /// Versioned unattended defaults used by both CLI front-ends.
    pub automatic_profile_version: String,
    pub preset: Preset,
    /// Frozen name for the controller this run used.
    pub controller_version: String,
    /// Colour path the run was invoked with, and its working space.
    pub raw_color_path: crate::color::RawColorPath,
    pub working_space: crate::color::WorkingSpace,
    pub dry_run: bool,
    pub total: usize,
    pub completed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub tonal_class_counts: BTreeMap<String, usize>,
    pub exposure_ev: Option<Distribution>,
    pub colourfulness: Option<Distribution>,
    /// Distribution of per-file `near_white_fraction`.
    pub near_white_fraction: Option<Distribution>,
    pub luminance_entropy: Option<Distribution>,
    pub average_gradient: Option<Distribution>,
    /// Distribution of the per-file SNR=10 crossing, in scene EV.
    pub snr10_ev: Option<Distribution>,
    /// How many files `--reference` managed to pair with a camera JPEG.
    pub reference_pairs: usize,
    /// Ours minus the camera's, over the paired files. The exposure difference
    /// is measured even under `--dry-run`; the rest needs a render.
    pub reference_center_weighted_key_ev_delta: Option<Distribution>,
    pub reference_colourfulness_delta: Option<Distribution>,
    pub reference_mean_level_delta: Option<Distribution>,
    /// Our saturation divided by the camera's; 1.0 is a match.
    pub reference_saturation_ratio: Option<Distribution>,
    /// The same, over highlights only.
    pub reference_highlight_saturation_ratio: Option<Distribution>,
    /// Per-file median hue difference against the camera, in degrees.
    pub reference_hue_median_degrees: Option<Distribution>,
    pub reference_hue_p90_degrees: Option<Distribution>,
    pub reference_hue_mean_degrees: Option<Distribution>,
    /// Of `reference_pairs`, how many were comparable per pixel.
    pub reference_hue_pairs: usize,
    /// How many files landed in each guidance mode.
    pub guidance_mode_counts: BTreeMap<String, usize>,
    /// The same reference measures as above, but over only the files whose
    /// exposure the controller decided by itself.
    pub independent: GuidanceScorecard,
    /// ...and over only the files that followed the camera's preview.
    pub preview_guided: GuidanceScorecard,
    /// Distribution of the owned colour path's `clip_cost.altered_fraction`:
    /// what share of each frame Rawler's `Calibrate` would have rewritten.
    /// Present only on `--raw-color-path owned`.
    pub clip_altered_fraction: Option<Distribution>,
    /// Distribution of `clip_cost.mean_abs_delta` — how far, on average, each
    /// channel would have moved.
    pub clip_mean_abs_delta: Option<Distribution>,
    /// Files whose calibration matrix Rawler would have chosen
    /// nondeterministically, because they carry several matrices and no D65 one.
    /// Any number above zero means the default path is not reproducible on those
    /// files and they should be developed with `--raw-color-path owned`.
    pub nondeterministic_illuminant_files: usize,
    /// Group key to frame count, for runs that pooled noise.
    pub pooled_groups: BTreeMap<String, usize>,
    pub files: Vec<SummaryEntry>,
}

/// Percentile summary of one measurement across a batch.
///
/// `Deserialize` so a noise profile written by `--noise-scan` can be read back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Distribution {
    pub count: usize,
    pub min: f32,
    pub p10: f32,
    pub median: f32,
    pub p90: f32,
    pub max: f32,
    pub mean: f32,
}

impl Distribution {
    /// Build a distribution from unsorted samples. Returns `None` when empty.
    pub fn from_samples(mut values: Vec<f32>) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        values.sort_by(f32::total_cmp);
        let at = |fraction: f32| {
            let index = ((values.len() - 1) as f32 * fraction).round() as usize;
            values[index]
        };
        Some(Self {
            count: values.len(),
            min: values[0],
            p10: at(0.10),
            median: at(0.50),
            p90: at(0.90),
            max: values[values.len() - 1],
            mean: values.iter().sum::<f32>() / values.len() as f32,
        })
    }
}
