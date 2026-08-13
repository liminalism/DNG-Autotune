use anyhow::{Context, Result};
use clap::Parser;
use raw_autotune::cli::Cli;
use raw_autotune::noiseprofile::{NoiseProfile, NoiseSample};
use raw_autotune::raw_highlight::HighlightMethod;
use raw_autotune::{files, memory, output, pipeline, types};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

mod interactive;
mod interactive_menu;

/// Reference measures collected for one guidance mode, before they become
/// distributions.
///
/// Exists so the independent and preview-guided halves of the scorecard are
/// accumulated by identical code — a split whose two sides are computed
/// differently would be worse than no split at all.
#[derive(Default)]
struct GuidanceAccumulator {
    files: usize,
    reference_pairs: usize,
    center_weighted_key_ev: Vec<f32>,
    saturation_ratio: Vec<f32>,
    highlight_saturation_ratio: Vec<f32>,
    colourfulness: Vec<f32>,
    mean_level: Vec<f32>,
    hue_median: Vec<f32>,
    hue_p90: Vec<f32>,
    hue_mean: Vec<f32>,
}

impl GuidanceAccumulator {
    fn finish(self) -> types::GuidanceScorecard {
        types::GuidanceScorecard {
            files: self.files,
            reference_pairs: self.reference_pairs,
            center_weighted_key_ev_delta: types::Distribution::from_samples(
                self.center_weighted_key_ev,
            ),
            saturation_ratio: types::Distribution::from_samples(self.saturation_ratio),
            highlight_saturation_ratio: types::Distribution::from_samples(
                self.highlight_saturation_ratio,
            ),
            colourfulness_delta: types::Distribution::from_samples(self.colourfulness),
            mean_level_delta: types::Distribution::from_samples(self.mean_level),
            hue_pairs: self.hue_median.len(),
            hue_median_degrees: types::Distribution::from_samples(self.hue_median),
            hue_p90_degrees: types::Distribution::from_samples(self.hue_p90),
            hue_mean_degrees: types::Distribution::from_samples(self.hue_mean),
        }
    }
}

/// Run `work` over every job on `worker_count` threads, returning results in
/// job order regardless of completion order.
fn run_over_jobs<T, F>(jobs: &[types::InputJob], worker_count: usize, work: F) -> Vec<T>
where
    T: Send,
    F: Fn(usize) -> T + Sync,
{
    let worker_count = worker_count.min(jobs.len()).max(1);
    let next_job = AtomicUsize::new(0);
    let completed = Mutex::new(Vec::with_capacity(jobs.len()));

    thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| {
                loop {
                    let index = next_job.fetch_add(1, Ordering::Relaxed);
                    if index >= jobs.len() {
                        break;
                    }
                    let result = work(index);
                    completed
                        .lock()
                        .expect("result mutex was poisoned")
                        .push((index, result));
                }
            });
        }
    });

    let mut completed = completed
        .into_inner()
        .expect("result mutex was poisoned after workers completed");
    completed.sort_by_key(|(index, _)| *index);
    completed.into_iter().map(|(_, result)| result).collect()
}

/// Decode each input far enough to fit its noise model, and pool the results.
fn scan_all(jobs: &[types::InputJob], workers: usize) -> NoiseProfile {
    let samples: Vec<Option<NoiseSample>> = run_over_jobs(jobs, workers, |index| {
        match pipeline::scan_noise(&jobs[index]) {
            Ok(sample) => sample,
            Err(error) => {
                eprintln!("SCAN  skipped: {error:#}");
                None
            }
        }
    });

    let samples: Vec<NoiseSample> = samples.into_iter().flatten().collect();
    NoiseProfile::build(&samples)
}

fn process_all(
    jobs: &[types::InputJob],
    options: &types::RunOptions,
    workers: usize,
    profile: Option<&NoiseProfile>,
    highlight_method: HighlightMethod,
) -> Vec<anyhow::Result<types::ProcessReport>> {
    run_over_jobs(jobs, workers, |index| {
        pipeline::process_job_with_highlight_method(
            &jobs[index],
            options,
            profile,
            highlight_method,
        )
    })
}

fn run() -> Result<i32> {
    // The bare executable, with no arguments, launches the interactive wizard.
    // Any argument at all routes through the flag-based CLI, so every documented
    // batch invocation keeps working unchanged.
    let (input_paths, options, highlight_method) = if std::env::args().len() <= 1 {
        match interactive::run()? {
            Some((inputs, options)) => {
                let highlight_method = options.highlight_method;
                (inputs, options, highlight_method)
            }
            None => return Ok(0),
        }
    } else {
        let cli = Cli::parse();
        cli.into_pipeline_options()?
    };

    run_pipeline(input_paths, options, highlight_method)
}

fn run_pipeline(
    input_paths: Vec<PathBuf>,
    options: types::RunOptions,
    highlight_method: HighlightMethod,
) -> Result<i32> {
    let jobs = files::discover_inputs(&input_paths, options.recursive)?;

    if !options.dry_run {
        std::fs::create_dir_all(&options.output_dir)?;
    }

    // Decided once, before anything is decoded, from the size of every input
    // and the memory the machine reports free. It changes only how many files
    // are in flight, never how any of them is rendered.
    let plan = memory::plan_with_highlight_method(
        options.jobs,
        &jobs,
        options.lens_correction,
        highlight_method,
    )
    .map_err(anyhow::Error::msg)?;

    println!(
        "raw-autotune v{} | {} file(s) | profile={} | preset={} | concurrent images={}{}{}",
        env!("CARGO_PKG_VERSION"),
        jobs.len(),
        types::RunOptions::AUTO_PROFILE_VERSION,
        options.preset.as_str(),
        plan.workers,
        plan.describe(options.jobs),
        if options.dry_run {
            " | analysis only"
        } else {
            ""
        }
    );

    // --noise-scan produces a profile and stops.
    if let Some(path) = &options.noise_scan {
        let profile = scan_all(&jobs, plan.workers);
        output::save_noise_profile(path, &profile)?;
        println!(
            "noise profile: {} ({} group(s))",
            path.display(),
            profile.groups.len()
        );
        return Ok(0);
    }

    let profile = match &options.noise_profile {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read noise profile {}", path.display()))?;
            let profile: NoiseProfile = serde_json::from_str(&text)
                .with_context(|| format!("failed to parse noise profile {}", path.display()))?;
            println!(
                "noise profile: {} ({} group(s))",
                path.display(),
                profile.groups.len()
            );
            Some(profile)
        }
        None if options.pool_noise => {
            let profile = scan_all(&jobs, plan.workers);
            println!(
                "pooled noise from this batch ({} group(s))",
                profile.groups.len()
            );
            Some(profile)
        }
        None => None,
    };

    let results = process_all(
        &jobs,
        &options,
        plan.workers,
        profile.as_ref(),
        highlight_method,
    );

    let mut completed = 0_usize;
    let mut skipped = 0_usize;
    let mut failed = 0_usize;
    let mut entries = Vec::with_capacity(jobs.len());
    let mut tonal_class_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut exposures = Vec::new();
    let mut colourfulness = Vec::new();
    let mut clipped = Vec::new();
    let mut luminance_entropy = Vec::new();
    let mut average_gradient = Vec::new();
    let mut snr10 = Vec::new();
    let mut low_light = Vec::new();
    let mut pooled_groups: BTreeMap<String, usize> = BTreeMap::new();
    let mut reference_pairs = 0_usize;
    let mut reference_center_weighted_key_ev = Vec::new();
    let mut reference_colourfulness = Vec::new();
    let mut reference_mean_level = Vec::new();
    let mut reference_saturation_ratio = Vec::new();
    let mut reference_highlight_saturation_ratio = Vec::new();
    let mut reference_hue_median = Vec::new();
    let mut reference_hue_p90 = Vec::new();
    let mut reference_hue_mean = Vec::new();
    let mut guidance_mode_counts: BTreeMap<String, usize> = BTreeMap::new();
    // The split scorecard `docs/REVIEW-2026-07-30.md` asks for: the same
    // reference measures accumulated separately per guidance mode, so a run can
    // say whether the controller's own judgement improved without the answer
    // being diluted by however many files happened to carry a usable preview.
    let mut split: BTreeMap<&'static str, GuidanceAccumulator> = BTreeMap::new();
    let mut clip_altered = Vec::new();
    let mut clip_mean_abs_delta = Vec::new();
    let mut hot_corrected = Vec::new();
    let mut highlight_reconstructed = Vec::new();
    // Tracked separately from the rebuilt count because it is the number that
    // predicts sky quality: these are the pixels whose hue this program decided
    // rather than measured. A frame where it is large has no sky detail left in
    // the raw at all, whatever the render does afterwards.
    let mut highlight_near_white = Vec::new();
    let mut dng_color_frames = 0_usize;
    let mut nondeterministic_illuminant_files = 0_usize;

    for (job, result) in jobs.iter().zip(results) {
        match result {
            Ok(report) if report.skipped => {
                skipped += 1;
                println!("SKIP  {} (output already exists)", report.input.display());
                entries.push(types::SummaryEntry {
                    input: report.input.to_string_lossy().into_owned(),
                    output: report.output.map(|p| p.to_string_lossy().into_owned()),
                    camera: report.camera,
                    status: "skipped",
                    elapsed_ms: report.elapsed_ms,
                    baseline_exposure_ev: report.baseline_exposure_ev,
                    analysis: None,
                    parameters: None,
                    measured: None,
                    noise: report.noise,
                    noise_floor: report.noise_floor,
                    shot: report.shot,
                    preview: report.preview,
                    local_tone: report.local_tone,
                    hdr: report.hdr,
                    chroma_denoise: report.chroma_denoise,
                    luma_denoise: report.luma_denoise,
                    scene: report.scene,
                    sharpen: report.sharpen,
                    reference: report.reference,
                    color: report.color,
                    guidance_mode: report.guidance_mode,
                    error: None,
                });
            }
            Ok(report) => {
                completed += 1;
                let destination = report
                    .output
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "analysis only".to_string());
                println!(
                    "OK    {} -> {} | {} | {:?} | exposure {:+.2} EV | {} ms{}",
                    report.input.display(),
                    destination,
                    report.camera,
                    report.tonal_class,
                    report.exposure_ev,
                    report.elapsed_ms,
                    if report.dry_run { " | dry-run" } else { "" }
                );

                *tonal_class_counts
                    .entry(format!("{:?}", report.tonal_class))
                    .or_default() += 1;
                exposures.push(report.exposure_ev);
                if let Some(stats) = &report.measured {
                    colourfulness.push(stats.colourfulness);
                    clipped.push(stats.near_white_fraction);
                    luminance_entropy.push(stats.luminance_entropy);
                    average_gradient.push(stats.average_gradient);
                }
                if let Some(estimate) = &report.noise {
                    snr10.push(estimate.snr10_ev);
                }
                if let Some(stats) = &report.analysis {
                    low_light.push(stats.low_light_score);
                }
                if let Some(floor) = &report.noise_floor
                    && floor.pooled
                    && let Some(key) = &floor.group_key
                {
                    *pooled_groups.entry(key.clone()).or_default() += 1;
                }
                if let Some(reference) = &report.reference {
                    reference_pairs += 1;
                    reference_center_weighted_key_ev
                        .push(reference.delta.center_weighted_key_display_ev);
                    reference_colourfulness.extend(reference.delta.colourfulness);
                    reference_mean_level.extend(reference.delta.mean_level);
                    reference_saturation_ratio.extend(reference.delta.saturation_ratio);
                    reference_highlight_saturation_ratio
                        .extend(reference.delta.highlight_saturation_ratio);
                    if let Some(hue) = &reference.delta.hue {
                        reference_hue_median.push(hue.median_degrees);
                        reference_hue_p90.push(hue.p90_degrees);
                        reference_hue_mean.push(hue.mean_degrees);
                    }
                }
                if let Some(mode) = report.guidance_mode {
                    *guidance_mode_counts
                        .entry(mode.as_str().to_string())
                        .or_default() += 1;
                    let accumulator = split.entry(mode.as_str()).or_default();
                    accumulator.files += 1;
                    if let Some(reference) = &report.reference {
                        accumulator.reference_pairs += 1;
                        accumulator
                            .center_weighted_key_ev
                            .push(reference.delta.center_weighted_key_display_ev);
                        accumulator
                            .saturation_ratio
                            .extend(reference.delta.saturation_ratio);
                        accumulator
                            .highlight_saturation_ratio
                            .extend(reference.delta.highlight_saturation_ratio);
                        accumulator
                            .colourfulness
                            .extend(reference.delta.colourfulness);
                        accumulator.mean_level.extend(reference.delta.mean_level);
                        if let Some(hue) = &reference.delta.hue {
                            accumulator.hue_median.push(hue.median_degrees);
                            accumulator.hue_p90.push(hue.p90_degrees);
                            accumulator.hue_mean.push(hue.mean_degrees);
                        }
                    }
                }
                if let Some(color) = &report.color {
                    if color.illuminant == "nondeterministic" {
                        nondeterministic_illuminant_files += 1;
                    }
                    if let Some(cost) = &color.clip_cost {
                        clip_altered.push(cost.altered_fraction);
                        clip_mean_abs_delta.push(cost.mean_abs_delta);
                    }
                    if let Some(hot) = &color.hot_pixels {
                        hot_corrected.push(hot.corrected());
                    }
                    if let Some(highlight) = &color.highlight_reconstruction {
                        highlight_reconstructed.push(highlight.reconstructed_pixels);
                        highlight_near_white.push(highlight.near_white_pixels);
                    }
                    if color.dng_color.is_some() {
                        dng_color_frames += 1;
                    }
                }
                entries.push(types::SummaryEntry {
                    input: report.input.to_string_lossy().into_owned(),
                    output: report.output.map(|p| p.to_string_lossy().into_owned()),
                    camera: report.camera,
                    status: if report.dry_run {
                        "analyzed"
                    } else {
                        "completed"
                    },
                    elapsed_ms: report.elapsed_ms,
                    baseline_exposure_ev: report.baseline_exposure_ev,
                    analysis: report.analysis,
                    parameters: report.parameters,
                    measured: report.measured,
                    noise: report.noise,
                    noise_floor: report.noise_floor,
                    shot: report.shot,
                    preview: report.preview,
                    local_tone: report.local_tone,
                    hdr: report.hdr,
                    chroma_denoise: report.chroma_denoise,
                    luma_denoise: report.luma_denoise,
                    scene: report.scene,
                    sharpen: report.sharpen,
                    reference: report.reference,
                    color: report.color,
                    guidance_mode: report.guidance_mode,
                    error: None,
                });
            }
            Err(error) => {
                failed += 1;
                eprintln!("ERROR {error:#}");
                entries.push(types::SummaryEntry {
                    input: job.input.to_string_lossy().into_owned(),
                    output: None,
                    camera: "not decoded".to_string(),
                    status: "failed",
                    elapsed_ms: 0,
                    baseline_exposure_ev: 0.0,
                    analysis: None,
                    parameters: None,
                    measured: None,
                    noise: None,
                    noise_floor: None,
                    shot: None,
                    preview: None,
                    local_tone: None,
                    hdr: None,
                    chroma_denoise: None,
                    luma_denoise: None,
                    scene: None,
                    sharpen: None,
                    reference: None,
                    color: None,
                    guidance_mode: None,
                    error: Some(format!("{error:#}")),
                });
            }
        }
    }

    println!(
        "finished: {} completed, {} skipped, {} failed",
        completed, skipped, failed
    );

    let median =
        |values: &[f32]| types::Distribution::from_samples(values.to_vec()).map(|d| d.median);

    if !guidance_mode_counts.is_empty() {
        println!(
            "guidance: {}",
            guidance_mode_counts
                .iter()
                .map(|(mode, count)| format!("{count} {mode}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    if options.reference.is_enabled() {
        println!(
            "reference: {} of {} paired | median key {:+.2} EV | saturation x{} | colourfulness {} | level {}",
            reference_pairs,
            completed,
            median(&reference_center_weighted_key_ev).unwrap_or(f32::NAN),
            median(&reference_saturation_ratio)
                .map_or_else(|| "-".to_string(), |value| format!("{value:.3}")),
            median(&reference_colourfulness)
                .map_or_else(|| "-".to_string(), |value| format!("{value:+.1}")),
            median(&reference_mean_level)
                .map_or_else(|| "-".to_string(), |value| format!("{value:+.1}")),
        );

        // The split, printed even when one side is empty: an absent
        // independent-Auto row is itself the finding that a run measured only
        // the program's ability to follow the vendor.
        for mode in [
            types::GuidanceMode::Independent,
            types::GuidanceMode::PreviewGuided,
        ] {
            let Some(accumulator) = split.get(mode.as_str()) else {
                continue;
            };
            println!(
                "  {:>15}: {} files, {} paired | median key {} EV | saturation x{}",
                mode.as_str(),
                accumulator.files,
                accumulator.reference_pairs,
                median(&accumulator.center_weighted_key_ev)
                    .map_or_else(|| "-".to_string(), |value| format!("{value:+.2}")),
                median(&accumulator.saturation_ratio)
                    .map_or_else(|| "-".to_string(), |value| format!("{value:.3}")),
            );
        }

        // The axis the 0.1.17 A/B lacked. Printed separately from the saturation
        // rows because it answers a different question: not "as colourful as the
        // camera" but "the same colour as the camera".
        if !reference_hue_median.is_empty() {
            println!(
                "hue: {} of {} pairs comparable | median {}\u{b0} | p90 {}\u{b0} | mean {}\u{b0} | highlight saturation x{}",
                reference_hue_median.len(),
                reference_pairs,
                median(&reference_hue_median)
                    .map_or_else(|| "-".to_string(), |value| format!("{value:.2}")),
                median(&reference_hue_p90)
                    .map_or_else(|| "-".to_string(), |value| format!("{value:.2}")),
                median(&reference_hue_mean)
                    .map_or_else(|| "-".to_string(), |value| format!("{value:.2}")),
                median(&reference_highlight_saturation_ratio)
                    .map_or_else(|| "-".to_string(), |value| format!("{value:.3}")),
            );
        }
    }

    // The measurement the owned colour path exists to produce.
    if !clip_altered.is_empty() {
        println!(
            "clip cost: rawler's Calibrate would have rewritten a median {:.3}% of pixels \
             (max {:.3}%), median mean movement {:.5}",
            median(&clip_altered).unwrap_or(f32::NAN) * 100.0,
            clip_altered.iter().copied().fold(0.0_f32, f32::max) * 100.0,
            median(&clip_mean_abs_delta).unwrap_or(f32::NAN),
        );
    }

    if !hot_corrected.is_empty() {
        let total: usize = hot_corrected.iter().sum();
        let frames = hot_corrected.iter().filter(|count| **count > 0).count();
        println!("hot/dead pixels: {total} site(s) corrected across {frames} frame(s)");
    }
    if !highlight_reconstructed.is_empty() {
        let total: usize = highlight_reconstructed.iter().sum();
        let frames = highlight_reconstructed
            .iter()
            .filter(|count| **count > 0)
            .count();
        println!("highlight reconstruction: {total} pixel(s) rebuilt across {frames} frame(s)");
    }
    if !highlight_near_white.is_empty() {
        let total: usize = highlight_near_white.iter().sum();
        let frames = highlight_near_white
            .iter()
            .filter(|count| **count > 0)
            .count();
        let worst = highlight_near_white.iter().copied().max().unwrap_or(0);
        if total > 0 {
            println!(
                "  of which near-white (2+ channels at the sensor clip): {total} pixel(s) \
                 across {frames} frame(s), worst frame {worst}"
            );
        }
    }
    if dng_color_frames > 0 {
        println!(
            "full DNG colour: composed the ForwardMatrix transform for {dng_color_frames} file(s)"
        );
    }

    if nondeterministic_illuminant_files > 0 {
        println!(
            "warning: {nondeterministic_illuminant_files} file(s) carry several calibration \
             matrices and no D65 one, so rawler's Calibrate picks between them by HashMap order \
             and does not develop them reproducibly. Use --raw-color-path owned for those files."
        );
    }

    if let Some(path) = &options.summary_path {
        let summary = types::BatchSummary {
            schema_version: types::report_schema_version(options.semantic),
            application_version: env!("CARGO_PKG_VERSION").to_string(),
            automatic_profile_version: types::RunOptions::AUTO_PROFILE_VERSION.to_string(),
            preset: options.preset,
            controller_version: types::CONTROLLER_VERSION.to_string(),
            raw_color_path: options.raw_color_path,
            working_space: options.working_space,
            dry_run: options.dry_run,
            total: entries.len(),
            completed,
            skipped,
            failed,
            tonal_class_counts,
            exposure_ev: types::Distribution::from_samples(exposures),
            colourfulness: types::Distribution::from_samples(colourfulness),
            near_white_fraction: types::Distribution::from_samples(clipped),
            luminance_entropy: types::Distribution::from_samples(luminance_entropy),
            average_gradient: types::Distribution::from_samples(average_gradient),
            snr10_ev: types::Distribution::from_samples(snr10),
            low_light_score: types::Distribution::from_samples(low_light),
            reference_pairs,
            reference_center_weighted_key_ev_delta: types::Distribution::from_samples(
                reference_center_weighted_key_ev,
            ),
            reference_colourfulness_delta: types::Distribution::from_samples(
                reference_colourfulness,
            ),
            reference_mean_level_delta: types::Distribution::from_samples(reference_mean_level),
            reference_saturation_ratio: types::Distribution::from_samples(
                reference_saturation_ratio,
            ),
            reference_highlight_saturation_ratio: types::Distribution::from_samples(
                reference_highlight_saturation_ratio,
            ),
            reference_hue_pairs: reference_hue_median.len(),
            reference_hue_median_degrees: types::Distribution::from_samples(reference_hue_median),
            reference_hue_p90_degrees: types::Distribution::from_samples(reference_hue_p90),
            reference_hue_mean_degrees: types::Distribution::from_samples(reference_hue_mean),
            guidance_mode_counts,
            independent: split
                .remove(types::GuidanceMode::Independent.as_str())
                .unwrap_or_default()
                .finish(),
            preview_guided: split
                .remove(types::GuidanceMode::PreviewGuided.as_str())
                .unwrap_or_default()
                .finish(),
            clip_altered_fraction: types::Distribution::from_samples(clip_altered),
            clip_mean_abs_delta: types::Distribution::from_samples(clip_mean_abs_delta),
            nondeterministic_illuminant_files,
            pooled_groups,
            files: entries,
        };
        output::save_summary(path, &summary)?;
        println!("summary: {}", path.display());
    }

    Ok(if failed == 0 { 0 } else { 2 })
}

fn main() {
    match run() {
        Ok(code) => process::exit(code),
        Err(error) => {
            eprintln!("fatal: {error:#}");
            process::exit(1);
        }
    }
}
