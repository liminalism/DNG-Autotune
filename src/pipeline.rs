use crate::analyze;
use crate::files;
use crate::noiseprofile::{NoiseFloor, NoiseProfile};
use crate::orientation;
use crate::output;
use crate::tone;
use crate::types::{CameraMetadata, InputJob, LinearImage, ProcessReport, RunOptions, Sidecar};
use anyhow::{Context, Result, anyhow, bail};
use rawler::RawImage;
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

fn panic_text(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn camera_metadata(raw: &RawImage) -> CameraMetadata {
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

fn develop_linear(raw: &RawImage) -> Result<LinearImage> {
    // Rawler 0.7.2 exposes `RawDevelop` as a plain step list. `ProcessingStep::SRgb`
    // is deliberately omitted so this crate receives scene-linear RGB and applies its
    // own view transform and transfer function.
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

    let intermediate = developer
        .develop_intermediate(raw)
        .context("Rawler failed during normalization/demosaic/color development")?;

    match intermediate {
        Intermediate::ThreeColor(pixels) => {
            LinearImage::new(pixels.width, pixels.height, pixels.into_inner())
        }
        Intermediate::Monochrome(pixels) => {
            let width = pixels.width;
            let height = pixels.height;
            let rgb = pixels
                .into_inner()
                .into_iter()
                .map(|value| [value; 3])
                .collect();
            LinearImage::new(width, height, rgb)
        }
        Intermediate::FourColor(_) => {
            bail!("Rawler returned an unsupported four-channel developed image")
        }
    }
}

pub fn process_job(
    job: &InputJob,
    options: &RunOptions,
    profile: Option<&NoiseProfile>,
) -> Result<ProcessReport> {
    let result = catch_unwind(AssertUnwindSafe(|| {
        process_job_inner(job, options, profile)
    }));
    match result {
        Ok(result) => result.with_context(|| format!("while processing {}", job.input.display())),
        Err(payload) => Err(anyhow!(
            "RAW decoder/developer panicked while processing {}: {}",
            job.input.display(),
            panic_text(payload)
        )),
    }
}

fn process_job_inner(
    job: &InputJob,
    options: &RunOptions,
    profile: Option<&NoiseProfile>,
) -> Result<ProcessReport> {
    let started = Instant::now();
    let paths = files::output_paths(job, options)?;

    if !options.dry_run && paths.image.exists() && !options.overwrite {
        return Ok(ProcessReport {
            input: job.input.clone(),
            output: Some(paths.image),
            camera: "not decoded".to_string(),
            tonal_class: crate::types::TonalClass::Normal,
            exposure_ev: 0.0,
            elapsed_ms: started.elapsed().as_millis(),
            skipped: true,
            dry_run: false,
            analysis: None,
            parameters: None,
            baseline_exposure_ev: 0.0,
            measured: None,
            noise: None,
            noise_floor: None,
            shot: None,
            preview: None,
            local_tone: None,
            chroma_denoise: None,
            sharpen: None,
            reference: None,
        });
    }

    eprintln!("PROCESS {}", job.input.display());

    // Read and reduce before the raw decode, for the same reason the preview is
    // read here: a 50-megapixel phone JPEG decodes to 150 MB, and holding that
    // alongside the developer's own peak would double the high-water mark for
    // no reason. Measurement only — nothing below this point renders differently
    // because a reference was found.
    let mut reference = options
        .reference
        .locate(&job.input)
        .and_then(|path| crate::reference::read(&path));
    if let Some(report) = &reference {
        eprintln!(
            "REF   {}: {} | {}x{} | subject {:+.2} EV | saturation {:.3} | colourfulness {:.1}",
            job.input.display(),
            report.path,
            report.width,
            report.height,
            report.subject_display_ev,
            report.measured.mean_saturation,
            report.measured.colourfulness,
        );
    } else if options.reference.is_enabled() {
        eprintln!("REF   {}: no paired camera JPEG", job.input.display());
    }

    // Since 0.1.13 the oracle is on by default, and the decision of whether it
    // applies is made by `preview::read`: it returns nothing for a file whose
    // only embedded image is a thumbnail, which is what makes "automatic" safe
    // without the program having to recognise camera models.
    let preview_strength = options
        .preview_exposure
        .unwrap_or(crate::preview::AUTO_STRENGTH);

    // Read before decoding the raw, and reduce to a handful of floats
    // immediately: peak memory then becomes the larger of the two stages rather
    // than their sum. A full-size preview decodes to about 150 MB, which is well
    // under rawler's own develop peak, so this costs nothing at the peak.
    let preview = (preview_strength > 0.0)
        .then(|| crate::preview::read(&job.input))
        .flatten();
    if let Some(oracle) = &preview {
        eprintln!(
            "PREV  {}: {}x{} {:?} | subject {:+.2} EV",
            job.input.display(),
            oracle.width,
            oracle.height,
            oracle.source,
            oracle.subject_display_ev
        );
    }

    let mut raw = rawler::decode_file(&job.input)
        .with_context(|| format!("failed to decode {}", job.input.display()))?;

    // Record the levels as the file stored them, then reconcile them into a
    // layout Rawler's developer accepts. The corrections are shared with the
    // debug examples so they observe exactly what the renderer does.
    let metadata = camera_metadata(&raw);
    let report = crate::apply_corrections(&job.input, &mut raw)
        .with_context(|| format!("failed to prepare {}", job.input.display()))?;
    let notes = report.notes;

    for note in &notes {
        eprintln!("NOTE  {}: {}", job.input.display(), note);
    }
    // Estimated before develop: demosaicing correlates neighbouring pixels and
    // would bias the variance downwards.
    let noise = crate::noise::estimate(&raw);
    let shot = crate::shotinfo::read(&job.input);
    if let Some(estimate) = &noise {
        eprintln!(
            "NOISE {}: ISO {} | slope {:.3} | SNR=10 at {:+.2} EV, SNR=1 at {:+.2} EV ({} tiles)",
            job.input.display(),
            shot.as_ref()
                .and_then(|info| info.iso)
                .map(|iso| iso.to_string())
                .unwrap_or_else(|| "?".to_string()),
            estimate.shot_slope,
            estimate.snr10_ev,
            estimate.snr1_ev,
            estimate.tiles_used
        );
    }

    // Pool with other frames of the same camera and ISO when a profile says to,
    // otherwise keep this frame's own estimate.
    let noise_floor: Option<NoiseFloor> = match profile {
        Some(profile) => profile.resolve(
            &raw.clean_make,
            &raw.clean_model,
            shot.as_ref().and_then(|info| info.iso),
            noise.as_ref(),
        ),
        None => crate::noiseprofile::frame_floor(noise.as_ref()),
    };
    if let Some(floor) = &noise_floor
        && floor.pooled
    {
        eprintln!(
            "POOL  {}: {} (n={}) slope {:.4} | SNR=1 at {:+.2} EV",
            job.input.display(),
            floor.group_key.as_deref().unwrap_or("?"),
            floor.group_n.unwrap_or(0),
            floor.shot_slope,
            floor.snr1_ev
        );
    }

    let camera_name = {
        let name = format!("{} {}", raw.clean_make, raw.clean_model)
            .trim()
            .to_string();
        if name.is_empty() {
            "unknown camera".to_string()
        } else {
            name
        }
    };
    let source_orientation = raw.orientation;

    let mut linear = develop_linear(&raw)?;
    drop(raw);

    // DNG BaselineExposure is defined as an offset to the scene-linear data
    // before the default rendering. Applying it here, rather than folding it
    // into the automatic exposure, keeps it out of the +/-5 EV controller clamp
    // and makes reported exposures comparable across cameras.
    if report.baseline_exposure_ev.abs() > 0.001 {
        let gain = report.baseline_exposure_ev.exp2();
        linear
            .pixels
            .iter_mut()
            .for_each(|pixel| pixel.iter_mut().for_each(|channel| *channel *= gain));
        eprintln!(
            "NOTE  {}: applied BaselineExposure {:+.2} EV",
            job.input.display(),
            report.baseline_exposure_ev
        );
    }

    let mut linear = orientation::apply_orientation(linear, source_orientation);

    // Before the colour work below and before analysis, because chroma noise is
    // colour as far as every later stage is concerned: it inflates the analyser's
    // mean chroma, it is what the saturation boost multiplies, and it is what a
    // multi-illuminant estimate would try to fit a light to.
    let chroma_denoise = crate::chroma::apply(
        &mut linear,
        noise_floor.as_ref().map(|floor| floor.snr10_ev),
        options.chroma_denoise,
    );
    if let Some(report) = &chroma_denoise {
        eprintln!(
            "CHROMA {}: strength {:.2} | radius {} px | SNR=10 at {:+.2} EV | chroma {:.5} -> {:.5}",
            job.input.display(),
            report.strength,
            report.radius,
            report.snr10_ev,
            report.mean_chroma_before,
            report.mean_chroma_after,
        );
    }

    // Applied to developed scene-linear RGB, on top of the camera's as-shot
    // white balance: this corrects the residual local cast that a single
    // global illuminant cannot.
    let local_white_balance = (options.local_white_balance > 0.0)
        .then(|| crate::whitebalance::apply(&mut linear, options.local_white_balance))
        .flatten();
    if let Some(result) = &local_white_balance {
        eprintln!(
            "WB    {}: {} light(s), separation {:.3}{}",
            job.input.display(),
            result.lights.len(),
            result.separation,
            if result.applied {
                ""
            } else {
                " | not mixed lighting, left alone"
            }
        );
    }
    let linear = linear;

    let (analysis, mut parameters) = analyze::analyze(
        &linear,
        &analyze::AnalysisInputs {
            max_samples: options.max_samples,
            preset: options.preset,
            exposure_bias_ev: options.exposure_bias_ev,
            noise_floor_ev: noise_floor.as_ref().map(|floor| floor.snr1_ev),
            preview: preview.as_ref(),
            preview_strength,
        },
    )?;

    // Applied after the curve is solved rather than inside `derive_params`,
    // because saturation is the one tone parameter nothing else is derived
    // from: no exponent, no black point and no oracle inversion depends on it.
    // At the default 1.0 the multiplication is exact, so output does not move.
    parameters.saturation *= options.saturation_scale;
    let parameters = parameters;

    // The comparison the corpus work actually runs on. Available here, before
    // anything is rendered, because the controller's target and the curve it
    // will be mapped through are both already decided.
    if let Some(report) = &mut reference {
        report.compare(
            crate::reference::predicted_subject_display_ev(analysis.target_median_ev, &parameters),
            None,
        );
        eprintln!(
            "REF   {}: subject {:+.2} EV vs camera {:+.2} EV ({:+.2})",
            job.input.display(),
            report.subject_display_ev + report.delta.subject_display_ev,
            report.subject_display_ev,
            report.delta.subject_display_ev,
        );
    }

    let local_tone = if options.local_tone > 0.0 {
        let map = crate::localtone::build(
            &linear,
            options.local_tone,
            noise_floor.as_ref().map(|floor| floor.snr1_ev),
        )?;
        let report = map.report();
        eprintln!(
            "LOCAL {}: strength {:.2} | correction {:+.2}..{:+.2} EV | lift {:.1}% compress {:.1}%",
            job.input.display(),
            report.strength,
            report.correction_min_ev,
            report.correction_max_ev,
            report.shadow_lift_fraction * 100.0,
            report.highlight_compression_fraction * 100.0,
        );
        Some(map)
    } else {
        None
    };
    let local_tone_report = local_tone.as_ref().map(|map| map.report().clone());

    if options.dry_run {
        return Ok(ProcessReport {
            input: job.input.clone(),
            output: None,
            camera: camera_name,
            tonal_class: analysis.tonal_class,
            exposure_ev: parameters.exposure_ev,
            elapsed_ms: started.elapsed().as_millis(),
            skipped: false,
            dry_run: true,
            analysis: Some(analysis),
            parameters: Some(parameters),
            baseline_exposure_ev: report.baseline_exposure_ev,
            measured: None,
            noise: noise.clone(),
            noise_floor: noise_floor.clone(),
            shot: shot.clone(),
            preview: preview.clone(),
            local_tone: local_tone_report,
            chroma_denoise,
            sharpen: None,
            reference,
        });
    }

    let mut rendered = tone::render(&linear, &parameters, local_tone.as_ref());

    // After the view transform, because acutance is a property of the displayed
    // image, and before measurement, because the sharpened render is the output.
    let sharpen = crate::sharpen::apply(
        &mut rendered,
        noise_floor.as_ref().map(|floor| floor.snr10_ev),
        options.sharpen,
    );
    if let Some(report) = &sharpen {
        eprintln!(
            "SHARP {}: amount {:.2} | radius {} px | mean correction {:.5} | headroom-limited {:.3}%",
            job.input.display(),
            report.amount,
            report.radius,
            report.mean_correction,
            report.headroom_limited_fraction * 100.0,
        );
    }
    let rendered = rendered;

    // Measure before handing the buffer to the encoder, which consumes it.
    let output_stats = crate::metrics::OutputStats::measure(&rendered);
    if let Some(report) = &mut reference {
        report.compare(
            crate::reference::predicted_subject_display_ev(analysis.target_median_ev, &parameters),
            Some(&output_stats),
        );
        eprintln!(
            "REF   {}: saturation {:.3} vs {:.3} (x{:.3}) | colourfulness {:+.1} | level {:+.1}",
            job.input.display(),
            output_stats.mean_saturation,
            report.measured.mean_saturation,
            report.delta.saturation_ratio.unwrap_or(f32::NAN),
            report.delta.colourfulness.unwrap_or(f32::NAN),
            report.delta.mean_level.unwrap_or(f32::NAN),
        );
    }
    output::save_image(&paths.image, rendered, options.format, options.jpeg_quality)?;

    if options.emit_baseline && (options.overwrite || !paths.baseline.exists()) {
        let baseline = tone::render_baseline(&linear);
        output::save_image(
            &paths.baseline,
            baseline,
            options.format,
            options.jpeg_quality,
        )?;
    }

    if options.write_sidecar {
        let sidecar = Sidecar {
            schema_version: crate::types::REPORT_SCHEMA_VERSION,
            application: "raw-autotune".to_string(),
            application_version: env!("CARGO_PKG_VERSION").to_string(),
            input: job.input.to_string_lossy().into_owned(),
            output: Some(paths.image.to_string_lossy().into_owned()),
            preset: options.preset,
            camera: metadata,
            developed_width: linear.width,
            developed_height: linear.height,
            level_normalization: notes.clone(),
            baseline_exposure_ev: report.baseline_exposure_ev,
            measured: Some(output_stats.clone()),
            noise: noise.clone(),
            noise_floor: noise_floor.clone(),
            shot: shot.clone(),
            local_white_balance: local_white_balance.clone(),
            local_tone: local_tone_report.clone(),
            chroma_denoise: chroma_denoise.clone(),
            sharpen: sharpen.clone(),
            preview: preview.clone(),
            reference: reference.clone(),
            analysis: analysis.clone(),
            parameters: parameters.clone(),
            limitations: vec![
                "global analysis only; no face, subject, scene, or depth model".to_string(),
                "uses Rawler's linear-sRGB calibration path; no wide-gamut working space"
                    .to_string(),
                "no explicit lens correction or camera-specific DCP look table".to_string(),
                "no dedicated highlight reconstruction or profiled denoising".to_string(),
                "output metadata/EXIF is not copied in v0.1".to_string(),
            ],
        };
        output::save_sidecar(&paths.sidecar, &sidecar)?;
    }

    Ok(ProcessReport {
        input: job.input.clone(),
        output: Some(paths.image),
        camera: camera_name,
        tonal_class: analysis.tonal_class,
        exposure_ev: parameters.exposure_ev,
        elapsed_ms: started.elapsed().as_millis(),
        skipped: false,
        dry_run: false,
        analysis: Some(analysis),
        parameters: Some(parameters),
        baseline_exposure_ev: report.baseline_exposure_ev,
        measured: Some(output_stats),
        noise,
        noise_floor,
        shot,
        preview,
        local_tone: local_tone_report,
        chroma_denoise,
        sharpen,
        reference,
    })
}

/// Decode far enough to fit the sensor noise model, and stop.
///
/// Used by `--noise-scan` and `--pool-noise`: no develop, no tone mapping, no
/// output, so a profile pass costs a fraction of a full run.
pub fn scan_noise(job: &InputJob) -> Result<Option<crate::noiseprofile::NoiseSample>> {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<_> {
        let (raw, _) = crate::decode_corrected(&job.input)?;
        let Some(estimate) = crate::noise::estimate(&raw) else {
            return Ok(None);
        };
        let shot = crate::shotinfo::read(&job.input);
        Ok(Some(crate::noiseprofile::NoiseSample {
            clean_make: raw.clean_make.clone(),
            clean_model: raw.clean_model.clone(),
            iso: shot.and_then(|info| info.iso),
            shot_slope: estimate.shot_slope,
        }))
    }));

    match result {
        Ok(result) => result.with_context(|| format!("while scanning {}", job.input.display())),
        Err(payload) => Err(anyhow!(
            "RAW decoder panicked while scanning {}: {}",
            job.input.display(),
            panic_text(payload)
        )),
    }
}
