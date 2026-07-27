use crate::types::{OutputFormat, Preset, RunOptions};
use anyhow::{Context, Result, ensure};
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "raw-autotune",
    version,
    about = "Batch-develop camera RAW files with a deterministic automatic tone controller",
    long_about = None
)]
pub struct Cli {
    /// One or more RAW files or directories.
    #[arg(value_name = "INPUT", required = true)]
    pub inputs: Vec<PathBuf>,

    /// Destination directory.
    #[arg(short, long, value_name = "DIR", default_value = "raw-autotune-output")]
    pub output: PathBuf,

    /// Output image format.
    #[arg(long, value_enum, default_value = "tiff")]
    pub format: OutputFormat,

    /// Automatic rendering style.
    #[arg(long, value_enum, default_value = "auto")]
    pub preset: Preset,

    /// Manual EV offset added after automatic exposure estimation.
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub exposure_bias: f32,

    /// Number of RAW files held and processed concurrently. Per-image stages may use all CPU cores.
    #[arg(short = 'j', long, default_value_t = 1)]
    pub jobs: usize,

    /// Replace existing output files instead of skipping them.
    #[arg(long)]
    pub overwrite: bool,

    /// Also emit a simple clipped-and-sRGB baseline render for comparison.
    #[arg(long, alias = "emit-neutral")]
    pub emit_baseline: bool,

    /// Do not write the JSON analysis/parameter sidecar.
    #[arg(long)]
    pub no_sidecar: bool,

    /// JPEG quality from 1 to 100.
    #[arg(long, default_value_t = 92)]
    pub jpeg_quality: u8,

    /// Approximate maximum number of pixels sampled for global analysis.
    #[arg(long, default_value_t = 250_000)]
    pub max_samples: usize,

    /// Do not recurse into subdirectories.
    #[arg(long)]
    pub no_recursive: bool,

    /// Decode and analyze but write no images or sidecars.
    #[arg(long)]
    pub dry_run: bool,

    /// Strength of the local multi-illuminant white balance, 0 to 1.
    /// Off by default: it neutralizes colour casts that are often intentional.
    /// Only acts on frames where two or more distinct illuminants are found.
    #[arg(long, value_name = "STRENGTH", default_value_t = 0.0)]
    pub local_white_balance: f32,

    /// Take the exposure target from the camera's own embedded preview, 0 to 1.
    /// Off by default: it changes the exposure of every file that has a preview.
    #[arg(long, value_name = "STRENGTH", default_value_t = 0.0)]
    pub preview_exposure: f32,

    /// Scan the inputs, write a pooled sensor-noise profile to this path, and
    /// stop. Writes no images and never touches the output directory.
    #[arg(long, value_name = "FILE")]
    pub noise_scan: Option<PathBuf>,

    /// Use a noise profile written by --noise-scan. Pools each frame's noise
    /// floor with others of the same camera and ISO, without an extra pass.
    #[arg(long, value_name = "FILE")]
    pub noise_profile: Option<PathBuf>,

    /// Build a noise profile from this batch before processing it. Costs one
    /// extra decode per file and makes output depend on the batch composition;
    /// --noise-profile is reproducible and preferred.
    #[arg(long)]
    pub pool_noise: bool,

    /// Write a machine-readable JSON report for the whole run to this path.
    /// Works with --dry-run, which is the fast way to survey a large batch.
    #[arg(long, value_name = "FILE")]
    pub summary: Option<PathBuf>,
}

impl Cli {
    pub fn into_options(self) -> Result<(Vec<PathBuf>, RunOptions)> {
        ensure!(self.jobs > 0, "--jobs must be at least 1");
        ensure!(
            (1..=100).contains(&self.jpeg_quality),
            "--jpeg-quality must be between 1 and 100"
        );
        ensure!(
            self.max_samples >= 1_000,
            "--max-samples must be at least 1000"
        );
        ensure!(
            self.exposure_bias.is_finite(),
            "--exposure-bias must be finite"
        );
        ensure!(
            self.local_white_balance.is_finite() && (0.0..=1.0).contains(&self.local_white_balance),
            "--local-white-balance must be between 0 and 1"
        );
        ensure!(
            self.preview_exposure.is_finite() && (0.0..=1.0).contains(&self.preview_exposure),
            "--preview-exposure must be between 0 and 1"
        );

        let output_dir = self.output;

        let inputs = self
            .inputs
            .into_iter()
            .map(|path| {
                if path.exists() {
                    Ok(path)
                } else {
                    Err(anyhow::anyhow!("input does not exist: {}", path.display()))
                }
            })
            .collect::<Result<Vec<_>>>()
            .context("invalid input path")?;

        Ok((
            inputs,
            RunOptions {
                output_dir,
                format: self.format,
                preset: self.preset,
                exposure_bias_ev: self.exposure_bias,
                jobs: self.jobs,
                overwrite: self.overwrite,
                emit_baseline: self.emit_baseline,
                write_sidecar: !self.no_sidecar,
                jpeg_quality: self.jpeg_quality,
                max_samples: self.max_samples,
                recursive: !self.no_recursive,
                dry_run: self.dry_run,
                local_white_balance: self.local_white_balance,
                preview_exposure: self.preview_exposure,
                noise_scan: self.noise_scan,
                noise_profile: self.noise_profile,
                pool_noise: self.pool_noise,
                summary_path: self.summary,
            },
        ))
    }
}
