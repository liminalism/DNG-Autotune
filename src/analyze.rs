use crate::types::{AnalysisStats, LinearImage, MID_GRAY, Preset, TonalClass, ToneParams};
use anyhow::{Result, bail};

#[inline]
pub fn luminance(rgb: [f32; 3]) -> f32 {
    // Rawler's calibrated RGB path targets an RGB display working space.
    // These Rec.709/sRGB luminance coefficients are a practical v0.1 choice.
    rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722
}

fn quantile(sorted: &[f32], q: f32) -> f32 {
    debug_assert!(!sorted.is_empty());

    let q = q.clamp(0.0, 1.0);
    let position = q * (sorted.len() - 1) as f32;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let fraction = position - lower as f32;

    sorted[lower] * (1.0 - fraction) + sorted[upper] * fraction
}

fn classify_tonality(key_score: f32, dynamic_range_ev: f32) -> TonalClass {
    if dynamic_range_ev >= 11.0 {
        TonalClass::HighDynamicRange
    } else if key_score >= 0.32 {
        TonalClass::HighKey
    } else if key_score <= -0.32 {
        TonalClass::LowKey
    } else if dynamic_range_ev <= 4.25 {
        TonalClass::Flat
    } else {
        TonalClass::Normal
    }
}

/// Hermite smoothstep on `[0, 1]`, clamped outside.
#[inline]
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Exposure value normalized to ISO 100, derived only from capture metadata.
///
/// This is a physical ambient-light cue, not scene recognition. A deliberately
/// underexposed daylight frame can be dark and noisy in the raw data, but its
/// short shutter and stopped-down aperture still give it a high EV100. Night
/// captures in the paired corpus are at or below about 4.3 EV100, while the
/// hostile high-ISO daylight negatives start at 9 EV100.
fn capture_ev100(
    exposure_time: Option<f32>,
    f_number: Option<f32>,
    iso: Option<u32>,
) -> Option<f32> {
    let exposure_time = exposure_time.filter(|value| value.is_finite() && *value > 0.0)?;
    let f_number = f_number.filter(|value| value.is_finite() && *value > 0.0)?;
    let iso = iso.filter(|value| *value > 0)? as f32;
    let ev100 = (f_number * f_number / exposure_time).log2() - (iso / 100.0).log2();
    ev100.is_finite().then_some(ev100)
}

/// How much a frame looks like a genuine low-light/night capture, 0 to 1.
///
/// The shape-based [`classify_tonality`] cannot answer this: a night scene with
/// a few bright lights reads as `Normal` or `HighDynamicRange`, and a dark
/// subject shot in daylight reads as dark without being low-light. So the score
/// is the product of three independent cues a genuine night capture shares:
///
/// * *darkness* — how far the scene median sits below middle grey (`p50_ev`),
///   ramping in from -1 EV to -3 EV. `p50_ev` is measured before the automatic
///   exposure lifts anything, so it reflects where the light actually was.
/// * *noise* — how hard the sensor was pushed, taken as the larger of the SNR=10
///   crossing (a measured property of this frame) and the ISO metadata.
/// * *ambient light* — EV100 derived from shutter, aperture and ISO. It is fully
///   open through 5 EV, fades smoothly, and is zero from 8 EV upward. This
///   rejects a dark raw caused by 1/1600 s at f/18 in daylight without needing
///   to know whether the pixels contain a waterfall, a person or a sky.
///
/// Multiplying them means a frame must be dark, noisy, and exposed under low
/// ambient light to score high, so a bright scene at maximum ISO and a dark,
/// high-dynamic-range daytime frame both score low — the cases that darkness or
/// ISO alone confuse.
fn low_light_score(
    p50_ev: f32,
    snr10_ev: Option<f32>,
    iso: Option<u32>,
    capture_ev100: Option<f32>,
) -> f32 {
    let darkness = smoothstep((-1.0 - p50_ev) / 2.0);
    let noise_from_snr = snr10_ev
        .filter(|value| value.is_finite())
        .map(|snr10| smoothstep((snr10 + 4.0) / 3.5))
        .unwrap_or(0.0);
    let noise_from_iso = iso
        .filter(|iso| *iso > 0)
        .map(|iso| smoothstep(((iso as f32).log2() - 1600.0f32.log2()) / 3.0))
        .unwrap_or(0.0);
    let noise = noise_from_snr.max(noise_from_iso);
    let ambient_light = capture_ev100
        .filter(|value| value.is_finite())
        .map(|ev100| 1.0 - smoothstep((ev100 - 5.0) / 3.0))
        // Missing exposure metadata must not disable an otherwise reliable
        // darkness-and-noise decision.
        .unwrap_or(1.0);
    (darkness * noise * ambient_light).clamp(0.0, 1.0)
}

/// Derive the tone curve from the measured statistics and a chosen exposure.
///
/// Split out of [`analyze`] so the preview oracle can re-solve it: the oracle's
/// target is expressed in display EV and has to be inverted through the curve,
/// which means a curve must exist first. Kept as a pure extraction, with the
/// float operations in their original order, so that a run with the oracle
/// disabled reproduces earlier output exactly.
fn derive_params(
    stats: &AnalysisStats,
    preset: Preset,
    exposure_ev: f32,
    noise_floor_ev: Option<f32>,
    highlight_contrast: f32,
) -> ToneParams {
    let (p005, p05, p995) = (stats.p005_ev, stats.p05_ev, stats.p995_ev);

    let safety_margin = match preset {
        Preset::Neutral => 0.65,
        Preset::Auto => 0.40,
        Preset::Standard | Preset::Vivid => 0.22,
    };

    let mut black_input_ev = p005 + exposure_ev - safety_margin;

    // Never place the black point below the level where the sensor stops
    // delivering signal. Stretching pure noise across the shadow range is worse
    // than crushing it, and the darkest percentile of a high-ISO frame is often
    // exactly that. The floor is the SNR=1 crossing, which on the Sony test
    // batch binds on 15% of frames and leaves clean ones untouched; SNR=10
    // would bind on 63% and crush real, if noisy, detail.
    //
    // The estimate is trustworthy in aggregate but not per frame: across 258
    // Sony files it tracks ISO at +0.91 EV per stop (R^2 0.92 on per-ISO
    // medians), yet individual base-ISO frames disagree by up to 7 EV, because
    // dense texture still inflates the fit. So the floor is only allowed to
    // raise the black point as far as the 5th percentile of the frame. However
    // wrong a single estimate is, at most about 5% of the image can be crushed.
    if let Some(floor_ev) = noise_floor_ev.filter(|value| value.is_finite()) {
        let raised = black_input_ev.max(floor_ev + exposure_ev);
        black_input_ev = raised.min(p05 + exposure_ev);
    }
    let mut black_input_ev = black_input_ev.clamp(-16.0, -1.5);
    let mut white_input_ev = (p995 + exposure_ev + safety_margin).clamp(1.5, 16.0);

    // Do not let very flat inputs produce an excessively steep transfer curve.
    let minimum_input_range = match preset {
        Preset::Neutral => 5.5,
        Preset::Auto => 5.0,
        Preset::Standard | Preset::Vivid => 4.5,
    };
    let input_range = white_input_ev - black_input_ev;
    if input_range < minimum_input_range {
        let expansion = (minimum_input_range - input_range) * 0.5;
        black_input_ev = (black_input_ev - expansion).max(-16.0);
        white_input_ev = (white_input_ev + expansion).min(16.0);
    }

    let final_input_range = white_input_ev - black_input_ev;
    let contrast = match preset {
        Preset::Neutral => (1.02 - 0.010 * (final_input_range - 7.0).max(0.0)).clamp(0.90, 1.02),
        Preset::Auto => (1.18 - 0.018 * (final_input_range - 7.0).max(0.0)).clamp(0.92, 1.18),
        Preset::Standard | Preset::Vivid => {
            (1.30 - 0.015 * (final_input_range - 7.0).max(0.0)).clamp(1.05, 1.30)
        }
    };

    // `highlight_norm` trades highlight retention against punch: at 1.0 no
    // channel can clip, at 0.0 the curve is driven by luminance alone and
    // saturated highlights blow out.
    //
    // `saturation` was set by eye until 0.1.12, when 29 RAW+JPEG pairs made it
    // measurable. Against them the old `auto` value of 1.02 rendered at 0.847
    // of the camera's own saturation — a flat deficit, the same on both phone
    // sources, which is the signature of a pipeline-wide shortfall rather than
    // a scene-dependent one. Scaling by 1.20 lands `auto` at 1.017 of the
    // camera while hard clipping stays at a 0.001% median against the camera's
    // 1.076%. 1.30 was measurably worse: clipping rose to a 0.308% median.
    // `standard` (formerly punchy) sits above `auto`; `neutral` keeps 1.00
    // because its documented job is to add no chroma opinion. `vivid` shares
    // the standard curve; the HueSatMap is applied later.
    let (
        black_output_linear,
        white_output_linear,
        saturation,
        vibrance,
        highlight_desaturation,
        highlight_norm,
    ) = match preset {
        Preset::Neutral => (0.0025, 0.965, 1.00, 0.00, 0.10, 1.00),
        Preset::Auto => (0.0012, 0.985, 1.22, 0.08, 0.16, 1.00),
        Preset::Standard | Preset::Vivid => (0.0008, 0.995, 1.27, 0.16, 0.13, 0.70),
    };

    let black_output_ev = (black_output_linear / MID_GRAY).log2();
    let white_output_ev = (white_output_linear / MID_GRAY).log2();

    // Choose curve exponents so the two curve segments meet at middle gray
    // with approximately the requested derivative ("contrast").
    //
    // `highlight_contrast` scales only the highlight exponent, which is what
    // makes it a *sky* control rather than a contrast control. `map_ev` is
    // anchored at middle grey and its two branches are independent, so raising
    // this steepens the highlight branch — brighter skies, reaching the
    // `highlight_desaturation` shoulder sooner — while the shadow branch, the
    // black point and the exposure the oracle solved for all stay exactly where
    // they were. Raising `contrast` instead would buy the same highlights by
    // darkening the shadows, and on the paired A7C frames the ground already
    // matches the camera to within 0.07 EV, so there is nothing there to spend.
    //
    // The default is 1.0 and multiplying by it is exact, so the derived exponent
    // is bit-identical to the pre-0.1.20 expression when the knob is untouched.
    // That matters more than usual here: this exponent feeds
    // `tone::inverse_map_ev`, so the preview oracle inverts its target through
    // whatever curve this produces. Hence the scale is applied inside the solve
    // rather than patched onto `ToneParams` afterwards the way
    // `saturation_scale` is — saturation is the one parameter nothing else is
    // derived from, and this is not it.
    let shadow_power = (contrast * (-black_input_ev) / (-black_output_ev)).clamp(0.45, 4.0);
    let highlight_power =
        ((contrast * highlight_contrast) * white_input_ev / white_output_ev).clamp(0.45, 4.0);

    ToneParams {
        exposure_ev,
        black_input_ev,
        white_input_ev,
        black_output_linear,
        white_output_linear,
        black_output_ev,
        white_output_ev,
        contrast,
        shadow_power,
        highlight_power,
        saturation,
        vibrance,
        highlight_desaturation,
        highlight_color_ratio_exponent: 1.0,
        highlight_norm,
        noise_floor_ev,
    }
}

/// How far the preview oracle may move the target away from the key-score
/// target. The motivating file needs about 3.5 EV, so a tighter cap would clip
/// exactly the case the oracle exists for.
const MAX_ORACLE_DEVIATION_EV: f32 = 4.0;
/// Most curve solves the oracle may perform, counting the original re-solve.
///
/// A fixed cap rather than iterate-to-convergence keeps the loop deterministic
/// even on a curve where the fixed point oscillates; four is enough for the
/// shoulder cases observed (`_DSC1291` converges on the second).
const ORACLE_RESOLVE_LIMIT: usize = 4;
/// Upward target movement below which the re-solve loop stops.
///
/// Also the guarantee that old output does not move: any frame whose second
/// inversion does not ask for at least this much *more brightness* keeps the
/// first solution untouched, which is bit-for-bit the pre-iteration behaviour
/// — including every night frame, where the pre-iteration behaviour is the
/// paired-validated one.
const ORACLE_RESOLVE_CONVERGED_EV: f32 = 0.01;
/// Absolute bounds on an oracle-derived target, deliberately asymmetric.
///
/// A vendor rendering a scene well below middle grey is the legitimate night
/// case. A vendor rendering it more than a stop *above* middle grey is far more
/// likely an HDR-fused or blown preview, which is the failure worth refusing —
/// *unless* the raw statistics independently say the scene is bright, in which
/// case the preview is corroborated rather than suspect and the ceiling rises
/// (see [`oracle_target_ceiling_ev`]).
const ORACLE_TARGET_FLOOR_EV: f32 = -4.0;
const ORACLE_TARGET_CEILING_EV: f32 = 1.0;
/// Extra ceiling headroom, in EV, once the key score fully corroborates.
///
/// Sized from the first paired high-key frame (`_DSC1291`: the camera renders
/// the subject at +1.63 EV, unclipped, and the +1.0 ceiling was leaving our
/// render 0.42 EV darker) with margin for brighter scenes such as snow, while
/// still refusing the multi-stop targets a blown preview would ask for.
const CORROBORATED_CEILING_EXTRA_EV: f32 = 1.0;
/// Key-score span over which the extra headroom ramps in.
///
/// The start is [`classify_tonality`]'s high-key threshold: below it the raw
/// statistics do not call the scene bright and the original suspicion stands in
/// full. Ramped rather than switched, like the chroma strength, so two frames a
/// hair apart in key score cannot render visibly differently.
const CEILING_CORROBORATION_START: f32 = 0.32;
const CEILING_CORROBORATION_FULL: f32 = 0.60;

/// Ceiling for an oracle-derived target, given the frame's own key score.
///
/// The 16 corpus files where the +1.0 ceiling binds split exactly along this
/// line: ten are independently classified high-key by the raw statistics —
/// bright preview corroborated, ceiling raised — and six are not, for which
/// the original blown-preview suspicion stands and the +1.0 cap holds.
fn oracle_target_ceiling_ev(key_score: f32) -> f32 {
    let ramp = ((key_score - CEILING_CORROBORATION_START)
        / (CEILING_CORROBORATION_FULL - CEILING_CORROBORATION_START))
        .clamp(0.0, 1.0);
    let corroboration = ramp * ramp * (3.0 - 2.0 * ramp);
    ORACLE_TARGET_CEILING_EV + corroboration * CORROBORATED_CEILING_EXTRA_EV
}

/// Bound an oracle-derived target against the controller's own target.
fn guard_oracle_target(wanted: f32, key_target_ev: f32, key_score: f32) -> f32 {
    if !wanted.is_finite() {
        return key_target_ev;
    }
    wanted
        .clamp(
            key_target_ev - MAX_ORACLE_DEVIATION_EV,
            key_target_ev + MAX_ORACLE_DEVIATION_EV,
        )
        .clamp(ORACLE_TARGET_FLOOR_EV, oracle_target_ceiling_ev(key_score))
}

/// Everything `analyze` needs beyond the image itself.
pub struct AnalysisInputs<'a> {
    pub max_samples: usize,
    pub preset: Preset,
    pub exposure_bias_ev: f32,
    /// Scene EV below which the sensor delivers no usable signal.
    pub noise_floor_ev: Option<f32>,
    /// Scene EV at which SNR falls to 10, from [`crate::noise`]. Feeds the
    /// low-light score only; does not affect any existing tone parameter.
    pub snr10_ev: Option<f32>,
    /// ISO from EXIF, a metadata noise cue and an input to capture EV100.
    pub iso: Option<u32>,
    /// Exposure time in seconds, used with aperture and ISO to reject dark raw
    /// values produced by short daylight exposures.
    pub exposure_time: Option<f32>,
    /// Aperture as an f-number, used only for the physical EV100 night gate.
    pub f_number: Option<f32>,
    /// The camera's own rendering of this capture, when one could be read.
    pub preview: Option<&'a crate::preview::PreviewOracle>,
    /// How far to move from the controller's target toward the oracle's, 0 to 1.
    pub preview_strength: f32,
    /// Multiplier on the tone curve's highlight exponent alone, 1.0 for the
    /// unmodified curve. See `derive_params` for why it lives in the solve.
    pub highlight_contrast: f32,
}

impl AnalysisInputs<'_> {
    /// Inputs that reproduce the controller's own behaviour, with no oracle.
    pub fn new(max_samples: usize, preset: Preset, exposure_bias_ev: f32) -> Self {
        Self {
            max_samples,
            preset,
            exposure_bias_ev,
            noise_floor_ev: None,
            snr10_ev: None,
            iso: None,
            exposure_time: None,
            f_number: None,
            preview: None,
            preview_strength: 0.0,
            highlight_contrast: 1.0,
        }
    }
}

pub fn analyze(
    image: &LinearImage,
    inputs: &AnalysisInputs<'_>,
) -> Result<(AnalysisStats, ToneParams)> {
    let AnalysisInputs {
        max_samples,
        preset,
        exposure_bias_ev,
        noise_floor_ev,
        ..
    } = *inputs;
    if image.width == 0 || image.height == 0 || image.pixels.is_empty() {
        bail!("cannot analyze an empty image");
    }

    let total_pixels = image.width.saturating_mul(image.height);
    let stride =
        (((total_pixels as f64 / max_samples.max(1) as f64).sqrt()).ceil() as usize).max(1);

    let center_x0 = image.width / 5;
    let center_x1 = image.width - center_x0;
    let center_y0 = image.height / 5;
    let center_y1 = image.height - center_y0;

    let mut ev_values = Vec::with_capacity((total_pixels / stride.saturating_mul(stride)).max(1));
    let mut center_ev_values = Vec::with_capacity(ev_values.capacity() / 3);
    let mut near_black = 0_usize;
    let mut near_white = 0_usize;
    let mut clipped_1 = 0_usize;
    let mut clipped_2 = 0_usize;
    let mut clipped_3 = 0_usize;
    let mut valid = 0_usize;
    let mut chroma_sum = 0.0_f64;

    for y in (0..image.height).step_by(stride) {
        for x in (0..image.width).step_by(stride) {
            let rgb = image.pixels[y * image.width + x];
            if !rgb.iter().all(|channel| channel.is_finite()) {
                continue;
            }

            let y_linear = luminance(rgb);
            if !y_linear.is_finite() || y_linear <= 1.0e-8 {
                continue;
            }

            let ev = (y_linear / MID_GRAY).log2();
            if !ev.is_finite() {
                continue;
            }

            let maximum = rgb[0].max(rgb[1]).max(rgb[2]);
            let minimum = rgb[0].min(rgb[1]).min(rgb[2]);
            chroma_sum += (maximum - minimum).max(0.0) as f64;

            if y_linear <= 0.001 {
                near_black += 1;
            }
            if maximum >= 0.995 {
                near_white += 1;
            }
            // Per-channel clipped counts using the same threshold as highlight reconstruction
            // (0.98 in camera space, here applied to scene-linear for metrics).
            const CLIP_THR: f32 = 0.98;
            let clipped = (rgb[0] >= CLIP_THR) as usize
                + (rgb[1] >= CLIP_THR) as usize
                + (rgb[2] >= CLIP_THR) as usize;
            match clipped {
                1 => clipped_1 += 1,
                2 => clipped_2 += 1,
                3 => clipped_3 += 1,
                _ => {}
            }
            valid += 1;
            ev_values.push(ev);

            if x >= center_x0 && x < center_x1 && y >= center_y0 && y < center_y1 {
                center_ev_values.push(ev);
            }
        }
    }

    if ev_values.len() < 64 {
        bail!(
            "too few valid pixels for analysis ({}); the decoded image may be empty or unsupported",
            ev_values.len()
        );
    }

    ev_values.sort_unstable_by(f32::total_cmp);
    center_ev_values.sort_unstable_by(f32::total_cmp);

    let p005 = quantile(&ev_values, 0.005);
    let p05 = quantile(&ev_values, 0.05);
    let p50 = quantile(&ev_values, 0.50);
    let p95 = quantile(&ev_values, 0.95);
    let p995 = quantile(&ev_values, 0.995);
    let center_median = if center_ev_values.is_empty() {
        p50
    } else {
        quantile(&center_ev_values, 0.50)
    };

    let measured_dynamic_range_ev = (p995 - p005).max(0.0);
    let lower_span = (p50 - p05).max(0.01);
    let upper_span = (p95 - p50).max(0.01);
    let key_score = ((lower_span - upper_span) / (lower_span + upper_span)).clamp(-1.0, 1.0);
    let tonal_class = classify_tonality(key_score, measured_dynamic_range_ev);

    // Center weighting is intentionally modest. It helps common portraits and
    // backlit subjects without claiming to be semantic subject detection.
    let center_weighted_key_ev = 0.60 * center_median + 0.40 * p50;
    let key_target_ev = match preset {
        Preset::Neutral => key_score * 0.35,
        Preset::Auto => key_score * 0.65,
        Preset::Standard | Preset::Vivid => key_score * 0.50,
    };

    let mut target_median_ev = key_target_ev;
    let mut exposure_ev =
        (target_median_ev - center_weighted_key_ev + exposure_bias_ev).clamp(-5.0, 5.0);

    let capture_ev100 = capture_ev100(inputs.exposure_time, inputs.f_number, inputs.iso);
    let mut stats = AnalysisStats {
        sampled_pixels: valid,
        sample_stride: stride,
        p005_ev: p005,
        p05_ev: p05,
        p50_ev: p50,
        p95_ev: p95,
        p995_ev: p995,
        center_median_ev: center_median,
        measured_dynamic_range_ev,
        key_score,
        target_median_ev,
        tonal_class,
        near_black_fraction: near_black as f32 / valid as f32,
        near_white_fraction: near_white as f32 / valid as f32,
        clipped_1_fraction: clipped_1 as f32 / valid as f32,
        clipped_2_fraction: clipped_2 as f32 / valid as f32,
        clipped_3_fraction: clipped_3 as f32 / valid as f32,
        mean_chroma: (chroma_sum / valid as f64) as f32,
        capture_ev100,
        low_light_score: low_light_score(p50, inputs.snr10_ev, inputs.iso, capture_ev100),
    };

    let mut params = derive_params(
        &stats,
        preset,
        exposure_ev,
        noise_floor_ev,
        inputs.highlight_contrast,
    );

    // The preview oracle works in display EV, so it needs a curve to invert
    // through. Solve once with the key-score target, adopt the oracle's target,
    // then re-solve. In the curve's linear region one re-solve is exact and the
    // loop below stops after it, reproducing the single-solve output bit for
    // bit. High-key frames live on the shoulder, where re-solving reshapes the
    // curve and the inversion's promise no longer holds — the first paired
    // high-key frame landed 0.24 EV under the oracle's ask this way — so the
    // target is re-inverted through each fresh curve for as long as doing so
    // asks for a brighter placement (and only brighter; see below).
    // The iteration cap is fixed, not convergence-timed, so the loop is
    // deterministic by construction, and `derive_params` is pure algebra, so
    // the cost is negligible next to the sampling loop above.
    if let Some(oracle) = inputs.preview.filter(|_| inputs.preview_strength > 0.0) {
        for iteration in 0..ORACLE_RESOLVE_LIMIT {
            let wanted =
                crate::tone::inverse_map_ev(oracle.center_weighted_key_display_ev, &params);
            let guarded = guard_oracle_target(wanted, key_target_ev, key_score);
            let next = key_target_ev + inputs.preview_strength * (guarded - key_target_ev);
            // The first pass is unconditional — it is the pre-existing single
            // re-solve. Later passes run only while the target is still moving
            // *brighter*. The asymmetry is deliberate and mirrors the
            // floor/ceiling bounds above: the paired high-key frame showed the
            // one-solve undershoot leaves a corroborated bright scene 0.24 EV
            // dark, but on night frames the same undershoot is what kept the
            // render away from vendor JPEGs the 0.1.14 pairs proved wrong —
            // iterating downward re-approaches exactly the rendering
            // `MAX_ORACLE_DEVIATION_EV` exists to refuse. Dark-side behaviour
            // therefore stays bit-for-bit what the night pairs validated.
            if iteration > 0 && next - target_median_ev < ORACLE_RESOLVE_CONVERGED_EV {
                break;
            }
            target_median_ev = next;
            exposure_ev =
                (target_median_ev - center_weighted_key_ev + exposure_bias_ev).clamp(-5.0, 5.0);
            stats.target_median_ev = target_median_ev;
            params = derive_params(
                &stats,
                preset,
                exposure_ev,
                noise_floor_ev,
                inputs.highlight_contrast,
            );
        }
    }

    Ok((stats, params))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant_image(value: f32) -> LinearImage {
        LinearImage::new(64, 64, vec![[value; 3]; 64 * 64]).unwrap()
    }

    #[test]
    fn night_capture_stays_above_the_automatic_tone_threshold() {
        // `_DSC1309`: the weakest-scoring positive in the night corpus.
        let ev100 = capture_ev100(Some(1.0 / 160.0), Some(2.8), Some(6400));
        assert!(ev100.unwrap() < 5.0);
        let score = low_light_score(-4.0766478, Some(-2.0345316), Some(6400), ev100);
        assert!(
            score > 0.7,
            "weakest night positive fell below the useful range: {score}"
        );
    }

    #[test]
    fn short_stopped_down_daylight_exposure_is_not_night() {
        // `_DSC1277`: ISO 8000 and a raw median near -5 EV used to score 0.87,
        // despite being an ordinary daylight waterfall shot. Its 1/1600 s,
        // f/18 capture metadata is the unambiguous physical counter-signal.
        let ev100 = capture_ev100(Some(1.0 / 1600.0), Some(18.0), Some(8000));
        assert!(ev100.unwrap() > 12.0);
        let score = low_light_score(-4.9708223, Some(-1.3921179), Some(8000), ev100);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn daylight_gate_rejects_even_maximum_iso_noise() {
        let score = low_light_score(-6.0, Some(2.0), Some(65_535), Some(9.0));
        assert_eq!(score, 0.0);
    }

    #[test]
    fn missing_exposure_metadata_preserves_darkness_and_noise_fallback() {
        let score = low_light_score(-4.0, Some(-1.0), Some(12_800), None);
        assert_eq!(score, 1.0);
    }

    #[test]
    fn capture_ev100_rejects_invalid_metadata() {
        assert_eq!(capture_ev100(Some(0.0), Some(2.8), Some(6400)), None);
        assert_eq!(capture_ev100(Some(0.1), Some(f32::NAN), Some(6400)), None);
        assert_eq!(capture_ev100(Some(0.1), Some(2.8), Some(0)), None);
    }

    /// A bright preview with nothing in the raw statistics to back it up is
    /// the blown/HDR-fused case the ceiling exists for: still clamped to +1.0.
    #[test]
    fn an_uncorroborated_bright_oracle_target_is_clamped() {
        assert_eq!(guard_oracle_target(2.5, 0.0, 0.0), ORACLE_TARGET_CEILING_EV);
        // At the high-key threshold itself the extension is still exactly zero.
        assert_eq!(
            guard_oracle_target(2.5, 0.0, CEILING_CORROBORATION_START),
            ORACLE_TARGET_CEILING_EV
        );
    }

    /// The paired case that motivated the extension: `_DSC1291`, key score
    /// 0.54, camera subject at +1.63 EV, unclipped. The raw statistics
    /// corroborate the bright preview, so the oracle may follow it.
    #[test]
    fn a_corroborated_high_key_target_passes_the_old_ceiling() {
        let guarded = guard_oracle_target(1.63, 0.35, 0.54);
        assert!(
            (guarded - 1.63).abs() < 1.0e-6,
            "corroborated +1.63 EV target was clamped to {guarded}"
        );
    }

    /// Corroboration buys one extra stop, never unbounded trust: even a
    /// fully high-key frame refuses a multi-stop preview target.
    #[test]
    fn even_full_corroboration_is_bounded() {
        assert_eq!(
            guard_oracle_target(3.5, 1.5, 1.0),
            ORACLE_TARGET_CEILING_EV + CORROBORATED_CEILING_EXTRA_EV
        );
    }

    /// The ceiling must be continuous in the key score — two frames a hair
    /// apart in brightness may not render visibly differently.
    #[test]
    fn the_ceiling_ramp_is_continuous_and_monotonic() {
        let mut previous = oracle_target_ceiling_ev(0.0);
        let mut score = 0.0f32;
        while score < 1.0 {
            let next = oracle_target_ceiling_ev(score);
            assert!(next >= previous, "ceiling fell as key score rose");
            assert!(next - previous < 0.02, "ceiling jumped at score {score}");
            previous = next;
            score += 0.002;
        }
    }

    #[test]
    fn dark_image_receives_positive_exposure() {
        let image = constant_image(0.045);
        let (_, params) = analyze(&image, &AnalysisInputs::new(10_000, Preset::Auto, 0.0)).unwrap();
        assert!(params.exposure_ev > 1.5);
    }

    /// Log-spaced ramp spanning about 8 stops, so the percentiles sit far apart
    /// and the black point lands well below the -1.5 EV clamp. A flat image
    /// cannot exercise the noise floor: its black point is clamp-bound either
    /// way.
    fn graded_image() -> LinearImage {
        let count = 64 * 64;
        let pixels = (0..count)
            .map(|index| {
                let position = index as f32 / (count - 1) as f32;
                let value = 0.002 * 256.0f32.powf(position);
                [value; 3]
            })
            .collect();
        LinearImage::new(64, 64, pixels).unwrap()
    }

    /// A noise floor above the darkest percentile must raise the black point,
    /// so that shadow range is not spent stretching pure noise.
    #[test]
    fn noise_floor_raises_the_black_point() {
        let image = graded_image();
        let (_, clean) = analyze(&image, &AnalysisInputs::new(10_000, Preset::Auto, 0.0)).unwrap();

        // Place the floor two stops above where the clean fit put the black
        // point, expressed in scene EV as the pipeline supplies it. The ramp
        // spans 8 stops, so this stays below the 5th percentile guard.
        let floor = clean.black_input_ev - clean.exposure_ev + 2.0;
        let (_, noisy) = analyze(
            &image,
            &AnalysisInputs {
                noise_floor_ev: Some(floor),
                ..AnalysisInputs::new(10_000, Preset::Auto, 0.0)
            },
        )
        .unwrap();

        assert!(
            noisy.black_input_ev > clean.black_input_ev,
            "floor should lift the black point: {} vs {}",
            noisy.black_input_ev,
            clean.black_input_ev
        );
        assert_eq!(noisy.noise_floor_ev, Some(floor));
    }

    /// A floor far below the scene must change nothing, so clean frames are
    /// rendered exactly as before.
    #[test]
    fn a_low_noise_floor_is_inert() {
        let image = graded_image();
        let (_, clean) = analyze(&image, &AnalysisInputs::new(10_000, Preset::Auto, 0.0)).unwrap();
        let (_, floored) = analyze(
            &image,
            &AnalysisInputs {
                noise_floor_ev: Some(-40.0),
                ..AnalysisInputs::new(10_000, Preset::Auto, 0.0)
            },
        )
        .unwrap();
        assert_eq!(floored.black_input_ev, clean.black_input_ev);
        assert_eq!(floored.white_input_ev, clean.white_input_ev);
    }

    /// However wrong the estimate, the black point must not rise above the 5th
    /// percentile, so a bad fit cannot crush most of the frame.
    #[test]
    fn an_absurd_noise_floor_is_capped_at_the_fifth_percentile() {
        let image = graded_image();
        let (stats, params) = analyze(
            &image,
            &AnalysisInputs {
                noise_floor_ev: Some(60.0),
                ..AnalysisInputs::new(10_000, Preset::Auto, 0.0)
            },
        )
        .unwrap();
        assert!(
            params.black_input_ev <= stats.p05_ev + params.exposure_ev + 1.0e-4,
            "black point {} exceeded p05 {} + exposure {}",
            params.black_input_ev,
            stats.p05_ev,
            params.exposure_ev
        );
    }

    #[test]
    fn a_non_finite_noise_floor_is_ignored() {
        let image = graded_image();
        let (_, clean) = analyze(&image, &AnalysisInputs::new(10_000, Preset::Auto, 0.0)).unwrap();
        let (_, broken) = analyze(
            &image,
            &AnalysisInputs {
                noise_floor_ev: Some(f32::NAN),
                ..AnalysisInputs::new(10_000, Preset::Auto, 0.0)
            },
        )
        .unwrap();
        assert_eq!(broken.black_input_ev, clean.black_input_ev);
    }

    #[test]
    fn manual_bias_is_added() {
        let image = constant_image(MID_GRAY);
        let (_, base) = analyze(&image, &AnalysisInputs::new(10_000, Preset::Auto, 0.0)).unwrap();
        let (_, biased) =
            analyze(&image, &AnalysisInputs::new(10_000, Preset::Auto, 0.75)).unwrap();
        assert!((biased.exposure_ev - base.exposure_ev - 0.75).abs() < 0.001);
    }

    /// The presets have to stay ordered in chroma for their names to mean
    /// anything: `neutral` adds no opinion, `auto` is the archive curve, and
    /// `standard` sits above it. `vivid` shares the standard curve.
    #[test]
    fn preset_saturation_stays_ordered() {
        let saturation = |preset| {
            analyze(&graded_image(), &AnalysisInputs::new(10_000, preset, 0.0))
                .unwrap()
                .1
                .saturation
        };
        let neutral = saturation(Preset::Neutral);
        let auto = saturation(Preset::Auto);
        let standard = saturation(Preset::Standard);
        let vivid = saturation(Preset::Vivid);

        assert_eq!(neutral, 1.00);
        assert_eq!(vivid, standard);
        assert!(
            neutral < auto && auto < standard,
            "neutral {neutral} < auto {auto} < standard {standard}"
        );
    }

    /// The knob's default has to be *exactly* inert, not approximately so.
    /// `highlight_power` feeds `tone::inverse_map_ev`, so a curve that differed
    /// by one ulp at the default would move the preview oracle's target and
    /// with it the exposure of every frame that carries a preview. Multiplying
    /// by 1.0 is exact in IEEE 754; this pins that the expression was written so
    /// that it stays exact.
    #[test]
    fn the_default_highlight_contrast_is_bit_identical() {
        let stats = analyze(
            &graded_image(),
            &AnalysisInputs::new(10_000, Preset::Auto, 0.0),
        )
        .unwrap()
        .0;
        for preset in [
            Preset::Neutral,
            Preset::Auto,
            Preset::Standard,
            Preset::Vivid,
        ] {
            for exposure_ev in [-1.5, 0.0, 0.85, 2.0] {
                let unscaled = derive_params(&stats, preset, exposure_ev, None, 1.0);
                // The pre-knob expression, spelled out.
                let expected = (unscaled.contrast * unscaled.white_input_ev
                    / unscaled.white_output_ev)
                    .clamp(0.45, 4.0);
                assert_eq!(
                    unscaled.highlight_power, expected,
                    "{preset:?} at {exposure_ev} EV moved at the default"
                );
            }
        }
    }

    /// What the knob is for: brighter highlights, and *only* highlights. The
    /// shadow branch, the black point and the exposure the controller solved
    /// for all have to be untouched, otherwise it is a contrast control wearing
    /// a different name and the ground follows the sky up.
    #[test]
    fn highlight_contrast_raises_highlights_and_leaves_everything_below_grey_alone() {
        let stats = analyze(
            &graded_image(),
            &AnalysisInputs::new(10_000, Preset::Auto, 0.0),
        )
        .unwrap()
        .0;
        let base = derive_params(&stats, Preset::Auto, 0.0, None, 1.0);
        let raised = derive_params(&stats, Preset::Auto, 0.0, None, 1.35);

        assert!(
            raised.highlight_power > base.highlight_power,
            "the highlight exponent must rise: {} -> {}",
            base.highlight_power,
            raised.highlight_power
        );
        assert_eq!(raised.shadow_power, base.shadow_power);
        assert_eq!(raised.black_input_ev, base.black_input_ev);
        assert_eq!(raised.black_output_linear, base.black_output_linear);
        assert_eq!(raised.exposure_ev, base.exposure_ev);
        assert_eq!(raised.contrast, base.contrast);

        // Middle grey is the curve's anchor and must not move at all; every
        // sampled highlight must render at or above where it did, and nothing
        // at or below grey may move.
        assert_eq!(
            crate::tone::map_ev(0.0, &raised),
            crate::tone::map_ev(0.0, &base)
        );
        for step in 1..=32 {
            let ev = base.white_input_ev * (step as f32 / 32.0);
            assert!(
                crate::tone::map_ev(ev, &raised) >= crate::tone::map_ev(ev, &base),
                "highlight at {ev} EV was not raised"
            );
        }
        for step in 0..=32 {
            let ev = base.black_input_ev * (step as f32 / 32.0);
            assert_eq!(
                crate::tone::map_ev(ev, &raised),
                crate::tone::map_ev(ev, &base),
                "shadow at {ev} EV moved"
            );
        }
    }

    #[test]
    fn auto_fully_protects_the_brightest_highlight_channel() {
        let (_, params) = analyze(
            &graded_image(),
            &AnalysisInputs::new(10_000, Preset::Auto, 0.0),
        )
        .unwrap();
        assert_eq!(params.highlight_norm, 1.0);
    }
}
