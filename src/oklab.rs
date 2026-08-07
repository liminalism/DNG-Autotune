//! Oklab, for the one question the existing metrics cannot answer.
//!
//! # Why this exists
//!
//! `docs/REVIEW-2026-07-30.md`'s A/B established that the paired scorecard is
//! *structurally* unable to reward the owned colour path. Its four axes are
//! highlights kept, shadows kept, tonal detail and local detail — and the thing
//! the owned path exists for, keeping an out-of-gamut colour's *hue* instead of
//! letting a clip shift it, is invisible to all four. `crushed_fraction` can only
//! punish the owned path (Rawler's output has no negatives to crush), so the gate
//! favoured the default for reasons that had nothing to do with image quality.
//!
//! Measuring a hue shift needs a space where "same hue, different lightness or
//! chroma" is a straight line and where equal angular distances mean roughly
//! equal perceived hue differences. sRGB is neither. CIELAB is closer but has a
//! well-known blue-hue nonlinearity — its hue angle bends for exactly the
//! saturated blues that dominate the out-of-gamut population here (skies, and the
//! blue channel a Sony A7C's white point multiplies by about 1.6). Oklab was
//! fitted to fix that specific defect, which makes it the right choice rather
//! than merely the fashionable one.
//!
//! # What this is not
//!
//! Not a working space, and not on the render path. Nothing here converts pixels
//! that get written to a file; `crate::color` owns the render's colour maths.
//! This module exists so `crate::metrics` can *describe* a rendering, and it is
//! deliberately kept separate so that a change to how we measure colour can never
//! silently change how we render it.

/// Oklab coordinates: perceptual lightness plus two opponent axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklab {
    /// Perceptual lightness, ~0 (black) to ~1 (diffuse white).
    pub l: f32,
    /// Green-red opponent axis.
    pub a: f32,
    /// Blue-yellow opponent axis.
    pub b: f32,
}

impl Oklab {
    /// Hue angle in radians on `(-pi, pi]`.
    ///
    /// Undefined for a neutral colour, where `a` and `b` are both zero and the
    /// angle is arbitrary. Callers comparing hues must gate on [`Self::chroma`]
    /// first — see [`hue_difference`], which does.
    #[inline]
    pub fn hue(self) -> f32 {
        self.b.atan2(self.a)
    }

    /// Distance from the neutral axis. Zero for any grey.
    #[inline]
    pub fn chroma(self) -> f32 {
        self.a.hypot(self.b)
    }
}

/// Convert linear-light sRGB to Oklab.
///
/// Björn Ottosson's published matrices, which factor the transform as
/// `linear sRGB -> a cone-response space -> cube root -> a final rotation`. The
/// cube root is what makes the space perceptual, and it is defined for negative
/// input — which matters here, because the whole point is measuring colours that
/// fall outside the display gamut and therefore have negative channels. Rust's
/// `cbrt` is signed, so no clamping is needed and none is done: clamping would
/// destroy exactly the signal this module was added to see.
#[inline]
pub fn from_linear_srgb(rgb: [f32; 3]) -> Oklab {
    let [r, g, b] = rgb;

    let long = 0.412_221_47 * r + 0.536_332_5 * g + 0.051_445_995 * b;
    let medium = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let short = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_5 * b;

    let long = long.cbrt();
    let medium = medium.cbrt();
    let short = short.cbrt();

    Oklab {
        l: 0.210_454_26 * long + 0.793_617_8 * medium - 0.004_072_047 * short,
        a: 1.977_998_5 * long - 2.428_592_2 * medium + 0.450_593_7 * short,
        b: 0.025_904_037 * long + 0.782_771_77 * medium - 0.808_675_77 * short,
    }
}

/// Convert Oklab to linear-light sRGB.
///
/// Inverse of [`from_linear_srgb`].
#[inline]
pub fn to_linear_srgb(lab: Oklab) -> [f32; 3] {
    let l = lab.l;
    let a = lab.a;
    let b = lab.b;
    // Inverse of the final rotation
    // From Ottosson's blog:
    // From Ottosson's blog: 
    // l_ = L + 0.3963377774*a + 0.2158037573*b
    // m_ = L - 0.1055613458*a - 0.0638541728*b
    // s_ = L - 0.0894841775*a - 1.2914855480*b
    // Then cube
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;
    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;
    // Convert LMS to linear sRGB (inverse of forward matrix)
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ]
}

/// Convert sRGB-encoded values on `0..=1` to Oklab.
///
/// The measurement path receives display-encoded pixels, so the transfer function
/// has to come off first. Reuses [`crate::tone::srgb_decode`] rather than a second
/// copy of the curve, so a rendering and a measurement of that rendering can never
/// disagree about what sRGB is.
#[inline]
pub fn from_encoded_srgb(rgb: [f32; 3]) -> Oklab {
    from_linear_srgb([
        crate::tone::srgb_decode(rgb[0]),
        crate::tone::srgb_decode(rgb[1]),
        crate::tone::srgb_decode(rgb[2]),
    ])
}

/// Smallest chroma at which a hue angle is meaningful.
///
/// Below this a colour is a grey whose hue is numerical noise: `atan2` on two
/// values near zero returns whatever the rounding happened to produce, so
/// averaging such angles into a hue statistic would swamp the real signal with
/// arbitrary numbers from the neutral parts of the frame. Oklab chroma runs to
/// about 0.32 for the most saturated sRGB colours, so 0.002 is well under one
/// percent of the range — restrictive enough to exclude greys, permissive enough
/// to keep every colour a viewer would call coloured.
pub const CHROMA_FLOOR: f32 = 0.002;

/// Absolute hue difference in radians on `[0, pi]`, or `None` when either colour
/// is too close to neutral for its hue to mean anything.
///
/// Wrapped, so the difference between hues either side of the +/-pi seam is small
/// rather than nearly a full turn. Getting that wrong would put the largest
/// reported errors on reds, which is where the seam falls.
#[inline]
pub fn hue_difference(a: Oklab, b: Oklab) -> Option<f32> {
    if a.chroma() < CHROMA_FLOOR || b.chroma() < CHROMA_FLOOR {
        return None;
    }
    let difference = (a.hue() - b.hue()).abs();
    Some(if difference > std::f32::consts::PI {
        std::f32::consts::TAU - difference
    } else {
        difference
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ottosson's own reference values: linear-sRGB white is L=1 on the neutral
    /// axis. This is the anchor that catches a transposed or mis-transcribed
    /// matrix, which is the realistic failure mode for hand-entered constants.
    #[test]
    fn white_is_unit_lightness_and_neutral() {
        let white = from_linear_srgb([1.0, 1.0, 1.0]);
        assert!((white.l - 1.0).abs() < 1.0e-3, "L was {}", white.l);
        assert!(
            white.chroma() < 1.0e-3,
            "white had chroma {}",
            white.chroma()
        );
    }

    #[test]
    fn black_is_zero_and_grey_is_neutral() {
        let black = from_linear_srgb([0.0; 3]);
        assert!(black.l.abs() < 1.0e-6);
        assert!(black.chroma() < 1.0e-6);

        for level in [0.05_f32, 0.18, 0.5, 0.9] {
            let grey = from_linear_srgb([level; 3]);
            assert!(
                grey.chroma() < 1.0e-3,
                "grey at {level} had chroma {}",
                grey.chroma()
            );
        }
    }

    /// Lightness must be monotonic, or a "hue at equal lightness" comparison
    /// would be meaningless.
    #[test]
    fn lightness_increases_with_level() {
        let mut previous = f32::NEG_INFINITY;
        for step in 0..=32 {
            let level = step as f32 / 32.0;
            let l = from_linear_srgb([level; 3]).l;
            assert!(l > previous, "L fell at level {level}");
            previous = l;
        }
    }

    /// The property the whole module rests on: a pure change of exposure must not
    /// change hue. If this fails, a hue delta between two renderings at different
    /// brightness would report a shift that is really just a level difference —
    /// exactly the confound `mean_saturation` was introduced to avoid for chroma.
    #[test]
    fn hue_is_invariant_under_exposure() {
        for colour in [
            [0.6_f32, 0.2, 0.1],
            [0.1, 0.5, 0.2],
            [0.05, 0.15, 0.7],
            [0.4, 0.4, 0.05],
        ] {
            let base = from_linear_srgb(colour);
            for gain in [0.25_f32, 0.5, 2.0, 4.0] {
                let scaled = from_linear_srgb(colour.map(|channel| channel * gain));
                let difference = hue_difference(base, scaled).expect("both are chromatic");
                assert!(
                    difference < 1.0e-3,
                    "{colour:?} at gain {gain} shifted hue by {difference} rad"
                );
            }
        }
    }

    /// Negative channels must survive. The owned colour path produces them by
    /// design, and a measurement that clamped them would report every
    /// out-of-gamut colour as sitting exactly on the gamut boundary — which is
    /// precisely the error this module was added to detect in Rawler's clip.
    #[test]
    fn out_of_gamut_input_is_not_clamped_to_the_boundary() {
        let out_of_gamut = [0.05_f32, 0.90, -0.15];
        let clipped = [0.05_f32, 0.90, 0.0];

        let wide = from_linear_srgb(out_of_gamut);
        let clamped = from_linear_srgb(clipped);

        assert!(wide.l.is_finite() && wide.a.is_finite() && wide.b.is_finite());
        let shift = hue_difference(wide, clamped).expect("both are chromatic");
        assert!(
            shift > 0.01,
            "clipping the negative channel should move the hue measurably, got {shift} rad"
        );
        // And it is the *out-of-gamut* colour that is more saturated, so a clip
        // costs chroma as well as hue.
        assert!(wide.chroma() > clamped.chroma());
    }

    /// Hue is an angle, so the difference either side of the +/-pi seam must be
    /// small. Without the wrap the largest errors in the corpus would all be
    /// reported on reds.
    #[test]
    fn hue_difference_wraps_across_the_seam() {
        // Two reds straddling the seam: hue near +pi and near -pi.
        let a = Oklab {
            l: 0.5,
            a: -0.10,
            b: 0.001,
        };
        let b = Oklab {
            l: 0.5,
            a: -0.10,
            b: -0.001,
        };
        assert!(a.hue() > 3.0 && b.hue() < -3.0, "not straddling the seam");
        let difference = hue_difference(a, b).expect("both are chromatic");
        assert!(
            difference < 0.05,
            "the wrap failed: reported {difference} rad for two near-identical reds"
        );
    }

    /// Greys have no hue, and pretending otherwise would fill a hue statistic
    /// with arbitrary angles from the neutral parts of a frame.
    #[test]
    fn neutral_colours_report_no_hue_difference() {
        let grey = from_linear_srgb([0.18; 3]);
        let other_grey = from_linear_srgb([0.42; 3]);
        let red = from_linear_srgb([0.6, 0.1, 0.1]);
        assert!(hue_difference(grey, other_grey).is_none());
        assert!(hue_difference(grey, red).is_none());
        assert!(hue_difference(red, red).is_some());
    }

    /// Opposite hues must be about pi apart, which pins the angle's scale.
    #[test]
    fn opposing_hues_are_half_a_turn_apart() {
        let red = from_linear_srgb([0.5, 0.1, 0.1]);
        let cyan = from_linear_srgb([0.1, 0.5, 0.5]);
        let difference = hue_difference(red, cyan).expect("both are chromatic");
        assert!(
            (difference - std::f32::consts::PI).abs() < 0.5,
            "red to cyan measured {difference} rad"
        );
    }

    /// The encoded entry point must agree with decoding by hand, so the two
    /// cannot drift apart.
    #[test]
    fn the_encoded_entry_point_matches_an_explicit_decode() {
        for encoded in [[0.2_f32, 0.5, 0.8], [0.9, 0.1, 0.3]] {
            let direct = from_encoded_srgb(encoded);
            let manual = from_linear_srgb(encoded.map(crate::tone::srgb_decode));
            assert_eq!(direct, manual);
        }
    }
}
