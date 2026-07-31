use crate::analyze;
use crate::color::{ColorReport, RawColorPath};
use crate::files;
use crate::noiseprofile::{NoiseFloor, NoiseProfile};
use crate::orientation;
use crate::output;
use crate::tone;
use crate::types::{
    CameraMetadata, GuidanceMode, InputJob, LinearImage, ProcessReport, RunOptions, Sidecar,
};
use anyhow::{Context, Result, anyhow, bail};
use rawler::RawImage;
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
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
    //
    // `Calibrate` is still in the list here, clipping and all. That is what
    // `--raw-color-path owned` exists to replace; this function is the control
    // arm of the A/B and so must not move. See `crate::color`.
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

/// Develop to scene-linear RGB through whichever colour path was asked for.
fn develop(raw: &RawImage, options: &RunOptions) -> Result<(LinearImage, ColorReport)> {
    match options.raw_color_path {
        RawColorPath::Rawler => Ok((develop_linear(raw)?, ColorReport::rawler(raw))),
        RawColorPath::Owned => crate::color::develop(
            raw,
            crate::color::DevelopOptions {
                working_space: options.working_space,
                sub_black: options.sub_black,
            },
        ),
    }
}

/// Write one scene-linear intermediate out for inspection.
///
/// Deliberately rendered through [`tone::render_baseline`], which applies the
/// transfer function and nothing else: a stage dump has to show what the data at
/// that point actually contains, not what the tone controller would like to make
/// of it. Values outside `[0, 1]` are therefore clipped *in the dump only* —
/// which on the owned path is exactly the highlight information the Rawler path
/// destroys for real, so comparing the two paths' `01-scene-linear` dumps shows
/// the loss directly.
///
/// Best effort: a failed dump is reported and ignored, never allowed to fail the
/// file it was diagnosing.
fn dump_stage(
    directory: &Path,
    input: &Path,
    stage: &str,
    image: &LinearImage,
    working_to_display: Option<&crate::color::Matrix3>,
) {
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".to_string());
    let path = directory.join(format!("{stem}-{stage}.png"));

    let write = || -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let rendered = tone::render_baseline(image, working_to_display);
        rendered.save(&path)?;
        Ok(())
    };

    match write() {
        Ok(()) => eprintln!("DUMP  {}: {}", input.display(), path.display()),
        Err(error) => eprintln!(
            "DUMP  {}: could not write {}: {error}",
            input.display(),
            path.display()
        ),
    }
}

/// What this rendering is honestly not, recorded in every sidecar.
///
/// Conditional on the colour path since 0.1.17: the standing "no wide-gamut
/// working space" caveat is simply false under `--raw-color-path owned
/// --working-space rec2020`, and a limitations list that lies in one
/// configuration is worse than none.
fn limitations(options: &RunOptions) -> Vec<String> {
    let mut limitations =
        vec!["global analysis only; no face, subject, scene, or depth model".to_string()];

    match options.raw_color_path {
        RawColorPath::Rawler => limitations.push(
            "uses Rawler's linear-sRGB calibration path, which clips out-of-gamut channels \
             and rewrites every pixel above 1.0 before this program sees it"
                .to_string(),
        ),
        RawColorPath::Owned => {
            limitations.push(format!(
                "owned colour path in {}; no ForwardMatrix, CameraCalibration, AnalogBalance, \
                 dual-illuminant interpolation or chromatic adaptation transform",
                options.working_space.as_str()
            ));
            // Conditional on the policy since 0.1.18: the rescale is this
            // program's own now (`crate::rescale`), so attributing the clip to
            // Rawler would be wrong, and asserting it happens at all would be
            // false under `--sub-black preserve`.
            if options.sub_black != crate::rescale::SubBlack::Preserve {
                limitations.push(format!(
                    "--sub-black {}: sensor samples below the black level are clipped to zero \
                     before demosaic, which rectifies the noise floor and leaves a small \
                     positive pedestal where the scene is black",
                    options.sub_black.as_str()
                ));
            }
        }
    }

    limitations.extend([
        "no explicit lens correction or camera-specific DCP look table".to_string(),
        "no dedicated highlight reconstruction; luma noise is not reduced".to_string(),
    ]);

    if options.write_metadata {
        // The EXIF copy is not total, and the two gaps are worth naming rather
        // than letting a reader assume the output is a full metadata clone.
        limitations.extend([
            "MakerNotes are not copied: vendor blobs contain absolute file offsets, \
             so relocating them corrupts them"
                .to_string(),
            "PNG carries its EXIF in an eXIf chunk, which many readers — including \
             Windows' own — ignore; JPEG and TIFF are read everywhere"
                .to_string(),
        ]);
    } else {
        limitations.push("--no-metadata: output carries no EXIF and no colour profile".to_string());
    }
    limitations
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
            color: None,
            guidance_mode: None,
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
            "REF   {}: {} | {}x{} | key {:+.2} EV | saturation {:.3} | colourfulness {:.1}",
            job.input.display(),
            report.path,
            report.width,
            report.height,
            report.center_weighted_key_display_ev,
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
            "PREV  {}: {}x{} {:?} | key {:+.2} EV",
            job.input.display(),
            oracle.width,
            oracle.height,
            oracle.source,
            oracle.center_weighted_key_display_ev
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

    let (mut linear, color) = develop(&raw, options)?;
    drop(raw);

    // `None` whenever the working space already has sRGB primaries, which is the
    // default and every configuration shipped before 0.1.17 — so the render is
    // byte-identical there. Rawler's `Calibrate` emits sRGB primaries regardless
    // of what `--working-space` says, so converting its output would be a second,
    // unearned transform.
    let working_to_display = options
        .working_space
        .to_display()
        .filter(|_| options.raw_color_path == RawColorPath::Owned);

    eprintln!(
        "COLOR {}: {} path | {} | illuminant {}{}",
        job.input.display(),
        color.path.as_str(),
        color.working_space.as_str(),
        color.illuminant,
        match &color.clip_cost {
            Some(cost) => format!(
                " | rawler's clip would have moved {:.3}% of pixels ({:.3}% negative, \
                 {:.3}% above 1.0), mean {:.5}, max {:.4}",
                cost.altered_fraction * 100.0,
                cost.negative_fraction * 100.0,
                cost.above_one_fraction * 100.0,
                cost.mean_abs_delta,
                cost.max_abs_delta,
            ),
            None => String::new(),
        }
    );

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

    if let Some(directory) = &options.dump_stages {
        dump_stage(
            directory,
            &job.input,
            "01-scene-linear",
            &linear,
            working_to_display.as_ref(),
        );
    }

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

    if let Some(directory) = &options.dump_stages {
        dump_stage(
            directory,
            &job.input,
            "02-after-chroma",
            &linear,
            working_to_display.as_ref(),
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

    if let Some(directory) = &options.dump_stages {
        dump_stage(
            directory,
            &job.input,
            "03-after-local-wb",
            &linear,
            working_to_display.as_ref(),
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

    // Exactly the condition `analyze` itself uses to decide whether to invert
    // the oracle's target through the curve, so the recorded mode cannot drift
    // from the decision it describes.
    let guidance_mode = if preview.is_some() && preview_strength > 0.0 {
        GuidanceMode::PreviewGuided
    } else {
        GuidanceMode::Independent
    };

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
            crate::reference::predicted_center_weighted_key_display_ev(
                analysis.target_median_ev,
                &parameters,
            ),
            None,
        );
        eprintln!(
            "REF   {}: key {:+.2} EV vs camera {:+.2} EV ({:+.2})",
            job.input.display(),
            report.center_weighted_key_display_ev + report.delta.center_weighted_key_display_ev,
            report.center_weighted_key_display_ev,
            report.delta.center_weighted_key_display_ev,
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
            color: Some(color),
            guidance_mode: Some(guidance_mode),
        });
    }

    // Read after the dry-run return: a dry run writes no file, so there is
    // nothing to tag. `SourceMetadata::read` is best effort and never fails — a
    // file whose EXIF cannot be parsed still gets Software, Orientation,
    // ColorSpace and the ICC profile, which is what a library needs least
    // wrongly.
    let exif = options
        .write_metadata
        .then(|| crate::metadata::SourceMetadata::read(&job.input));

    let mut rendered = tone::render(
        &linear,
        &parameters,
        local_tone.as_ref(),
        working_to_display.as_ref(),
    );

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
            crate::reference::predicted_center_weighted_key_display_ev(
                analysis.target_median_ev,
                &parameters,
            ),
            Some(&output_stats),
        );
        eprintln!(
            "REF   {}: saturation {:.3} vs {:.3} (x{:.3}) | highlight x{:.3} | colourfulness {:+.1} | level {:+.1}",
            job.input.display(),
            output_stats.mean_saturation,
            report.measured.mean_saturation,
            report.delta.saturation_ratio.unwrap_or(f32::NAN),
            report.delta.highlight_saturation_ratio.unwrap_or(f32::NAN),
            report.delta.colourfulness.unwrap_or(f32::NAN),
            report.delta.mean_level.unwrap_or(f32::NAN),
        );

        // The axis the 0.1.17 A/B was missing: whether our colour *is* the
        // camera's colour, not merely as saturated. Declining is reported, never
        // silently recorded as agreement.
        match report.compare_pixels(&rendered) {
            Ok(()) => {
                if let Some(hue) = &report.delta.hue {
                    eprintln!(
                        "HUE   {}: median {:.2}\u{b0} | p90 {:.2}\u{b0} | max {:.2}\u{b0} | chroma x{:.3} | {} of {} grid px",
                        job.input.display(),
                        hue.median_degrees,
                        hue.p90_degrees,
                        hue.max_degrees,
                        hue.chroma_ratio,
                        hue.comparable_pixels,
                        hue.grid_width * hue.grid_height,
                    );
                }
            }
            Err(reason) => eprintln!("HUE   {}: not compared, {reason}", job.input.display()),
        }
    }
    output::save_image_with_metadata(
        &paths.image,
        rendered,
        options.format,
        options.jpeg_quality,
        exif.as_ref(),
    )?;

    if options.emit_baseline && (options.overwrite || !paths.baseline.exists()) {
        let baseline = tone::render_baseline(&linear, working_to_display.as_ref());
        output::save_image_with_metadata(
            &paths.baseline,
            baseline,
            options.format,
            options.jpeg_quality,
            exif.as_ref(),
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
            color: color.clone(),
            guidance_mode,
            controller_version: crate::types::CONTROLLER_VERSION.to_string(),
            analysis: analysis.clone(),
            parameters: parameters.clone(),
            limitations: limitations(options),
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
        color: Some(color),
        guidance_mode: Some(guidance_mode),
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
