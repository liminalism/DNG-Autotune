//! Clipped-highlight reconstruction, in white-balanced camera space.
//!
//! # Why this is only now possible
//!
//! For the whole life of this project this could not work, because Rawler's
//! `Calibrate` ran `clip_euclidean_norm_avg` on every pixel before this crate
//! saw it, destroying the very channel relationships a reconstruction needs (see
//! [`crate::color`]). Since 0.1.18 the colour conversion is ours and nothing is
//! clipped, so the surviving channels of a partially-blown highlight reach this
//! stage intact. `docs/PLAN.md` item 2: "Rebuilding a clipped channel from the
//! surviving ones beats the current compress-only behaviour on skies."
//!
//! # The failure it fixes
//!
//! A sensor saturates each channel at the same raw level, but as-shot white
//! balance multiplies the channels apart — on an A7C, red by ~2.3 and blue by
//! ~1.6 against green at 1.0. So in a bright, near-neutral highlight the channel
//! that reaches the sensor's clip first in white-balanced terms stops climbing
//! while the others keep going, and the recorded colour drifts away from the
//! neutral it should be: a cloud edge or a sky near the sun comes out tinted.
//! The camera's own JPEG reconstructs this; a compress-only roll-off cannot,
//! because the information it needs — which channels clipped — is not in their
//! magnitudes.
//!
//! # The method
//!
//! Reconstruction runs on demosaiced camera RGB, before the white balance and
//! colour matrix are applied, because "was this channel at the sensor's clip
//! point?" is a question about the raw sample, not about the matrixed result.
//! For each pixel:
//!
//! 1. A channel is clipped if its normalized camera value is at or above
//!    [`CLIP_THRESHOLD`] — just under 1.0, the recorded white level, with a
//!    little slack because the demosaic averages a saturated site with its
//!    neighbours and can land a hair below full scale.
//! 2. The anchor depends on *how many* channels clipped, because that is what
//!    says whether the pixel is a coloured highlight or a near-white one:
//!    - **One channel clipped** — a genuinely coloured highlight. The two
//!      survivors are exact, so the anchor is the brighter of them in
//!      white-balanced space: the most reliable lower bound on how bright the
//!      highlight really was, and a hue assumption, so `strength` applies.
//!    - **Two or three channels clipped** — a near-white highlight; see
//!      [`NEAR_WHITE_CLIPPED_CHANNELS`]. The anchor is the largest
//!      white-balanced value over *all* channels, clipped ones included, and it
//!      is applied at full strength.
//! 3. Raise each clipped channel's white-balanced value to at least that anchor,
//!    never lowering it. Converting back through the channel's own white-balance
//!    coefficient gives the reconstructed camera value.
//!
//! This lifts a clipped channel only when it fell below the anchor — precisely
//! the false-colour case — and leaves a genuinely coloured highlight with a
//! surviving channel, where the clipped channel is already the brightest,
//! untouched. It cannot darken a pixel, and it pulls a blown highlight toward
//! neutral rather than to a guessed hue, which is the conservative direction for
//! an unattended archiver.
//!
//! # Why the near-white case takes all of the anchor and all of the strength
//!
//! Two clipped channels carry no information about their ratio to each other:
//! both stopped at the same raw level, and after white balance that shared level
//! becomes *different* numbers, one per coefficient. Anchoring only on the lone
//! survivor leaves that spread standing, because a clipped channel whose own
//! white-balanced value already exceeds the survivor is never touched — so the
//! two clipped channels keep the coefficient ratio between them and the pixel
//! renders as the white balance's own colour rather than as white.
//!
//! On the A7C (red ~2.2x, blue ~1.8x, green 1.0x) that is a lavender-magenta
//! sky, and it was the dominant defect on the tropical-daylight batch: on
//! `_DSC1283` 12.4% of pixels have two or three channels at the sensor clip, and
//! in that cohort green rendered at 0.63 of the brightest channel. Taking the
//! anchor over all channels equalizes the clipped ones onto the same
//! white-balanced level and removes the deficit entirely (measured 0.0%), while
//! the one-clipped cohort renders identically to before.
//!
//! Full strength is not a separate opinion, it is the same argument: `strength`
//! exists to be cautious about *inventing* a hue, which is what the one-clipped
//! case does. Here the determination is that the pixel is achromatic, and a
//! partial blend toward achromatic is by construction a fraction of the cast —
//! at 0.75 it leaves a quarter of the white-balance spread, which is exactly the
//! artefact this module exists to remove. Blending an anchor that is itself the
//! answer only reintroduces the bug at reduced amplitude.
//!
//! `strength` in `(0, 1]` therefore governs the one-clipped case. `strength == 0`
//! is not represented — the caller skips this module — so the owned path stays
//! byte-identical when the feature is off.
//!
//! # Status
//!
//! On by default (`--highlight-reconstruction 0.75`) as part of the
//! `archive-auto-v2` profile. Its interaction with `tone::compress_gamut` and
//! the `highlight_norm` roll-off is the specific thing grading has to check,
//! since both act on the same highlight range — `compress_gamut` is hue-
//! preserving, so a colour cast left in by this module survives it unchanged.
//! That is why the cast has to die here and cannot be cleaned up downstream.

use crate::types::{CameraRgb, Image};
use rayon::prelude::*;
use serde::Serialize;

/// Normalized camera value at or above which a channel is treated as clipped.
///
/// The white level normalizes to 1.0, so a saturated photosite sits at 1.0
/// exactly; the small slack below it absorbs the demosaic's averaging of a
/// clipped site with unclipped neighbours, which can pull the interpolated value
/// a little under full scale. Too low and unblown highlights get reconstructed;
/// this value is deliberately close to 1.0.
const CLIP_THRESHOLD: f32 = 0.98;

/// Clipped-channel count at or above which a pixel is treated as near-white
/// rather than as a coloured highlight.
///
/// Two is the threshold because two clipped channels are already enough to
/// destroy the pixel's hue: whatever ratio they had is gone, and only the shared
/// raw clip level is left, which white balance then spreads apart. One clipped
/// channel still leaves two exact survivors defining a real colour, so that case
/// keeps the survivor anchor and the `strength` blend.
const NEAR_WHITE_CLIPPED_CHANNELS: usize = 2;

/// What reconstruction did to one frame.
#[derive(Debug, Clone, Serialize)]
pub struct HighlightReport {
    /// Strength requested, in `(0, 1]`.
    pub strength: f32,
    /// Pixels with at least one clipped channel.
    pub clipped_pixels: usize,
    /// Pixels that were partially clipped and therefore reconstructable
    /// (some channels clipped, at least one not).
    pub reconstructed_pixels: usize,
    /// Pixels with at least [`NEAR_WHITE_CLIPPED_CHANNELS`] channels clipped —
    /// the near-white cohort, anchored across all channels at full strength.
    ///
    /// This is the number to watch when grading skies: it is the fraction of the
    /// frame whose hue is a decision of this module rather than a measurement,
    /// and it counts the pixels that used to carry the white-balance cast.
    pub near_white_pixels: usize,
    /// Pixels with every channel clipped — fully blown, nothing to rebuild from.
    /// A subset of `near_white_pixels`.
    pub fully_clipped_pixels: usize,
    /// Largest single-channel lift applied, in white-balanced normalized units.
    pub max_lift: f32,
}

/// Reconstruct clipped highlights in place on a camera-RGB image.
///
/// `white_balance` is the as-shot coefficient per camera channel, in the file's
/// own order; a non-positive coefficient (an unfilled channel) disables the lift
/// for that channel, since the round-trip through it is undefined.
pub fn reconstruct(
    image: &mut Image<CameraRgb>,
    white_balance: [f32; 3],
    strength: f32,
) -> HighlightReport {
    let strength = strength.clamp(0.0, 1.0);

    #[derive(Default, Clone, Copy)]
    struct Tally {
        clipped: u64,
        reconstructed: u64,
        near_white: u64,
        fully_clipped: u64,
        max_lift: f32,
    }

    const CHUNK: usize = 65_536;
    let tallies: Vec<Tally> = image
        .pixels
        .par_chunks_mut(CHUNK)
        .map(|chunk| {
            let mut tally = Tally::default();
            for pixel in chunk {
                let clipped = [
                    pixel[0] >= CLIP_THRESHOLD,
                    pixel[1] >= CLIP_THRESHOLD,
                    pixel[2] >= CLIP_THRESHOLD,
                ];
                let clipped_count = clipped.iter().filter(|c| **c).count();
                if clipped_count == 0 {
                    continue;
                }
                tally.clipped += 1;

                // Brightest surviving channel, in white-balanced space: the most
                // reliable lower bound on the highlight's true neutral level.
                let mut anchor = f32::NEG_INFINITY;
                for channel in 0..3 {
                    if !clipped[channel] {
                        let wb = pixel[channel] * white_balance[channel];
                        if wb > anchor {
                            anchor = wb;
                        }
                    }
                }

                let near_white = clipped_count >= NEAR_WHITE_CLIPPED_CHANNELS;
                if near_white {
                    tally.near_white += 1;
                    if clipped_count == 3 {
                        tally.fully_clipped += 1;
                    }
                    // The clipped channels all stopped at the same raw level, so
                    // their ratio to each other is not a measurement — it is the
                    // white-balance coefficients, and leaving it standing renders
                    // the sky as the white balance's own colour (lavender-magenta
                    // on the A7C). Widening the anchor over every channel, not
                    // just the survivors, puts them all on one white-balanced
                    // level: the pixel comes out as bright as its brightest
                    // evidence and as neutral as the evidence allows.
                    //
                    // A fully-clipped pixel is the degenerate case of the same
                    // rule — no survivors at all, so the anchor is simply the
                    // largest coefficient, which is exactly the one that renders
                    // a truly neutral target as neutral.
                    for channel in 0..3 {
                        if white_balance[channel] <= 0.0 {
                            continue;
                        }
                        let wb = pixel[channel] * white_balance[channel];
                        if wb > anchor {
                            anchor = wb;
                        }
                    }
                }

                if !anchor.is_finite() {
                    continue;
                }

                // Near-white pixels take the whole lift. `strength` is caution
                // about inventing a hue, and there is no hue here to invent; a
                // partial blend toward neutral is just a fraction of the cast.
                // See the module docs.
                let effective_strength = if near_white { 1.0 } else { strength };

                let mut lifted = false;
                for channel in 0..3 {
                    if !clipped[channel] || white_balance[channel] <= 0.0 {
                        continue;
                    }
                    let original_wb = pixel[channel] * white_balance[channel];
                    if anchor > original_wb {
                        let target_wb = original_wb + (anchor - original_wb) * effective_strength;
                        let lift = target_wb - original_wb;
                        if lift > tally.max_lift {
                            tally.max_lift = lift;
                        }
                        pixel[channel] = target_wb / white_balance[channel];
                        lifted = true;
                    }
                }
                if lifted {
                    tally.reconstructed += 1;
                }
            }
            tally
        })
        .collect();

    let mut total = Tally::default();
    for tally in tallies {
        total.clipped += tally.clipped;
        total.reconstructed += tally.reconstructed;
        total.near_white += tally.near_white;
        total.fully_clipped += tally.fully_clipped;
        total.max_lift = total.max_lift.max(tally.max_lift);
    }

    HighlightReport {
        strength,
        clipped_pixels: total.clipped as usize,
        reconstructed_pixels: total.reconstructed as usize,
        near_white_pixels: total.near_white as usize,
        fully_clipped_pixels: total.fully_clipped as usize,
        max_lift: total.max_lift,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(pixels: Vec<[f32; 3]>) -> Image<CameraRgb> {
        let len = pixels.len();
        Image::new(len, 1, pixels).expect("valid image")
    }

    /// The canonical false-colour case: green clipped low against a bright,
    /// surviving neutral. It must be lifted toward the anchor at full strength.
    #[test]
    fn a_channel_clipped_below_the_survivors_is_lifted() {
        let mut img = image(vec![[0.60, 0.99, 0.50]]);
        let report = reconstruct(&mut img, [2.0, 1.0, 1.6], 1.0);
        assert_eq!(report.reconstructed_pixels, 1);
        assert!((img.pixels[0][1] - 1.20).abs() < 1e-5);
        assert!((img.pixels[0][0] - 0.60).abs() < 1e-6);
        assert!((img.pixels[0][2] - 0.50).abs() < 1e-6);
    }

    /// A clipped channel that is already the brightest (a genuinely coloured
    /// highlight) must not be touched.
    #[test]
    fn a_coloured_highlight_is_left_alone() {
        let mut img = image(vec![[0.99, 0.30, 0.20]]);
        let before = img.pixels[0];
        let report = reconstruct(&mut img, [2.0, 1.0, 1.6], 1.0);
        assert_eq!(report.clipped_pixels, 1);
        assert_eq!(report.reconstructed_pixels, 0);
        assert_eq!(img.pixels[0], before);
    }

    /// A fully-blown pixel has no surviving channel to anchor on, but its raw
    /// channels are all near the same clip point, so it must still be pulled
    /// toward neutral using the clipped channel with the largest white-balance
    /// coefficient as the anchor — otherwise the raw white-balance spread comes
    /// out as a colour cast (magenta on the A7C, where this pixel is a stand-in
    /// for a sun glint or lens-flare core with red at 2.0x and blue at 1.6x
    /// against green's 1.0x).
    #[test]
    fn a_fully_clipped_pixel_is_pulled_toward_the_largest_white_balance_channel() {
        let mut img = image(vec![[1.0, 1.0, 1.0]]);
        let report = reconstruct(&mut img, [2.0, 1.0, 1.6], 1.0);
        assert_eq!(report.fully_clipped_pixels, 1);
        assert_eq!(report.reconstructed_pixels, 1);
        // Red has the largest white-balance coefficient (2.0), so it anchors
        // the highlight and is itself left untouched; green and blue are
        // raised to the same white-balanced level (2.0) and converted back
        // through their own coefficients.
        assert!((img.pixels[0][0] - 1.0).abs() < 1e-6);
        assert!((img.pixels[0][1] - 2.0).abs() < 1e-5);
        assert!((img.pixels[0][2] - 1.25).abs() < 1e-5);
    }

    /// A fully-clipped pixel whose channels are already balanced (equal
    /// white-balance coefficients) has nothing to correct: the anchor equals
    /// every channel's own value, so no lift is applied.
    #[test]
    fn a_fully_clipped_already_balanced_pixel_is_left_alone() {
        let mut img = image(vec![[1.0, 1.0, 1.0]]);
        let before = img.pixels[0];
        let report = reconstruct(&mut img, [1.0, 1.0, 1.0], 1.0);
        assert_eq!(report.fully_clipped_pixels, 1);
        assert_eq!(report.reconstructed_pixels, 0);
        assert_eq!(img.pixels[0], before);
    }

    /// Partial strength lands between the original and the full reconstruction.
    ///
    /// One clipped channel only — the case `strength` still governs.
    #[test]
    fn partial_strength_lands_between_original_and_full() {
        let mut img = image(vec![[0.60, 0.99, 0.50]]);
        reconstruct(&mut img, [2.0, 1.0, 1.6], 0.5);
        assert!((img.pixels[0][1] - 1.095).abs() < 1e-5);
    }

    /// The A7C daylight coefficients that produced the lavender skies. Red and
    /// blue both sit well above green, which is what makes the residual spread
    /// read as magenta rather than as a mild warmth.
    const A7C_DAYLIGHT_WB: [f32; 3] = [2.219, 1.0, 1.773];

    /// The defect this rule exists for: a bright sky saturates green and blue at
    /// the sensor while red survives. Both clipped channels stopped at the same
    /// raw level, so they must come out on the same white-balanced level — any
    /// gap between them is the white-balance spread showing through as a cast.
    #[test]
    fn two_clipped_channels_land_on_one_white_balanced_level() {
        let mut img = image(vec![[0.75, 1.0, 1.0]]);
        let report = reconstruct(&mut img, A7C_DAYLIGHT_WB, 0.75);

        assert_eq!(report.near_white_pixels, 1);
        assert_eq!(report.fully_clipped_pixels, 0);

        let green = img.pixels[0][1] * A7C_DAYLIGHT_WB[1];
        let blue = img.pixels[0][2] * A7C_DAYLIGHT_WB[2];
        assert!(
            (green - blue).abs() < 1e-5,
            "the clipped channels must be equalized, got green {green} vs blue {blue}"
        );
        // The surviving red channel is a measurement, so it is left exactly
        // where it was.
        assert!((img.pixels[0][0] - 0.75).abs() < 1e-6);
    }

    /// The near-white lift does not listen to `strength`. Before this rule the
    /// default 0.75 left a quarter of the white-balance spread standing, which
    /// is precisely the cast; a knob that can dial the bug back in is not a
    /// knob worth having here.
    #[test]
    fn near_white_reconstruction_is_independent_of_strength() {
        let full = {
            let mut img = image(vec![[0.75, 1.0, 1.0]]);
            reconstruct(&mut img, A7C_DAYLIGHT_WB, 1.0);
            img.pixels[0]
        };
        for strength in [0.05, 0.25, 0.5, 0.75] {
            let mut img = image(vec![[0.75, 1.0, 1.0]]);
            reconstruct(&mut img, A7C_DAYLIGHT_WB, strength);
            assert_eq!(
                img.pixels[0], full,
                "strength {strength} changed a near-white pixel"
            );
        }
    }

    /// Widening the anchor must never *lower* it: when the lone survivor is
    /// brighter than either clipped channel's white-balanced level, it still
    /// sets the level, because it is the only exact measurement in the pixel.
    #[test]
    fn a_bright_survivor_still_sets_the_near_white_anchor() {
        let mut img = image(vec![[0.95, 1.0, 1.0]]);
        reconstruct(&mut img, A7C_DAYLIGHT_WB, 0.75);
        let survivor = 0.95 * A7C_DAYLIGHT_WB[0];
        for channel in [1, 2] {
            let value = img.pixels[0][channel] * A7C_DAYLIGHT_WB[channel];
            assert!(
                (value - survivor).abs() < 1e-4,
                "channel {channel} landed at {value}, expected the survivor level {survivor}"
            );
        }
    }

    /// The whole point of the two-channel threshold: a single clipped channel is
    /// still a colour, and must render exactly as it did before this rule
    /// existed. This is the guarantee that the fix is confined to near-white
    /// pixels and cannot desaturate a red flower or a green specular.
    #[test]
    fn one_clipped_channel_keeps_the_survivor_anchor_and_the_strength_blend() {
        for source in [[0.99, 0.30, 0.20], [0.60, 0.99, 0.50], [0.20, 0.35, 0.99]] {
            for strength in [0.25, 0.75, 1.0] {
                let mut img = image(vec![source]);
                let report = reconstruct(&mut img, A7C_DAYLIGHT_WB, strength);
                assert_eq!(report.near_white_pixels, 0, "{source:?} is not near-white");

                // Reproduce the survivor-anchored blend independently.
                let clipped = [
                    source[0] >= CLIP_THRESHOLD,
                    source[1] >= CLIP_THRESHOLD,
                    source[2] >= CLIP_THRESHOLD,
                ];
                let anchor = (0..3)
                    .filter(|channel| !clipped[*channel])
                    .map(|channel| source[channel] * A7C_DAYLIGHT_WB[channel])
                    .fold(f32::NEG_INFINITY, f32::max);
                let mut expected = source;
                for channel in 0..3 {
                    if !clipped[channel] {
                        continue;
                    }
                    let original = source[channel] * A7C_DAYLIGHT_WB[channel];
                    if anchor > original {
                        expected[channel] = (original + (anchor - original) * strength)
                            / A7C_DAYLIGHT_WB[channel];
                    }
                }
                assert_eq!(
                    img.pixels[0], expected,
                    "{source:?} at strength {strength} took the near-white path"
                );
            }
        }
    }

    /// Reconstruction may only ever brighten. A rule that widens the anchor is
    /// the kind that could start pulling a channel down by accident, so this is
    /// checked across the whole clip-count range, including the fully-clipped
    /// and no-clip ends.
    #[test]
    fn reconstruction_never_darkens_a_channel() {
        let sources = [
            [0.75, 1.0, 1.0],
            [0.95, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.40, 1.0],
            [0.99, 0.30, 0.20],
            [0.10, 0.20, 0.30],
            [0.0, 1.0, 1.0],
        ];
        for source in sources {
            for strength in [0.25, 0.75, 1.0] {
                let mut img = image(vec![source]);
                reconstruct(&mut img, A7C_DAYLIGHT_WB, strength);
                for channel in 0..3 {
                    assert!(
                        img.pixels[0][channel] >= source[channel] - 1e-6,
                        "{source:?} channel {channel} darkened to {} at strength {strength}",
                        img.pixels[0][channel]
                    );
                }
            }
        }
    }

    /// A channel whose white-balance coefficient is missing has no defined
    /// round trip, so it must be left alone even when the pixel is near-white
    /// and the other channels are being equalized around it.
    #[test]
    fn a_missing_white_balance_coefficient_disables_only_its_own_channel() {
        let mut img = image(vec![[0.75, 1.0, 1.0]]);
        let report = reconstruct(&mut img, [2.219, 0.0, 1.773], 0.75);
        assert_eq!(report.near_white_pixels, 1);
        assert_eq!(img.pixels[0][1], 1.0, "the unfilled channel must not move");
    }

    #[test]
    fn no_clipping_leaves_the_image_untouched() {
        let mut img = image(vec![[0.10, 0.20, 0.30], [0.40, 0.50, 0.60]]);
        let before = img.pixels.clone();
        let report = reconstruct(&mut img, [2.0, 1.0, 1.6], 1.0);
        assert_eq!(report.clipped_pixels, 0);
        assert_eq!(img.pixels, before);
    }
}
