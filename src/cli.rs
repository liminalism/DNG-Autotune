use crate::color::{RawColorPath, WorkingSpace};
use crate::memory::JobCount;
use crate::reference::ReferenceSource;
use crate::rescale::SubBlack;
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

    /// Number of RAW files held and processed concurrently, or `auto` to choose
    /// from the memory the machine has free and the size of the largest input.
    /// Per-image stages use all CPU cores whatever this is set to, and the
    /// rendered output never depends on it.
    #[arg(short = 'j', long, value_name = "N|auto", default_value_t = JobCount::Auto)]
    pub jobs: JobCount,

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

    /// Full-resolution local tone adaptation strength, 0 to 1.
    /// Off by default. Memory intensive, but it peaks below the chroma-denoise
    /// stage that --jobs auto already budgets for, so it needs no lower --jobs.
    #[arg(
        long,
        value_name = "STRENGTH",
        default_value_t = 0.0,
        allow_hyphen_values = true
    )]
    pub local_tone: f32,

    /// Take the exposure target from the camera's own embedded preview, 0 to 1.
    /// Automatic when omitted: full strength on files carrying a real preview,
    /// and inert on files carrying only a thumbnail. Pass 0 to disable it.
    #[arg(long, value_name = "STRENGTH")]
    pub preview_exposure: Option<f32>,

    /// Ignore the camera's embedded preview entirely and let the controller
    /// decide exposure on its own. Equivalent to --preview-exposure 0, named for
    /// what it is for: the independent-Auto column of the scorecard, which
    /// measures this program's own judgement rather than its ability to follow
    /// the vendor's.
    #[arg(long, conflicts_with = "preview_exposure")]
    pub no_preview: bool,

    /// Which colour conversion to develop through.
    ///
    /// `owned` is the default since 0.1.18: it uses the same matrix composition
    /// Rawler does but discards nothing. `rawler` is Rawler's `Calibrate`, which
    /// clips out-of-gamut channels to zero and replaces every pixel above 1.0
    /// with a colour/norm average, rewriting most of the highlight range of most
    /// frames before this program sees it. It is kept as the control arm of the
    /// A/B, and for reproducing pre-0.1.18 output.
    ///
    /// The flip was made on the scorecard, as `docs/REVIEW-2026-07-30.md` required:
    /// `owned` beats `rawler` on local detail (84 frames better, 24 worse) and on
    /// hue against the camera (a median 0.9 degrees closer over the 30 most
    /// out-of-gamut frames), ties on crushed shadows, and loses only on
    /// `luminance_entropy` — by a median of four millionths of a bit, on a metric
    /// this project has twice recorded as not validly one-directional.
    #[arg(long, value_enum, default_value = "owned")]
    pub raw_color_path: RawColorPath,

    /// Linear RGB space the owned colour path works in. Ignored by
    /// --raw-color-path rawler, which is hardcoded to sRGB primaries.
    ///
    /// `srgb` keeps the owned path a controlled comparison against Rawler.
    /// `rec2020` is wide enough to hold essentially every real camera colour, so
    /// the intermediate stages see far fewer out-of-gamut channels.
    #[arg(long, value_enum, default_value = "srgb")]
    pub working_space: WorkingSpace,

    /// What the owned colour path does with sensor samples below the black level.
    ///
    /// `preserve` is the default. `clip` zeroes sub-black but is otherwise
    /// correct; `rawler-compat` additionally reproduces `RawImage::apply_scaling`
    /// bit for bit, defects included, and exists as the control arm that proved the
    /// port faithful. Hidden because the last two are diagnostics, not settings.
    ///
    /// **Why `preserve` won, and why the metrics said otherwise.** Clipping
    /// sub-black rectifies the sensor noise floor, which lifts each channel's mean
    /// above true black. White balance then multiplies that pedestal unequally — on
    /// an A7C, red by about 2.3 and blue by 1.6 against green at 1.0 — so the
    /// shadows acquire a **magenta cast**. It is plainly visible on a night frame
    /// with 28% sub-black samples: the whole lower half of the image is tinted, and
    /// the darkest crop is a field of magenta speckle. Under `preserve` the noise
    /// stays symmetric about zero, averages neutral, and the cast is simply gone.
    ///
    /// The scorecard argued against this. `crushed_fraction` gets *worse* on 18 of
    /// 108 frames, and `mean_level` falls by up to 14. Both are real and both are
    /// the metric describing the fix rather than a defect: what turns black is
    /// noise that carried no detail, and it previously turned into coloured haze
    /// instead. Structure is unchanged — rock and foliage texture survive the switch
    /// intact. This is the case `docs/PLAN.md` means by "read the number, then open
    /// the pair".
    #[arg(long, value_enum, default_value = "preserve", hide = true)]
    pub sub_black: SubBlack,

    /// Write the scene-linear intermediate at each pipeline stage into DIR, as
    /// plain sRGB-encoded PNGs with no tone curve applied. Diagnostic only; it
    /// never changes what is rendered.
    #[arg(long, value_name = "DIR")]
    pub dump_stages: Option<PathBuf>,

    /// Do not copy the source EXIF or embed an sRGB ICC profile in the output.
    /// The pixels are unaffected either way; this only strips the tags, which
    /// leaves a photo library with no capture date, camera or lens to show.
    #[arg(long)]
    pub no_metadata: bool,

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

    /// Measure each render against the camera's own JPEG of the same capture
    /// and report the difference. Pairs by filename stem with a JPEG sitting
    /// next to the RAW. Measurement only: it never changes what is rendered.
    #[arg(long)]
    pub reference: bool,

    /// Look for the camera JPEGs in DIR rather than next to each RAW.
    /// Implies --reference.
    #[arg(long, value_name = "DIR")]
    pub reference_dir: Option<PathBuf>,

    /// Multiply the automatic chroma-noise-reduction strength. 1.0 is the
    /// automatic decision, which is inert on clean frames; 0 disables it.
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    pub chroma_denoise: f32,

    /// Multiply the automatic output-sharpening amount. 1.0 is the automatic
    /// decision, which fades out on noisy frames; 0 disables it.
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    pub sharpen: f32,

    /// Multiply the preset's saturation. 1.0 leaves every preset exactly as
    /// tuned; this exists so the chroma path can be swept against a corpus of
    /// RAW+JPEG pairs, the way --exposure-bias offsets the automatic exposure.
    #[arg(long, value_name = "FACTOR", default_value_t = 1.0)]
    pub saturation_scale: f32,
}

impl Cli {
    pub fn into_options(self) -> Result<(Vec<PathBuf>, RunOptions)> {
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
            self.local_tone.is_finite() && (0.0..=1.0).contains(&self.local_tone),
            "--local-tone must be between 0 and 1"
        );
        ensure!(
            self.preview_exposure
                .is_none_or(|value| value.is_finite() && (0.0..=1.0).contains(&value)),
            "--preview-exposure must be between 0 and 1"
        );
        ensure!(
            self.saturation_scale.is_finite() && (0.0..=4.0).contains(&self.saturation_scale),
            "--saturation-scale must be between 0 and 4"
        );
        ensure!(
            self.chroma_denoise.is_finite() && (0.0..=2.0).contains(&self.chroma_denoise),
            "--chroma-denoise must be between 0 and 2"
        );
        ensure!(
            self.sharpen.is_finite() && (0.0..=3.0).contains(&self.sharpen),
            "--sharpen must be between 0 and 3"
        );

        let reference = match self.reference_dir {
            Some(directory) => {
                ensure!(
                    directory.is_dir(),
                    "--reference-dir is not a directory: {}",
                    directory.display()
                );
                ReferenceSource::Directory(directory)
            }
            None if self.reference => ReferenceSource::Sibling,
            None => ReferenceSource::Disabled,
        };

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
                local_tone: self.local_tone,
                // `--no-preview` is the same decision as `--preview-exposure 0`;
                // clap rejects passing both, so this cannot silently override a
                // strength the user asked for.
                preview_exposure: if self.no_preview {
                    Some(0.0)
                } else {
                    self.preview_exposure
                },
                raw_color_path: self.raw_color_path,
                working_space: self.working_space,
                sub_black: self.sub_black,
                dump_stages: self.dump_stages,
                write_metadata: !self.no_metadata,
                noise_scan: self.noise_scan,
                noise_profile: self.noise_profile,
                pool_noise: self.pool_noise,
                summary_path: self.summary,
                reference,
                saturation_scale: self.saturation_scale,
                chroma_denoise: self.chroma_denoise,
                sharpen: self.sharpen,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options_with_local_tone(value: &str) -> Result<(Vec<PathBuf>, RunOptions)> {
        Cli::try_parse_from(["raw-autotune", ".", "--local-tone", value])
            .unwrap()
            .into_options()
    }

    #[test]
    fn local_tone_accepts_the_closed_unit_interval() {
        assert_eq!(options_with_local_tone("0").unwrap().1.local_tone, 0.0);
        assert_eq!(options_with_local_tone("1").unwrap().1.local_tone, 1.0);
    }

    /// Omitting the flag has to be distinguishable from passing 0, or the
    /// automatic decision could not be turned off.
    #[test]
    fn preview_exposure_is_automatic_when_omitted_and_zero_disables_it() {
        let options = |arguments: &[&str]| {
            Cli::try_parse_from(arguments)
                .unwrap()
                .into_options()
                .unwrap()
                .1
                .preview_exposure
        };
        assert_eq!(options(&["raw-autotune", "."]), None);
        assert_eq!(
            options(&["raw-autotune", ".", "--preview-exposure", "0"]),
            Some(0.0)
        );
        assert_eq!(
            options(&["raw-autotune", ".", "--preview-exposure", "0.5"]),
            Some(0.5)
        );
    }

    /// Rejection may happen in clap or in `into_options` — a leading minus is
    /// an unknown argument before it is ever an out-of-range float. What must
    /// not happen is a bad strength reaching the controller.
    #[test]
    fn preview_exposure_still_rejects_out_of_range_values() {
        for value in ["-0.01", "1.01", "NaN", "inf"] {
            let accepted = Cli::try_parse_from(["raw-autotune", ".", "--preview-exposure", value])
                .is_ok_and(|cli| cli.into_options().is_ok());
            assert!(!accepted, "{value} was accepted");
        }
    }

    #[test]
    fn local_tone_rejects_out_of_range_and_non_finite_values() {
        for value in ["-0.01", "1.01", "NaN", "inf"] {
            assert!(
                options_with_local_tone(value).is_err(),
                "{value} was accepted"
            );
        }
    }
}
