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
fn map_ev(ev: f32, params: &ToneParams) -> f32 {
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

#[inline]
fn render_pixel(source: [f32; 3], params: &ToneParams) -> [u16; 3] {
    let exposure_gain = params.exposure_ev.exp2();
    let exposed = [
        source[0] * exposure_gain,
        source[1] * exposure_gain,
        source[2] * exposure_gain,
    ];

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
    let mapped_luminance = luminance(rgb).clamp(0.0, 1.0);

    let normalized_highlight = (output_ev / params.white_output_ev.max(1.0e-4)).clamp(0.0, 1.0);
    let highlight_saturation = 1.0 - params.highlight_desaturation * normalized_highlight.powi(2);

    let maximum = rgb[0].max(rgb[1]).max(rgb[2]);
    let minimum = rgb[0].min(rgb[1]).min(rgb[2]);
    let chroma = (maximum - minimum).clamp(0.0, 1.0);
    let midtone_weight = 1.0 - ((mapped_luminance - 0.5).abs() * 2.0).clamp(0.0, 1.0);
    let adaptive_vibrance = 1.0 + params.vibrance * (1.0 - chroma) * midtone_weight;
    let chroma_scale = params.saturation * adaptive_vibrance * highlight_saturation;

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

pub fn render(image: &LinearImage, params: &ToneParams) -> Rgb16Image {
    let mut output = vec![0_u16; image.pixels.len() * 3];

    output
        .par_chunks_exact_mut(3)
        .zip(image.pixels.par_iter())
        .for_each(|(destination, source)| {
            let rendered = render_pixel(*source, params);
            destination.copy_from_slice(&rendered);
        });

    ImageBuffer::from_raw(image.width as u32, image.height as u32, output)
        .expect("rendered buffer dimensions are internally consistent")
}

pub fn render_baseline(image: &LinearImage) -> Rgb16Image {
    let mut output = vec![0_u16; image.pixels.len() * 3];

    output
        .par_chunks_exact_mut(3)
        .zip(image.pixels.par_iter())
        .for_each(|(destination, source)| {
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

    /// The blend must not disturb midtones, which are what the exposure
    /// controller is actually anchored on.
    #[test]
    fn middle_gray_is_unaffected_by_the_norm_blend() {
        let plain = render_pixel([MID_GRAY; 3], &parameters());
        let protected = render_pixel([MID_GRAY; 3], &parameters_protected());
        assert_eq!(plain, protected);
    }
}
