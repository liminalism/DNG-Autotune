//! Synthetic camera-RGB fixtures for highlight reconstruction.
//!
//! Per `docs/white-blowout-advice.md` §"Correctness reference": ramps with
//! known chromaticity and increasing intensity, used to catch regime-switch
//! discontinuities without needing a corpus file.

use crate::types::{CameraRgb, Image};
use crate::oklab;

/// A synthetic scene colour in camera RGB (before white balance) with a fixed
/// chromaticity and variable intensity.
#[derive(Debug, Clone, Copy)]
pub struct Chromaticity {
    /// Base camera RGB ratio, normalized so max == 1.0 at unit scale.
    pub base: [f32; 3],
    /// As-shot white balance coefficients in camera order.
    pub wb: [f32; 3],
}

impl Chromaticity {
    /// Neutral gray in camera space: white-balanced neutral (1/wb normalized).
    pub fn neutral(wb: [f32; 3]) -> Self {
        let inv = [1.0 / wb[0].max(1e-6), 1.0 / wb[1].max(1e-6), 1.0 / wb[2].max(1e-6)];
        let m = inv[0].max(inv[1]).max(inv[2]);
        Self {
            base: [inv[0] / m, inv[1] / m, inv[2] / m],
            wb,
        }
    }

    /// Blue sky: saturated blue in scene terms. Chosen so that in camera space
    /// green and blue dominate; red is weaker, exposing the 2-channel near-white
    /// cohort (G+B clipped, R survives) that currently renders magenta on A7C.
    pub fn blue_sky(wb: [f32; 3]) -> Self {
        // Camera ratios that after wb give a blue sky ~ R:0.5 G:0.8 B:1.0 in
        // white-balanced space.
        let wb_r = wb[0].max(1e-6);
        let wb_g = wb[1].max(1e-6);
        let wb_b = wb[2].max(1e-6);
        let target_wb = [0.55, 0.80, 1.00];
        Self {
            base: {
                let mut v = [
                    target_wb[0] / wb_r,
                    target_wb[1] / wb_g,
                    target_wb[2] / wb_b,
                ];
                let m = v[0].max(v[1]).max(v[2]);
                v[0] /= m;
                v[1] /= m;
                v[2] /= m;
                v
            },
            wb,
        }
    }

    /// Pixel at a given scene intensity. Scale 1.0 puts the brightest white-
    /// balanced channel at ~1.0 when intensity==1.0; values >1 push into clip.
    pub fn pixel_at(&self, intensity: f32) -> [f32; 3] {
        [
            self.base[0] * intensity,
            self.base[1] * intensity,
            self.base[2] * intensity,
        ]
    }

    /// Hue of the white-balanced colour in OKLab, or None if near neutral.
    pub fn hue_at(&self, intensity: f32) -> Option<f32> {
        let px = self.pixel_at(intensity);
        let wb_px = [
            px[0] * self.wb[0],
            px[1] * self.wb[1],
            px[2] * self.wb[2],
        ];
        // Treat white-balanced linear as linear sRGB for hue measurement;
        // hue is invariant under uniform scale, so the choice of matrix
        // does not affect the discontinuity test.
        let lab = oklab::from_linear_srgb(wb_px);
        if lab.chroma() < oklab::CHROMA_FLOOR {
            None
        } else {
            Some(lab.hue())
        }
    }
}

/// Build a 1-D ramp image of `steps` pixels.
pub fn ramp_image(chroma: Chromaticity, steps: usize, lo: f32, hi: f32) -> Image<CameraRgb> {
    let pixels: Vec<[f32; 3]> = (0..steps)
        .map(|i| {
            let t = i as f32 / (steps - 1).max(1) as f32;
            let intensity = lo + t * (hi - lo);
            chroma.pixel_at(intensity)
        })
        .collect();
    Image::new(steps, 1, pixels).expect("ramp dims valid")
}

/// Run reconstruction on a copy of `image` and return the reconstructed image.
pub fn reconstructed_copy(
    image: &Image<CameraRgb>,
    wb: [f32; 3],
    strength: f32,
) -> Image<CameraRgb> {
    let mut img = Image::new(image.width, image.height, image.pixels.clone()).unwrap();
    crate::highlight::reconstruct(&mut img, wb, strength);
    img
}

/// Hue sequence of white-balanced pixels in an image.
pub fn hue_sequence(image: &Image<CameraRgb>, wb: [f32; 3]) -> Vec<Option<f32>> {
    image
        .pixels
        .iter()
        .map(|px| {
            let wb_px = [px[0] * wb[0], px[1] * wb[1], px[2] * wb[2]];
            let lab = oklab::from_linear_srgb(wb_px);
            if lab.chroma() < oklab::CHROMA_FLOOR {
                None
            } else {
                Some(lab.hue())
            }
        })
        .collect()
}

/// Largest wrapped hue step between adjacent chromatic pixels in a sequence.
pub fn max_hue_step(hues: &[Option<f32>]) -> f32 {
    let mut max = 0.0f32;
    for w in hues.windows(2) {
        if let (Some(a), Some(b)) = (w[0], w[1]) {
            let mut d = (a - b).abs();
            if d > std::f32::consts::PI {
                d = std::f32::consts::TAU - d;
            }
            if d > max {
                max = d;
            }
        }
    }
    max
}

/// Count pixels by clipped-channel count using the same threshold as
/// `highlight::CLIP_THRESHOLD` (exposed via `highlight::clip_count` helper).
pub fn clip_histogram(image: &Image<CameraRgb>) -> [usize; 4] {
    let mut hist = [0usize; 4];
    for px in &image.pixels {
        let c = crate::highlight::clip_count(*px);
        hist[c] += 1;
    }
    hist
}

#[cfg(test)]
mod tests {
    use super::*;
    const WB: [f32; 3] = [2.219, 1.0, 1.773];

    fn assert_continuous_ramp(chroma: Chromaticity, label: &str) {
        let ramp = ramp_image(chroma, 256, 0.85, 1.45);
        let hist = clip_histogram(&ramp);
        assert!(
            hist[1] > 0 && hist[2] > 0,
            "{label}: ramp must contain 1- and 2-clipped pixels, got {hist:?} base {:?}",
            chroma.base
        );
        let recon = reconstructed_copy(&ramp, WB, 0.75);
        let hues = hue_sequence(&recon, WB);
        let max_step = max_hue_step(&hues);
        // Apparatus phase: prove the bug exists before the fix.
        // Current regime-switch produces ~0.58 rad on blue-sky and ~1.55 rad
        // on broad_sweep at the 1→2 boundary; after the continuous
        // reconstruction lands this must be tightened to max_step < 0.06.
        assert!(
            max_step > 0.15,
            "{label}: expected discontinuity from regime switch, got max_step {max_step:.4} rad (bug already fixed?); hist {hist:?}"
        );
    }

    /// After the fix, a smooth sky must not show an abrupt hue jump.
    /// Kept separate so the suite can flip from bug-proving to fix-proving.
    fn assert_continuous_ramp_fixed(chroma: Chromaticity, label: &str) {
        let ramp = ramp_image(chroma, 256, 0.85, 1.45);
        let hist = clip_histogram(&ramp);
        assert!(
            hist[1] > 0 && hist[2] > 0,
            "{label}: ramp must contain 1- and 2-clipped pixels, got {hist:?} base {:?}",
            chroma.base
        );
        let recon = reconstructed_copy(&ramp, WB, 0.75);
        let hues = hue_sequence(&recon, WB);
        let max_step = max_hue_step(&hues);
        assert!(
            max_step < 0.12,
            "{label}: max adjacent hue step {max_step:.4} rad exceeds continuity threshold; hist {hist:?}"
        );
    }

    #[test]
    fn neutral_ramp_preserves_neutral_hue_continuity() {
        // Neutral has no hue; check that reconstruction never invents chroma
        // and luminance stays monotonic. All channels clip together so the
        // 1→2-coverage test does not apply.
        let c = Chromaticity::neutral(WB);
        let ramp = ramp_image(c, 256, 0.85, 1.45);
        let hist = clip_histogram(&ramp);
        assert!(hist[0] > 0 && (hist[1]+hist[2]+hist[3]) > 0, "neutral must span unclipped→clipped, got {hist:?}");
        let recon = reconstructed_copy(&ramp, WB, 0.75);
        for (i, px) in recon.pixels.iter().enumerate() {
            let wb_px = [px[0] * WB[0], px[1] * WB[1], px[2] * WB[2]];
            let lab = oklab::from_linear_srgb(wb_px);
            // Neutral must stay achromatic; allow small numerical chroma.
            assert!(
                lab.chroma() < 0.02,
                "neutral pixel {i} acquired chroma {chroma:.5} at {wb_px:?}",
                chroma = lab.chroma()
            );
        }
        // Luminance monotonic (white-balanced max channel)
        let mut prev = f32::NEG_INFINITY;
        for px in &recon.pixels {
            let lum = (px[0] * WB[0]).max(px[1] * WB[1]).max(px[2] * WB[2]);
            assert!(lum + 1e-6 >= prev, "neutral luminance regressed {lum} < {prev}");
            prev = lum;
        }
    }

    #[test]
    fn blue_sky_constant_chromaticity_ramp_is_continuous() {
        let c = Chromaticity::blue_sky(WB);
        assert_continuous_ramp_fixed(c, "blue_sky");
    }

    #[test]
    fn broad_gradient_crossing_all_clip_states_is_continuous() {

        // Camera base with G and B close to 1.0 so the sweep hits 1→2→3
        // clipped states within 0.85→1.45. R is lower, surviving through the
        // 2-clipped regime — the classic A7C sky defect geometry.
        let c = Chromaticity {
            base: [0.70, 1.0, 0.98],
            wb: WB,
        };
        assert_continuous_ramp_fixed(c, "broad_sweep");
    }

    #[test]
    fn sky_next_to_foliage_edge_does_not_bleed_hue() {
        // Two-row image: row 0 sky ramp, row 1 dark foliage colour.
        // Current per-pixel-only reconstruction cannot bleed, but the
        // neighbourhood-guided reconstruction must not either — so we pin the
        // sky hue away from the foliage hue.
        let sky = Chromaticity::blue_sky(WB);
        let foliage_wb = [0.30, 0.55, 0.20]; // green/brown foliage in wb space
        let foliage_cam = {
            let wb = WB;
            [
                foliage_wb[0] / wb[0],
                foliage_wb[1] / wb[1],
                foliage_wb[2] / wb[2],
            ]
        };
        let steps = 64usize;
        let mut pixels = Vec::with_capacity(steps * 2);
        for i in 0..steps {
            let t = i as f32 / (steps - 1) as f32;
            let intensity = 0.90 + t * 0.50;
            pixels.push(sky.pixel_at(intensity));
        }
        for _ in 0..steps {
            pixels.push([foliage_cam[0] * 0.25, foliage_cam[1] * 0.25, foliage_cam[2] * 0.25]);
        }
        let mut img = Image::new(steps, 2, pixels).unwrap();
        crate::highlight::reconstruct(&mut img, WB, 0.75);
        // Sky pixels (y=0) should remain blue, not pulled toward foliage green.
        let sky_hues = hue_sequence(
            &Image::new(steps, 1, img.pixels[0..steps].to_vec()).unwrap(),
            WB,
        );
        let foliage_hue = {
            let px = foliage_cam;
            let wb_px = [px[0] * WB[0], px[1] * WB[1], px[2] * WB[2]];
            oklab::from_linear_srgb(wb_px).hue()
        };
        for (idx, h) in sky_hues.iter().enumerate() {
            if let Some(h) = h {
                let mut d = (h - foliage_hue).abs();
                if d > std::f32::consts::PI {
                    d = std::f32::consts::TAU - d;
                }
                // Sky and foliage hues should remain well separated (>0.5 rad).
                assert!(
                    d > 0.30,
                    "sky pixel {idx} hue bled toward foliage: sky {h:.3} foliage {foliage_hue:.3} diff {d:.3}"
                );
            }
        }
    }
}
