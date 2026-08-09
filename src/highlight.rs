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
//! 2. The anchor depends on how *near-white* the pixel is, because that is what
//!    says whether it is a coloured highlight or a blown one:
//!    - **One channel clipped** — a genuinely coloured highlight. The two
//!      survivors are exact, so the anchor is the brighter of them in
//!      white-balanced space: the most reliable lower bound on how bright the
//!      highlight really was, and a hue assumption, so `strength` applies.
//!    - **Two or three channels clipped** — a near-white highlight. The anchor
//!      is the largest white-balanced value over *all* channels, clipped ones
//!      included, and it is applied at full strength.
//!
//!    Those are the two ends of one continuum, not two branches: see
//!    [`near_whiteness`] for the weight that moves between them without a step.
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
//! # Why the two rules are one continuum
//!
//! Selecting the rule on the *count* of clipped channels makes the whole
//! treatment of a pixel hinge on whether one channel is a hair above or a hair
//! below [`CLIP_THRESHOLD`]. On a smooth sky gradient that threshold is a
//! contour, and the two rules disagree hard across it — the anchor widens, and
//! `strength` jumps from 0.75 to 1.0 — so the contour is visible as a hue step.
//!
//! [`near_whiteness`] replaces the count with the *second-largest* clip
//! confidence: the continuous form of "at least two channels are at the clip
//! point". It drives both halves of the rule, and reduces to the counted
//! version exactly at the ends, so the measured behaviour of the two cohorts is
//! unchanged while the boundary between them stops being a step:
//!
//! * the anchor takes each channel's white-balanced value at weight
//!   `1 - c·(1 - n)` — a survivor (`c = 0`) always contributes in full, and a
//!   clipped channel's own value counts only as far as the pixel is near-white;
//! * the lift runs at `strength + (1 - strength)·n`, which is `strength` for a
//!   coloured highlight and 1.0 for a near-white one;
//! * each channel takes its share of that lift in proportion to its own clip
//!   confidence, so a channel entering the clip ramp does not snap to the
//!   anchor the moment it crosses the threshold.
//!
//! A previous attempt at continuity replaced the anchor with a local
//! chromaticity prior and a least-squares intensity solve. It is not what
//! shipped, and `CHANGELOG.md` records why: with no unclipped neighbour inside
//! a large blown region the prior falls back to the frame's mean chromaticity —
//! vegetation, on a landscape — so blown skies took on the scene's average hue
//! and the fully-clipped interior was left with most of its white-balance cast.
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

/// Lower and upper end of the continuous clip ramp.
///
/// A channel below [`CLIP_RAMP_LOW`] is fully trusted, one at or above
/// [`CLIP_RAMP_HIGH`] is fully synthesised, and the Hermite ramp between them is
/// what keeps the 0→1→2→3 clipped-channel boundaries from becoming visible
/// contours. Every consumer of "how much of this pixel's colour did we invent?"
/// shares these two numbers through [`clip_confidence`].
pub const CLIP_RAMP_LOW: f32 = 0.92;
pub const CLIP_RAMP_HIGH: f32 = 0.985;

/// Value below which a channel is too dim for raw-site clip evidence to mean
/// anything about it.
///
/// [`reconstruct_with_confidence`] can be handed the per-channel clip
/// confidence of the *mosaic sites* behind each output pixel, which is stronger
/// evidence than the demosaiced value: averaging a saturated site with its
/// neighbours pulls the interpolated value below [`CLIP_THRESHOLD`] and hides
/// the clip. But the same averaging runs at every edge, so a leaf pixel against
/// a blown sky inherits some of the sky's saturated sites, and taking that at
/// face value would reconstruct the leaf. Raw-site evidence therefore ramps in
/// over `[RAW_EVIDENCE_RAMP_LOW, CLIP_RAMP_LOW]`: it is ignored on a pixel too
/// dim for the question to be meaningful, and counts in full by the point the
/// value-based ramp takes over.
const RAW_EVIDENCE_RAMP_LOW: f32 = 0.84;

/// Hermite ramp between `low` and `high`, clamped outside.
#[inline]
fn ramp(value: f32, low: f32, high: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Per-channel clip confidence: 0 = the recorded value is real, 1 = it is at the
/// sensor's clip point and whatever is there was reconstructed.
#[inline]
pub fn clip_confidence(value: f32) -> f32 {
    ramp(value, CLIP_RAMP_LOW, CLIP_RAMP_HIGH)
}

/// How near-white a pixel is, as a weight in `[0, 1]`.
///
/// This is the continuous form of "at least [`NEAR_WHITE_CLIPPED_CHANNELS`]
/// channels are at the sensor's clip point": the second-largest of the three
/// per-channel clip confidences. One channel at the clip point leaves it at 0
/// however hard that channel is clipped — a coloured highlight stays a coloured
/// highlight — and it reaches 1 only once a second channel is fully clipped
/// too, which is the point at which the pixel's hue stops being a measurement.
///
/// Using the second-largest rather than a count is what removes the contour:
/// the transition happens over the [`clip_confidence`] ramp of whichever
/// channel is clipping second, not at a threshold crossing.
#[inline]
pub fn near_whiteness(confidence: [f32; 3]) -> f32 {
    let [a, b, c] = confidence;
    // Second largest of three, branch-predictable and allocation-free.
    a.min(b).max(a.min(c)).max(b.min(c))
}

/// Collapse three per-channel confidences into the single gate weight the
/// renderer and the local-tone operators apply.
///
/// `r = max(c)` says *any* channel was invented; the `mean` term says how much
/// of the pixel was. One clipped channel lands at ≈0.83, three at 1.0, and the
/// blend is continuous so the 1→2 clipped boundary is not a step.
#[inline]
pub fn synthesis_gate_from_confidence(c: [f32; 3]) -> f32 {
    let r = c[0].max(c[1]).max(c[2]);
    let mean_c = (c[0] + c[1] + c[2]) / 3.0;
    (r * (0.75 + 0.25 * mean_c)).clamp(0.0, 1.0)
}

/// The same gate, recovered from the scalar reconstruction uncertainty map that
/// [`reconstruct_with_confidence_and_uncertainty`] emits.
///
/// That map stores the *mean* of the three confidences (one f32 per pixel rather
/// than three), so `max` is not stored. `min(3u, 1)` recovers it exactly in the
/// case that matters — a highlight where one or two channels are hard-clipped
/// and the rest are not — which is where the gate does its work. The fixed
/// points match [`synthesis_gate_from_confidence`]: u=1/3 → 0.83, u=2/3 → 0.92,
/// u=1 → 1.0.
#[inline]
pub fn synthesis_gate(uncertainty: f32) -> f32 {
    let u = uncertainty.clamp(0.0, 1.0);
    let r = (3.0 * u).min(1.0);
    (r * (0.75 + 0.25 * u)).clamp(0.0, 1.0)
}

/// Clipped-channel count at or above which a pixel is reported as near-white.
///
/// Two is the threshold because two clipped channels are already enough to
/// destroy the pixel's hue: whatever ratio they had is gone, and only the shared
/// raw clip level is left, which white balance then spreads apart. One clipped
/// channel still leaves two exact survivors defining a real colour.
///
/// This governs the *report* only. The rule itself moves between the two
/// regimes continuously — see [`near_whiteness`] — so that counting a pixel one
/// way or the other never decides how it is rendered.
const NEAR_WHITE_CLIPPED_CHANNELS: usize = 2;

/// What clipped-highlight reconstruction did to one frame.
#[derive(Debug, Clone, Serialize)]
pub struct HighlightReport {
    /// Estimator that produced this report. `current` is the unattended
    /// post-demosaic path; the spatial methods are explicit CLI experiments.
    pub method: crate::raw_highlight::HighlightMethod,
    /// Strength requested, in `(0, 1]`.
    pub strength: f32,
    /// Pixels with at least one channel at or above [`CLIP_THRESHOLD`].
    pub clipped_pixels: usize,
    /// Pixels this module actually moved.
    ///
    /// Not bounded by `clipped_pixels`: the lift ramps in over
    /// [`clip_confidence`], which starts below [`CLIP_THRESHOLD`], so a pixel
    /// whose brightest channel is inside the ramp but under the threshold can
    /// take a partial lift without being counted as clipped. That is the point
    /// of the ramp — the alternative is a step at the threshold.
    pub reconstructed_pixels: usize,
    /// Pixels with at least [`NEAR_WHITE_CLIPPED_CHANNELS`] channels clipped —
    /// the near-white cohort.
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
    /// Raw CFA sites classified as clipped by a pre-demosaic method.
    pub clipped_cfa_sites: usize,
    /// Raw CFA sites actually raised by a pre-demosaic method.
    pub reconstructed_cfa_sites: usize,
    /// Demosaiced output pixels whose source neighbourhood contained clipped
    /// raw evidence.
    pub affected_output_pixels: usize,
    /// Eight-connected clipped regions processed sequentially.
    pub connected_regions: usize,
    /// Sites lifted by a verified sensor-knee inverse.
    pub knee_corrected_sites: usize,
    /// Mean accepted colour-line R² for spatial fitting.
    pub mean_fit_quality: f32,
    /// Connected regions with no surviving channel in their interior.
    pub fully_clipped_cores: usize,
    /// Regions handled by the deterministic iterative solver fallback.
    pub solver_fallbacks: usize,
}

/// Reconstruct clipped highlights in place on a camera-RGB image.
///
/// `white_balance` is the as-shot coefficient per camera channel, in the file's
/// own order; a non-positive coefficient (an unfilled channel) disables the lift
/// for that channel, since the round-trip through it is undefined.
///
/// `confidence`, when supplied, is the per-channel clip confidence of the mosaic
/// sites behind each output pixel — see [`RAW_EVIDENCE_RAMP_LOW`]. A map whose
/// length does not match the image is ignored rather than indexed into.
pub fn reconstruct_with_confidence(
    image: &mut Image<CameraRgb>,
    white_balance: [f32; 3],
    strength: f32,
    confidence: Option<&[[f32; 3]]>,
) -> HighlightReport {
    let confidence = confidence.filter(|c| c.len() == image.pixels.len());
    reconstruct_inner(image, white_balance, strength, confidence, None)
}

/// Reconstruct and, alongside the report, emit a per-pixel *reconstruction
/// uncertainty* map aligned to the image grid: 0 where the pixel was fully
/// trusted, rising with the clipped-channel confidence (≈1/3 for one clipped
/// channel, ≈2/3 for two, ≈1 for three). This is the raw-domain clip confidence
/// the renderer needs to know *which* highlight colour it synthesised, so the
/// chroma boost can be withheld there regardless of how bright the pixel ends up
/// after the tone curve — the gap a display-brightness proxy cannot see.
pub fn reconstruct_with_confidence_and_uncertainty(
    image: &mut Image<CameraRgb>,
    white_balance: [f32; 3],
    strength: f32,
    confidence: Option<&[[f32; 3]]>,
) -> (HighlightReport, Vec<f32>) {
    let mut uncertainty = vec![0.0_f32; image.pixels.len()];
    let confidence = confidence.filter(|c| c.len() == image.pixels.len());
    let report = reconstruct_inner(
        image,
        white_balance,
        strength,
        confidence,
        Some(&mut uncertainty),
    );
    (report, uncertainty)
}

pub fn reconstruct(
    image: &mut Image<CameraRgb>,
    white_balance: [f32; 3],
    strength: f32,
) -> HighlightReport {
    reconstruct_inner(image, white_balance, strength, None, None)
}

/// Per-channel clip confidence for one pixel, combining the demosaiced value
/// with the raw-site evidence when there is any.
#[inline]
fn pixel_confidence(pixel: [f32; 3], mosaic: Option<[f32; 3]>) -> [f32; 3] {
    let mut c = [
        clip_confidence(pixel[0]),
        clip_confidence(pixel[1]),
        clip_confidence(pixel[2]),
    ];
    if let Some(mosaic) = mosaic {
        for channel in 0..3 {
            let gate = ramp(pixel[channel], RAW_EVIDENCE_RAMP_LOW, CLIP_RAMP_LOW);
            c[channel] = c[channel].max(mosaic[channel] * gate);
        }
    }
    c
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

impl Tally {
    fn count(&mut self, clipped_count: usize) {
        if clipped_count == 0 {
            return;
        }
        self.clipped += 1;
        match clipped_count {
            1 => self.clipped_1 += 1,
            2 => self.clipped_2 += 1,
            _ => self.clipped_3 += 1,
        }
        if clipped_count >= NEAR_WHITE_CLIPPED_CHANNELS {
            self.near_white += 1;
            if clipped_count == 3 {
                self.fully_clipped += 1;
            }
        }
    }

    fn merge(&mut self, other: &Tally) {
        self.clipped += other.clipped;
        self.reconstructed += other.reconstructed;
        self.near_white += other.near_white;
        self.fully_clipped += other.fully_clipped;
        self.clipped_1 += other.clipped_1;
        self.clipped_2 += other.clipped_2;
        self.clipped_3 += other.clipped_3;
        self.max_lift = self.max_lift.max(other.max_lift);
    }

    fn into_report(self, strength: f32) -> HighlightReport {
        HighlightReport {
            method: crate::raw_highlight::HighlightMethod::Current,
            strength,
            clipped_pixels: self.clipped as usize,
            reconstructed_pixels: self.reconstructed as usize,
            near_white_pixels: self.near_white as usize,
            fully_clipped_pixels: self.fully_clipped as usize,
            clipped_1_pixels: self.clipped_1 as usize,
            clipped_2_pixels: self.clipped_2 as usize,
            clipped_3_pixels: self.clipped_3 as usize,
            max_lift: self.max_lift,
            clipped_cfa_sites: 0,
            reconstructed_cfa_sites: 0,
            affected_output_pixels: 0,
            connected_regions: 0,
            knee_corrected_sites: 0,
            mean_fit_quality: 0.0,
            fully_clipped_cores: 0,
            solver_fallbacks: 0,
        }
    }
}

/// Fold a pre-demosaic report and the original propagated clip evidence into
/// the same sidecar shape as the current estimator.  The uncertainty map is
/// derived only from the original raw evidence: reconstructed values above one
/// must not classify themselves as newly clipped.
pub(crate) fn spatial_report_and_uncertainty(
    image: &Image<CameraRgb>,
    strength: f32,
    confidence: Option<&[[f32; 3]]>,
    raw: crate::raw_highlight::RawHighlightReport,
) -> (HighlightReport, Vec<f32>) {
    let confidence = confidence.filter(|map| map.len() == image.pixels.len());
    let mut uncertainty = Vec::with_capacity(image.pixels.len());
    let mut tally = Tally::default();
    let mut affected = 0_usize;
    for (index, pixel) in image.pixels.iter().enumerate() {
        let c = confidence.map_or_else(
            || pixel.map(clip_confidence),
            |map| map[index].map(|value| value.clamp(0.0, 1.0)),
        );
        let clipped = c.iter().filter(|value| **value >= 0.5).count();
        tally.count(clipped);
        let u = ((c[0] + c[1] + c[2]) / 3.0).clamp(0.0, 1.0);
        affected += (u > 0.0) as usize;
        uncertainty.push(u);
    }
    let mut report = tally.into_report(strength);
    report.method = raw.method;
    report.reconstructed_pixels = affected;
    report.max_lift = raw.max_lift;
    report.clipped_cfa_sites = raw.clipped_cfa_sites;
    report.reconstructed_cfa_sites = raw.reconstructed_cfa_sites;
    report.affected_output_pixels = affected;
    report.connected_regions = raw.connected_regions;
    report.knee_corrected_sites = raw.knee_corrected_sites;
    report.mean_fit_quality = raw.mean_fit_quality;
    report.fully_clipped_cores = raw.fully_clipped_cores;
    report.solver_fallbacks = raw.solver_fallbacks;
    (report, uncertainty)
}

/// One pixel of the rule. Returns the lift applied, in white-balanced units, or
/// `None` if nothing moved.
///
/// Split out so the whole rule fits on a screen and so the tests can reason
/// about it without a parallel iterator in the way.
#[inline]
fn reconstruct_pixel(
    pixel: &mut [f32; 3],
    white_balance: [f32; 3],
    strength: f32,
    confidence: [f32; 3],
) -> Option<f32> {
    let near_white = near_whiteness(confidence);

    // The anchor is the brightest white-balanced level the pixel's own channels
    // justify. Every channel's recorded value is a lower bound on what it
    // really was; the question is whether a *clipped* channel's bound may set
    // the level for the others. For a coloured highlight it must not — that is
    // how a red specular stays red instead of being flattened to white — so a
    // clipped channel's contribution is scaled by how near-white the pixel is,
    // reaching its full value only once the hue has stopped being a measurement
    // anyway. A survivor always contributes in full.
    let mut anchor = f32::NEG_INFINITY;
    for channel in 0..3 {
        if white_balance[channel] <= 0.0 {
            continue;
        }
        let weight = 1.0 - confidence[channel] * (1.0 - near_white);
        let contribution = pixel[channel] * white_balance[channel] * weight;
        if contribution > anchor {
            anchor = contribution;
        }
    }
    if !anchor.is_finite() {
        return None;
    }

    // `strength` is caution about inventing a hue, and a near-white pixel has no
    // hue left to invent: a partial blend toward neutral is just a fraction of
    // the cast. So the lift runs at `strength` for a coloured highlight and
    // reaches 1.0 as the pixel becomes near-white.
    let effective_strength = strength + (1.0 - strength) * near_white;

    let mut max_lift = 0.0_f32;
    let mut lifted = false;
    for channel in 0..3 {
        if white_balance[channel] <= 0.0 || confidence[channel] <= 0.0 {
            continue;
        }
        let original = pixel[channel] * white_balance[channel];
        if anchor <= original {
            continue;
        }
        // A channel takes its share of the lift in proportion to its own clip
        // confidence, so entering the clip ramp is a ramp and not a snap.
        let weight = (effective_strength * confidence[channel]).clamp(0.0, 1.0);
        let target = original + (anchor - original) * weight;
        let lift = target - original;
        if lift <= 0.0 {
            continue;
        }
        pixel[channel] = target / white_balance[channel];
        max_lift = max_lift.max(lift);
        lifted = true;
    }

    lifted.then_some(max_lift)
}

fn reconstruct_inner(
    image: &mut Image<CameraRgb>,
    white_balance: [f32; 3],
    strength: f32,
    mosaic_confidence: Option<&[[f32; 3]]>,
    uncertainty_out: Option<&mut [f32]>,
) -> HighlightReport {
    let strength = strength.clamp(0.0, 1.0);
    if strength == 0.0 || image.pixels.is_empty() {
        // The caller skips this module entirely at strength 0; this arm exists
        // so a direct call still reports what it saw without touching a pixel.
        let mut tally = Tally::default();
        for pixel in &image.pixels {
            tally.count(clip_count(*pixel));
        }
        return tally.into_report(strength);
    }

    const CHUNK: usize = 65_536;
    // One pass. The uncertainty map is read off the pixel *before* the lift
    // touches it, which is the only ordering that works in place, and chunking
    // both buffers identically keeps the indices aligned without arithmetic.
    let mut scratch;
    let uncertainty: &mut [f32] = match uncertainty_out {
        Some(slice) => slice,
        None => {
            scratch = vec![0.0_f32; image.pixels.len()];
            &mut scratch
        }
    };
    debug_assert_eq!(uncertainty.len(), image.pixels.len());

    let tallies: Vec<Tally> = image
        .pixels
        .par_chunks_mut(CHUNK)
        .zip(uncertainty.par_chunks_mut(CHUNK))
        .enumerate()
        .map(|(chunk_index, (pixels, uncertainty))| {
            let base = chunk_index * CHUNK;
            let mut tally = Tally::default();
            for (offset, pixel) in pixels.iter_mut().enumerate() {
                tally.count(clip_count(*pixel));

                let mosaic = mosaic_confidence.map(|map| map[base + offset]);
                let confidence = pixel_confidence(*pixel, mosaic);
                uncertainty[offset] =
                    ((confidence[0] + confidence[1] + confidence[2]) / 3.0).clamp(0.0, 1.0);
                if confidence == [0.0; 3] {
                    continue;
                }

                if let Some(lift) = reconstruct_pixel(pixel, white_balance, strength, confidence) {
                    tally.reconstructed += 1;
                    tally.max_lift = tally.max_lift.max(lift);
                }
            }
            tally
        })
        .collect();

    let mut total = Tally::default();
    for tally in &tallies {
        total.merge(tally);
    }
    total.into_report(strength)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(pixels: Vec<[f32; 3]>) -> Image<CameraRgb> {
        let len = pixels.len();
        Image::new(len, 1, pixels).expect("valid image")
    }

    /// The A7C daylight coefficients that produced the lavender skies. Red and
    /// blue both sit well above green, which is what makes the residual spread
    /// read as magenta rather than as a mild warmth.
    const A7C_DAYLIGHT_WB: [f32; 3] = [2.219, 1.0, 1.773];

    /// The canonical false-colour case: green clipped low against a bright,
    /// surviving neutral. It must be lifted to the anchor at full strength.
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

    /// The whole point of the near-white weight: a single clipped channel is
    /// still a colour, and must render exactly as the survivor-anchored blend
    /// says. This is the guarantee that the rule is confined to near-white
    /// pixels and cannot desaturate a red flower or a green specular.
    #[test]
    fn one_clipped_channel_keeps_the_survivor_anchor_and_the_strength_blend() {
        for source in [[0.99, 0.30, 0.20], [0.60, 0.99, 0.50], [0.20, 0.35, 0.99]] {
            for strength in [0.25, 0.75, 1.0] {
                let mut img = image(vec![source]);
                let report = reconstruct(&mut img, A7C_DAYLIGHT_WB, strength);
                assert_eq!(report.near_white_pixels, 0, "{source:?} is not near-white");
                assert_eq!(
                    near_whiteness([
                        clip_confidence(source[0]),
                        clip_confidence(source[1]),
                        clip_confidence(source[2]),
                    ]),
                    0.0,
                    "{source:?} must sit at the coloured-highlight end of the weight"
                );

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
                        expected[channel] =
                            (original + (anchor - original) * strength) / A7C_DAYLIGHT_WB[channel];
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

    /// The near-white weight is the second-largest confidence, which is what
    /// makes it the continuous form of "two or more channels clipped".
    #[test]
    fn near_whiteness_is_the_second_largest_confidence() {
        assert_eq!(near_whiteness([0.0, 0.0, 0.0]), 0.0);
        assert_eq!(near_whiteness([1.0, 0.0, 0.0]), 0.0);
        assert_eq!(near_whiteness([1.0, 1.0, 0.0]), 1.0);
        assert_eq!(near_whiteness([1.0, 1.0, 1.0]), 1.0);
        assert_eq!(near_whiteness([0.25, 1.0, 0.5]), 0.5);
        assert_eq!(near_whiteness([0.5, 0.25, 1.0]), 0.5);
    }

    /// The reason the weight exists: sweeping one channel across the clip ramp
    /// must move the result continuously, with no step at `CLIP_THRESHOLD`
    /// where the counted rule used to change regime.
    #[test]
    fn the_near_white_transition_has_no_step_at_the_threshold() {
        // Blue hard-clipped, red a comfortable survivor, green sweeping up
        // through the ramp: the 1→2 boundary the counted rule turned into a
        // contour on every sky gradient in the corpus.
        let sweep: Vec<[f32; 3]> = (0..=200)
            .map(|i| [0.70, 0.90 + i as f32 * 0.001, 1.0])
            .collect();
        let mut img = image(sweep.clone());
        reconstruct(&mut img, A7C_DAYLIGHT_WB, 0.75);

        let mut largest = 0.0_f32;
        for window in img.pixels.windows(2) {
            for (channel, coefficient) in A7C_DAYLIGHT_WB.iter().enumerate() {
                let step = ((window[1][channel] - window[0][channel]) * coefficient).abs();
                largest = largest.max(step);
            }
        }
        // One input step is 0.001 in camera units; the largest white-balanced
        // output step must stay the same order. The counted rule jumped by the
        // whole coefficient spread (>0.5) at the threshold.
        assert!(
            largest < 0.05,
            "reconstruction stepped by {largest} across the clip ramp"
        );
    }

    /// Raw-site evidence may strengthen the case for a clipped channel, but not
    /// enlist a pixel that is nowhere near the clip point — otherwise every edge
    /// against a blown region inherits its neighbour's saturation.
    #[test]
    fn raw_site_evidence_is_ignored_on_a_dim_pixel() {
        let dim = [0.30, 0.20, 0.10];
        let mut img = image(vec![dim]);
        let confidence = vec![[1.0_f32, 1.0, 1.0]];
        let report = reconstruct_with_confidence(&mut img, A7C_DAYLIGHT_WB, 1.0, Some(&confidence));
        assert_eq!(report.reconstructed_pixels, 0);
        assert_eq!(img.pixels[0], dim);
    }

    /// ...but on a pixel the demosaic pulled just under the threshold, the raw
    /// sites still decide. This is the case the value test alone cannot see.
    #[test]
    fn raw_site_evidence_reconstructs_what_the_demosaic_averaged_down() {
        let source = [0.75, 0.96, 0.96];
        let mut without = image(vec![source]);
        reconstruct(&mut without, A7C_DAYLIGHT_WB, 0.75);

        let mut with = image(vec![source]);
        let confidence = vec![[0.0_f32, 1.0, 1.0]];
        reconstruct_with_confidence(&mut with, A7C_DAYLIGHT_WB, 0.75, Some(&confidence));

        let green_without = without.pixels[0][1] * A7C_DAYLIGHT_WB[1];
        let green_with = with.pixels[0][1] * A7C_DAYLIGHT_WB[1];
        assert!(
            green_with > green_without,
            "raw-site evidence must lift green further: {green_with} vs {green_without}"
        );
        let blue = with.pixels[0][2] * A7C_DAYLIGHT_WB[2];
        assert!(
            (green_with - blue).abs() < 1e-5,
            "the two clipped channels must still land together, {green_with} vs {blue}"
        );
    }

    /// A confidence map of the wrong length is ignored, not indexed into.
    #[test]
    fn a_mismatched_confidence_map_is_ignored() {
        let source = [0.75, 1.0, 1.0];
        let mut with = image(vec![source; 4]);
        let confidence = vec![[1.0_f32, 1.0, 1.0]; 2];
        let report =
            reconstruct_with_confidence(&mut with, A7C_DAYLIGHT_WB, 0.75, Some(&confidence));
        assert_eq!(report.near_white_pixels, 4);

        let mut without = image(vec![source; 4]);
        reconstruct(&mut without, A7C_DAYLIGHT_WB, 0.75);
        assert_eq!(with.pixels, without.pixels);
    }

    /// The uncertainty map the renderer gates on is read from the pixel before
    /// the lift moves it, so it describes what was measured, not what was built.
    #[test]
    fn the_uncertainty_map_describes_the_original_clip_state() {
        let mut img = image(vec![[0.10, 0.20, 0.30], [0.75, 1.0, 1.0], [1.0, 1.0, 1.0]]);
        let (_, uncertainty) =
            reconstruct_with_confidence_and_uncertainty(&mut img, A7C_DAYLIGHT_WB, 0.75, None);
        assert_eq!(uncertainty.len(), 3);
        assert_eq!(uncertainty[0], 0.0, "an unclipped pixel invented nothing");
        assert!(
            (uncertainty[1] - 2.0 / 3.0).abs() < 1e-6,
            "two clipped channels should read 2/3, got {}",
            uncertainty[1]
        );
        assert!(
            (uncertainty[2] - 1.0).abs() < 1e-6,
            "a fully clipped pixel should read 1, got {}",
            uncertainty[2]
        );
    }

    /// Strength 0 is the caller's "off" switch and must not touch a pixel, while
    /// still reporting what it saw.
    #[test]
    fn strength_zero_reports_without_reconstructing() {
        let pixels = vec![[0.75, 1.0, 1.0], [0.10, 0.20, 0.30]];
        let mut img = image(pixels.clone());
        let report = reconstruct(&mut img, A7C_DAYLIGHT_WB, 0.0);
        assert_eq!(img.pixels, pixels);
        assert_eq!(report.clipped_pixels, 1);
        assert_eq!(report.near_white_pixels, 1);
        assert_eq!(report.reconstructed_pixels, 0);
    }
}
