//! Synthetic camera-RGB fixtures for highlight reconstruction.
//!
//! Per `docs/white-blowout-advice.md` §"Correctness reference": ramps with
//! known chromaticity and increasing intensity, used to catch regime-switch
//! discontinuities without needing a corpus file.

use crate::oklab;
use crate::types::{CameraRgb, Image, LinearImage, SceneLinear, ToneParams};
use rawler::CFA;
use rawler::cfa::CFAColor;
use rawler::imgop::{Dim2, Point, Rect};

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
        let inv = [
            1.0 / wb[0].max(1e-6),
            1.0 / wb[1].max(1e-6),
            1.0 / wb[2].max(1e-6),
        ];
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
        let wb_px = [px[0] * self.wb[0], px[1] * self.wb[1], px[2] * self.wb[2]];
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

/// Largest colour step between adjacent pixels, as Euclidean distance in the
/// OKLab `(a, b)` plane.
///
/// Not a hue step. Hue is an angle about the achromatic axis and is
/// ill-conditioned near it: a reconstruction that walks a sky *through* neutral
/// — which is exactly what a correct one does as the second channel clips —
/// swings the angle by radians while moving the colour by almost nothing. On the
/// `broad_sweep` fixture the largest hue step lands at chroma 0.012, six times
/// the `oklab::CHROMA_FLOOR`, and reports 0.42 rad for a step the eye cannot
/// see. The Cartesian distance is defined through neutral and is what a contour
/// in a smooth gradient actually looks like.
pub fn max_chroma_step(image: &Image<CameraRgb>, wb: [f32; 3]) -> f32 {
    let ab: Vec<(f32, f32)> = image
        .pixels
        .iter()
        .map(|px| {
            let lab = oklab::from_linear_srgb([px[0] * wb[0], px[1] * wb[1], px[2] * wb[2]]);
            (lab.a, lab.b)
        })
        .collect();
    ab.windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .fold(0.0_f32, f32::max)
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

/// Synthetic scene families used by the raw-to-render truth bench.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureKind {
    NeutralRamp,
    BlueRamp,
    BlueToNeutralSky,
    CorrelatedDetail,
    IndependentChannels,
    OccludedSky,
    ColoredHighlights,
    FullyClippedCore,
    SensorKnee,
}

impl FixtureKind {
    pub const ALL: [Self; 9] = [
        Self::NeutralRamp,
        Self::BlueRamp,
        Self::BlueToNeutralSky,
        Self::CorrelatedDetail,
        Self::IndependentChannels,
        Self::OccludedSky,
        Self::ColoredHighlights,
        Self::FullyClippedCore,
        Self::SensorKnee,
    ];
}

/// Absolute, truth-referenced measures from one complete synthetic render.
#[derive(Debug, Clone, Copy)]
pub struct BenchMetrics {
    pub normalized_linear_rmse: f32,
    pub normalized_luminance_rmse: f32,
    pub final_oklab_rmse: f32,
    pub boundary_p99_ab_error: f32,
    pub boundary_p90_hue_error_degrees: f32,
    pub tone_boundary_p99_step: f32,
    pub post_reconstruction_boundary_p99_step: f32,
    pub luminance_reversals: usize,
    pub false_color_fraction: f32,
    pub edge_energy_ratio: f32,
    pub peak_luminance_ratio: f32,
}

const BENCH_WB: [f32; 3] = [2.219, 1.0, 1.773];
const CAMERA_TO_SCENE: crate::color::Matrix3 = [
    [1.08, -0.04, -0.04],
    [-0.03, 1.06, -0.03],
    [-0.02, -0.06, 1.08],
];

#[derive(Clone)]
struct RawFixture {
    width: usize,
    height: usize,
    scene: Vec<[f32; 3]>,
    knee: bool,
}

#[inline]
fn matrix_vector(matrix: &crate::color::Matrix3, value: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|row| {
        matrix[row][0] * value[0] + matrix[row][1] * value[1] + matrix[row][2] * value[2]
    })
}

fn fixture(kind: FixtureKind, width: usize, height: usize) -> RawFixture {
    let mut scene = Vec::with_capacity(width * height);
    let center_x = (width - 1) as f32 * 0.5;
    let center_y = (height - 1) as f32 * 0.45;
    for y in 0..height {
        for x in 0..width {
            let tx = x as f32 / (width - 1).max(1) as f32;
            let ty = y as f32 / (height - 1).max(1) as f32;
            let ramp = 0.42 + 2.55 * tx;
            let blue = [0.42, 0.72, 1.0];
            let pixel = match kind {
                FixtureKind::NeutralRamp => [ramp; 3],
                FixtureKind::BlueRamp => blue.map(|c| c * ramp),
                FixtureKind::BlueToNeutralSky => {
                    let neutral = ((tx - 0.55) / 0.35).clamp(0.0, 1.0);
                    std::array::from_fn(|c| (blue[c] + (1.0 - blue[c]) * neutral) * ramp)
                }
                FixtureKind::CorrelatedDetail => {
                    let detail = 1.0
                        + 0.035 * (x as f32 * 0.31).sin()
                        + 0.018 * (y as f32 * 0.77).cos()
                        + 0.012 * ((x + y) as f32 * 1.9).sin();
                    blue.map(|c| c * ramp * detail)
                }
                FixtureKind::IndependentChannels => [
                    ramp * (0.55 + 0.10 * (x as f32 * 0.73).sin()),
                    ramp * (0.78 + 0.09 * (y as f32 * 0.91).cos()),
                    ramp * (0.94 + 0.08 * ((x + 2 * y) as f32 * 0.63).sin()),
                ],
                FixtureKind::OccludedSky if ty > 0.70 + 0.04 * (x as f32 * 0.2).sin() => {
                    let foliage = 0.11 + 0.05 * ((x * 17 + y * 31) % 13) as f32 / 12.0;
                    [foliage * 0.55, foliage, foliage * 0.38]
                }
                FixtureKind::OccludedSky => blue.map(|c| c * ramp),
                FixtureKind::ColoredHighlights => {
                    let stripe = (x / 16) % 3;
                    let color = match stripe {
                        0 => [1.0, 0.24, 0.18],
                        1 => [0.20, 1.0, 0.30],
                        _ => [0.22, 0.34, 1.0],
                    };
                    color.map(|c| c * (0.5 + 2.2 * tx))
                }
                FixtureKind::FullyClippedCore => {
                    let dx = (x as f32 - center_x) / (width as f32 * 0.28);
                    let dy = (y as f32 - center_y) / (height as f32 * 0.42);
                    let dome = 0.62 + 2.9 * (-(dx * dx + dy * dy)).exp();
                    [dome * 0.92, dome, dome * 0.96]
                }
                FixtureKind::SensorKnee => blue.map(|c| c * ramp),
            };
            scene.push(pixel);
        }
    }
    RawFixture {
        width,
        height,
        scene,
        knee: kind == FixtureKind::SensorKnee,
    }
}

fn scene_to_camera(scene: [f32; 3]) -> [f32; 3] {
    let inverse = crate::color::invert3(CAMERA_TO_SCENE).expect("bench matrix is invertible");
    let neutral = matrix_vector(&inverse, scene);
    std::array::from_fn(|c| neutral[c] / BENCH_WB[c])
}

fn camera_to_scene(camera: [f32; 3]) -> [f32; 3] {
    let neutral = std::array::from_fn(|c| camera[c] * BENCH_WB[c]);
    matrix_vector(&CAMERA_TO_SCENE, neutral)
}

fn sensor_knee(value: f32) -> f32 {
    if value <= 0.80 {
        value
    } else {
        // Analytic monotone shoulder with a known inverse. This fixture keeps
        // the roll-off distinct from the hard clip so the two controls can be
        // judged independently.
        0.80 + 0.65 * (value - 0.80)
    }
    .min(1.0)
}

fn sample_rggb(fixture: &RawFixture) -> (Vec<f32>, Vec<f32>, CFA) {
    let cfa = CFA::new("RGGB");
    let mut truth = Vec::with_capacity(fixture.scene.len());
    let mut captured = Vec::with_capacity(fixture.scene.len());
    for y in 0..fixture.height {
        for x in 0..fixture.width {
            let camera = scene_to_camera(fixture.scene[y * fixture.width + x]);
            let c = match cfa.cfa_color_at(y, x) {
                CFAColor::RED => 0,
                CFAColor::GREEN => 1,
                CFAColor::BLUE => 2,
                _ => unreachable!("RGB Bayer fixture"),
            };
            truth.push(camera[c]);
            captured.push(if fixture.knee {
                sensor_knee(camera[c])
            } else {
                camera[c].min(1.0)
            });
        }
    }
    (truth, captured, cfa)
}

fn full_roi(width: usize, height: usize) -> Rect {
    Rect::new(Point::zero(), Dim2::new(width, height))
}

fn demosaic_scene(samples: &[f32], width: usize, height: usize, cfa: &CFA) -> LinearImage {
    let camera = crate::demosaic::demosaic_bayer(
        samples,
        width,
        height,
        cfa,
        full_roi(width, height),
        crate::demosaic::DemosaicMethod::Rcd,
    );
    let scene = camera.into_iter().map(camera_to_scene).collect();
    Image::<SceneLinear>::new(width, height, scene).expect("fixture dimensions are valid")
}

fn bench_tone_parameters() -> ToneParams {
    ToneParams {
        exposure_ev: -0.35,
        black_input_ev: -8.0,
        white_input_ev: 4.0,
        black_output_linear: 0.001,
        white_output_linear: 0.99,
        black_output_ev: (0.001 / crate::types::MID_GRAY).log2(),
        white_output_ev: (0.99 / crate::types::MID_GRAY).log2(),
        contrast: 1.08,
        shadow_power: 8.0 / (-(0.001 / crate::types::MID_GRAY).log2()),
        highlight_power: 4.0 / (0.99 / crate::types::MID_GRAY).log2(),
        saturation: 1.22,
        vibrance: 0.08,
        highlight_desaturation: 0.16,
        highlight_color_ratio_exponent: 1.0,
        highlight_norm: 1.0,
        noise_floor_ev: None,
    }
}

#[inline]
fn luminance(pixel: [f32; 3]) -> f32 {
    pixel[0] * 0.2126 + pixel[1] * 0.7152 + pixel[2] * 0.0722
}

fn percentile(mut values: Vec<f32>, p: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f32::total_cmp);
    let index = ((values.len() - 1) as f32 * p).round() as usize;
    values[index]
}

fn ab_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let a = oklab::from_linear_srgb(a);
    let b = oklab::from_linear_srgb(b);
    (a.a - b.a).hypot(a.b - b.b)
}

fn boundary_pairs(width: usize, height: usize, state: &[u8]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if x + 1 < width && state[i] != state[i + 1] {
                pairs.push((i, i + 1));
            }
            if y + 1 < height && state[i] != state[i + width] {
                pairs.push((i, i + width));
            }
        }
    }
    pairs
}

/// Run one first-principles fixture from analytical scene values through CFA
/// sampling, clipping, production demosaic/colour conversion, reconstruction,
/// and the renderer-exact tone checkpoints.
pub fn evaluate_raw_to_render(
    kind: FixtureKind,
    method: crate::raw_highlight::HighlightMethod,
) -> BenchMetrics {
    let fixture = fixture(kind, 128, 64);
    let (truth_raw, mut captured, cfa) = sample_rggb(&fixture);
    let confidence: Vec<f32> = captured
        .iter()
        .map(|value| crate::highlight::clip_confidence(*value))
        .collect();
    let propagated = crate::color::propagate_mosaic_confidence(
        &confidence,
        fixture.width,
        fixture.height,
        &cfa,
        full_roi(fixture.width, fixture.height),
        fixture.width,
        fixture.height,
    );
    let state: Vec<u8> = propagated
        .iter()
        .map(|c| c.iter().filter(|value| **value >= 0.5).count() as u8)
        .collect();

    let reference = demosaic_scene(&truth_raw, fixture.width, fixture.height, &cfa);
    let (candidate, uncertainty) = match method {
        crate::raw_highlight::HighlightMethod::Current => {
            let mut camera = crate::demosaic::demosaic_bayer(
                &captured,
                fixture.width,
                fixture.height,
                &cfa,
                full_roi(fixture.width, fixture.height),
                crate::demosaic::DemosaicMethod::Rcd,
            );
            let mut camera_image =
                Image::<CameraRgb>::new(fixture.width, fixture.height, camera).unwrap();
            let (_, uncertainty) = crate::highlight::reconstruct_with_confidence_and_uncertainty(
                &mut camera_image,
                BENCH_WB,
                1.0,
                Some(&propagated),
            );
            camera = camera_image.pixels;
            let scene = camera.into_iter().map(camera_to_scene).collect();
            (
                Image::<SceneLinear>::new(fixture.width, fixture.height, scene).unwrap(),
                uncertainty,
            )
        }
        spatial => {
            crate::raw_highlight::reconstruct_cfa(
                &mut captured,
                fixture.width,
                fixture.height,
                &cfa,
                Some(&confidence),
                BENCH_WB,
                spatial,
            )
            .expect("synthetic CFA is valid");
            let mut candidate = demosaic_scene(&captured, fixture.width, fixture.height, &cfa);
            if matches!(spatial, crate::raw_highlight::HighlightMethod::Harmonic) {
                let _ = crate::highlight::transport_spatial_chromaticity(
                    &mut candidate,
                    Some(&propagated),
                    crate::color::WorkingSpace::Srgb.to_xyz_d65(),
                );
            }
            let uncertainty = propagated
                .iter()
                .map(|c| (c[0] + c[1] + c[2]) / 3.0)
                .collect();
            (candidate, uncertainty)
        }
    };

    let params = bench_tone_parameters();
    let reference_stages = crate::tone::render_checkpoints(&reference, &params, None, None);
    let candidate_stages =
        crate::tone::render_checkpoints(&candidate, &params, Some(&uncertainty), None);
    let pairs = boundary_pairs(fixture.width, fixture.height, &state);

    let normalized_linear_rmse = {
        let mse = candidate
            .pixels
            .iter()
            .zip(&reference.pixels)
            .flat_map(|(candidate, reference)| {
                (0..3).map(move |c| f64::from(candidate[c] - reference[c]).powi(2))
            })
            .sum::<f64>()
            / (candidate.pixels.len() * 3) as f64;
        mse.sqrt() as f32
            / reference
                .pixels
                .iter()
                .flatten()
                .copied()
                .fold(1.0, f32::max)
    };
    let final_oklab_rmse = {
        let mse = candidate_stages
            .iter()
            .zip(&reference_stages)
            .map(|(candidate, reference)| {
                let a = oklab::from_linear_srgb(candidate.gamut_post);
                let b = oklab::from_linear_srgb(reference.gamut_post);
                f64::from((a.l - b.l).powi(2) + (a.a - b.a).powi(2) + (a.b - b.b).powi(2))
            })
            .sum::<f64>()
            / candidate_stages.len() as f64;
        mse.sqrt() as f32
    };
    let normalized_luminance_rmse = {
        let mse = candidate
            .pixels
            .iter()
            .zip(&reference.pixels)
            .map(|(candidate, reference)| {
                f64::from(luminance(*candidate) - luminance(*reference)).powi(2)
            })
            .sum::<f64>()
            / candidate.pixels.len() as f64;
        let scale = reference
            .pixels
            .iter()
            .map(|pixel| luminance(*pixel))
            .fold(1.0_f32, f32::max);
        mse.sqrt() as f32 / scale
    };
    let boundary_p99_ab_error = percentile(
        pairs
            .iter()
            .map(|&(a, b)| {
                let candidate_step = ab_distance(
                    candidate_stages[a].gamut_post,
                    candidate_stages[b].gamut_post,
                );
                let reference_step = ab_distance(
                    reference_stages[a].gamut_post,
                    reference_stages[b].gamut_post,
                );
                (candidate_step - reference_step).abs()
            })
            .collect(),
        0.99,
    );
    let boundary_p90_hue_error_degrees = percentile(
        pairs
            .iter()
            .flat_map(|&(a, b)| [a, b])
            .filter_map(|i| {
                let candidate = oklab::from_linear_srgb(candidate_stages[i].gamut_post);
                let reference = oklab::from_linear_srgb(reference_stages[i].gamut_post);
                (reference.chroma() >= 0.02)
                    .then(|| oklab::hue_difference(candidate, reference))
                    .flatten()
                    .map(f32::to_degrees)
            })
            .collect(),
        0.90,
    );
    let post_reconstruction_boundary_p99_step = percentile(
        pairs
            .iter()
            .map(|&(a, b)| ab_distance(candidate.pixels[a], candidate.pixels[b]))
            .collect(),
        0.99,
    );
    let tone_boundary_p99_step = percentile(
        pairs
            .iter()
            .map(|&(a, b)| {
                ab_distance(
                    candidate_stages[a].after_curve,
                    candidate_stages[b].after_curve,
                )
            })
            .collect(),
        0.99,
    );
    let luminance_reversals = candidate_stages
        .chunks(fixture.width)
        .map(|row| {
            row.windows(2)
                .filter(|window| {
                    luminance(window[1].gamut_post) + 1.0e-4 < luminance(window[0].gamut_post)
                })
                .count()
        })
        .sum();
    let false_color_fraction = candidate_stages
        .iter()
        .zip(&reference_stages)
        .filter(|(candidate, reference)| {
            ab_distance(candidate.gamut_post, reference.gamut_post) > 0.02
        })
        .count() as f32
        / candidate_stages.len() as f32;
    let edge_energy = |stages: &[crate::tone::RenderPixelStages]| -> f64 {
        stages
            .chunks(fixture.width)
            .flat_map(|row| row.windows(2))
            .map(|window| f64::from(ab_distance(window[0].gamut_post, window[1].gamut_post)))
            .sum()
    };
    let edge_energy_ratio =
        (edge_energy(&candidate_stages) / edge_energy(&reference_stages).max(1.0e-8)) as f32;
    let peak_luminance = |image: &LinearImage| {
        image
            .pixels
            .iter()
            .map(|pixel| luminance(*pixel))
            .fold(0.0_f32, f32::max)
    };
    let peak_luminance_ratio = peak_luminance(&candidate) / peak_luminance(&reference).max(1.0e-8);

    BenchMetrics {
        normalized_linear_rmse,
        normalized_luminance_rmse,
        final_oklab_rmse,
        boundary_p99_ab_error,
        boundary_p90_hue_error_degrees,
        tone_boundary_p99_step,
        post_reconstruction_boundary_p99_step,
        luminance_reversals,
        false_color_fraction,
        edge_energy_ratio,
        peak_luminance_ratio,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const WB: [f32; 3] = [2.219, 1.0, 1.773];

    /// A smooth intensity ramp must reconstruct to a smooth colour ramp: no
    /// contour where the clipped-channel count changes.
    ///
    /// The threshold has room on both sides. Reconstruction driven by
    /// `near_whiteness` measures 0.005 on `blue_sky` and 0.007 on `broad_sweep`;
    /// the counted rule it replaced measured 0.032 and 0.089 on the same two
    /// fixtures, so this fails by a factor of two even on the milder of them.
    fn assert_continuous_ramp(chroma: Chromaticity, label: &str) {
        let ramp = ramp_image(chroma, 256, 0.85, 1.45);
        let hist = clip_histogram(&ramp);
        assert!(
            hist[1] > 0 && hist[2] > 0,
            "{label}: ramp must contain 1- and 2-clipped pixels, got {hist:?} base {:?}",
            chroma.base
        );
        let recon = reconstructed_copy(&ramp, WB, 0.75);
        let max_step = max_chroma_step(&recon, WB);
        assert!(
            max_step < 0.015,
            "{label}: max adjacent colour step {max_step:.4} in OKLab (a, b) exceeds \
             the continuity threshold; hist {hist:?}"
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
        assert!(
            hist[0] > 0 && (hist[1] + hist[2] + hist[3]) > 0,
            "neutral must span unclipped→clipped, got {hist:?}"
        );
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
            assert!(
                lum + 1e-6 >= prev,
                "neutral luminance regressed {lum} < {prev}"
            );
            prev = lum;
        }
    }

    #[test]
    fn blue_sky_constant_chromaticity_ramp_is_continuous() {
        let c = Chromaticity::blue_sky(WB);
        assert_continuous_ramp(c, "blue_sky");
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
        assert_continuous_ramp(c, "broad_sweep");
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
            pixels.push([
                foliage_cam[0] * 0.25,
                foliage_cam[1] * 0.25,
                foliage_cam[2] * 0.25,
            ]);
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

    #[test]
    fn raw_to_render_bench_covers_every_first_principles_fixture() {
        for kind in FixtureKind::ALL {
            let metrics =
                evaluate_raw_to_render(kind, crate::raw_highlight::HighlightMethod::Current);
            assert!(metrics.normalized_linear_rmse.is_finite(), "{kind:?}");
            assert!(metrics.final_oklab_rmse.is_finite(), "{kind:?}");
            assert!(metrics.boundary_p99_ab_error.is_finite(), "{kind:?}");
            assert!(metrics.edge_energy_ratio.is_finite(), "{kind:?}");
        }
    }

    #[test]
    fn spatial_methods_are_measured_after_the_production_tone_shoulder() {
        for method in [
            crate::raw_highlight::HighlightMethod::RawPyramid,
            crate::raw_highlight::HighlightMethod::Harmonic,
        ] {
            for kind in [FixtureKind::BlueRamp, FixtureKind::BlueToNeutralSky] {
                let metrics = evaluate_raw_to_render(kind, method);
                eprintln!("{method:?} {kind:?}: {metrics:?}");
                if method == crate::raw_highlight::HighlightMethod::RawPyramid
                    || kind == FixtureKind::BlueRamp
                {
                    assert!(metrics.normalized_linear_rmse < 0.25, "{method:?} {kind:?}");
                }
                assert!(metrics.final_oklab_rmse < 0.12, "{method:?} {kind:?}");
                let boundary_limit = match method {
                    crate::raw_highlight::HighlightMethod::RawPyramid => 0.015,
                    crate::raw_highlight::HighlightMethod::Harmonic => 0.01,
                    crate::raw_highlight::HighlightMethod::Current => unreachable!(),
                };
                assert!(
                    metrics.boundary_p99_ab_error <= boundary_limit,
                    "{method:?} {kind:?}: {metrics:?}"
                );
                if method == crate::raw_highlight::HighlightMethod::Harmonic {
                    assert!(
                        metrics.boundary_p90_hue_error_degrees <= 5.0,
                        "{method:?} {kind:?}: {metrics:?}"
                    );
                }
                assert!(
                    metrics.tone_boundary_p99_step
                        <= (1.5 * metrics.post_reconstruction_boundary_p99_step).max(0.03),
                    "tone shoulder amplified {method:?} {kind:?}: {metrics:?}"
                );
            }
        }
    }

    #[test]
    fn clipped_samples_do_not_encode_post_clip_chromaticity() {
        let blue = fixture(FixtureKind::BlueRamp, 128, 64);
        let varying = fixture(FixtureKind::BlueToNeutralSky, 128, 64);
        let (blue_truth, blue_captured, _) = sample_rggb(&blue);
        let (varying_truth, varying_captured, _) = sample_rggb(&varying);

        let ambiguous_sites = blue_captured
            .iter()
            .zip(&varying_captured)
            .zip(blue_truth.iter().zip(&varying_truth))
            .filter(|((blue_observed, varying_observed), (blue, varying))| {
                **blue_observed == 1.0
                    && **varying_observed == 1.0
                    && (**blue - **varying).abs() > 0.05
            })
            .count();
        assert!(
            ambiguous_sites > 500,
            "the fixture must contain many clipped sites with different hidden truths, got {ambiguous_sites}"
        );
    }

    #[test]
    fn harmonic_bench_exposes_absolute_quality_across_fixture_families() {
        for kind in FixtureKind::ALL {
            let metrics =
                evaluate_raw_to_render(kind, crate::raw_highlight::HighlightMethod::Harmonic);
            eprintln!("Harmonic {kind:?}: {metrics:?}");
            assert!(metrics.normalized_linear_rmse.is_finite());
            assert!(metrics.final_oklab_rmse.is_finite());
            assert!(metrics.boundary_p99_ab_error.is_finite());
            assert!(metrics.boundary_p90_hue_error_degrees.is_finite());
            match kind {
                FixtureKind::NeutralRamp
                | FixtureKind::BlueRamp
                | FixtureKind::CorrelatedDetail => {
                    assert!(
                        metrics.normalized_linear_rmse <= 0.03,
                        "linear truth gate failed for {kind:?}: {metrics:?}"
                    );
                    assert!(
                        metrics.final_oklab_rmse <= 0.02,
                        "rendered truth gate failed for {kind:?}: {metrics:?}"
                    );
                }
                FixtureKind::BlueToNeutralSky => {
                    assert!(metrics.boundary_p99_ab_error <= 0.01, "{metrics:?}");
                    assert!(metrics.boundary_p90_hue_error_degrees <= 5.0, "{metrics:?}");
                    assert!(metrics.normalized_luminance_rmse <= 0.10, "{metrics:?}");
                    assert!(metrics.peak_luminance_ratio <= 1.20, "{metrics:?}");
                    assert!(metrics.tone_boundary_p99_step <= 0.03, "{metrics:?}");
                }
                FixtureKind::IndependentChannels => {
                    assert!(metrics.edge_energy_ratio <= 1.10, "{metrics:?}");
                    assert!(metrics.peak_luminance_ratio <= 1.10, "{metrics:?}");
                }
                FixtureKind::OccludedSky => {
                    assert!(metrics.edge_energy_ratio <= 1.10, "{metrics:?}");
                }
                FixtureKind::ColoredHighlights => {
                    assert!(metrics.peak_luminance_ratio <= 1.0, "{metrics:?}");
                }
                FixtureKind::FullyClippedCore => {
                    assert!(metrics.normalized_luminance_rmse <= 0.05, "{metrics:?}");
                    assert!(
                        (0.8..=1.2).contains(&metrics.peak_luminance_ratio),
                        "{metrics:?}"
                    );
                    assert!(metrics.boundary_p99_ab_error <= 0.01, "{metrics:?}");
                }
                FixtureKind::SensorKnee => {
                    assert!(metrics.final_oklab_rmse <= 0.02, "{metrics:?}");
                }
            }
        }
    }
}
