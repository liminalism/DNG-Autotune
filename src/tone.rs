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
    // Shadow case (negative channel): use anchor-floored scale that was fixed
    // for the 0.1.17 crushed regression. Oklab path would collapse to black.
    if minimum < 0.0 {
        let mut scale = 1.0f32;
        let denominator = anchor - minimum;
        if denominator > 1.0e-6 {
            scale = scale.min(anchor / denominator);
        } else {
            scale = 0.0;
        }
        if maximum > 1.0 {
            let denom = maximum - anchor;
            if denom > 1.0e-6 {
                scale = scale.min((1.0 - anchor) / denom);
            } else {
                scale = 0.0;
            }
        }
        for ch in &mut rgb {
            *ch = anchor + (*ch - anchor) * scale.clamp(0.0, 1.0);
            *ch = (*ch).clamp(0.0, 1.0);
        }
        return rgb;
    }
    // Highlight case: OKLab hue-preserving chroma reduction.
    let in_gamut = rgb[0] >= 0.0
        && rgb[0] <= 1.0
        && rgb[1] >= 0.0
        && rgb[1] <= 1.0
        && rgb[2] >= 0.0
        && rgb[2] <= 1.0;
    if in_gamut {
        return rgb;
    }
    let lab = crate::oklab::from_linear_srgb(rgb);
    let chroma = lab.chroma();
    if chroma < crate::oklab::CHROMA_FLOOR {
        // Near-neutral but out of gamut (e.g., L out of range): clamp
        return [
            rgb[0].clamp(0.0, 1.0),
            rgb[1].clamp(0.0, 1.0),
            rgb[2].clamp(0.0, 1.0),
        ];
    }
    // Binary search for max chroma scale that yields in-gamut sRGB.
    let mut low = 0.0f32;
    let mut high = 1.0f32;
    let achro = crate::oklab::to_linear_srgb(crate::oklab::Oklab {
        l: lab.l,
        a: 0.0,
        b: 0.0,
    });
    let mut best = [
        achro[0].clamp(0.0, 1.0),
        achro[1].clamp(0.0, 1.0),
        achro[2].clamp(0.0, 1.0),
    ];
    // Achromatic is fallback, but we search for largest feasible chroma.
    for _ in 0..20 {
        let mid = (low + high) * 0.5;
        let test_lab = crate::oklab::Oklab {
            l: lab.l,
            a: lab.a * mid,
            b: lab.b * mid,
        };
        let test_rgb = crate::oklab::to_linear_srgb(test_lab);
        let feasible = test_rgb[0] >= 0.0
            && test_rgb[0] <= 1.0
            && test_rgb[1] >= 0.0
            && test_rgb[1] <= 1.0
            && test_rgb[2] >= 0.0
            && test_rgb[2] <= 1.0;
        if feasible {
            low = mid;
            best = test_rgb;
        } else {
            high = mid;
        }
    }
    // If even the low end is not feasible (shouldn't happen for L in [0,1]), clamp
    best[0] = best[0].clamp(0.0, 1.0);
    best[1] = best[1].clamp(0.0, 1.0);
    best[2] = best[2].clamp(0.0, 1.0);
    best
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

/// Samples per octave in [`ToneLut`]. 64 keeps the interpolation error of the
/// curve, which is smooth in log2 space, around one part in 10^5 of the value —
/// two orders of magnitude below one 8-bit JPEG step — while the whole table
/// still fits in 40 KiB.
const LUT_SHIFT: u32 = 23 - 6;
/// Bit pattern of 2^-38, the bottom of the tabulated range. `map_ev` clamps its
/// position to `[0, 1]`, so the curve is *constant* below `black_input_ev` and
/// above `white_input_ev`; the tabulated range covers both knees by a wide
/// margin (2^-38 ≈ -35.8 EV, 2^14 ≈ +16.5 EV around middle grey), which makes
/// clamping to the end entries exact, not an approximation.
const LUT_MIN_BITS: u32 = 89 << 23;
/// Bit pattern of 2^14, the top of the tabulated range.
const LUT_MAX_BITS: u32 = 141 << 23;
const LUT_LEN: usize = (((LUT_MAX_BITS - LUT_MIN_BITS) >> LUT_SHIFT) + 1) as usize;

/// Locate a positive linear value in the table: entry index plus the fraction
/// toward the next entry.
///
/// For positive finite floats the IEEE 754 bit pattern is monotonic in the
/// value and piecewise linear within each octave, and every table segment lies
/// inside one octave (64 divides the octave boundary exactly), so interpolating
/// on the mantissa-bit fraction *is* linear interpolation in the value itself —
/// no logarithm needed to index a table that is uniform in EV.
#[inline]
fn lut_position(value: f32) -> (usize, f32) {
    let bits = value.to_bits();
    if bits <= LUT_MIN_BITS {
        return (0, 0.0);
    }
    if bits >= LUT_MAX_BITS {
        return (LUT_LEN - 1, 0.0);
    }
    let offset = bits - LUT_MIN_BITS;
    let index = (offset >> LUT_SHIFT) as usize;
    let fraction = (offset & ((1 << LUT_SHIFT) - 1)) as f32 * (1.0 / (1u32 << LUT_SHIFT) as f32);
    (index, fraction)
}

/// Per-image tables for the three transcendental stages of the tone curve.
///
/// `render_pixel_linear` used to spend one `log2f`, one `powf` (inside
/// [`map_ev`]) and one `exp2f` per pixel — about a tenth of the whole
/// program's retired instructions. All three are 1-D functions of a single
/// linear value once `ToneParams` is fixed, so `render` tabulates them once
/// per frame (3,329 entries, microseconds) and the hot loop does two
/// interpolated lookups instead. Curve accuracy is verified by
/// `lut_matches_the_exact_curve` below.
struct ToneLut {
    /// Per entry: `[mapped_norm, normalized_highlight]` as functions of the
    /// post-exposure norm — the clamped curve output
    /// `MID_GRAY * 2^map_ev(log2(x / MID_GRAY))` and the `[0, 1]` highlight
    /// position `output_ev / white_output_ev` that drives desaturation.
    curve: Vec<[f32; 2]>,
    /// Highlight-norm blend weight as a function of source luminance, with
    /// `params.highlight_norm` already folded in. Every entry at or below
    /// middle grey is exactly `0.0`, so shadows and midtones — everything more
    /// than one table segment (~1.1%) below middle grey — still blend to the
    /// pure luminance norm exactly as before; only the single segment
    /// straddling middle grey interpolates toward the first nonzero entry.
    weight: Vec<f32>,
}

impl ToneLut {
    fn new(params: &ToneParams) -> Self {
        let ramp_end = (params.white_input_ev * HIGHLIGHT_NORM_RAMP).max(1.0e-4);
        let highlight_norm = params.highlight_norm.clamp(0.0, 1.0);
        let white_output = params.white_output_ev.max(1.0e-4);

        let mut curve = vec![[0.0_f32; 2]; LUT_LEN];
        let mut weight = vec![0.0_f32; LUT_LEN];
        for (index, (curve_entry, weight_entry)) in
            curve.iter_mut().zip(weight.iter_mut()).enumerate()
        {
            let value = f32::from_bits(LUT_MIN_BITS + (index as u32) * (1 << LUT_SHIFT));
            let ev = (value / MID_GRAY).log2();
            let output_ev = map_ev(ev, params);
            let mapped_norm = (MID_GRAY * output_ev.exp2())
                .clamp(params.black_output_linear, params.white_output_linear);
            let normalized_highlight = (output_ev / white_output).clamp(0.0, 1.0);
            *curve_entry = [mapped_norm, normalized_highlight];
            *weight_entry = smoothstep(ev / ramp_end) * highlight_norm;
        }

        Self { curve, weight }
    }

    /// `(mapped_norm, normalized_highlight)` for a post-exposure norm.
    #[inline]
    fn curve(&self, norm: f32) -> (f32, f32) {
        let (index, fraction) = lut_position(norm);
        let low = self.curve[index];
        let high = self.curve[(index + 1).min(LUT_LEN - 1)];
        (
            low[0] + (high[0] - low[0]) * fraction,
            low[1] + (high[1] - low[1]) * fraction,
        )
    }

    /// Highlight-norm blend weight for a source luminance.
    #[inline]
    fn weight(&self, source_luminance: f32) -> f32 {
        let (index, fraction) = lut_position(source_luminance);
        let low = self.weight[index];
        let high = self.weight[(index + 1).min(LUT_LEN - 1)];
        low + (high - low) * fraction
    }
}

/// Everything `render_pixel_local` does except the final sRGB encode: the
/// exposure gain, the highlight-aware tone curve, chroma/vibrance, and
/// `compress_gamut`. Returns linear RGB already clamped to `[0, 1]` by
/// `compress_gamut`, ready for the transfer function.
///
/// Split out so `render` can batch the encode step across a whole image via
/// `linear_srgb`'s SIMD slice API instead of one `srgb_encode` call per
/// channel per pixel — the single-value call is not the crate's fast path
/// (measured slower than our own `powf`, in fact); the ~4-16x win the crate
/// advertises is specifically for slices, which is why this is a separate
/// pass rather than inlined here.
///
/// Takes `exposure_gain` already computed rather than `local_ev` +
/// `params.exposure_ev`, so the caller can hoist `(exposure_ev + 0.0).exp2()`
/// out of the per-pixel loop when there is no local tone map: `local_ev` is
/// then the same `0.0` for every pixel in the image, so the gain is one
/// constant, not up to ~31M redundant `exp2()` calls on a 10.5 MP frame.
/// One pixel, rendered to display-linear RGB with diagnostic checkpoints after
/// the tone curve, chroma scale, highlight white mix, and [`compress_gamut`].
///
/// `--dump-stages` needs every checkpoint and the renderer needs the last; they
/// must be the same arithmetic or the diagnostic is measuring a different
/// program from the one that shipped, which is how it was before this was one
/// function.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderPixelStages {
    pub(crate) after_curve: [f32; 3],
    pub(crate) after_chroma: [f32; 3],
    pub(crate) gamut_pre: [f32; 3],
    pub(crate) gamut_post: [f32; 3],
}

#[inline]
fn render_pixel_stages(
    source: [f32; 3],
    params: &ToneParams,
    lut: &ToneLut,
    exposure_gain: f32,
    reconstruction_uncertainty: f32,
    working_to_display: Option<&crate::color::Matrix3>,
) -> RenderPixelStages {
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
    let highlight_weight = lut.weight(source_luminance);
    let norm = source_luminance * (1.0 - highlight_weight) + maximum_channel * highlight_weight;
    let (mapped_norm, normalized_highlight) = lut.curve(norm);

    let ratio = (mapped_norm / norm).clamp(0.0, 64.0);
    let mut rgb = [exposed[0] * ratio, exposed[1] * ratio, exposed[2] * ratio];
    let after_curve = rgb;

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

    // Mantiuk et al.'s display-adaptive reconstruction expresses colour as a
    // ratio to luminance and raises that ratio to an exponent `s`. Applying it
    // only while the curve is actually compressing keeps shadows and midtones
    // byte-identical, while `s=1` is an explicit exact no-op. Renormalising the
    // candidate to the already-mapped luminance prevents the colour experiment
    // from becoming a second exposure control.
    let requested_exponent = params.highlight_color_ratio_exponent.clamp(0.0, 1.0);
    if requested_exponent < 1.0
        && mapped_luminance > 1.0e-8
        && rgb
            .iter()
            .all(|channel| *channel > 0.0 && channel.is_finite())
    {
        let curve_ratio = (mapped_norm / norm).clamp(0.0, 1.0);
        let compression_weight = smoothstep(((1.0 - curve_ratio) * 2.0).clamp(0.0, 1.0));
        let exponent = 1.0 - (1.0 - requested_exponent) * compression_weight;
        if exponent < 1.0 {
            let mut candidate = rgb.map(|channel| (channel / mapped_luminance).powf(exponent));
            let candidate_luminance = luminance(candidate);
            if candidate_luminance.is_finite() && candidate_luminance > 1.0e-8 {
                let preserve_luminance = mapped_luminance / candidate_luminance;
                candidate
                    .iter_mut()
                    .for_each(|channel| *channel *= preserve_luminance);
                rgb = candidate;
            }
        }
    }

    let maximum = rgb[0].max(rgb[1]).max(rgb[2]);
    let minimum = rgb[0].min(rgb[1]).min(rgb[2]);
    let chroma = (maximum - minimum).clamp(0.0, 1.0);
    let midtone_weight = 1.0 - ((mapped_luminance - 0.5).abs() * 2.0).clamp(0.0, 1.0);
    let adaptive_vibrance = 1.0 + params.vibrance * (1.0 - chroma) * midtone_weight;
    // Highlight desaturation is applied below as a path *toward white*: lower
    // channels rise toward the brightest one while the brightest channel stays
    // fixed. Its complementary scale remains useful as the lowest scale the
    // headroom cap may choose: unlike an unconditional identity floor, it lets
    // an already out-of-range chromatic highlight contract just enough to avoid
    // clipping before taking the max-preserving path to white.
    let highlight_white_mix =
        (params.highlight_desaturation * normalized_highlight.powi(2)).clamp(0.0, 1.0);
    let highlight_chroma_floor = 1.0 - highlight_white_mix;

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
    // This is a safety floor for the cap, not the scale ultimately rendered.
    // Keeping adaptive vibrance without the highlight factor here forced the
    // result above available headroom on the corpus; using the shoulder's own
    // floor retains the old no-new-clipping bound.
    let headroom_floor = adaptive_vibrance * highlight_chroma_floor;
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
        upper.min(lower).max(headroom_floor)
    };

    // Withhold the chroma *boost* where highlight reconstruction synthesised the
    // colour. The gate is the per-pixel reconstruction uncertainty carried from
    // `highlight::reconstruct` (raw-domain clip confidence), not the pixel's
    // display brightness — so a reconstructed highlight that tone-maps dim is
    // still covered, which a brightness proxy cannot see.
    //
    // Withholding means interpolating back toward the adaptive-vibrance scale,
    // but never past the scale that actually fits the available headroom.
    // Scaling the result *down*
    // instead, as this once did, does not withhold an opinion; it imposes the
    // opposite one. At full uncertainty a factor of 0.15 collapsed every channel
    // onto `mapped_luminance`, and since the tone curve lands a blown sky well
    // under white, the most-blown pixels in the frame came out as the darkest:
    // flat grey interiors ringed by the brighter, less-clipped pixels along
    // every foliage edge. Reconstruction makes those pixels neutral in the raw
    // domain; there is no cast left for a chroma cut to remove.
    let gate = crate::highlight::synthesis_gate(reconstruction_uncertainty);
    let boosted = (params.saturation * adaptive_vibrance).min(headroom_scale);
    let gated_scale = adaptive_vibrance.min(headroom_scale);
    let chroma_scale = (boosted + (gated_scale - boosted) * gate).min(boosted);

    for channel in &mut rgb {
        *channel = mapped_luminance + (*channel - mapped_luminance) * chroma_scale;
    }
    let after_chroma = rgb;

    // Travel toward white without darkening.  The peak is invariant, every
    // other channel is monotone non-decreasing, and the operation is continuous
    // in both source brightness and `highlight_desaturation`.  This is what
    // removes a recorded blue/lavender strip beside a reconstructed near-white
    // region without inventing a spatial halo or collapsing the region onto its
    // much lower luminance.
    let white_anchor = rgb[0].max(rgb[1]).max(rgb[2]);
    for channel in &mut rgb {
        *channel += (white_anchor - *channel) * highlight_white_mix;
    }

    let gamut_anchor = luminance(rgb).clamp(params.black_output_linear, 1.0);
    RenderPixelStages {
        after_curve,
        after_chroma,
        gamut_pre: rgb,
        gamut_post: compress_gamut(rgb, gamut_anchor),
    }
}

/// Renderer-exact display-linear checkpoints for an in-memory benchmark.
///
/// This is intentionally crate-visible rather than a second test renderer:
/// synthetic truth and captured candidates must pass through the same
/// arithmetic as production or the current tone-shoulder amplification is
/// invisible to the gate.
pub(crate) fn render_checkpoints(
    image: &LinearImage,
    params: &ToneParams,
    reconstruction_uncertainty: Option<&[f32]>,
    working_to_display: Option<&crate::color::Matrix3>,
) -> Vec<RenderPixelStages> {
    if let Some(map) = reconstruction_uncertainty {
        assert_eq!(map.len(), image.pixels.len());
    }
    let lut = ToneLut::new(params);
    let exposure_gain = params.exposure_ev.exp2();
    image
        .pixels
        .par_iter()
        .enumerate()
        .map(|(index, source)| {
            render_pixel_stages(
                *source,
                params,
                &lut,
                exposure_gain,
                reconstruction_uncertainty.map_or(0.0, |map| map[index]),
                working_to_display,
            )
        })
        .collect()
}

/// One pixel, rendered to display-linear RGB.
#[inline]
fn render_pixel_linear(
    source: [f32; 3],
    params: &ToneParams,
    lut: &ToneLut,
    exposure_gain: f32,
    reconstruction_uncertainty: f32,
    working_to_display: Option<&crate::color::Matrix3>,
) -> [f32; 3] {
    render_pixel_stages(
        source,
        params,
        lut,
        exposure_gain,
        reconstruction_uncertainty,
        working_to_display,
    )
    .gamut_post
}

/// One pixel, sRGB-encoded. Test-only: `render`'s hot loop does not call
/// this — it batches the encode step separately, see [`render_pixel_linear`].
#[cfg(test)]
#[inline]
fn render_pixel_local(
    source: [f32; 3],
    params: &ToneParams,
    local_ev: f32,
    working_to_display: Option<&crate::color::Matrix3>,
) -> [u16; 3] {
    let exposure_gain = (params.exposure_ev + local_ev).exp2();
    let lut = ToneLut::new(params);
    let rgb = render_pixel_linear(source, params, &lut, exposure_gain, 0.0, working_to_display);
    [
        to_u16(srgb_encode(rgb[0])),
        to_u16(srgb_encode(rgb[1])),
        to_u16(srgb_encode(rgb[2])),
    ]
}

/// One pixel, no local correction, no working-space conversion.
///
/// Test-only since 0.1.17: `render` calls `render_pixel_linear` directly so it
/// can thread the working-space matrix through, and every invariant test below
/// is about the sRGB-working-space case, which is what this spells.
#[cfg(test)]
#[inline]
fn render_pixel(source: [f32; 3], params: &ToneParams) -> [u16; 3] {
    render_pixel_local(source, params, 0.0, None)
}

pub fn render(
    image: &LinearImage,
    params: &ToneParams,
    local_tone: Option<&dyn crate::localtone::CorrectionField>,
    reconstruction_uncertainty: Option<&[f32]>,
    working_to_display: Option<&crate::color::Matrix3>,
) -> Rgb16Image {
    let mut linear = vec![0.0_f32; image.pixels.len() * 3];
    let lut = ToneLut::new(params);

    if let Some(local_tone) = local_tone {
        assert_eq!(
            local_tone.dimensions(),
            (image.width, image.height),
            "local tone map dimensions must match the rendered image"
        );
    }
    if let Some(map) = reconstruction_uncertainty {
        assert_eq!(
            map.len(),
            image.pixels.len(),
            "reconstruction uncertainty map must match the rendered image"
        );
    }
    // With no local tone map every pixel shares one exposure gain, so the
    // `exp2()` is hoisted out of the loop rather than run ~31M times.
    let base_gain = params.exposure_ev.exp2();

    linear
        .par_chunks_exact_mut(3)
        .enumerate()
        .zip(image.pixels.par_iter())
        .for_each(|((index, destination), source)| {
            let exposure_gain = match local_tone {
                Some(local_tone) => {
                    (params.exposure_ev + local_tone.corrections_ev()[index]).exp2()
                }
                None => base_gain,
            };
            let uncertainty = reconstruction_uncertainty.map_or(0.0, |map| map[index]);
            let rendered = render_pixel_linear(
                *source,
                params,
                &lut,
                exposure_gain,
                uncertainty,
                working_to_display,
            );
            destination.copy_from_slice(&rendered);
        });

    let output = encode_srgb_u16(&linear);

    ImageBuffer::from_raw(image.width as u32, image.height as u32, output)
        .expect("rendered buffer dimensions are internally consistent")
}

/// Write display-linear diagnostics after the curve, chroma scale, highlight
/// white mix, and gamut compression.
///
/// Best-effort, never fails the file. Called from the pipeline when
/// `--dump-stages` is set, after `ToneParams` are solved.
pub(crate) fn dump_gamut_diagnostics(
    image: &LinearImage,
    params: &ToneParams,
    local_tone: Option<&dyn crate::localtone::CorrectionField>,
    reconstruction_uncertainty: Option<&[f32]>,
    working_to_display: Option<&crate::color::Matrix3>,
    dump_dir: &std::path::Path,
    stem: &str,
) {
    let res: anyhow::Result<()> = (|| {
        let lut = ToneLut::new(params);
        let base_gain = params.exposure_ev.exp2();
        type StageSelector = fn(RenderPixelStages) -> [f32; 3];
        let stages: [(&str, StageSelector); 4] = [
            ("tone-after-curve", |stage| stage.after_curve),
            ("tone-after-chroma", |stage| stage.after_chroma),
            ("gamut-pre", |stage| stage.gamut_pre),
            ("gamut-post", |stage| stage.gamut_post),
        ];
        for (label, select) in stages {
            // Render one diagnostic plane at a time. Four passes cost more CPU,
            // but `--dump-stages` is explicitly diagnostic and this keeps peak
            // memory to one full-resolution RGB plane instead of four.
            let mut linear = vec![0.0f32; image.pixels.len() * 3];
            linear
                .par_chunks_exact_mut(3)
                .enumerate()
                .zip(image.pixels.par_iter())
                .for_each(|((index, destination), src)| {
                    let gain = match local_tone {
                        Some(lt) => (params.exposure_ev + lt.corrections_ev()[index]).exp2(),
                        None => base_gain,
                    };
                    let u = reconstruction_uncertainty.map_or(0.0, |map| map[index]);
                    let stage =
                        render_pixel_stages(*src, params, &lut, gain, u, working_to_display);
                    destination.copy_from_slice(&select(stage));
                });
            let out = encode_srgb_u16(&linear);
            let p = dump_dir.join(format!("{stem}-{label}.png"));
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let img: Rgb16Image =
                ImageBuffer::from_raw(image.width as u32, image.height as u32, out).expect("dims");
            img.save(&p)?;
            eprintln!("DUMP  {label}: {}", p.display());
        }
        Ok(())
    })();
    if let Err(e) = res {
        eprintln!("DUMP  gamut diagnostics failed for {stem}: {e}");
    }
}

/// Write the exact encoded buffer that the pipeline is about to hand to the
/// output writer. This is deliberately separate from `gamut-post`: output
/// sharpening is a later stage, so the post-sharpen checkpoint is the only
/// diagnostic that can be byte-identical to a default CLI output.
pub(crate) fn dump_final_diagnostic(rendered: &Rgb16Image, dump_dir: &std::path::Path, stem: &str) {
    let res: anyhow::Result<()> = (|| {
        let p = dump_dir.join(format!("{stem}-final.png"));
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        rendered.save(&p)?;
        eprintln!("DUMP  final: {}", p.display());
        Ok(())
    })();
    if let Err(e) = res {
        eprintln!("DUMP  final diagnostic failed for {stem}: {e}");
    }
}

/// Batch sRGB encode: linear `[0, 1]` values straight to `u16`, in chunks run
/// across the rayon pool, each chunk SIMD-dispatched by `linear_srgb`. This is
/// where the per-pixel `powf`/rayon-bridge cost in `render_pixel_local` moved
/// to — one call per chunk instead of 3 `srgb_encode` calls per pixel.
///
/// Uses `linear_srgb`'s C0-continuous constants rather than this crate's own
/// textbook IEC ones (see `srgb_encode`) — measured max 1 u16-level deviation,
/// mean 0.2, over an exhaustive `[0, 1]` sweep against the exact powf curve;
/// invisible at the 8-bit JPEG output this pipeline defaults to. See
/// `CHANGELOG.md` for the measurement that justified adopting it here.
fn encode_srgb_u16(linear: &[f32]) -> Vec<u16> {
    const CHUNK: usize = 65_536;
    let mut output = vec![0_u16; linear.len()];
    output
        .par_chunks_mut(CHUNK)
        .zip(linear.par_chunks(CHUNK))
        .for_each(|(destination, source)| {
            linear_srgb::default::linear_to_srgb_u16_slice(source, destination);
        });
    output
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
            highlight_color_ratio_exponent: 1.0,
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

    #[test]
    fn color_ratio_exponent_reduces_only_compressed_chroma_and_preserves_luminance() {
        let source = [6.0, 2.0, 6.0];
        let baseline_params = parameters_protected();
        let baseline_lut = ToneLut::new(&baseline_params);
        let baseline =
            render_pixel_stages(source, &baseline_params, &baseline_lut, 1.0, 0.0, None).gamut_pre;
        let paper_params = ToneParams {
            highlight_color_ratio_exponent: 0.6,
            ..baseline_params
        };
        let paper_lut = ToneLut::new(&paper_params);
        let paper =
            render_pixel_stages(source, &paper_params, &paper_lut, 1.0, 0.0, None).gamut_pre;

        let spread = |pixel: [f32; 3]| {
            pixel.into_iter().fold(f32::NEG_INFINITY, f32::max)
                - pixel.into_iter().fold(f32::INFINITY, f32::min)
        };
        assert!(spread(paper) < spread(baseline));
        assert!((luminance(paper) - luminance(baseline)).abs() < 1.0e-6);

        let midtone = [0.24, 0.12, 0.20];
        let plain_mid = render_pixel_stages(
            midtone,
            &parameters_protected(),
            &ToneLut::new(&parameters_protected()),
            1.0,
            0.0,
            None,
        )
        .gamut_pre;
        let paper_mid =
            render_pixel_stages(midtone, &paper_params, &paper_lut, 1.0, 0.0, None).gamut_pre;
        assert_eq!(plain_mid, paper_mid, "uncompressed midtones must be exact");
    }

    /// The last diagnostic checkpoint is the renderer's output, not a second
    /// approximation of it.  Keep this assertion on the encoded u16 buffer so
    /// a future change cannot make `gamut-post` look equivalent while differing
    /// at values that the lossless stage dump still preserves.
    #[test]
    fn final_checkpoint_is_byte_identical_to_render_output() {
        let image = LinearImage::new(
            3,
            2,
            vec![
                [0.10, 0.30, 1.20],
                [2.00, 0.20, 0.10],
                [0.40, 0.60, 0.20],
                [-0.10, 0.40, 0.90],
                [0.80, 1.10, 2.50],
                [MID_GRAY; 3],
            ],
        )
        .unwrap();
        let params = ToneParams {
            highlight_desaturation: 0.16,
            saturation: 1.22,
            ..parameters_protected()
        };
        let uncertainty = vec![0.0, 0.25, 0.5, 0.75, 1.0, 0.0];
        let checkpoints = render_checkpoints(&image, &params, Some(&uncertainty), None);
        let rendered = render(&image, &params, None, Some(&uncertainty), None);
        let gamut_post: Vec<f32> = checkpoints
            .iter()
            .flat_map(|stage| stage.gamut_post)
            .collect();
        let checkpoint_output = encode_srgb_u16(&gamut_post);

        assert_eq!(rendered.as_raw(), checkpoint_output.as_slice());
    }

    /// The per-pixel reconstruction-uncertainty map, not a display-brightness
    /// proxy, drives the chroma gate, and `u = 0` is byte-identical to passing no
    /// map at all.
    ///
    /// What the gate does is withhold the preset's saturation *boost*: at full
    /// uncertainty the pixel renders as it would with no saturation opinion, and
    /// no further. It does not cut chroma below that, and — the property the
    /// `0.85` suppression factor broke — it does not change how bright the pixel
    /// is. Collapsing chroma toward the mapped luminance darkens a blown pixel,
    /// which is how the most-clipped pixels in a frame came out darker than the
    /// less-clipped ones around them.
    #[test]
    fn reconstruction_uncertainty_withholds_the_chroma_boost_without_darkening() {
        let mut params = parameters();
        params.saturation = 1.22; // the auto preset's boost, which amplified the cast
        // A bright pixel with green trailing red/blue — the white-balance spread a
        // blown highlight leaves behind, i.e. magenta.
        let magenta = [0.60_f32, 0.35, 0.60];
        let image = LinearImage::new(1, 1, vec![magenta]).unwrap();

        let trusted = render(&image, &params, None, Some(&[0.0]), None).into_raw();
        let no_map = render(&image, &params, None, None, None).into_raw();
        let synthesised = render(&image, &params, None, Some(&[1.0]), None).into_raw();

        assert_eq!(
            trusted, no_map,
            "u = 0 must be byte-identical to passing no uncertainty map"
        );

        // The boost withheld is exactly the preset's saturation opinion.
        let mut unopinionated = params.clone();
        unopinionated.saturation = 1.0;
        let expected = render(&image, &unopinionated, None, Some(&[0.0]), None).into_raw();
        assert_eq!(
            synthesised, expected,
            "at full uncertainty the pixel must render as it would with no \
             saturation boost, got {synthesised:?} against {expected:?}"
        );

        let chroma = |p: &[u16]| {
            (p[0] as i32 - p[1] as i32)
                .abs()
                .max((p[2] as i32 - p[1] as i32).abs())
        };
        assert!(chroma(&trusted) > 0, "a trusted pixel keeps its colour");
        assert!(
            chroma(&synthesised) < chroma(&trusted),
            "the boost must be withheld: kept {}, gated {}",
            chroma(&trusted),
            chroma(&synthesised)
        );

        // Rec. 709 luminance in u16 counts: the gate must not move it.
        let luma = |p: &[u16]| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
        assert!(
            luma(&synthesised) >= luma(&trusted) - 0.01 * luma(&trusted),
            "gating darkened the pixel: {} against {}",
            luma(&synthesised),
            luma(&trusted)
        );
    }

    /// Highlight desaturation follows a max-preserving path to white.  At full
    /// strength it may only lift the lower channels; contracting around
    /// luminance would lower blue here and recreate the dark rim this control
    /// exists to avoid.
    #[test]
    fn highlight_desaturation_lifts_to_white_without_lowering_the_peak() {
        let sky = [0.60, 1.10, 4.40];
        let coloured = render_pixel(sky, &parameters_protected());
        let white = render_pixel(
            sky,
            &ToneParams {
                highlight_desaturation: 1.0,
                ..parameters_protected()
            },
        );

        assert_eq!(white[0], white[1]);
        assert_eq!(white[1], white[2]);
        assert!(
            white
                .iter()
                .zip(coloured)
                .all(|(after, before)| *after >= before),
            "path to white lowered a channel: {coloured:?} -> {white:?}"
        );
        assert_eq!(
            white[2], coloured[2],
            "the brightest channel must stay fixed"
        );
    }

    /// The table is an approximation of the exact transcendental curve; this
    /// pins how good it has to be. Swept densely (16 probes per table segment)
    /// across the whole active EV range and beyond it, for both the plain and
    /// the highlight-protected presets. The mapped-norm tolerance of one part
    /// in 10^4 is two orders of magnitude below one 8-bit output step at
    /// middle grey.
    #[test]
    fn lut_matches_the_exact_curve() {
        for params in [parameters(), parameters_protected()] {
            let lut = ToneLut::new(&params);
            let ramp_end = (params.white_input_ev * HIGHLIGHT_NORM_RAMP).max(1.0e-4);
            let highlight_norm = params.highlight_norm.clamp(0.0, 1.0);
            let white_output = params.white_output_ev.max(1.0e-4);

            let steps = 4096;
            for step in 0..=steps {
                let ev = -40.0 + 60.0 * (step as f32 / steps as f32);
                let value = MID_GRAY * ev.exp2();

                let output_ev = map_ev(ev, &params);
                let exact_norm = (MID_GRAY * output_ev.exp2())
                    .clamp(params.black_output_linear, params.white_output_linear);
                let exact_highlight = (output_ev / white_output).clamp(0.0, 1.0);
                let exact_weight = smoothstep(ev / ramp_end) * highlight_norm;

                let (mapped_norm, normalized_highlight) = lut.curve(value);
                let weight = lut.weight(value);

                // Relative near and above middle grey, absolute (1e-6 linear,
                // well under one 16-bit output step) deep in the shadows where
                // the curvature at the black knee dominates a tiny value.
                assert!(
                    (mapped_norm - exact_norm).abs() <= 1.0e-4 * exact_norm.max(0.01),
                    "ev {ev}: mapped_norm {mapped_norm} vs exact {exact_norm}"
                );
                assert!(
                    (normalized_highlight - exact_highlight).abs() <= 1.0e-3,
                    "ev {ev}: highlight {normalized_highlight} vs exact {exact_highlight}"
                );
                assert!(
                    (weight - exact_weight).abs() <= 1.0e-3,
                    "ev {ev}: weight {weight} vs exact {exact_weight}"
                );
            }
        }
    }

    /// Below middle grey the blend weight must be *exactly* zero — the table
    /// entries there are exact zeros and interpolating between zeros is zero —
    /// so shadows and midtones use the pure luminance norm, as they always
    /// did. The sweep stops 2% below middle grey: the one table segment that
    /// straddles it interpolates toward the first nonzero entry, so exactness
    /// holds everywhere except within one segment (~1.1%) of the boundary.
    #[test]
    fn lut_weight_is_exactly_zero_below_middle_gray() {
        let lut = ToneLut::new(&parameters_protected());
        for step in 0..=256 {
            let value = 0.98 * MID_GRAY * (step as f32 / 256.0);
            assert_eq!(lut.weight(value.max(1.0e-8)), 0.0, "value {value}");
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
