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
//! 2. If some but not all channels are clipped, take the brightest unclipped
//!    channel in white-balanced space as the neutral anchor — the most reliable
//!    lower bound on how bright the highlight really was.
//! 3. Raise each clipped channel's white-balanced value to at least that anchor,
//!    never lowering it. Converting back through the channel's own white-balance
//!    coefficient gives the reconstructed camera value.
//!
//! This lifts a clipped channel only when it fell below the brightest surviving
//! channel — precisely the false-colour case — and leaves genuinely coloured
//! highlights, where the clipped channel is already the brightest, untouched. It
//! cannot darken a pixel, and at full strength it pulls a partially-blown
//! highlight toward neutral rather than to a guessed hue, which is the
//! conservative direction for an unattended archiver.
//!
//! `strength` in `(0, 1]` blends between the original and reconstructed camera
//! value. `strength == 0` is not represented — the caller skips this module — so
//! the owned path stays byte-identical when the feature is off.
//!
//! # Status
//!
//! Off by default. `docs/PLAN.md` names the deliberately-clipped corpus class as
//! the test set for this operator, and that corpus is not gathered yet. The
//! operator is complete and deterministic; the default stays off until there are
//! frames to prove it never over-reconstructs. Its interaction with
//! `tone::compress_gamut` and the `highlight_norm` roll-off is the specific thing
//! grading will have to check, since both act on the same highlight range.

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
    /// Pixels with every channel clipped — fully blown, nothing to rebuild from.
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
                if clipped_count == 3 {
                    tally.fully_clipped += 1;
                    continue;
                }

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
                if !anchor.is_finite() {
                    continue;
                }

                let mut lifted = false;
                for channel in 0..3 {
                    if !clipped[channel] || white_balance[channel] <= 0.0 {
                        continue;
                    }
                    let original_wb = pixel[channel] * white_balance[channel];
                    if anchor > original_wb {
                        let target_wb = original_wb + (anchor - original_wb) * strength;
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
        total.fully_clipped += tally.fully_clipped;
        total.max_lift = total.max_lift.max(tally.max_lift);
    }

    HighlightReport {
        strength,
        clipped_pixels: total.clipped as usize,
        reconstructed_pixels: total.reconstructed as usize,
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

    /// A fully-blown pixel has nothing to rebuild from.
    #[test]
    fn a_fully_clipped_pixel_is_left_alone() {
        let mut img = image(vec![[1.0, 1.0, 1.0]]);
        let before = img.pixels[0];
        let report = reconstruct(&mut img, [2.0, 1.0, 1.6], 1.0);
        assert_eq!(report.fully_clipped_pixels, 1);
        assert_eq!(report.reconstructed_pixels, 0);
        assert_eq!(img.pixels[0], before);
    }

    /// Partial strength lands between the original and the full reconstruction.
    #[test]
    fn partial_strength_lands_between_original_and_full() {
        let mut img = image(vec![[0.60, 0.99, 0.50]]);
        reconstruct(&mut img, [2.0, 1.0, 1.6], 0.5);
        assert!((img.pixels[0][1] - 1.095).abs() < 1e-5);
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
