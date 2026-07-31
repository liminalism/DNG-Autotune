use crate::analyze::luminance;
use crate::types::{LinearImage, MID_GRAY, ToneParams};
use image::{ImageBuffer, Rgb};
use rayon::prelude::*;

pub type Rgb16Image = ImageBuffer<Rgb<u16>, Vec<u16>>;

#[inline]
pub(crate) fn srgb_encode(linear: f32) -> f32 {
    let linear = linear.clamp(0.0, 1.0);
    if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// Inverse of [`srgb_encode`].
///
/// Embedded camera previews are sRGB-encoded; the pipeline works in scene-linear
/// light, so a preview has to be linearized before its brightness can be
/// compared with anything here. The breakpoint is `0.0031308 * 12.92`.
#[inline]
pub(crate) fn srgb_decode(encoded: f32) -> f32 {
    let encoded = encoded.clamp(0.0, 1.0);
    if encoded <= 0.040_449_936 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn to_u16(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * u16::MAX as f32 + 0.5) as u16
}

/// Fraction of the highlight range over which the brightest-channel norm ramps
/// in. Below this the curve follows luminance; above it a pixel with
/// `highlight_norm == 1.0` cannot clip.
const HIGHLIGHT_NORM_RAMP: f32 = 0.5;

/// Hermite ramp on `[0, 1]`, clamped outside.
#[inline]
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
pub(crate) fn map_ev(ev: f32, params: &ToneParams) -> f32 {
    if ev <= 0.0 {
        let denominator = (-params.black_input_ev).max(1.0e-4);
        let position = ((ev - params.black_input_ev) / denominator).clamp(0.0, 1.0);
        params.black_output_ev + (0.0 - params.black_output_ev) * position.powf(params.shadow_power)
    } else {
        let denominator = params.white_input_ev.max(1.0e-4);
        let position = (ev / denominator).clamp(0.0, 1.0);
        params.white_output_ev * (1.0 - (1.0 - position).powf(params.highlight_power))
    }
}

/// Inverse of [`map_ev`]: given a display EV, the curve-input EV that produces it.
///
/// The preview oracle measures the vendor's rendering in *display* EV, but the
/// controller's target is *curve-input* EV. Near middle grey the curve's slope is
/// `contrast` (about 1.18 for the auto preset), so using a display EV directly as
/// a target is a systematic error of roughly that factor. Both branches of
/// `map_ev` are analytically invertible, so the conversion is exact wherever the
/// curve has slope. It is ill-conditioned at the endpoints, where the curve is
/// flat and a tiny display change corresponds to a large input change; values
/// are clamped just inside the output range so `powf` never sees a zero base.
#[inline]
pub(crate) fn inverse_map_ev(display_ev: f32, params: &ToneParams) -> f32 {
    // Stay strictly inside the open interval: at the endpoints the powf below
    // sees a zero or negative base.
    let epsilon = 1.0e-4;
    let display_ev = display_ev.clamp(
        params.black_output_ev + epsilon,
        params.white_output_ev - epsilon,
    );

    if display_ev <= 0.0 {
        let black_output = params.black_output_ev;
        if black_output >= -epsilon {
            return 0.0;
        }
        // out = b_out + (0 - b_out) * position^shadow_power
        let position = ((display_ev - black_output) / -black_output).clamp(0.0, 1.0);
        let inverted = position.powf(1.0 / params.shadow_power.max(1.0e-4));
        params.black_input_ev + inverted * -params.black_input_ev
    } else {
        let white_output = params.white_output_ev.max(epsilon);
        // out = w_out * (1 - (1 - position)^highlight_power)
        let ratio = (1.0 - display_ev / white_output).clamp(0.0, 1.0);
        let position = 1.0 - ratio.powf(1.0 / params.highlight_power.max(1.0e-4));
        position * params.white_input_ev
    }
}

#[inline]
fn compress_gamut(mut rgb: [f32; 3], anchor: f32) -> [f32; 3] {
    let anchor = anchor.clamp(0.0, 1.0);
    let minimum = rgb[0].min(rgb[1]).min(rgb[2]);
    let maximum = rgb[0].max(rgb[1]).max(rgb[2]);

    let mut scale = 1.0_f32;

    if minimum < 0.0 {
        let denominator = anchor - minimum;
        if denominator > 1.0e-6 {
            scale = scale.min(anchor / denominator);
        } else {
            scale = 0.0;
        }
    }

    if maximum > 1.0 {
        let denominator = maximum - anchor;
        if denominator > 1.0e-6 {
            scale = scale.min((1.0 - anchor) / denominator);
        } else {
            scale = 0.0;
        }
    }

    for channel in &mut rgb {
        *channel = anchor + (*channel - anchor) * scale.clamp(0.0, 1.0);
        *channel = (*channel).clamp(0.0, 1.0);
    }

    rgb
}

/// Convert working-space linear RGB to display (sRGB primaries) linear RGB.
///
/// Applied at the very top of the render, before anything measures the pixel, so
/// every stage below stays exactly the code it was when the working space and
/// the display space were the same thing. That placement is not just
/// convenience: what actually clips is a *display* channel, so the highlight
/// protection and `compress_gamut` have to be reasoning about display channels
/// to do their jobs. Doing the conversion later would leave them protecting a
/// wide-gamut maximum that is not the one at risk.
///
/// Order does not matter against the exposure gain — both are linear — so
/// folding this in after the gain rather than before it costs nothing.
#[inline]
fn to_display(rgb: [f32; 3], matrix: &crate::color::Matrix3) -> [f32; 3] {
    [
        matrix[0][0] * rgb[0] + matrix[0][1] * rgb[1] + matrix[0][2] * rgb[2],
        matrix[1][0] * rgb[0] + matrix[1][1] * rgb[1] + matrix[1][2] * rgb[2],
        matrix[2][0] * rgb[0] + matrix[2][1] * rgb[1] + matrix[2][2] * rgb[2],
    ]
}

#[inline]
fn render_pixel_local(
    source: [f32; 3],
    params: &ToneParams,
    local_ev: f32,
    working_to_display: Option<&crate::color::Matrix3>,
) -> [u16; 3] {
    let exposure_gain = (params.exposure_ev + local_ev).exp2();
    let exposed = [
        source[0] * exposure_gain,
        source[1] * exposure_gain,
        source[2] * exposure_gain,
    ];
    // `None` when the working space already has sRGB primaries, which is the
    // default: no matrix, no arithmetic, byte-identical to before this existed.
    let exposed = match working_to_display {
        Some(matrix) => to_display(exposed, matrix),
        None => exposed,
    };

    let source_luminance = luminance(exposed).max(1.0e-8);
    let maximum_channel = exposed[0].max(exposed[1]).max(exposed[2]).max(1.0e-8);

    // Driving the curve purely by luminance clips saturated highlights. A blue
    // sky carries roughly 1.7x more signal in its brightest channel than in its
    // luminance, so that channel passes 1.0 well before luminance reaches the
    // white point. `compress_gamut` then has to desaturate it back into range,
    // which is what turns skies into flat white.
    //
    // Blending the curve's input toward the brightest channel in the highlights
    // moves the roll-off onto the curve, where it is smooth, instead of leaving
    // it to a clamp afterwards. At `highlight_norm == 1.0` the brightest channel
    // itself lands on the curve output and so cannot clip.
    // The blend is inert on neutral pixels, where luminance and the brightest
    // channel are equal, so it can reach full strength well below the white
    // point without touching greys. Ramping in over the lower half of the
    // highlight range keeps saturated midtones close to their luminance
    // rendering while still fully protecting anything genuinely bright.
    let luminance_ev = (source_luminance / MID_GRAY).log2();
    let ramp_end = (params.white_input_ev * HIGHLIGHT_NORM_RAMP).max(1.0e-4);
    let highlight_weight =
        smoothstep(luminance_ev / ramp_end) * params.highlight_norm.clamp(0.0, 1.0);
    let norm = source_luminance * (1.0 - highlight_weight) + maximum_channel * highlight_weight;

    let input_ev = (norm / MID_GRAY).log2();
    let output_ev = map_ev(input_ev, params);
    let mapped_norm =
        (MID_GRAY * output_ev.exp2()).clamp(params.black_output_linear, params.white_output_linear);

    let ratio = (mapped_norm / norm).clamp(0.0, 64.0);
    let mut rgb = [exposed[0] * ratio, exposed[1] * ratio, exposed[2] * ratio];

    // Chroma work still anchors on the pixel's own rendered luminance, so the
    // norm blend changes how far highlights are rolled back but not the hue or
    // the relationship between the channels.
    //
    // The floor is `black_output_linear`, not 0, and that detail is load-bearing.
    // `luminance` is a *signed* weighted sum, so a pixel with a large enough
    // negative channel — which the owned colour path produces by design, for
    // colours outside the working space's gamut — has negative luminance. Clamped
    // to 0 it anchored `compress_gamut` at exactly zero, whose scale is then
    // `anchor / (anchor - min)` = `0 / |min|` = 0, multiplying *every* channel by
    // zero and collapsing the pixel to pure black — including its positive
    // channels. Rawler's per-channel clip keeps those, so on that cohort the
    // owned path was strictly more destructive than the clip it replaced. That is
    // the whole of the `crushed_fraction` regression the 0.1.17 A/B measured, and
    // `crate::color`'s `negative_luminance_fraction` predicted it exactly: zero
    // false positives and zero false negatives over 108 frames.
    //
    // Flooring here also removes a plain inconsistency. `mapped_norm` above is
    // already clamped into `[black_output_linear, white_output_linear]`, so the
    // curve and the chroma anchor disagreed about where black is. They now agree.
    let mapped_luminance = luminance(rgb).clamp(params.black_output_linear, 1.0);

    let normalized_highlight = (output_ev / params.white_output_ev.max(1.0e-4)).clamp(0.0, 1.0);
    let highlight_saturation = 1.0 - params.highlight_desaturation * normalized_highlight.powi(2);

    let maximum = rgb[0].max(rgb[1]).max(rgb[2]);
    let minimum = rgb[0].min(rgb[1]).min(rgb[2]);
    let chroma = (maximum - minimum).clamp(0.0, 1.0);
    let midtone_weight = 1.0 - ((mapped_luminance - 0.5).abs() * 2.0).clamp(0.0, 1.0);
    let adaptive_vibrance = 1.0 + params.vibrance * (1.0 - chroma) * midtone_weight;

    // Chroma is expanded around the pixel's own rendered luminance, so a large
    // enough scale drives the outermost channel past the white point the curve
    // just placed it under. On a blue sky that is the whole frame at once: with
    // `highlight_norm == 1.0` the curve deliberately lands the brightest channel
    // just below white, and a chroma boost applied afterwards spends exactly the
    // headroom that protection created. Raising `auto`'s saturation without this
    // cap took the worst Sony frame from 0% to 23% of pixels with a channel at
    // full scale, undoing 0.1.10.
    //
    // So the scale is capped at the value that lands the outermost channel *on*
    // the curve's own output range rather than past it: the boost may spend
    // headroom the curve left unused, and nothing more. `compress_gamut` still
    // follows, as the guard for what the curve itself put out of range.
    //
    // The cap's floor is the scale the pixel would get with no saturation
    // opinion at all, not 1.0. In the deep highlights `highlight_saturation`
    // asks for less than 1.0, and a floor of 1.0 would *overrule* it — turning
    // the guard into a way to push chroma up on exactly the pixels the preset
    // wanted pulled in. Flooring at the unboosted scale means the cap can only
    // ever withhold the boost, so a capped pixel renders as it did before the
    // preset gained one.
    let unboosted_scale = adaptive_vibrance * highlight_saturation;
    let headroom_scale = {
        let upper = if maximum > mapped_luminance {
            (params.white_output_linear - mapped_luminance) / (maximum - mapped_luminance)
        } else {
            f32::INFINITY
        };
        let lower = if minimum < mapped_luminance {
            (mapped_luminance - params.black_output_linear) / (mapped_luminance - minimum)
        } else {
            f32::INFINITY
        };
        upper.min(lower).max(unboosted_scale)
    };

    let chroma_scale = (params.saturation * unboosted_scale).min(headroom_scale);

    for channel in &mut rgb {
        *channel = mapped_luminance + (*channel - mapped_luminance) * chroma_scale;
    }

    let rgb = compress_gamut(rgb, mapped_luminance);
    [
        to_u16(srgb_encode(rgb[0])),
        to_u16(srgb_encode(rgb[1])),
        to_u16(srgb_encode(rgb[2])),
    ]
}

/// One pixel, no local correction, no working-space conversion.
///
/// Test-only since 0.1.17: `render` calls `render_pixel_local` directly so it can
/// thread the working-space matrix through, and every invariant test below is
/// about the sRGB-working-space case, which is what this spells.
#[cfg(test)]
#[inline]
fn render_pixel(source: [f32; 3], params: &ToneParams) -> [u16; 3] {
    render_pixel_local(source, params, 0.0, None)
}

pub fn render(
    image: &LinearImage,
    params: &ToneParams,
    local_tone: Option<&crate::localtone::LocalToneMap>,
    working_to_display: Option<&crate::color::Matrix3>,
) -> Rgb16Image {
    let mut output = vec![0_u16; image.pixels.len() * 3];

    match local_tone {
        Some(local_tone) => {
            assert_eq!(
                local_tone.dimensions(),
                (image.width, image.height),
                "local tone map dimensions must match the rendered image"
            );
            output
                .par_chunks_exact_mut(3)
                .zip(image.pixels.par_iter())
                .zip(local_tone.corrections_ev().par_iter())
                .for_each(|((destination, source), local_ev)| {
                    let rendered =
                        render_pixel_local(*source, params, *local_ev, working_to_display);
                    destination.copy_from_slice(&rendered);
                });
        }
        None => {
            output
                .par_chunks_exact_mut(3)
                .zip(image.pixels.par_iter())
                .for_each(|(destination, source)| {
                    let rendered = render_pixel_local(*source, params, 0.0, working_to_display);
                    destination.copy_from_slice(&rendered);
                });
        }
    }

    ImageBuffer::from_raw(image.width as u32, image.height as u32, output)
        .expect("rendered buffer dimensions are internally consistent")
}

pub fn render_baseline(
    image: &LinearImage,
    working_to_display: Option<&crate::color::Matrix3>,
) -> Rgb16Image {
    let mut output = vec![0_u16; image.pixels.len() * 3];

    output
        .par_chunks_exact_mut(3)
        .zip(image.pixels.par_iter())
        .for_each(|(destination, source)| {
            let source = match working_to_display {
                Some(matrix) => to_display(*source, matrix),
                None => *source,
            };
            destination[0] = to_u16(srgb_encode(source[0]));
            destination[1] = to_u16(srgb_encode(source[1]));
            destination[2] = to_u16(srgb_encode(source[2]));
        });

    ImageBuffer::from_raw(image.width as u32, image.height as u32, output)
        .expect("baseline buffer dimensions are internally consistent")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parameters() -> ToneParams {
        ToneParams {
            exposure_ev: 0.0,
            black_input_ev: -8.0,
            white_input_ev: 4.0,
            black_output_linear: 0.001,
            white_output_linear: 0.99,
            black_output_ev: (0.001 / MID_GRAY).log2(),
            white_output_ev: (0.99 / MID_GRAY).log2(),
            contrast: 1.0,
            shadow_power: 8.0 / (-(0.001 / MID_GRAY).log2()),
            highlight_power: 4.0 / (0.99 / MID_GRAY).log2(),
            saturation: 1.0,
            vibrance: 0.0,
            highlight_desaturation: 0.0,
            highlight_norm: 0.0,
            noise_floor_ev: None,
        }
    }

    /// Same curve, but with full brightest-channel protection in highlights.
    fn parameters_protected() -> ToneParams {
        ToneParams {
            highlight_norm: 1.0,
            ..parameters()
        }
    }

    /// The oracle converts a display EV back to a curve-input EV, so the
    /// inversion has to be exact wherever the curve is well conditioned.
    ///
    /// The last 2% at each end is excluded deliberately: the curve is flat at
    /// its endpoints, so the inverse is ill-conditioned there by construction —
    /// the epsilon that keeps `powf` off a zero base costs about 0.01 EV at the
    /// white point. Real oracle values never sit at the endpoints, and clamping
    /// just inside them is the intended behaviour.
    #[test]
    fn inverse_map_ev_round_trips_across_the_curve() {
        for params in [parameters(), parameters_protected()] {
            let steps = 64;
            for step in 0..=steps {
                let fraction = 0.02 + 0.96 * (step as f32 / steps as f32);
                let ev = params.black_input_ev
                    + fraction * (params.white_input_ev - params.black_input_ev);
                let display = map_ev(ev, &params);
                let recovered = inverse_map_ev(display, &params);
                assert!(
                    (recovered - ev).abs() < 1.0e-3,
                    "ev {ev} -> display {display} -> {recovered}"
                );
            }
        }
    }

    /// At the endpoints the inversion must still be bounded and land inside the
    /// input range, even though it cannot be exact.
    #[test]
    fn inverse_map_ev_is_bounded_at_the_endpoints() {
        let params = parameters();
        for ev in [params.black_input_ev, params.white_input_ev] {
            let recovered = inverse_map_ev(map_ev(ev, &params), &params);
            assert!(
                (recovered - ev).abs() < 0.05,
                "endpoint {ev} recovered as {recovered}"
            );
            assert!(recovered >= params.black_input_ev && recovered <= params.white_input_ev);
        }
    }

    #[test]
    fn srgb_round_trips() {
        for step in 0..=100 {
            let linear = step as f32 / 100.0;
            let recovered = srgb_decode(srgb_encode(linear));
            assert!(
                (recovered - linear).abs() < 1.0e-5,
                "{linear} -> {recovered}"
            );
        }
    }

    #[test]
    fn middle_gray_stays_near_middle_gray() {
        let output = render_pixel([MID_GRAY; 3], &parameters());
        let encoded = output[0] as f32 / u16::MAX as f32;
        let expected = srgb_encode(MID_GRAY);
        assert!((encoded - expected).abs() < 0.002);
    }

    #[test]
    fn extreme_input_renders_without_wrapping() {
        let output = render_pixel([20.0, 0.2, -0.1], &parameters());
        assert_ne!(output, [0, 0, 0]);
    }

    /// A saturated blue sky: the brightest channel sits well above luminance,
    /// so a luminance-driven curve pins it to white and the pixel goes grey.
    /// With the norm blend it must keep both its headroom and its colour.
    #[test]
    fn saturated_highlight_keeps_colour_instead_of_clipping() {
        // Bright enough that the norm ramp is fully in, so `highlight_norm`
        // 1.0 gives a hard no-clip guarantee.
        let sky = [0.60, 1.10, 4.40];

        let luminance_driven = render_pixel(sky, &parameters());
        let protected = render_pixel(sky, &parameters_protected());

        let spread = |pixel: [u16; 3]| {
            let maximum = pixel.iter().copied().max().unwrap();
            let minimum = pixel.iter().copied().min().unwrap();
            maximum - minimum
        };

        assert_eq!(
            luminance_driven[2],
            u16::MAX,
            "the luminance-driven curve is expected to clip the blue channel"
        );
        assert!(
            spread(luminance_driven) < spread(protected) / 2,
            "clipping should have collapsed most of the colour"
        );
        assert!(
            protected[2] < u16::MAX,
            "the brightest channel must not clip when highlight_norm is 1.0"
        );
        assert!(
            spread(protected) > spread(luminance_driven),
            "protecting the highlight must retain more colour, got {protected:?} vs {luminance_driven:?}"
        );
    }

    /// The saturation boost must not be what blows a highlight. A saturated
    /// bright pixel — a blue sky is the everyday case — has to come out below
    /// full scale however much saturation the preset asks for.
    #[test]
    fn saturation_cannot_push_a_highlight_channel_to_full_scale() {
        let sky = [0.60, 1.10, 4.40];
        for saturation in [1.0, 1.22, 1.6, 3.0] {
            let params = ToneParams {
                saturation,
                ..parameters_protected()
            };
            let rendered = render_pixel(sky, &params);
            assert!(
                rendered.iter().all(|channel| *channel < u16::MAX),
                "saturation {saturation} clipped a channel: {rendered:?}"
            );
        }
    }

    /// The cap withholds a boost; it must never take away chroma the pixel had
    /// without one, or a bright saturated subject would render flatter than it
    /// does today.
    #[test]
    fn the_headroom_cap_never_removes_existing_chroma() {
        let spread = |pixel: [u16; 3]| {
            pixel.iter().copied().max().unwrap() - pixel.iter().copied().min().unwrap()
        };
        for source in [[0.60, 1.10, 4.40], [0.30, 0.22, 0.10], [0.9, 0.9, 0.2]] {
            let plain = render_pixel(source, &parameters_protected());
            let boosted = render_pixel(
                source,
                &ToneParams {
                    saturation: 1.22,
                    ..parameters_protected()
                },
            );
            assert!(
                spread(boosted) >= spread(plain),
                "boosting saturation reduced chroma on {source:?}: {plain:?} -> {boosted:?}"
            );
        }
    }

    /// Raising saturation must never be what pins a channel at full scale.
    ///
    /// This is the invariant the Sony corpus caught: without the cap, `auto`'s
    /// larger saturation took the worst frame from 0% to 23% of pixels with a
    /// channel at full scale. It is checked over saturated colours at a range
    /// of brightnesses, and under `highlight_norm` 0.0 as well as 1.0, because
    /// only the protected curve keeps the brightest channel off the ceiling on
    /// its own — the cap has to hold for the other one too.
    #[test]
    fn raising_saturation_never_adds_a_clipped_channel() {
        let colours = [
            [0.10, 0.55, 6.00],
            [0.60, 1.10, 4.40],
            [3.00, 0.40, 0.25],
            [0.95, 0.90, 0.20],
            [2.20, 2.00, 0.30],
            [0.30, 0.22, 0.10],
        ];
        for base in [parameters(), parameters_protected()] {
            let unopinionated = ToneParams {
                saturation: 1.0,
                highlight_desaturation: 0.16,
                ..base
            };
            let boosted = ToneParams {
                saturation: 1.22,
                ..unopinionated
            };
            for colour in colours {
                let plain = render_pixel(colour, &unopinionated);
                let raised = render_pixel(colour, &boosted);
                for channel in 0..3 {
                    assert!(
                        raised[channel] < u16::MAX || plain[channel] == u16::MAX,
                        "saturation pinned channel {channel} of {colour:?}: \
                         {plain:?} -> {raised:?} (highlight_norm {})",
                        base.highlight_norm
                    );
                }
            }
        }
    }

    /// Where there is headroom the boost has to actually arrive, otherwise the
    /// cap would have quietly disabled the preset it is protecting.
    #[test]
    fn a_midtone_with_headroom_still_gains_chroma() {
        let midtone = [0.20, 0.16, 0.11];
        let plain = render_pixel(midtone, &parameters_protected());
        let boosted = render_pixel(
            midtone,
            &ToneParams {
                saturation: 1.22,
                ..parameters_protected()
            },
        );
        let spread = |pixel: [u16; 3]| {
            (pixel.iter().copied().max().unwrap() - pixel.iter().copied().min().unwrap()) as f32
        };
        assert!(
            spread(boosted) > spread(plain) * 1.10,
            "expected a real chroma gain, got {plain:?} -> {boosted:?}"
        );
    }

    /// The blend must not disturb midtones, which are what the exposure
    /// controller is actually anchored on.
    #[test]
    fn middle_gray_is_unaffected_by_the_norm_blend() {
        let plain = render_pixel([MID_GRAY; 3], &parameters());
        let protected = render_pixel([MID_GRAY; 3], &parameters_protected());
        assert_eq!(plain, protected);
    }

    #[test]
    fn local_exposure_is_one_hue_preserving_rgb_gain() {
        let source = [0.07, 0.18, 0.31];
        let local_ev: f32 = 0.65;
        let gain = local_ev.exp2();
        let pre_scaled = [source[0] * gain, source[1] * gain, source[2] * gain];
        assert_eq!(
            render_pixel_local(source, &parameters_protected(), local_ev, None),
            render_pixel(pre_scaled, &parameters_protected())
        );
    }

    /// A wide working space must be a change of coordinates, not a change of
    /// colour: the *same colour* expressed in BT.2020 has to render to the same
    /// output pixel as it does in sRGB. If this fails, `--working-space rec2020`
    /// is silently shifting hue and the A/B against the Rawler path is measuring
    /// two things at once.
    #[test]
    fn a_wide_working_space_renders_the_same_colour_the_same_way() {
        use crate::color::WorkingSpace;

        let rec2020_to_display = WorkingSpace::Rec2020
            .to_display()
            .expect("BT.2020 needs a conversion");

        // Invert it so a target sRGB colour can be expressed in BT.2020 first.
        let display_to_rec2020 =
            crate::color::invert3(rec2020_to_display).expect("the conversion is invertible");

        // A neutral midtone, a saturated sky, and an out-of-sRGB-gamut green.
        for srgb in [[MID_GRAY; 3], [0.60, 1.10, 4.40], [0.05, 0.90, 0.10]] {
            let as_rec2020 = [
                display_to_rec2020[0][0] * srgb[0]
                    + display_to_rec2020[0][1] * srgb[1]
                    + display_to_rec2020[0][2] * srgb[2],
                display_to_rec2020[1][0] * srgb[0]
                    + display_to_rec2020[1][1] * srgb[1]
                    + display_to_rec2020[1][2] * srgb[2],
                display_to_rec2020[2][0] * srgb[0]
                    + display_to_rec2020[2][1] * srgb[1]
                    + display_to_rec2020[2][2] * srgb[2],
            ];

            let direct = render_pixel_local(srgb, &parameters_protected(), 0.0, None);
            let converted = render_pixel_local(
                as_rec2020,
                &parameters_protected(),
                0.0,
                Some(&rec2020_to_display),
            );

            for channel in 0..3 {
                let difference = direct[channel].abs_diff(converted[channel]);
                assert!(
                    difference < 96,
                    "channel {channel} of {srgb:?} rendered as {direct:?} in sRGB but \
                     {converted:?} via BT.2020 (difference {difference} of 65535)"
                );
            }
        }
    }
}
