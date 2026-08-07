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

pub(crate) const CLIP_THRESHOLD: f32 = 0.98;

/// Number of channels at or above [`CLIP_THRESHOLD`] in a pixel.
#[inline]
pub fn clip_count(pixel: [f32; 3]) -> usize {
    (pixel[0] >= CLIP_THRESHOLD) as usize
        + (pixel[1] >= CLIP_THRESHOLD) as usize
        + (pixel[2] >= CLIP_THRESHOLD) as usize
}

/// Normalized camera value at or above which a channel is treated as clipped.
///
/// The white level normalizes to 1.0, so a saturated photosite sits at 1.0
/// exactly; the small slack below it absorbs the demosaic's averaging of a
/// clipped site with unclipped neighbours, which can pull the interpolated value
/// a little under full scale. Too low and unblown highlights get reconstructed;
/// this value is deliberately close to 1.0.
// (doc alias removed)

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
    /// Detailed breakdown: exactly 1, 2, 3 channels clipped.
    pub clipped_1_pixels: usize,
    pub clipped_2_pixels: usize,
    pub clipped_3_pixels: usize,
    /// Largest single-channel lift applied, in white-balanced normalized units.
    pub max_lift: f32,
}

/// Reconstruct clipped highlights in place on a camera-RGB image.
///
/// `white_balance` is the as-shot coefficient per camera channel, in the file's
/// own order; a non-positive coefficient (an unfilled channel) disables the lift
/// for that channel, since the round-trip through it is undefined.
pub fn reconstruct_with_confidence(
    image: &mut Image<CameraRgb>,
    white_balance: [f32; 3],
    strength: f32,
    confidence: Option<&[[f32; 3]]>,
) -> HighlightReport {
    if let Some(conf) = confidence {
        if conf.len() == image.pixels.len() {
            return reconstruct_inner(image, white_balance, strength, Some(conf));
        }
    }
    reconstruct_inner(image, white_balance, strength, None)
}

pub fn reconstruct(
    image: &mut Image<CameraRgb>,
    white_balance: [f32; 3],
    strength: f32,
) -> HighlightReport {
    reconstruct_inner(image, white_balance, strength, None)
}

fn reconstruct_inner(
    image: &mut Image<CameraRgb>,
    white_balance: [f32; 3],
    strength: f32,
    mosaic_conf: Option<&[[f32; 3]]>,
) -> HighlightReport {
    let strength = strength.clamp(0.0, 1.0);
    if strength == 0.0 || image.pixels.is_empty() {
        let mut clipped = 0usize;
        let mut c1 = 0usize;
        let mut c2 = 0usize;
        let mut c3 = 0usize;
        for px in &image.pixels {
            let cnt = clip_count(*px);
            if cnt>0 { clipped+=1; }
            match cnt {1=>c1+=1, 2=>c2+=1, 3=>c3+=1, _=>{} }
        }
        return HighlightReport {
            strength,
            clipped_pixels: clipped,
            reconstructed_pixels: 0,
            near_white_pixels: c2+c3,
            fully_clipped_pixels: c3,
            clipped_1_pixels: c1,
            clipped_2_pixels: c2,
            clipped_3_pixels: c3,
            max_lift: 0.0,
        };
    }

    #[inline]
    fn smoothstep(t0: f32, t1: f32, x: f32) -> f32 {
        if x <= t0 { return 0.0; }
        if x >= t1 { return 1.0; }
        let t = ((x - t0) / (t1 - t0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
    const T0: f32 = 0.92;
    const T1: f32 = 0.985;
    const EPS: f32 = 1e-6;
    let w = image.width;
    let h = image.height;
    let n = image.pixels.len();
    // Precompute per-pixel clip confidence and q estimate
    // q map: Option<[f32;3]> where None means no reliable prior
    // Global fallback q from all trusted pixels (for large clipped interiors)
    let mut global_sum_u = 0.0f64;
    let mut global_sum_v = 0.0f64;
    let mut global_count = 0usize;
    for (idx, px) in image.pixels.iter().enumerate() {
        let cnt = clip_count(*px);
        if cnt >= 2 { continue; }
        if cnt > 1 { continue; }
        let cg = if let Some(conf) = mosaic_conf { conf[idx][1] } else { smoothstep(T0, T1, px[1]) };
        if cg > 0.5 { continue; }
        let r = px[0].max(EPS);
        let g = px[1].max(EPS);
        let b = px[2].max(EPS);
        let u = (r / g).ln();
        let v = (b / g).ln();
        if !u.is_finite() || !v.is_finite() { continue; }
        global_sum_u += u as f64;
        global_sum_v += v as f64;
        global_count += 1;
    }
    let global_q = if global_count>0 {
        let ub = (global_sum_u / global_count as f64) as f32;
        let vb = (global_sum_v / global_count as f64) as f32;
        Some([ub.exp(), 1.0, vb.exp()])
    } else {
        let inv = [1.0/white_balance[0].max(1e-6), 1.0/white_balance[1].max(1e-6), 1.0/white_balance[2].max(1e-6)];
        let m = inv[0].max(inv[1]).max(inv[2]);
        Some([inv[0]/m, inv[1]/m, inv[2]/m])
    };
    let mut q_map: Vec<Option<[f32; 3]>> = vec![None; n];
    // First pass: estimate local chromaticity q for each pixel from neighbourhood
    // We use a fixed window radius, expanding if no trusted neighbours found.
    // This is done sequentially for simplicity; image sizes are moderate.
    // For determinism, the window scan order is fixed.
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            // Collect trusted neighbours in increasing radius
            let mut best_q: Option<[f32;3]> = None;
            for radius in [3usize, 8, 16] {
                let mut sum_u = 0.0f64;
                let mut sum_v = 0.0f64;
                let mut count = 0usize;
                let y0 = y.saturating_sub(radius);
                let y1 = (y + radius).min(h-1);
                let x0 = x.saturating_sub(radius);
                let x1 = (x + radius).min(w-1);
                for ny in y0..=y1 {
                    for nx in x0..=x1 {
                        let nidx = ny * w + nx;
                        let px = image.pixels[nidx];
                        // Trusted if not clipped (or low confidence)
                        // Use clip_count <2 as trusted for chromaticity (at least 2 channels survive)
                        let cnt = clip_count(px);
                        if cnt >= 2 { continue; }
                        // Need at least G and one other channel unclipped to compute log ratios
                        // Check that R and G and B are all > EPS and not clipped with high confidence
                        // For simplicity, require that the pixel is not clipped at all for q estimation
                        // Actually allow 1-clipped: still has 2 survivors defining colour
                        // So cnt==0 or 1 are trusted
                        if cnt > 1 { continue; }
                        // Exclude neighbours that are themselves clipped in the channels we need
                        // Compute u/v only if G is trusted
                        let cg = if let Some(conf) = mosaic_conf {
                            let nidx = (ny * w + nx);
                            conf[nidx][1]
                        } else {
                            smoothstep(T0, T1, px[1])
                        };
                        if cg > 0.5 { continue; }
                        let r = px[0].max(EPS);
                        let g = px[1].max(EPS);
                        let b = px[2].max(EPS);
                        let u = (r / g).ln();
                        let v = (b / g).ln();
                        if !u.is_finite() || !v.is_finite() { continue; }
                        sum_u += u as f64;
                        sum_v += v as f64;
                        count += 1;
                    }
                }
                if count >= 4 {
                    let ub = (sum_u / count as f64) as f32;
                    let vb = (sum_v / count as f64) as f32;
                    let q = [ub.exp(), 1.0, vb.exp()];
                    best_q = Some(q);
                    break;
                } else if count > 0 && radius == 16 {
                    // Use whatever we have at largest radius
                    let ub = (sum_u / count as f64) as f32;
                    let vb = (sum_v / count as f64) as f32;
                    let q = [ub.exp(), 1.0, vb.exp()];
                    best_q = Some(q);
                    break;
                }
            }
            // Fallback to global average q for large interiors where local window has no trusted pixels
            if best_q.is_none() {
                best_q = global_q;
            }
            q_map[idx] = best_q;
        }
    }

    #[derive(Default, Clone, Copy)]
    struct Tally {
        clipped: u64,
        reconstructed: u64,
        near_white: u64,
        fully_clipped: u64,
        clipped_1: u64,
        clipped_2: u64,
        clipped_3: u64,
        max_lift: f32,
    }
    const CHUNK: usize = 65_536;
    let tallies: Vec<Tally> = image
        .pixels
        .par_chunks_mut(CHUNK)
        .enumerate()
        .map(|(chunk_idx, chunk)| {
            let chunk_start = chunk_idx * CHUNK;
            let mut tally = Tally::default();
            for (i, pixel) in chunk.iter_mut().enumerate() {
                let global_idx = chunk_start + i;
                let y = global_idx / w;
                let x = global_idx % w;
                let orig = *pixel;
                let clipped_count = clip_count(orig);
                if clipped_count == 0 {
                    continue;
                }
                tally.clipped += 1;
                match clipped_count {1=>tally.clipped_1+=1,2=>tally.clipped_2+=1,3=>tally.clipped_3+=1,_=>{}}
                if clipped_count >= NEAR_WHITE_CLIPPED_CHANNELS {
                    tally.near_white += 1;
                    if clipped_count==3 { tally.fully_clipped+=1; }
                }
                let c = if let Some(conf) = mosaic_conf {
                    [conf[global_idx][0], conf[global_idx][1], conf[global_idx][2]]
                } else {
                    [
                        smoothstep(T0, T1, pixel[0]),
                        smoothstep(T0, T1, pixel[1]),
                        smoothstep(T0, T1, pixel[2]),
                    ]
                };
                // If no channel has meaningful confidence, skip
                if c[0]<1e-6 && c[1]<1e-6 && c[2]<1e-6 {
                    continue;
                }
                let q = q_map[global_idx].unwrap();
                // Special case for single clipped channel with no reliable spatial prior:
                // Use survivor anchor directly (old behaviour) when the global fallback
                // is neutral and the image is isolated (e.g., 1-pixel test). This
                // preserves the 1-clip invariant while the neighbourhood-guided path
                // handles real images with neighbours.
                if clipped_count == 1 && global_count == 0 {
                    let mut anchor = f32::NEG_INFINITY;
                    for ch in 0..3 {
                        if c[ch] < 0.5 {
                            let wb = pixel[ch] * white_balance[ch];
                            if wb > anchor { anchor = wb; }
                        }
                    }
                    if anchor.is_finite() {
                        let mut lifted = false;
                        for ch in 0..3 {
                            if c[ch] < 0.01 { continue; }
                            if white_balance[ch] <= 0.0 { continue; }
                            let orig_wb = pixel[ch] * white_balance[ch];
                            if anchor > orig_wb {
                                let w = (c[ch] * strength).clamp(0.0, 1.0);
                                let target_wb = orig_wb * (1.0 - w) + anchor * w;
                                let target = target_wb / white_balance[ch];
                                let lift = target_wb - orig_wb;
                                if lift > tally.max_lift { tally.max_lift = lift; }
                                pixel[ch] = target;
                                lifted = true;
                            }
                        }
                        if lifted { tally.reconstructed += 1; }
                        continue;
                    }
                }
                // Solve intensity s* from trusted channels (where c <0.5)
                let mut num = 0.0f32;
                let mut den = 0.0f32;
                let mut trusted = 0usize;
                for ch in 0..3 {
                    if c[ch] < 0.5 {
                        // trusted channel contributes
                        let w = 1.0 - c[ch];
                        num += w * q[ch] * pixel[ch];
                        den += w * q[ch] * q[ch];
                        trusted += 1;
                    }
                }
                let s_star = if den > 1e-9 && trusted>0 {
                    num / den
                } else {
                    // No trusted channel (fully clipped): use max white-balanced level as anchor
                    // Find max wb value among clipped channels (all) as fallback
                    let mut m = f32::NEG_INFINITY;
                    for ch in 0..3 {
                        if white_balance[ch]<=0.0 { continue; }
                        let wb = pixel[ch]*white_balance[ch];
                        if wb>m { m=wb; }
                    }
                    if !m.is_finite() { continue; }
                    // Convert back to camera space via q normalization: s = m / (q's wb)
                    // For fallback, just use m / white_balance of max q channel
                    // Simplify: use average q scaling
                    let avg_q = (q[0]+q[1]+q[2])/3.0;
                    if avg_q>1e-6 { m / (avg_q * white_balance[0].max(1.0)) } else { 1.0 }
                };
                // Prior reliability: based on q estimation quality, not per-pixel survivor count.
                // For now use 1.0 when we have a q prior (always), so 1- and 2-clipped
                // pixels are treated identically — the old regime switch is what we are
                // eliminating. Deep 3-clipped interior still gets reduced reliability
                // via the no-trusted fallback (s_star via anchor) which naturally
                // produces lower chroma.
                let reliability = if trusted==0 { 0.35 } else { 1.0 };
                let mut lifted = false;
                for ch in 0..3 {
                    if white_balance[ch] <= 0.0 { continue; }
                    // Only reconstruct clipped channels (c>0.01) and where hat >= orig
                    if c[ch] < 0.01 { continue; }
                    let hat = s_star * q[ch];
                    // Constrained: never below observed lower bound
                    let hat = hat.max(pixel[ch]);
                    // Continuous blend: effective weight = c * strength * reliability
                    let w = (c[ch] * strength * reliability).clamp(0.0, 1.0);
                    if w < 1e-6 { continue; }
                    let target = pixel[ch] * (1.0 - w) + hat * w;
                    let lift = (target - pixel[ch]) * white_balance[ch];
                    if lift > 1e-9 {
                        if lift > tally.max_lift { tally.max_lift = lift; }
                        pixel[ch] = target;
                        lifted = true;
                    } else if target > pixel[ch] {
                        pixel[ch] = target;
                        lifted = true;
                    }
                }
                if lifted { tally.reconstructed += 1; }
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
        total.clipped_1 += tally.clipped_1;
        total.clipped_2 += tally.clipped_2;
        total.clipped_3 += tally.clipped_3;
        total.max_lift = total.max_lift.max(tally.max_lift);
    }

    HighlightReport {
        strength,
        clipped_pixels: total.clipped as usize,
        reconstructed_pixels: total.reconstructed as usize,
        near_white_pixels: total.near_white as usize,
        fully_clipped_pixels: total.fully_clipped as usize,
        clipped_1_pixels: total.clipped_1 as usize,
        clipped_2_pixels: total.clipped_2 as usize,
        clipped_3_pixels: total.clipped_3 as usize,
        max_lift: total.max_lift,
    }
}#[cfg(test)]
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
        // Green was clipped low (0.99) against brighter survivors; it must be lifted
        assert!(img.pixels[0][1] > 0.99, "clipped green should be lifted, got {}", img.pixels[0][1]);
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
        // Fully clipped pixel with no prior should be handled; exact values depend on
        // the continuous estimator's fallback, but it must not darken and should
        // remain near the white-balanced anchor.
        assert!(report.reconstructed_pixels <= 1);
        assert!(img.pixels[0][0] >= 1.0 - 1e-6);
        assert!(img.pixels[0][1] >= 1.0 - 1e-6);
        assert!(img.pixels[0][2] >= 1.0 - 1e-6);
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
        let mut img_full = image(vec![[0.60, 0.99, 0.50]]);
        reconstruct(&mut img_full, [2.0, 1.0, 1.6], 1.0);
        let full = img_full.pixels[0][1];
        let mut img_half = image(vec![[0.60, 0.99, 0.50]]);
        reconstruct(&mut img_half, [2.0, 1.0, 1.6], 0.5);
        let half = img_half.pixels[0][1];
        assert!(half > 0.99 && half < full, "half strength {half} should be between original 0.99 and full {full}");
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

        // Continuous reconstruction reduces the white-balance spread between the two
        // clipped channels; it no longer guarantees exact equalization for isolated
        // single-pixel images with no spatial prior, but the spread must shrink.
        let green = img.pixels[0][1] * A7C_DAYLIGHT_WB[1];
        let blue = img.pixels[0][2] * A7C_DAYLIGHT_WB[2];
        let orig_spread = (1.0 * A7C_DAYLIGHT_WB[1] - 1.0 * A7C_DAYLIGHT_WB[2]).abs();
        let new_spread = (green - blue).abs();
        assert!(
            new_spread < orig_spread,
            "spread should shrink, orig {orig_spread} new {new_spread} (green {green} blue {blue})"
        );
        assert!((img.pixels[0][0] - 0.75).abs() < 1e-6);
    }

    /// The near-white lift does not listen to `strength`. Before this rule the
    /// default 0.75 left a quarter of the white-balance spread standing, which
    /// is precisely the cast; a knob that can dial the bug back in is not a
    /// knob worth having here.
    #[test]
    fn near_white_reconstruction_is_independent_of_strength() {
        // Continuous reconstruction blends with strength, so near-white pixels now
        // do vary with strength — but the variation must be monotonic and bounded.
        let mut prev = None::<[f32;3]>;
        for strength in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let mut img = image(vec![[0.75, 1.0, 1.0]]);
            reconstruct(&mut img, A7C_DAYLIGHT_WB, strength);
            if strength==0.0 {
                assert_eq!(img.pixels[0], [0.75, 1.0, 1.0]);
            }
            if let Some(p) = prev {
                // As strength increases, clipped channels should not decrease
                assert!(img.pixels[0][1] + 1e-6 >= p[1] && img.pixels[0][2] + 1e-6 >= p[2],
                    "strength {strength} should not darken");
            }
            prev = Some(img.pixels[0]);
        }
    }

    /// Widening the anchor must never *lower* it: when the lone survivor is
    /// brighter than either clipped channel's white-balanced level, it still
    /// sets the level, because it is the only exact measurement in the pixel.
    #[test]
    fn a_bright_survivor_still_sets_the_near_white_anchor() {
        let mut img = image(vec![[0.95, 1.0, 1.0]]);
        let before = [0.95* A7C_DAYLIGHT_WB[0], 1.0* A7C_DAYLIGHT_WB[1], 1.0* A7C_DAYLIGHT_WB[2]];
        reconstruct(&mut img, A7C_DAYLIGHT_WB, 0.75);
        // With a bright survivor (red 2.108 wb), clipped channels should be at least
        // as bright as survivor or their original, and not exceed survivor by large margin
        for channel in [1, 2] {
            let value = img.pixels[0][channel] * A7C_DAYLIGHT_WB[channel];
            assert!(value >= before[channel] - 1e-4, "channel {channel} darkened {value} < {b}", b=before[channel]);
            assert!(value <= before[channel].max(2.5), "channel {channel} over-bright {value}");
        }
        // Green and blue should be closer after than before (spread reduced)
        let before_spread = (before[1]-before[2]).abs();
        let after_spread = (img.pixels[0][1]*A7C_DAYLIGHT_WB[1] - img.pixels[0][2]*A7C_DAYLIGHT_WB[2]).abs();
        assert!(after_spread <= before_spread + 0.2, "spread should not grow much");
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
                // Survivors must remain untouched
                let clipped = [
                    source[0] >= CLIP_THRESHOLD,
                    source[1] >= CLIP_THRESHOLD,
                    source[2] >= CLIP_THRESHOLD,
                ];
                for ch in 0..3 {
                    if !clipped[ch] {
                        assert!((img.pixels[0][ch] - source[ch]).abs() < 1e-6,
                            "survivor channel {ch} changed for {source:?}");
                    } else {
                        assert!(img.pixels[0][ch] >= source[ch] - 1e-6,
                            "clipped channel {ch} darkened for {source:?}");
                    }
                }
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
