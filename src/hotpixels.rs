//! Hot and dead pixel suppression, on the CFA mosaic before demosaic.
//!
//! # Why this is the first correction in the queue
//!
//! `docs/PLAN.md` puts hot/dead-pixel suppression first among the photographic
//! corrections: it is cheap, mechanical, and phone sensors need it — a single
//! stuck photosite becomes a coloured speck the demosaic then smears across a
//! 2x2 (or larger) neighbourhood, so it is strictly cheaper to kill it before
//! interpolation than after. A hot pixel is a photosite reading far *above* its
//! true light; a dead one reads far *below* (often stuck near black). Both are
//! isolated single-site outliers, which is exactly what separates them from real
//! detail: genuine texture is correlated across neighbouring same-colour sites,
//! a defect is not.
//!
//! # The detector
//!
//! For each photosite this compares its value against its **same-colour**
//! neighbours only — comparing a red site to the green sites beside it would read
//! the mosaic itself as an outlier. Same-colour neighbours are found by asking
//! the CFA for the colour at each offset in a 5x5 window, so the two green phases
//! of a Bayer array are handled correctly (a green's nearest same-colour
//! neighbours are the diagonal greens one pixel away; a red's or blue's are two
//! pixels away orthogonally and diagonally).
//!
//! A site is corrected only when it stands clear of **every** same-colour
//! neighbour — above the brightest, or below the darkest — by more than a
//! margin set by the local contrast:
//!
//! ```text
//! scale = (max_neighbour - min_neighbour) + FLOOR
//! hot   ⇔  value - max_neighbour > k * scale
//! dead  ⇔  min_neighbour - value > k * scale
//! ```
//!
//! Comparing against the extreme neighbour rather than the median is what makes
//! this selective: a genuine defect is an *isolated* spike that overshoots even
//! the brightest of its own-colour neighbours, whereas noise and fine detail sit
//! within the spread of the surrounding same-colour sites. The `(max - min)`
//! term makes it edge-preserving — across a real edge the neighbours already
//! span a wide range, so the margin is large and a bright pixel on the bright
//! side of the edge is not flagged. `FLOOR` is an absolute term (normalized DN)
//! that sets the sensitivity in a genuinely flat region, where the neighbour
//! range collapses to near zero; it is chosen well above ordinary shot noise so
//! that only a real outlier trips the detector. A corrected site is replaced
//! with the **median** of its same-colour neighbours, which is robust to a
//! second defect sitting in the same window.
//!
//! # Strength, and why it ships off by default
//!
//! `strength` in `(0, 1]` scales the sensitivity: `k` falls from `K_MAX` toward
//! `K_MIN` as strength rises, so a higher strength corrects more aggressively.
//! `strength == 0` is not represented here at all — the caller skips this module
//! entirely, which is what keeps the owned path byte-identical when the feature
//! is off.
//!
//! It is **off by default**, like `--local-tone` and `--local-white-balance`,
//! and for the same reason: there is no corpus of frames with known hot pixels
//! to tune `K`/`FLOOR` against yet, and a too-eager detector would erase real
//! stars, specular glints and fine bright detail — a criterion-3 ("no frame
//! ruined") risk. The constants below are defensible defaults awaiting that
//! corpus; every one is named and its role stated so the tuning is mechanical
//! when the frames exist.
//!
//! # Determinism
//!
//! Corrections are computed from the *unmodified* mosaic into a per-row-chunk
//! list, concatenated in chunk order, and only then written back. So no
//! correction can influence another's detection, and the result does not depend
//! on `--jobs` or thread scheduling. Every read is from the original buffer;
//! every same-colour neighbour is an original sample.

use rawler::cfa::CFA;
use rayon::prelude::*;
use serde::Serialize;
#[cfg(test)]
use rawler::cfa::CFAColor;

/// Rows handed to one parallel task. Fixed by the data, not the thread count,
/// so the work split is a property of the frame and not of the machine.
const ROWS_PER_CHUNK: usize = 64;

/// Half-width of the search window, in pixels. Two is the smallest window that
/// reaches the nearest same-colour site in every direction for a 2x2 CFA.
const RADIUS: isize = 2;

/// Minimum same-colour neighbours a site needs before it can be judged an
/// outlier at all. Below this — at the very corners of the frame — the site is
/// left untouched rather than corrected on thin evidence.
const MIN_NEIGHBOURS: usize = 3;

/// Absolute floor added to the local dispersion, in normalized DN.
///
/// About 3% of full scale. This is the margin a site must clear its brightest
/// (or darkest) same-colour neighbour by in a flat region, so it is set well
/// above ordinary shot noise: a lower value flags thousands of noise peaks on an
/// ordinary daylight frame, which is the false-positive failure mode this
/// correction exists to avoid. Awaits tuning against a corpus of frames with
/// known defects.
const FLOOR: f32 = 0.03;

/// Sensitivity multipliers on the local dispersion, at the two ends of the
/// strength range. `k = K_MAX - (K_MAX - K_MIN) * strength`, so strength 1.0
/// gives `K_MIN` (most sensitive) and a strength approaching 0 gives `K_MAX`
/// (least). Both await corpus tuning; they are set so that strength 1.0 catches
/// a site overshooting its brightest same-colour neighbour by a clear margin
/// while leaving textured detail, which never overshoots its own-colour
/// neighbours by much, alone.
const K_MIN: f32 = 1.5;
const K_MAX: f32 = 6.0;

/// What the detector did to one frame. Recorded in the colour report so a survey
/// can tell how often the correction fires and on what.
#[derive(Debug, Clone, Serialize)]
pub struct HotPixelReport {
    /// Strength requested, in `(0, 1]`.
    pub strength: f32,
    /// Sites that had enough same-colour neighbours to be judged.
    pub examined: usize,
    /// Sites replaced because they sat above every same-colour neighbour.
    pub hot: usize,
    /// Sites replaced because they sat below every same-colour neighbour.
    pub dead: usize,
    /// Largest absolute correction applied, in normalized DN.
    pub max_correction: f32,
}

impl HotPixelReport {
    /// Total sites corrected.
    pub fn corrected(&self) -> usize {
        self.hot + self.dead
    }
}

/// One candidate correction, `(flat index, replacement value, was_hot)`.
type Correction = (usize, f32, bool);

/// Same-colour offsets within the search window, precomputed once per CFA
/// pattern instead of asked of `cfa.cfa_color_at` for every candidate of
/// every photosite.
///
/// The CFA pattern repeats every `cfa.height x cfa.width` sites, so which
/// window offsets share a photosite's colour depends only on that photosite's
/// *phase* — `(row % height, col % width)` — not on its absolute position.
/// Flamegraph profiling found `gather_same_colour`'s per-candidate colour
/// comparison at ~26% of the whole program's samples even after the median
/// sort above was deferred to the correction path: it ran up to 24 times per
/// photosite, for every photosite. Looking the answer up per phase instead of
/// recomputing it per site removes the comparison from the hot loop entirely
/// — same offsets, same colours, same correction, computed once instead of
/// once per pixel.
struct SameColourOffsets {
    /// `offsets[(row % height) * width + (col % width)]` is that phase's list
    /// of same-colour `(dy, dx)` offsets inside the search window.
    offsets: Vec<Vec<(i8, i8)>>,
    height: usize,
    width: usize,
}

impl SameColourOffsets {
    fn new(cfa: &CFA) -> Self {
        let height = cfa.height.max(1);
        let width = cfa.width.max(1);
        let mut offsets = Vec::with_capacity(height * width);
        for phase_row in 0..height {
            for phase_col in 0..width {
                let colour = cfa.cfa_color_at(phase_row, phase_col);
                let mut same = Vec::new();
                for dy in -RADIUS..=RADIUS {
                    for dx in -RADIUS..=RADIUS {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        // `cfa_color_at` only depends on the offset modulo the
                        // pattern size, so probing near a multiple of the
                        // period stays positive without changing the phase.
                        let probe_row = (phase_row as isize + dy + height as isize * 4) as usize;
                        let probe_col = (phase_col as isize + dx + width as isize * 4) as usize;
                        if cfa.cfa_color_at(probe_row, probe_col) == colour {
                            same.push((dy as i8, dx as i8));
                        }
                    }
                }
                offsets.push(same);
            }
        }
        Self {
            offsets,
            height,
            width,
        }
    }

    #[inline]
    fn at(&self, row: usize, col: usize) -> &[(i8, i8)] {
        &self.offsets[(row % self.height) * self.width + (col % self.width)]
    }
}

/// Correct hot and dead sites on a CFA mosaic in place.
///
/// `samples` is the normalized mosaic, row-major, one sample per photosite.
/// `cfa` colours are read in full-frame coordinates, which is the space the
/// decoder has already shifted the pattern into (see [`crate::rescale`]).
/// `strength` must be in `(0, 1]`; the caller is responsible for skipping this
/// function entirely at strength 0 so the off path allocates nothing.
pub fn correct_cfa(
    samples: &mut [f32],
    width: usize,
    height: usize,
    cfa: &CFA,
    strength: f32,
) -> HotPixelReport {
    let strength = strength.clamp(0.0, 1.0);
    let k = K_MAX - (K_MAX - K_MIN) * strength;
    let same_colour = SameColourOffsets::new(cfa);

    // First pass: read the unmodified mosaic and collect the corrections, in
    // row-chunk order so the concatenation is deterministic.
    let per_chunk: Vec<(Vec<Correction>, usize)> = (0..height)
        .into_par_iter()
        .step_by(ROWS_PER_CHUNK)
        .map(|first_row| {
            let last_row = (first_row + ROWS_PER_CHUNK).min(height);
            let mut corrections = Vec::new();
            let mut examined = 0usize;
            let mut neighbours = [0.0_f32; 24];
            for y in first_row..last_row {
                for x in 0..width {
                    let count = gather_same_colour(
                        samples,
                        width,
                        height,
                        &same_colour,
                        x,
                        y,
                        &mut neighbours,
                    );
                    if count < MIN_NEIGHBOURS {
                        continue;
                    }
                    examined += 1;

                    let used = &mut neighbours[..count];
                    // min/max before the sort, median after; total_cmp keeps NaN
                    // out of the ordering path entirely.
                    let mut min = f32::INFINITY;
                    let mut max = f32::NEG_INFINITY;
                    for &value in used.iter() {
                        if value < min {
                            min = value;
                        }
                        if value > max {
                            max = value;
                        }
                    }
                    let value = samples[y * width + x];
                    let scale = (max - min) + FLOOR;
                    let threshold = k * scale;

                    // Sorting for the median was previously unconditional — paid
                    // on every examined site to serve a replacement value that
                    // only the rare correction actually uses. Flamegraph
                    // profiling found this function at ~41% of the whole
                    // program's samples, dominated by exactly this sort, on a
                    // corpus where corrections are a tiny fraction of a percent
                    // of examined sites. Deferring the sort into the two
                    // branches below moves that cost off the common path
                    // without changing which sites get corrected or what they
                    // get replaced with — same slice, same sort, same median.
                    if value - max > threshold {
                        used.sort_unstable_by(f32::total_cmp);
                        corrections.push((y * width + x, used[count / 2], true));
                    } else if min - value > threshold {
                        used.sort_unstable_by(f32::total_cmp);
                        corrections.push((y * width + x, used[count / 2], false));
                    }
                }
            }
            (corrections, examined)
        })
        .collect();

    let mut hot = 0usize;
    let mut dead = 0usize;
    let mut examined = 0usize;
    let mut max_correction = 0.0_f32;
    // Second pass: apply in chunk order. Indices are unique, so order does not
    // affect the result — but a fixed order keeps the report's `max_correction`
    // independent of scheduling too.
    for (corrections, chunk_examined) in &per_chunk {
        examined += *chunk_examined;
        for &(index, replacement, was_hot) in corrections {
            let delta = (samples[index] - replacement).abs();
            if delta > max_correction {
                max_correction = delta;
            }
            samples[index] = replacement;
            if was_hot {
                hot += 1;
            } else {
                dead += 1;
            }
        }
    }

    HotPixelReport {
        strength,
        examined,
        hot,
        dead,
        max_correction,
    }
}

/// Fill `out` with the values of every same-colour neighbour of `(x, y)` inside
/// the search window, returning how many there were.
#[inline]
#[allow(clippy::too_many_arguments)]
fn gather_same_colour(
    samples: &[f32],
    width: usize,
    height: usize,
    same_colour: &SameColourOffsets,
    x: usize,
    y: usize,
    out: &mut [f32; 24],
) -> usize {
    let mut count = 0usize;
    for &(dy, dx) in same_colour.at(y, x) {
        let ny = y as isize + dy as isize;
        if ny < 0 || ny >= height as isize {
            continue;
        }
        let nx = x as isize + dx as isize;
        if nx < 0 || nx >= width as isize {
            continue;
        }
        out[count] = samples[ny as usize * width + nx as usize];
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawler::CFA;

    /// A flat green field on an RGGB mosaic, with one hot and one dead green
    /// site, plus a bright textured edge that must survive.
    fn rggb() -> CFA {
        CFA::new("RGGB")
    }

    fn flat(width: usize, height: usize, level: f32) -> Vec<f32> {
        vec![level; width * height]
    }

    #[test]
    fn an_isolated_hot_green_site_is_replaced_with_the_neighbour_median() {
        let (w, h) = (16, 16);
        let mut samples = flat(w, h, 0.30);
        // (4,4) is green on RGGB (row even, col even is R; row0col1 = G). Pick a
        // known green site: row 0, col 1.
        let (gx, gy) = (5, 4); // row 4 (even) col 5 (odd) -> green on RGGB
        assert_eq!(rggb().cfa_color_at(gy, gx), CFAColor::GREEN);
        samples[gy * w + gx] = 0.95;
        let report = correct_cfa(&mut samples, w, h, &rggb(), 1.0);
        assert_eq!(report.hot, 1);
        assert_eq!(report.dead, 0);
        assert!((samples[gy * w + gx] - 0.30).abs() < 1e-6);
    }

    #[test]
    fn an_isolated_dead_site_is_lifted_to_the_neighbour_median() {
        let (w, h) = (16, 16);
        let mut samples = flat(w, h, 0.60);
        let (rx, ry) = (6, 6); // even/even -> red on RGGB
        assert_eq!(rggb().cfa_color_at(ry, rx), CFAColor::RED);
        samples[ry * w + rx] = 0.02;
        let report = correct_cfa(&mut samples, w, h, &rggb(), 1.0);
        assert_eq!(report.dead, 1);
        assert!((samples[ry * w + rx] - 0.60).abs() < 1e-6);
    }

    #[test]
    fn a_real_edge_is_not_flagged() {
        // Left half dark, right half bright, per same-colour column. The bright
        // pixels sit within their same-colour neighbours' range, so none is an
        // outlier despite the large absolute step.
        let (w, h) = (16, 16);
        let mut samples = vec![0.0_f32; w * h];
        for y in 0..h {
            for x in 0..w {
                samples[y * w + x] = if x >= w / 2 { 0.80 } else { 0.10 };
            }
        }
        let before = samples.clone();
        let report = correct_cfa(&mut samples, w, h, &rggb(), 1.0);
        assert_eq!(report.corrected(), 0);
        assert_eq!(samples, before);
    }

    #[test]
    fn correction_is_deterministic_across_runs() {
        let (w, h) = (64, 48);
        let mut a = vec![0.25_f32; w * h];
        for (i, value) in a.iter_mut().enumerate() {
            // A reproducible sprinkle of spikes.
            if i % 197 == 0 {
                *value = 0.99;
            }
        }
        let mut b = a.clone();
        let ra = correct_cfa(&mut a, w, h, &rggb(), 0.8);
        let rb = correct_cfa(&mut b, w, h, &rggb(), 0.8);
        assert_eq!(a, b);
        assert_eq!(ra.corrected(), rb.corrected());
    }
}
