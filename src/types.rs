use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const MID_GRAY: f32 = 0.18;

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

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub output_dir: PathBuf,
    pub format: OutputFormat,
    pub preset: Preset,
    pub exposure_bias_ev: f32,
    pub jobs: usize,
    pub overwrite: bool,
    pub emit_baseline: bool,
    pub write_sidecar: bool,
    pub jpeg_quality: u8,
    pub max_samples: usize,
    pub recursive: bool,
    pub dry_run: bool,
    pub local_white_balance: f32,
    pub preview_exposure: f32,
    pub noise_scan: Option<PathBuf>,
    pub noise_profile: Option<PathBuf>,
    pub pool_noise: bool,
    pub summary_path: Option<PathBuf>,
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

#[derive(Debug, Clone)]
pub struct LinearImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[f32; 3]>,
}

impl LinearImage {
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
        })
    }
}

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
    pub error: Option<String>,
}

/// Machine-readable report for a whole run, written by `--summary`.
///
/// This exists so a survey of a few hundred files does not mean parsing console
/// text: `--dry-run --summary out.json` measures a batch without writing images.
#[derive(Debug, Serialize)]
pub struct BatchSummary {
    pub schema_version: u32,
    pub application_version: String,
    pub preset: Preset,
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
    /// Distribution of the per-file SNR=10 crossing, in scene EV.
    pub snr10_ev: Option<Distribution>,
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
