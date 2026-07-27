use anyhow::{Context, Result};
use clap::Parser;
use raw_autotune::cli::Cli;
use raw_autotune::noiseprofile::{NoiseProfile, NoiseSample};
use raw_autotune::{files, output, pipeline, types};
use std::collections::BTreeMap;
use std::process;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

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
fn scan_all(jobs: &[types::InputJob], options: &types::RunOptions) -> NoiseProfile {
    let samples: Vec<Option<NoiseSample>> = run_over_jobs(jobs, options.jobs, |index| {
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
    profile: Option<&NoiseProfile>,
) -> Vec<anyhow::Result<types::ProcessReport>> {
    run_over_jobs(jobs, options.jobs, |index| {
        pipeline::process_job(&jobs[index], options, profile)
    })
}

fn run() -> Result<i32> {
    let cli = Cli::parse();
    let (input_paths, options) = cli.into_options()?;
    let jobs = files::discover_inputs(&input_paths, options.recursive)?;

    if !options.dry_run {
        std::fs::create_dir_all(&options.output_dir)?;
    }

    println!(
        "raw-autotune v{} | {} file(s) | preset={} | concurrent images={}{}",
        env!("CARGO_PKG_VERSION"),
        jobs.len(),
        options.preset.as_str(),
        options.jobs,
        if options.dry_run {
            " | analysis only"
        } else {
            ""
        }
    );

    // --noise-scan produces a profile and stops.
    if let Some(path) = &options.noise_scan {
        let profile = scan_all(&jobs, &options);
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
            let profile = scan_all(&jobs, &options);
            println!(
                "pooled noise from this batch ({} group(s))",
                profile.groups.len()
            );
            Some(profile)
        }
        None => None,
    };

    let results = process_all(&jobs, &options, profile.as_ref());

    let mut completed = 0_usize;
    let mut skipped = 0_usize;
    let mut failed = 0_usize;
    let mut entries = Vec::with_capacity(jobs.len());
    let mut tonal_class_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut exposures = Vec::new();
    let mut colourfulness = Vec::new();
    let mut clipped = Vec::new();
    let mut snr10 = Vec::new();
    let mut pooled_groups: BTreeMap<String, usize> = BTreeMap::new();

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
                }
                if let Some(estimate) = &report.noise {
                    snr10.push(estimate.snr10_ev);
                }
                if let Some(floor) = &report.noise_floor
                    && floor.pooled
                    && let Some(key) = &floor.group_key
                {
                    *pooled_groups.entry(key.clone()).or_default() += 1;
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
                    error: Some(format!("{error:#}")),
                });
            }
        }
    }

    println!(
        "finished: {} completed, {} skipped, {} failed",
        completed, skipped, failed
    );

    if let Some(path) = &options.summary_path {
        let summary = types::BatchSummary {
            schema_version: 1,
            application_version: env!("CARGO_PKG_VERSION").to_string(),
            preset: options.preset,
            dry_run: options.dry_run,
            total: entries.len(),
            completed,
            skipped,
            failed,
            tonal_class_counts,
            exposure_ev: types::Distribution::from_samples(exposures),
            colourfulness: types::Distribution::from_samples(colourfulness),
            near_white_fraction: types::Distribution::from_samples(clipped),
            snr10_ev: types::Distribution::from_samples(snr10),
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
