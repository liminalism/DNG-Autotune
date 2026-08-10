use crate::color::{RawColorPath, WorkingSpace};
use crate::demosaic::DemosaicMethod;
use crate::lens::LensCorrectionMode;
use crate::memory::JobCount;
use crate::raw_highlight::HighlightMethod;
use crate::reference::ReferenceSource;
use crate::rescale::SubBlack;
use crate::types::{JpegSettings, JpegSubsampling, OutputFormat, Preset, RunOptions};
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
    #[arg(long, value_enum, default_value = "jpeg")]
    pub format: OutputFormat,

    /// Automatic rendering style.
    #[arg(long, value_enum, default_value = "auto")]
    pub preset: Preset,

    /// Manual EV offset added after automatic exposure estimation.
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub exposure_bias: f32,

    /// Number of RAW files held and processed concurrently, or `auto` to choose
    /// from the memory the machine has free and the size of the largest input.
    /// A numeric request is still capped by the memory-safety ceiling.
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

    /// Write a JSON analysis/parameter sidecar next to every image. The
    /// automatic archive profile writes one batch summary instead.
    #[arg(long)]
    pub sidecar: bool,

    /// Deprecated compatibility spelling. Sidecars are already off by default.
    #[arg(long, hide = true, conflicts_with = "sidecar")]
    pub no_sidecar: bool,

    /// JPEG quality from 1 to 100.
    #[arg(long, default_value_t = 95)]
    pub jpeg_quality: u8,

    /// Emit a progressive JPEG instead of a baseline one. Smaller files, slower
    /// to encode and decode. Only affects `--format jpeg`.
    #[arg(long)]
    pub jpeg_progressive: bool,

    /// Keep JPEG Huffman optimization disabled. The automatic archive profile
    /// enables it because it changes file size, not pixels.
    #[arg(long)]
    pub no_jpeg_optimize: bool,

    /// JPEG chroma subsampling. `auto` uses 4:4:4 at quality >= 90 and 4:2:0
    /// below it. Only affects `--format jpeg`.
    #[arg(long, value_enum, default_value = "444")]
    pub jpeg_subsampling: JpegSubsampling,

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

    /// HDR-like single-frame local tone, 0 to 1. Off by default. Edge-aware
    /// base/detail split in log luminance: compresses the broad sky/ground range
    /// while keeping local contrast, one scalar gain per pixel. Not wired into
    /// the automatic profile yet; mutually exclusive with --local-tone.
    #[arg(
        long,
        value_name = "STRENGTH",
        default_value_t = 0.0,
        allow_hyphen_values = true
    )]
    pub hdr: f32,

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

    /// Bayer demosaic policy. Auto keeps mature PPG for textured/noisy mosaics
    /// and uses owned RCD/AMaZE-class interpolation only behind strict guards.
    #[arg(long, value_enum, default_value = "auto")]
    pub demosaic: DemosaicMethod,

    /// Multiply the preset's saturation. 1.0 leaves every preset exactly as
    /// tuned; this exists so the chroma path can be swept against a corpus of
    /// RAW+JPEG pairs, the way --exposure-bias offsets the automatic exposure.
    #[arg(long, value_name = "FACTOR", default_value_t = 1.0)]
    pub saturation_scale: f32,

    /// Multiply the tone curve's highlight exponent. 1.0 leaves every preset
    /// exactly as tuned. Above 1.0 places bright regions higher on the curve
    /// without moving middle grey, the black point or the shadows, so a sky
    /// brightens and reaches the highlight-desaturation shoulder while the
    /// ground stays put. This is the sweep knob for the one documented
    /// divergence from the camera's own rendering (docs/STATUS.md: the camera
    /// desaturates its shoulder hard and this program deliberately does not) —
    /// a taste decision that belongs to whoever is grading the corpus.
    #[arg(long, value_name = "FACTOR", default_value_t = 1.0)]
    pub highlight_contrast: f32,

    /// Compression-aware highlight color/luminance ratio exponent, 0 to 1.
    /// 1 preserves the existing rendering. Values below 1 are experimental;
    /// the display-adaptive paper used 0.6.
    #[arg(long, value_name = "EXPONENT", default_value_t = 1.0)]
    pub highlight_color_ratio_exponent: f32,

    /// Suppress hot and dead pixels on the CFA mosaic before demosaic, 0 to 1.
    /// Defaults to a conservative automatic strength. A single stuck
    /// photosite becomes a coloured speck the
    /// demosaic then smears, so it is cheaper to kill before interpolation.
    /// Owned colour path only (the default); ignored with --raw-color-path rawler.
    #[arg(long, value_name = "STRENGTH", default_value_t = 0.5)]
    pub hot_pixels: f32,

    /// Reconstruct clipped highlights from their surviving channels, 0 to 1.
    /// The automatic harmonic estimator requires full strength. Rebuilds a channel that saturated before the others so a
    /// partially-blown highlight renders neutral instead of tinted. Owned colour
    /// path only (the default); ignored with --raw-color-path rawler.
    #[arg(long, value_name = "STRENGTH", default_value_t = 1.0)]
    pub highlight_reconstruction: f32,

    /// Highlight estimator. Harmonic is the automatic first-principles path;
    /// every spatial method requires `--highlight-reconstruction 1`.
    #[arg(long, value_enum, default_value = "harmonic")]
    pub highlight_method: HighlightMethod,

    /// Compatibility spelling: the DNG matrix-profile path is already automatic.
    #[arg(long, conflicts_with = "no_dng_color", hide = true)]
    pub dng_color: bool,

    /// Disable the automatic DNG matrix-profile path.
    #[arg(long, conflicts_with = "dng_color")]
    pub no_dng_color: bool,

    /// Post-demosaic lens correction. `embedded` preserves the unattended
    /// DNG-only policy; `profile-exact` opts into the pinned Lensfun database
    /// when no embedded warp exists; `off` disables correction.
    #[arg(long, value_enum, default_value = "embedded")]
    pub lens_correction: LensCorrectionMode,

    /// Compatibility alias for `--lens-correction off`.
    #[arg(long, conflicts_with = "lens_correction")]
    pub no_lens_correction: bool,

    /// Do not write the automatic batch summary. `--summary FILE` still
    /// chooses a custom path.
    #[arg(long, conflicts_with = "summary")]
    pub no_summary: bool,
}

impl Cli {
    /// Existing library-facing conversion. The selected estimator is retained
    /// in `RunOptions`, so automatic and explicitly configured callers agree.
    pub fn into_options(self) -> Result<(Vec<PathBuf>, RunOptions)> {
        let (inputs, options, _) = self.into_pipeline_options()?;
        Ok((inputs, options))
    }

    /// Convert CLI arguments, retaining the internal estimator selector used
    /// by the binary's batch entry point.
    pub fn into_pipeline_options(self) -> Result<(Vec<PathBuf>, RunOptions, HighlightMethod)> {
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
            self.hdr.is_finite() && (0.0..=1.0).contains(&self.hdr),
            "--hdr must be between 0 and 1"
        );
        ensure!(
            !(self.hdr > 0.0 && self.local_tone > 0.0),
            "--hdr and --local-tone are two local-tone operators; pass only one"
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
        // The lower bound is not 0: the exponent is clamped to 0.45 downstream,
        // but a zero multiplier would mean "no highlight branch at all", which
        // the curve has no sensible reading of. The upper bound is where the
        // 0.45..4.0 clamp on the exponent binds for every realistic input range.
        ensure!(
            self.highlight_contrast.is_finite() && (0.25..=4.0).contains(&self.highlight_contrast),
            "--highlight-contrast must be between 0.25 and 4"
        );
        ensure!(
            self.highlight_color_ratio_exponent.is_finite()
                && (0.0..=1.0).contains(&self.highlight_color_ratio_exponent),
            "--highlight-color-ratio-exponent must be between 0 and 1"
        );
        ensure!(
            self.chroma_denoise.is_finite() && (0.0..=2.0).contains(&self.chroma_denoise),
            "--chroma-denoise must be between 0 and 2"
        );
        ensure!(
            self.sharpen.is_finite() && (0.0..=3.0).contains(&self.sharpen),
            "--sharpen must be between 0 and 3"
        );
        ensure!(
            self.hot_pixels.is_finite() && (0.0..=1.0).contains(&self.hot_pixels),
            "--hot-pixels must be between 0 and 1"
        );
        ensure!(
            self.highlight_reconstruction.is_finite()
                && (0.0..=1.0).contains(&self.highlight_reconstruction),
            "--highlight-reconstruction must be between 0 and 1"
        );
        if self.highlight_method.is_spatial() {
            ensure!(
                self.raw_color_path == RawColorPath::Owned,
                "--highlight-method {} requires --raw-color-path owned",
                self.highlight_method.as_str()
            );
            ensure!(
                self.highlight_reconstruction == 1.0,
                "--highlight-method {} requires --highlight-reconstruction 1",
                self.highlight_method.as_str()
            );
        }
        if self.raw_color_path == RawColorPath::Rawler
            && (self.hot_pixels > 0.0 || self.highlight_reconstruction > 0.0 || self.dng_color)
        {
            eprintln!(
                "note: --hot-pixels, --highlight-reconstruction and --dng-color act only on the \
                 owned colour path; they are ignored with --raw-color-path rawler"
            );
        }

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
        let summary_path = if self.no_summary {
            None
        } else {
            Some(
                self.summary
                    .unwrap_or_else(|| output_dir.join("summary.json")),
            )
        };

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

        let mut options = RunOptions::automatic(output_dir);
        options.format = self.format;
        options.preset = self.preset;
        options.exposure_bias_ev = self.exposure_bias;
        options.jobs = self.jobs;
        options.overwrite = self.overwrite;
        options.emit_baseline = self.emit_baseline;
        options.write_sidecar = self.sidecar && !self.no_sidecar;
        options.jpeg = JpegSettings {
            quality: self.jpeg_quality,
            progressive: self.jpeg_progressive,
            optimized_huffman: !self.no_jpeg_optimize,
            subsampling: self.jpeg_subsampling,
        };
        options.max_samples = self.max_samples;
        options.recursive = !self.no_recursive;
        options.dry_run = self.dry_run;
        options.local_white_balance = self.local_white_balance;
        options.local_tone = self.local_tone;
        options.hdr = self.hdr;
        // `--no-preview` is the same decision as `--preview-exposure 0`;
        // clap rejects passing both, so this cannot silently override a
        // strength the user asked for.
        options.preview_exposure = if self.no_preview {
            Some(0.0)
        } else {
            self.preview_exposure
        };
        options.raw_color_path = self.raw_color_path;
        options.working_space = self.working_space;
        options.sub_black = self.sub_black;
        options.dump_stages = self.dump_stages;
        options.write_metadata = !self.no_metadata;
        options.noise_scan = self.noise_scan;
        options.noise_profile = self.noise_profile;
        options.pool_noise = self.pool_noise;
        options.summary_path = summary_path;
        options.reference = reference;
        options.saturation_scale = self.saturation_scale;
        options.highlight_contrast = self.highlight_contrast;
        options.highlight_color_ratio_exponent = self.highlight_color_ratio_exponent;
        options.chroma_denoise = self.chroma_denoise;
        options.sharpen = self.sharpen;
        options.demosaic = self.demosaic;
        options.hot_pixels = self.hot_pixels;
        options.highlight_reconstruction = self.highlight_reconstruction;
        options.highlight_method = self.highlight_method;
        options.full_dng_color = !self.no_dng_color;
        options.lens_correction = if self.no_lens_correction {
            LensCorrectionMode::Off
        } else {
            self.lens_correction
        };

        Ok((inputs, options, self.highlight_method))
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

    #[test]
    fn ordinary_cli_uses_the_unattended_archive_profile() {
        let options = Cli::try_parse_from(["raw-autotune", "."])
            .unwrap()
            .into_options()
            .unwrap()
            .1;
        let expected = RunOptions::automatic(PathBuf::from("raw-autotune-output"));

        assert_eq!(options.output_dir, expected.output_dir);
        assert_eq!(options.format, expected.format);
        assert_eq!(options.preset, expected.preset);
        assert_eq!(options.jobs, expected.jobs);
        assert_eq!(options.write_sidecar, expected.write_sidecar);
        assert_eq!(options.jpeg.quality, expected.jpeg.quality);
        assert_eq!(
            options.jpeg.optimized_huffman,
            expected.jpeg.optimized_huffman
        );
        assert_eq!(options.jpeg.subsampling, expected.jpeg.subsampling);
        assert_eq!(options.raw_color_path, expected.raw_color_path);
        assert_eq!(options.sub_black, expected.sub_black);
        assert_eq!(options.demosaic, expected.demosaic);
        assert_eq!(options.hot_pixels, expected.hot_pixels);
        assert_eq!(options.highlight_contrast, expected.highlight_contrast);
        assert_eq!(
            options.highlight_reconstruction,
            expected.highlight_reconstruction
        );
        assert_eq!(options.highlight_method, expected.highlight_method);
        assert_eq!(options.full_dng_color, expected.full_dng_color);
        assert_eq!(options.lens_correction, expected.lens_correction);
        assert_eq!(options.summary_path, expected.summary_path);
    }

    #[test]
    fn standardized_lens_correction_can_be_disabled() {
        let options = Cli::try_parse_from(["raw-autotune", ".", "--no-lens-correction"])
            .unwrap()
            .into_options()
            .unwrap()
            .1;
        assert_eq!(options.lens_correction, LensCorrectionMode::Off);
    }

    #[test]
    fn exact_profile_lens_correction_is_explicitly_opt_in() {
        let options =
            Cli::try_parse_from(["raw-autotune", ".", "--lens-correction", "profile-exact"])
                .unwrap()
                .into_options()
                .unwrap()
                .1;
        assert_eq!(options.lens_correction, LensCorrectionMode::ProfileExact);
    }

    #[test]
    fn spatial_highlight_methods_require_full_strength() {
        for method in ["raw-pyramid", "harmonic"] {
            let rejected = Cli::try_parse_from([
                "raw-autotune",
                ".",
                "--highlight-method",
                method,
                "--highlight-reconstruction",
                "0.75",
            ])
            .unwrap()
            .into_pipeline_options();
            assert!(
                rejected.is_err(),
                "{method} accepted partial reconstruction strength"
            );

            let (_, _, selected) = Cli::try_parse_from([
                "raw-autotune",
                ".",
                "--highlight-method",
                method,
                "--highlight-reconstruction",
                "1",
            ])
            .unwrap()
            .into_pipeline_options()
            .unwrap();
            assert!(selected.is_spatial());
        }

        let (_, options, selected) = Cli::try_parse_from(["raw-autotune", "."])
            .unwrap()
            .into_pipeline_options()
            .unwrap();
        assert_eq!(selected, HighlightMethod::Harmonic);
        assert_eq!(options.highlight_method, HighlightMethod::Harmonic);
        assert_eq!(options.highlight_reconstruction, 1.0);

        let (_, options, selected) = Cli::try_parse_from([
            "raw-autotune",
            ".",
            "--highlight-method",
            "current",
            "--highlight-reconstruction",
            "0.75",
        ])
        .unwrap()
        .into_pipeline_options()
        .unwrap();
        assert_eq!(selected, HighlightMethod::Current);
        assert_eq!(options.highlight_method, HighlightMethod::Current);
    }
}
