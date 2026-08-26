//! Optional scene-perception evidence and explicitly requested experiments.
//!
//! The default path is downstream-only: it observes the scene-linear image
//! after chroma denoise without changing exposure, colour, or output pixels.
//! Experimental consumers must be explicitly enabled, preserve the legacy
//! controller decision, and record the spatial policy they applied.

use crate::analyze::luminance;
use crate::color::Matrix3;
use crate::types::{LinearImage, MID_GRAY};
use anyhow::{Context, Result, anyhow, ensure};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const PROXY_SIZE: usize = 512;
pub const DEFAULT_MODEL_DIR: &str = "models/artifacts";
const MASK_THRESHOLD: u8 = 128;
const SKY_POLICY_MIN_AREA: f32 = 0.05;
const SKY_POLICY_MIN_MEAN_CONFIDENCE: f32 = 0.75;
const SKY_POLICY_MIN_CLIPPED_FRACTION: f32 = 0.10;
const SKY_CONFIDENCE_START: f32 = 0.75;
const SKY_CONFIDENCE_FULL: f32 = 0.95;
const SKY_HIGHLIGHT_START: f32 = 0.75;
const SKY_HIGHLIGHT_FULL: f32 = 1.25;
const SKY_MAX_COMPRESSION_EV: f32 = 0.35;
const SKY_MAGENTA_A_START: f32 = 0.005;
const SKY_MAGENTA_A_FULL: f32 = 0.015;
const SKY_MAX_MAGENTA_REDUCTION: f32 = 0.80;

/// Fixed, neutral sRGB view used only for perception.
#[derive(Debug, Clone)]
pub struct SemanticProxy {
    pub width: usize,
    pub height: usize,
    pub content_x: usize,
    pub content_y: usize,
    pub content_width: usize,
    pub content_height: usize,
    pub source_width: usize,
    pub source_height: usize,
    pub rgb: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticProxyInfo {
    pub width: usize,
    pub height: usize,
    pub content_x: usize,
    pub content_y: usize,
    pub content_width: usize,
    pub content_height: usize,
    pub source_width: usize,
    pub source_height: usize,
    pub rendering: &'static str,
}

impl From<&SemanticProxy> for SemanticProxyInfo {
    fn from(proxy: &SemanticProxy) -> Self {
        Self {
            width: proxy.width,
            height: proxy.height,
            content_x: proxy.content_x,
            content_y: proxy.content_y,
            content_width: proxy.content_width,
            content_height: proxy.content_height,
            source_width: proxy.source_width,
            source_height: proxy.source_height,
            rendering: "fixed-neutral-srgb-v1",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionKind {
    Face,
    Person,
    Sky,
    Vegetation,
    Water,
    SnowOrSand,
    BuildingOrInterior,
    TextOrDocument,
    SalientForeground,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionMask {
    pub kind: RegionKind,
    pub width: usize,
    pub height: usize,
    pub threshold: f32,
    #[serde(skip)]
    pub confidence: Vec<u8>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RegionMasks {
    pub width: usize,
    pub height: usize,
    pub masks: Vec<RegionMask>,
}

impl RegionMasks {
    fn blank() -> Self {
        let kinds = [
            RegionKind::Face,
            RegionKind::Person,
            RegionKind::Sky,
            RegionKind::Vegetation,
            RegionKind::Water,
            RegionKind::SnowOrSand,
            RegionKind::BuildingOrInterior,
            RegionKind::TextOrDocument,
            RegionKind::SalientForeground,
        ];
        let len = PROXY_SIZE * PROXY_SIZE;
        Self {
            width: PROXY_SIZE,
            height: PROXY_SIZE,
            masks: kinds
                .into_iter()
                .map(|kind| RegionMask {
                    kind,
                    width: PROXY_SIZE,
                    height: PROXY_SIZE,
                    threshold: MASK_THRESHOLD as f32 / 255.0,
                    confidence: vec![0; len],
                })
                .collect(),
        }
    }

    fn get_mut(&mut self, kind: RegionKind) -> &mut RegionMask {
        self.masks
            .iter_mut()
            .find(|mask| mask.kind == kind)
            .expect("all RegionKind masks are allocated")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionStats {
    pub kind: RegionKind,
    pub area_fraction: f32,
    pub mean_confidence: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p10_ev: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50_ev: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p90_ev: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clipped_fraction: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconstruction_uncertainty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_chroma: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noise_snr10_ev: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharpness: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelProvenance {
    pub role: String,
    pub path: String,
    pub sha256: String,
    pub input_name: String,
    pub input_shape: Vec<usize>,
    pub outputs: Vec<String>,
    pub preprocessing: String,
    pub backend: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SceneEvidence {
    pub proxy: SemanticProxyInfo,
    pub embedded_preview_semantic_eligible: bool,
    pub models: Vec<ModelProvenance>,
    /// Mean softmax confidence for every native Cityscapes class. These are raw
    /// model outputs, not a winner-take-all scene label.
    pub raw_scores: BTreeMap<String, f32>,
    pub regions: Vec<RegionStats>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inference_errors: Vec<String>,
    /// Empty for observation-only runs. Explicit policy experiments append an
    /// explanation here whenever they consume semantic evidence.
    pub policy_adjustments: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sky_highlight_adjustment: Option<SkyHighlightAdjustmentReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sky_chroma_adjustment: Option<SkyChromaAdjustmentReport>,
    pub masks: RegionMasks,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyHighlightAdjustmentReport {
    pub version: &'static str,
    pub strength: f32,
    pub eligible: bool,
    pub sky_area_fraction: f32,
    pub sky_mean_confidence: f32,
    pub sky_clipped_fraction: f32,
    pub confidence_start: f32,
    pub confidence_full: f32,
    pub highlight_start: f32,
    pub highlight_full: f32,
    pub max_compression_ev: f32,
    pub affected_pixels: usize,
    pub affected_fraction: f32,
    pub correction_min_ev: f32,
    pub correction_mean_abs_ev: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyChromaAdjustmentReport {
    pub version: &'static str,
    pub strength: f32,
    pub eligible: bool,
    pub correction_axis: &'static str,
    pub preserves: [&'static str; 2],
    pub sky_area_fraction: f32,
    pub sky_mean_confidence: f32,
    pub sky_clipped_fraction: f32,
    pub magenta_a_start: f32,
    pub magenta_a_full: f32,
    pub max_magenta_reduction: f32,
    pub affected_pixels: usize,
    pub affected_fraction: f32,
    pub mean_positive_a_before: f32,
    pub mean_a_reduction: f32,
}

/// Full-resolution scene-linear EV corrections derived from the dense sky
/// confidence plane. One scalar is applied to all three channels at each pixel.
pub struct SkyHighlightMap {
    width: usize,
    height: usize,
    corrections_ev: Vec<f32>,
    report: SkyHighlightAdjustmentReport,
}

/// Full-resolution weights for the opt-in sky chroma experiment. The semantic
/// label locates pixels only; the direction comes from measured positive Oklab
/// `a` (magenta), never from an assumed neutral sky white point.
pub struct SkyChromaMap {
    width: usize,
    height: usize,
    weights: Vec<f32>,
    working_to_display: Option<Matrix3>,
    display_to_working: Option<Matrix3>,
    report: SkyChromaAdjustmentReport,
}

impl SkyChromaMap {
    pub fn report(&self) -> &SkyChromaAdjustmentReport {
        &self.report
    }

    pub fn weights(&self) -> &[f32] {
        &self.weights
    }

    /// Reduce only the measured magenta opponent component. Oklab `b/L` is
    /// held fixed so natural sky blue is not neutralized, and linear luminance
    /// is restored exactly before conversion back to the working space.
    pub fn apply(&self, image: &mut LinearImage) -> Result<()> {
        ensure!(
            image.width == self.width
                && image.height == self.height
                && image.pixels.len() == self.weights.len(),
            "sky chroma map must match the image it adjusts"
        );
        image
            .pixels
            .par_iter_mut()
            .zip(self.weights.par_iter())
            .for_each(|(pixel, weight)| {
                if *weight == 0.0 {
                    return;
                }
                let display = matrix_pixel(self.working_to_display.as_ref(), *pixel);
                let before_y = luminance(display);
                let lab = crate::oklab::from_linear_srgb(display);
                if lab.a <= 0.0 {
                    return;
                }
                let mut corrected = crate::oklab::to_linear_srgb(crate::oklab::Oklab {
                    l: lab.l,
                    a: lab.a * (1.0 - SKY_MAX_MAGENTA_REDUCTION * *weight),
                    b: lab.b,
                });
                let after_y = luminance(corrected);
                if before_y.is_finite() && after_y.is_finite() && after_y.abs() > 1.0e-8 {
                    let gain = before_y / after_y;
                    corrected.iter_mut().for_each(|channel| *channel *= gain);
                }
                *pixel = matrix_pixel(self.display_to_working.as_ref(), corrected);
            });
        Ok(())
    }
}

impl SkyHighlightMap {
    pub fn report(&self) -> &SkyHighlightAdjustmentReport {
        &self.report
    }

    pub fn corrections_ev(&self) -> &[f32] {
        &self.corrections_ev
    }

    /// Apply the already measured map without feeding it back into global
    /// exposure analysis. Scaling RGB together preserves scene chromaticity.
    pub fn apply(&self, image: &mut LinearImage) -> Result<()> {
        ensure!(
            image.width == self.width
                && image.height == self.height
                && image.pixels.len() == self.corrections_ev.len(),
            "sky highlight map must match the image it adjusts"
        );
        image
            .pixels
            .par_iter_mut()
            .zip(self.corrections_ev.par_iter())
            .for_each(|(pixel, correction_ev)| {
                if *correction_ev == 0.0 {
                    return;
                }
                let gain = correction_ev.exp2();
                pixel.iter_mut().for_each(|channel| *channel *= gain);
            });
        Ok(())
    }
}

#[inline]
fn matrix_pixel(matrix: Option<&Matrix3>, pixel: [f32; 3]) -> [f32; 3] {
    match matrix {
        Some(m) => [
            m[0][0] * pixel[0] + m[0][1] * pixel[1] + m[0][2] * pixel[2],
            m[1][0] * pixel[0] + m[1][1] * pixel[1] + m[1][2] * pixel[2],
            m[2][0] * pixel[0] + m[2][1] * pixel[1] + m[2][2] * pixel[2],
        ],
        None => pixel,
    }
}

#[inline]
fn neutral_map(value: f32) -> f32 {
    // Non-clipping global shoulder with middle grey at display-linear 0.5.
    // It is fixed rather than derived from the frame, so model input does not
    // inherit any controller decision that semantics may later help make.
    let value = value.max(0.0);
    value / (value + MID_GRAY)
}

fn bilinear(image: &LinearImage, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0.0, image.width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, image.height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(image.width - 1);
    let y1 = (y0 + 1).min(image.height - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let a = image.pixels[y0 * image.width + x0];
    let b = image.pixels[y0 * image.width + x1];
    let c = image.pixels[y1 * image.width + x0];
    let d = image.pixels[y1 * image.width + x1];
    let mut out = [0.0; 3];
    for channel in 0..3 {
        let top = a[channel] + (b[channel] - a[channel]) * fx;
        let bottom = c[channel] + (d[channel] - c[channel]) * fx;
        out[channel] = top + (bottom - top) * fy;
    }
    out
}

pub fn build_proxy(
    image: &LinearImage,
    working_to_display: Option<&Matrix3>,
) -> Result<SemanticProxy> {
    ensure!(
        image.width > 0 && image.height > 0,
        "semantic proxy source is empty"
    );
    let (content_width, content_height) = if image.width >= image.height {
        (
            PROXY_SIZE,
            ((image.height * PROXY_SIZE + image.width / 2) / image.width).max(1),
        )
    } else {
        (
            ((image.width * PROXY_SIZE + image.height / 2) / image.height).max(1),
            PROXY_SIZE,
        )
    };
    let content_x = (PROXY_SIZE - content_width) / 2;
    let content_y = (PROXY_SIZE - content_height) / 2;
    let mut rgb = vec![0_u8; PROXY_SIZE * PROXY_SIZE * 3];

    for dy in 0..content_height {
        let sy = ((dy as f32 + 0.5) * image.height as f32 / content_height as f32 - 0.5)
            .clamp(0.0, image.height.saturating_sub(1) as f32);
        for dx in 0..content_width {
            let sx = ((dx as f32 + 0.5) * image.width as f32 / content_width as f32 - 0.5)
                .clamp(0.0, image.width.saturating_sub(1) as f32);
            let pixel = matrix_pixel(working_to_display, bilinear(image, sx, sy));
            let offset = ((content_y + dy) * PROXY_SIZE + content_x + dx) * 3;
            for channel in 0..3 {
                let encoded = crate::tone::srgb_encode(neutral_map(pixel[channel]));
                rgb[offset + channel] = (encoded * 255.0 + 0.5) as u8;
            }
        }
    }

    Ok(SemanticProxy {
        width: PROXY_SIZE,
        height: PROXY_SIZE,
        content_x,
        content_y,
        content_width,
        content_height,
        source_width: image.width,
        source_height: image.height,
        rgb,
    })
}

/// Run the optional perception stack. Errors are evidence rather than render
/// errors: model absence or an unsupported graph falls back to the unchanged
/// controller and is recorded for corpus diagnosis.
pub fn observe(
    image: &LinearImage,
    working_to_display: Option<&Matrix3>,
    reconstruction_uncertainty: Option<&[f32]>,
    noise_snr10_ev: Option<f32>,
    embedded_preview_semantic_eligible: bool,
    model_dir: &Path,
    faces: bool,
) -> Result<(SemanticProxy, SceneEvidence)> {
    let proxy = build_proxy(image, working_to_display)?;
    let mut evidence = SceneEvidence {
        proxy: SemanticProxyInfo::from(&proxy),
        embedded_preview_semantic_eligible,
        models: Vec::new(),
        raw_scores: BTreeMap::new(),
        regions: Vec::new(),
        inference_errors: Vec::new(),
        policy_adjustments: Vec::new(),
        sky_highlight_adjustment: None,
        sky_chroma_adjustment: None,
        masks: RegionMasks::blank(),
    };

    infer(&proxy, model_dir, faces, &mut evidence);

    evidence.regions = measure_regions(
        image,
        working_to_display,
        reconstruction_uncertainty,
        noise_snr10_ev,
        &proxy,
        &evidence.masks,
    );
    Ok((proxy, evidence))
}

#[inline]
fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

#[inline]
fn mask_confidence_at_source(
    mask: &RegionMask,
    proxy: &SemanticProxy,
    source_x: usize,
    source_y: usize,
) -> f32 {
    // Invert build_proxy's pixel-centre transform. Clamp to content pixels,
    // never the black letterbox, so interpolation cannot leak through padding.
    let proxy_x = proxy.content_x as f32
        + (source_x as f32 + 0.5) * proxy.content_width as f32 / proxy.source_width as f32
        - 0.5;
    let proxy_y = proxy.content_y as f32
        + (source_y as f32 + 0.5) * proxy.content_height as f32 / proxy.source_height as f32
        - 0.5;
    let min_x = proxy.content_x as f32;
    let min_y = proxy.content_y as f32;
    let max_x = (proxy.content_x + proxy.content_width - 1) as f32;
    let max_y = (proxy.content_y + proxy.content_height - 1) as f32;
    let proxy_x = proxy_x.clamp(min_x, max_x);
    let proxy_y = proxy_y.clamp(min_y, max_y);
    let x0 = proxy_x.floor() as usize;
    let y0 = proxy_y.floor() as usize;
    let x1 = (x0 + 1).min(proxy.content_x + proxy.content_width - 1);
    let y1 = (y0 + 1).min(proxy.content_y + proxy.content_height - 1);
    let fx = proxy_x - x0 as f32;
    let fy = proxy_y - y0 as f32;
    let sample = |x: usize, y: usize| mask.confidence[y * mask.width + x] as f32 / 255.0;
    let top = sample(x0, y0) + (sample(x1, y0) - sample(x0, y0)) * fx;
    let bottom = sample(x0, y1) + (sample(x1, y1) - sample(x0, y1)) * fx;
    top + (bottom - top) * fy
}

/// Build the opt-in sky highlight experiment from the actual dense mask.
///
/// The aggregate region measurements are only a false-positive gate. Every
/// correction below is independently weighted by the full spatial confidence
/// mask and by that pixel's highlight/reconstruction evidence.
pub fn build_sky_highlight_map(
    image: &LinearImage,
    working_to_display: Option<&Matrix3>,
    reconstruction_uncertainty: Option<&[f32]>,
    proxy: &SemanticProxy,
    evidence: &SceneEvidence,
    strength: f32,
) -> Result<SkyHighlightMap> {
    ensure!(
        strength.is_finite() && (0.0..=1.0).contains(&strength),
        "semantic sky highlight strength must be between 0 and 1"
    );
    ensure!(
        strength > 0.0,
        "semantic sky highlight strength must be above zero"
    );
    ensure!(
        image.width == proxy.source_width && image.height == proxy.source_height,
        "semantic proxy must describe the image adjusted by its mask"
    );
    ensure!(
        reconstruction_uncertainty.is_none_or(|map| map.len() == image.pixels.len()),
        "highlight uncertainty map must match the semantic source image"
    );
    let sky_mask = evidence
        .masks
        .masks
        .iter()
        .find(|mask| mask.kind == RegionKind::Sky)
        .context("scene evidence has no sky confidence mask")?;
    let sky_stats = evidence
        .regions
        .iter()
        .find(|region| region.kind == RegionKind::Sky)
        .context("scene evidence has no sky region statistics")?;
    let sky_clipped_fraction = sky_stats.clipped_fraction.unwrap_or(0.0);
    let eligible = sky_stats.area_fraction >= SKY_POLICY_MIN_AREA
        && sky_stats.mean_confidence >= SKY_POLICY_MIN_MEAN_CONFIDENCE
        && sky_clipped_fraction >= SKY_POLICY_MIN_CLIPPED_FRACTION;

    let mut corrections_ev = vec![0.0_f32; image.pixels.len()];
    if eligible {
        corrections_ev
            .par_iter_mut()
            .enumerate()
            .for_each(|(index, correction)| {
                let source_x = index % image.width;
                let source_y = index / image.width;
                let confidence = mask_confidence_at_source(sky_mask, proxy, source_x, source_y);
                let sky_weight = smoothstep(
                    (confidence - SKY_CONFIDENCE_START)
                        / (SKY_CONFIDENCE_FULL - SKY_CONFIDENCE_START),
                );
                if sky_weight == 0.0 {
                    return;
                }
                let display = matrix_pixel(working_to_display, image.pixels[index]);
                let maximum = display.into_iter().fold(f32::NEG_INFINITY, f32::max);
                let brightness_weight = smoothstep(
                    (maximum - SKY_HIGHLIGHT_START) / (SKY_HIGHLIGHT_FULL - SKY_HIGHLIGHT_START),
                );
                let uncertainty_weight = reconstruction_uncertainty
                    .and_then(|map| map.get(index))
                    .map_or(0.0, |value| smoothstep(*value / 0.5));
                let highlight_weight = brightness_weight.max(uncertainty_weight);
                *correction = -SKY_MAX_COMPRESSION_EV * strength * sky_weight * highlight_weight;
            });
    }

    let affected_pixels = corrections_ev
        .iter()
        .filter(|value| **value < -1.0e-6)
        .count();
    let correction_min_ev = corrections_ev.iter().copied().fold(0.0_f32, f32::min);
    let correction_mean_abs_ev = corrections_ev
        .iter()
        .map(|value| value.abs() as f64)
        .sum::<f64>() as f32
        / corrections_ev.len().max(1) as f32;
    let pixel_count = corrections_ev.len().max(1);
    Ok(SkyHighlightMap {
        width: image.width,
        height: image.height,
        corrections_ev,
        report: SkyHighlightAdjustmentReport {
            version: "sky-highlight-luma-v1",
            strength,
            eligible,
            sky_area_fraction: sky_stats.area_fraction,
            sky_mean_confidence: sky_stats.mean_confidence,
            sky_clipped_fraction,
            confidence_start: SKY_CONFIDENCE_START,
            confidence_full: SKY_CONFIDENCE_FULL,
            highlight_start: SKY_HIGHLIGHT_START,
            highlight_full: SKY_HIGHLIGHT_FULL,
            max_compression_ev: SKY_MAX_COMPRESSION_EV,
            affected_pixels,
            affected_fraction: affected_pixels as f32 / pixel_count as f32,
            correction_min_ev,
            correction_mean_abs_ev,
        },
    })
}

/// Build a bounded chromatic correction from measured sky pixels.
///
/// This is intentionally not sky white balance: sky confidence contributes
/// only a spatial feather. A pixel must independently contain highlight or
/// reconstruction evidence and a positive Oklab `a` component before it moves.
pub fn build_sky_chroma_map(
    image: &LinearImage,
    working_to_display: Option<&Matrix3>,
    reconstruction_uncertainty: Option<&[f32]>,
    proxy: &SemanticProxy,
    evidence: &SceneEvidence,
    strength: f32,
) -> Result<SkyChromaMap> {
    ensure!(
        strength.is_finite() && (0.0..=1.0).contains(&strength) && strength > 0.0,
        "semantic sky chroma strength must be above zero and at most one"
    );
    ensure!(
        image.width == proxy.source_width && image.height == proxy.source_height,
        "semantic proxy must describe the image adjusted by its mask"
    );
    ensure!(
        reconstruction_uncertainty.is_none_or(|map| map.len() == image.pixels.len()),
        "highlight uncertainty map must match the semantic source image"
    );
    let sky_mask = evidence
        .masks
        .masks
        .iter()
        .find(|mask| mask.kind == RegionKind::Sky)
        .context("scene evidence has no sky confidence mask")?;
    let sky_stats = evidence
        .regions
        .iter()
        .find(|region| region.kind == RegionKind::Sky)
        .context("scene evidence has no sky region statistics")?;
    let sky_clipped_fraction = sky_stats.clipped_fraction.unwrap_or(0.0);
    let eligible = sky_stats.area_fraction >= SKY_POLICY_MIN_AREA
        && sky_stats.mean_confidence >= SKY_POLICY_MIN_MEAN_CONFIDENCE
        && sky_clipped_fraction >= SKY_POLICY_MIN_CLIPPED_FRACTION;
    let display_to_working = working_to_display
        .copied()
        .map(|matrix| crate::color::invert3(matrix).context("working/display matrix is singular"))
        .transpose()?;

    let mut weights = vec![0.0_f32; image.pixels.len()];
    if eligible {
        weights
            .par_iter_mut()
            .enumerate()
            .for_each(|(index, weight)| {
                let source_x = index % image.width;
                let source_y = index / image.width;
                let confidence = mask_confidence_at_source(sky_mask, proxy, source_x, source_y);
                let sky_weight = smoothstep(
                    (confidence - SKY_CONFIDENCE_START)
                        / (SKY_CONFIDENCE_FULL - SKY_CONFIDENCE_START),
                );
                if sky_weight == 0.0 {
                    return;
                }
                let display = matrix_pixel(working_to_display, image.pixels[index]);
                let maximum = display.into_iter().fold(f32::NEG_INFINITY, f32::max);
                let brightness_weight = smoothstep(
                    (maximum - SKY_HIGHLIGHT_START) / (SKY_HIGHLIGHT_FULL - SKY_HIGHLIGHT_START),
                );
                let uncertainty_weight = reconstruction_uncertainty
                    .and_then(|map| map.get(index))
                    .map_or(0.0, |value| smoothstep(*value / 0.5));
                let evidence_weight = brightness_weight.max(uncertainty_weight);
                let magenta_a = crate::oklab::from_linear_srgb(display).a;
                let magenta_weight = smoothstep(
                    (magenta_a - SKY_MAGENTA_A_START) / (SKY_MAGENTA_A_FULL - SKY_MAGENTA_A_START),
                );
                *weight = strength * sky_weight * evidence_weight * magenta_weight;
            });
    }

    let mut affected_pixels = 0usize;
    let mut positive_a = 0.0_f64;
    let mut a_reduction = 0.0_f64;
    for (index, weight) in weights.iter().copied().enumerate() {
        if weight <= 1.0e-6 {
            continue;
        }
        affected_pixels += 1;
        let display = matrix_pixel(working_to_display, image.pixels[index]);
        let a = crate::oklab::from_linear_srgb(display).a.max(0.0);
        positive_a += a as f64;
        a_reduction += (a * SKY_MAX_MAGENTA_REDUCTION * weight) as f64;
    }
    let affected_denominator = affected_pixels.max(1) as f64;
    Ok(SkyChromaMap {
        width: image.width,
        height: image.height,
        weights,
        working_to_display: working_to_display.copied(),
        display_to_working,
        report: SkyChromaAdjustmentReport {
            version: "sky-magenta-opponent-v1",
            strength,
            eligible,
            correction_axis: "positive_oklab_a",
            preserves: ["linear_luminance", "oklab_b_over_l"],
            sky_area_fraction: sky_stats.area_fraction,
            sky_mean_confidence: sky_stats.mean_confidence,
            sky_clipped_fraction,
            magenta_a_start: SKY_MAGENTA_A_START,
            magenta_a_full: SKY_MAGENTA_A_FULL,
            max_magenta_reduction: SKY_MAX_MAGENTA_REDUCTION,
            affected_pixels,
            affected_fraction: affected_pixels as f32 / image.pixels.len().max(1) as f32,
            mean_positive_a_before: (positive_a / affected_denominator) as f32,
            mean_a_reduction: (a_reduction / affected_denominator) as f32,
        },
    })
}

/// Whether the face detector runs at all.
///
/// **Off.** The YuNet path is wired, works, and has never produced a true
/// positive: over the 74-image day/night/extreme-bright corpus it returned zero
/// faces, and the corpus's only person response was a low-confidence false
/// positive on a faucet still life
/// (`raw-autotune.assessment.scene-evidence-usefulness`). Nothing consumes the
/// mask — no exposure policy, no encoder ROI — so running it spent an ONNX
/// session per frame to write an empty plane.
///
/// It is gated rather than deleted because the model and its provenance are
/// prepared and pinned in `models/manifest.json`, and the honest reading of the
/// corpus result is "this detector, at this 512 px letterboxed proxy, on this
/// corpus" rather than "face detection is impossible here". `--semantic-faces`
/// turns it back on for anyone re-opening that.
///
/// `RegionMasks::blank()` already seeds an empty `Face` plane, so a run with the
/// detector off has the same sidecar *shape* as one with it on — one fewer
/// entry in `models`, and a `Face` mask that stays blank.
pub const FACE_DETECTION_DEFAULT: bool = false;

fn infer(proxy: &SemanticProxy, model_dir: &Path, faces: bool, evidence: &mut SceneEvidence) {
    match infer_segmenter(proxy, model_dir) {
        Ok((masks, scores, provenance)) => {
            evidence.models.push(provenance);
            evidence.raw_scores = scores;
            merge_masks(&mut evidence.masks, masks);
        }
        Err(error) => evidence
            .inference_errors
            .push(format!("segmenter: {error:#}")),
    }
    if !faces {
        return;
    }
    match infer_faces(proxy, model_dir) {
        Ok((face, provenance)) => {
            evidence.models.push(provenance);
            *evidence.masks.get_mut(RegionKind::Face) = face;
        }
        Err(error) => evidence.inference_errors.push(format!("face: {error:#}")),
    }
}

pub(crate) fn model_provenance(
    path: &Path,
    role: &str,
    preprocessing: &str,
    backend: &str,
    session: &lege_gpu::vision::OnnxSession,
) -> Result<ModelProvenance> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let input_shape = session
        .input_shape()
        .context("prepared model has no static input shape")?
        .iter()
        .map(|value| usize::try_from(*value).context("negative model input dimension"))
        .collect::<Result<Vec<_>>>()?;
    Ok(ModelProvenance {
        role: role.to_string(),
        path: path.to_string_lossy().into_owned(),
        sha256,
        input_name: session.input_name().to_string(),
        input_shape,
        outputs: session.output_names().to_vec(),
        preprocessing: preprocessing.to_string(),
        backend: backend.to_string(),
    })
}

fn run_preferred(
    session: &lege_gpu::vision::OnnxSession,
    input: &lege_gpu::vision::Tensor,
) -> Result<(
    std::collections::HashMap<String, lege_gpu::vision::Tensor>,
    String,
)> {
    match lege_gpu::compute::SharedGpuContext::get() {
        Ok(context) if context.adapter_info().is_hardware_gpu() => {
            let adapter = context.adapter_info();
            match session.run_gpu(input) {
                Ok(outputs) => Ok((
                    outputs,
                    format!("lege-gpu-wgpu:{:?}:{}", adapter.backend, adapter.name),
                )),
                Err(gpu_error) => {
                    let outputs = session.run_cpu(input).with_context(|| {
                        format!("GPU inference failed ({gpu_error:#}); CPU fallback also failed")
                    })?;
                    Ok((
                        outputs,
                        format!(
                            "lege-gpu-cpu-reference; GPU fallback from {:?}:{} ({gpu_error})",
                            adapter.backend, adapter.name
                        ),
                    ))
                }
            }
        }
        Ok(context) => {
            let adapter = context.adapter_info();
            Ok((
                session.run_cpu(input)?,
                format!(
                    "lege-gpu-cpu-reference; non-hardware adapter {:?}:{}",
                    adapter.backend, adapter.name
                ),
            ))
        }
        Err(error) => Ok((
            session.run_cpu(input)?,
            format!("lege-gpu-cpu-reference; GPU unavailable ({error})"),
        )),
    }
}

fn imagenet_tensor(proxy: &SemanticProxy) -> Result<lege_gpu::vision::Tensor> {
    let plane = PROXY_SIZE * PROXY_SIZE;
    let mut data = vec![0.0_f32; plane * 3];
    let mean = [0.485_f32, 0.456, 0.406];
    let std = [0.229_f32, 0.224, 0.225];
    for index in 0..plane {
        for channel in 0..3 {
            let value = proxy.rgb[index * 3 + channel] as f32 / 255.0;
            data[channel * plane + index] = (value - mean[channel]) / std[channel];
        }
    }
    lege_gpu::vision::Tensor::new(vec![1, 3, PROXY_SIZE, PROXY_SIZE], data)
}

fn infer_segmenter(
    proxy: &SemanticProxy,
    model_dir: &Path,
) -> Result<(RegionMasks, BTreeMap<String, f32>, ModelProvenance)> {
    use lege_gpu::vision::OnnxSession;
    let path = model_dir.join("lraspp_mnv3_cityscapes.prepared.onnx");
    let session = OnnxSession::from_path(&path)?;
    let input = imagenet_tensor(proxy)?;
    let (outputs, backend) = run_preferred(&session, &input)?;
    let provenance = model_provenance(
        &path,
        "cityscapes-segmenter",
        "rgb-u8/255-imagenet-mean-std-v1",
        &backend,
        &session,
    )?;
    let logits = outputs
        .get("logits")
        .context("segmenter did not return logits")?;
    ensure!(
        logits.shape == [1, 19, PROXY_SIZE, PROXY_SIZE],
        "unexpected segmenter output shape {:?}",
        logits.shape
    );

    const NAMES: [&str; 19] = [
        "road",
        "sidewalk",
        "building",
        "wall",
        "fence",
        "pole",
        "traffic_light",
        "traffic_sign",
        "vegetation",
        "terrain",
        "sky",
        "person",
        "rider",
        "car",
        "truck",
        "bus",
        "train",
        "motorcycle",
        "bicycle",
    ];
    let plane = PROXY_SIZE * PROXY_SIZE;
    let mut sums = [0.0_f64; 19];
    let mut masks = RegionMasks::blank();
    let content_x1 = proxy.content_x + proxy.content_width;
    let content_y1 = proxy.content_y + proxy.content_height;

    for y in proxy.content_y..content_y1 {
        for x in proxy.content_x..content_x1 {
            let index = y * PROXY_SIZE + x;
            let max = (0..19)
                .map(|class| logits.data[class * plane + index])
                .fold(f32::NEG_INFINITY, f32::max);
            let denominator: f32 = (0..19)
                .map(|class| (logits.data[class * plane + index] - max).exp())
                .sum();
            let mut probabilities = [0.0_f32; 19];
            for class in 0..19 {
                probabilities[class] =
                    (logits.data[class * plane + index] - max).exp() / denominator;
                sums[class] += probabilities[class] as f64;
            }
            set_confidence(&mut masks, RegionKind::Sky, index, probabilities[10]);
            set_confidence(&mut masks, RegionKind::Vegetation, index, probabilities[8]);
            set_confidence(
                &mut masks,
                RegionKind::Person,
                index,
                probabilities[11].max(probabilities[12]),
            );
            set_confidence(
                &mut masks,
                RegionKind::BuildingOrInterior,
                index,
                probabilities[2].max(probabilities[3]).max(probabilities[4]),
            );
            // Cityscapes `terrain` is not evidence of snow or sand, so that
            // RegionMask stays empty rather than receiving a convenient but
            // semantically false remap. Water and text are likewise absent.
            let foreground = probabilities[5..8]
                .iter()
                .chain(probabilities[11..].iter())
                .copied()
                .fold(0.0_f32, f32::max);
            set_confidence(&mut masks, RegionKind::SalientForeground, index, foreground);
        }
    }
    let content_pixels = (proxy.content_width * proxy.content_height) as f64;
    let scores = NAMES
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name.to_string(), (sums[index] / content_pixels) as f32))
        .collect();
    Ok((masks, scores, provenance))
}

fn set_confidence(masks: &mut RegionMasks, kind: RegionKind, index: usize, value: f32) {
    masks.get_mut(kind).confidence[index] = (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
}

fn merge_masks(target: &mut RegionMasks, source: RegionMasks) {
    for source_mask in source.masks {
        let target_mask = target.get_mut(source_mask.kind);
        for (target, source) in target_mask
            .confidence
            .iter_mut()
            .zip(source_mask.confidence)
        {
            *target = (*target).max(source);
        }
    }
}

fn infer_faces(proxy: &SemanticProxy, model_dir: &Path) -> Result<(RegionMask, ModelProvenance)> {
    use lege_gpu::vision::{OnnxSession, Tensor};
    let path = model_dir.join("yunet.prepared.onnx");
    let session = OnnxSession::from_path(&path)?;
    let plane = PROXY_SIZE * PROXY_SIZE;
    let mut data = vec![0.0_f32; plane * 3];
    for index in 0..plane {
        data[index] = proxy.rgb[index * 3 + 2] as f32;
        data[plane + index] = proxy.rgb[index * 3 + 1] as f32;
        data[2 * plane + index] = proxy.rgb[index * 3] as f32;
    }
    let input = Tensor::new(vec![1, 3, PROXY_SIZE, PROXY_SIZE], data)?;
    let (outputs, backend) = run_preferred(&session, &input)?;
    let provenance = model_provenance(&path, "face-detector", "bgr-u8-v1", &backend, &session)?;
    let mut mask = RegionMask {
        kind: RegionKind::Face,
        width: PROXY_SIZE,
        height: PROXY_SIZE,
        threshold: MASK_THRESHOLD as f32 / 255.0,
        confidence: vec![0; plane],
    };

    for stride in [8_usize, 16, 32] {
        let suffix = stride.to_string();
        let cls = outputs
            .get(&format!("cls_{suffix}"))
            .with_context(|| format!("YuNet missing cls_{suffix}"))?;
        let obj = outputs
            .get(&format!("obj_{suffix}"))
            .with_context(|| format!("YuNet missing obj_{suffix}"))?;
        let bbox = outputs
            .get(&format!("bbox_{suffix}"))
            .with_context(|| format!("YuNet missing bbox_{suffix}"))?;
        let grid = PROXY_SIZE / stride;
        ensure!(cls.data.len() == grid * grid, "unexpected YuNet cls shape");
        ensure!(obj.data.len() == grid * grid, "unexpected YuNet obj shape");
        ensure!(
            bbox.data.len() == grid * grid * 4,
            "unexpected YuNet bbox shape"
        );
        for index in 0..grid * grid {
            let score = (cls.data[index].clamp(0.0, 1.0) * obj.data[index].clamp(0.0, 1.0)).sqrt();
            if score < 0.80 {
                continue;
            }
            let cx = (index % grid) as f32 * stride as f32 + stride as f32 * 0.5;
            let cy = (index / grid) as f32 * stride as f32 + stride as f32 * 0.5;
            let box_offset = index * 4;
            let x0 = (cx - bbox.data[box_offset] * stride as f32)
                .floor()
                .clamp(0.0, (PROXY_SIZE - 1) as f32) as usize;
            let y0 = (cy - bbox.data[box_offset + 1] * stride as f32)
                .floor()
                .clamp(0.0, (PROXY_SIZE - 1) as f32) as usize;
            let x1 = (cx + bbox.data[box_offset + 2] * stride as f32)
                .ceil()
                .clamp(0.0, PROXY_SIZE as f32) as usize;
            let y1 = (cy + bbox.data[box_offset + 3] * stride as f32)
                .ceil()
                .clamp(0.0, PROXY_SIZE as f32) as usize;
            let confidence = (score * 255.0 + 0.5) as u8;
            for y in y0..y1 {
                for x in x0..x1 {
                    let value = &mut mask.confidence[y * PROXY_SIZE + x];
                    *value = (*value).max(confidence);
                }
            }
        }
    }
    Ok((mask, provenance))
}

fn source_index(proxy: &SemanticProxy, x: usize, y: usize) -> usize {
    let sx = ((x - proxy.content_x) * proxy.source_width / proxy.content_width)
        .min(proxy.source_width - 1);
    let sy = ((y - proxy.content_y) * proxy.source_height / proxy.content_height)
        .min(proxy.source_height - 1);
    sy * proxy.source_width + sx
}

fn quantile(sorted: &[f32], q: f32) -> f32 {
    let position = q * (sorted.len() - 1) as f32;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let fraction = position - lower as f32;
    sorted[lower] * (1.0 - fraction) + sorted[upper] * fraction
}

fn measure_regions(
    image: &LinearImage,
    working_to_display: Option<&Matrix3>,
    reconstruction_uncertainty: Option<&[f32]>,
    noise_snr10_ev: Option<f32>,
    proxy: &SemanticProxy,
    masks: &RegionMasks,
) -> Vec<RegionStats> {
    let content_area = (proxy.content_width * proxy.content_height).max(1);
    masks
        .masks
        .iter()
        .map(|mask| {
            let mut ev = Vec::new();
            let mut confidence_sum = 0.0_f64;
            let mut clipped = 0usize;
            let mut chroma = 0.0_f64;
            let mut uncertainty = 0.0_f64;
            let mut uncertainty_n = 0usize;
            let mut sharpness = 0.0_f64;
            for y in proxy.content_y..proxy.content_y + proxy.content_height {
                for x in proxy.content_x..proxy.content_x + proxy.content_width {
                    let mask_index = y * PROXY_SIZE + x;
                    let confidence = mask.confidence[mask_index];
                    if confidence < MASK_THRESHOLD {
                        continue;
                    }
                    confidence_sum += confidence as f64 / 255.0;
                    let index = source_index(proxy, x, y);
                    let pixel = matrix_pixel(working_to_display, image.pixels[index]);
                    let y_linear = luminance(pixel).max(1.0e-8);
                    ev.push((y_linear / MID_GRAY).log2());
                    if pixel.iter().any(|channel| *channel >= 0.995) {
                        clipped += 1;
                    }
                    let max = pixel.into_iter().fold(f32::NEG_INFINITY, f32::max);
                    let min = pixel.into_iter().fold(f32::INFINITY, f32::min);
                    chroma += (max - min).max(0.0) as f64;
                    if let Some(map) = reconstruction_uncertainty
                        && let Some(value) = map.get(index)
                    {
                        uncertainty += *value as f64;
                        uncertainty_n += 1;
                    }
                    if index % image.width + 1 < image.width {
                        let neighbour = matrix_pixel(working_to_display, image.pixels[index + 1]);
                        sharpness += (luminance(pixel) - luminance(neighbour)).abs() as f64;
                    }
                }
            }
            ev.sort_unstable_by(f32::total_cmp);
            let count = ev.len();
            RegionStats {
                kind: mask.kind,
                area_fraction: count as f32 / content_area as f32,
                mean_confidence: if count == 0 {
                    0.0
                } else {
                    (confidence_sum / count as f64) as f32
                },
                p10_ev: (!ev.is_empty()).then(|| quantile(&ev, 0.10)),
                p50_ev: (!ev.is_empty()).then(|| quantile(&ev, 0.50)),
                p90_ev: (!ev.is_empty()).then(|| quantile(&ev, 0.90)),
                clipped_fraction: (count > 0).then_some(clipped as f32 / count as f32),
                reconstruction_uncertainty: (uncertainty_n > 0)
                    .then_some((uncertainty / uncertainty_n as f64) as f32),
                mean_chroma: (count > 0).then_some((chroma / count as f64) as f32),
                noise_snr10_ev,
                sharpness: (count > 0).then_some((sharpness / count as f64) as f32),
            }
        })
        .collect()
}

pub fn dump(
    directory: &Path,
    input: &Path,
    proxy: &SemanticProxy,
    masks: &RegionMasks,
) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".to_string());
    let rgb = image::RgbImage::from_raw(proxy.width as u32, proxy.height as u32, proxy.rgb.clone())
        .ok_or_else(|| anyhow!("invalid semantic proxy buffer"))?;
    rgb.save(directory.join(format!("{stem}-semantic-proxy.png")))?;

    let palette = [
        [255_u8, 96, 96],
        [255, 160, 64],
        [64, 160, 255],
        [64, 220, 96],
        [64, 220, 220],
        [240, 240, 240],
        [190, 130, 255],
        [255, 220, 64],
        [255, 64, 200],
    ];
    let mut overlay = rgb;
    for (mask, color) in masks.masks.iter().zip(palette) {
        for (index, confidence) in mask.confidence.iter().copied().enumerate() {
            if confidence < MASK_THRESHOLD {
                continue;
            }
            let alpha = confidence as f32 / 255.0 * 0.48;
            let pixel =
                overlay.get_pixel_mut((index % PROXY_SIZE) as u32, (index / PROXY_SIZE) as u32);
            for channel in 0..3 {
                pixel[channel] = (pixel[channel] as f32 * (1.0 - alpha)
                    + color[channel] as f32 * alpha
                    + 0.5) as u8;
            }
        }
    }
    overlay.save(directory.join(format!("{stem}-semantic-overlay.png")))?;
    Ok(())
}

/// Write a full-resolution 8-bit visualization of the policy weight.
/// Black is untouched; white is the configured maximum compression.
pub fn dump_sky_highlight_map(directory: &Path, input: &Path, map: &SkyHighlightMap) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".to_string());
    let pixels: Vec<u8> = map
        .corrections_ev
        .iter()
        .map(|correction| {
            ((-*correction / SKY_MAX_COMPRESSION_EV).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
        })
        .collect();
    let image = image::GrayImage::from_raw(map.width as u32, map.height as u32, pixels)
        .ok_or_else(|| anyhow!("invalid full-resolution sky policy buffer"))?;
    image.save(directory.join(format!("{stem}-semantic-sky-policy.png")))?;
    Ok(())
}

/// Write a full-resolution 8-bit visualization of the sky chroma weight.
pub fn dump_sky_chroma_map(directory: &Path, input: &Path, map: &SkyChromaMap) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".to_string());
    let pixels: Vec<u8> = map
        .weights
        .iter()
        .map(|weight| (weight.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        .collect();
    let image = image::GrayImage::from_raw(map.width as u32, map.height as u32, pixels)
        .ok_or_else(|| anyhow!("invalid full-resolution sky chroma policy buffer"))?;
    image.save(directory.join(format!("{stem}-semantic-sky-chroma.png")))?;
    Ok(())
}

pub fn model_dir(path: &Path) -> PathBuf {
    if path.as_os_str().is_empty() {
        PathBuf::from(DEFAULT_MODEL_DIR)
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: usize, height: usize, value: [f32; 3]) -> LinearImage {
        LinearImage::new(width, height, vec![value; width * height]).unwrap()
    }

    fn eligible_evidence(proxy: &SemanticProxy) -> SceneEvidence {
        let mut masks = RegionMasks::blank();
        let sky = masks.get_mut(RegionKind::Sky);
        for y in proxy.content_y..proxy.content_y + proxy.content_height {
            for x in proxy.content_x..proxy.content_x + proxy.content_width {
                sky.confidence[y * PROXY_SIZE + x] = 255;
            }
        }
        SceneEvidence {
            proxy: SemanticProxyInfo::from(proxy),
            embedded_preview_semantic_eligible: false,
            models: Vec::new(),
            raw_scores: BTreeMap::new(),
            regions: vec![RegionStats {
                kind: RegionKind::Sky,
                area_fraction: 0.5,
                mean_confidence: 1.0,
                p10_ev: Some(1.0),
                p50_ev: Some(2.0),
                p90_ev: Some(3.0),
                clipped_fraction: Some(0.5),
                reconstruction_uncertainty: Some(0.2),
                mean_chroma: Some(0.1),
                noise_snr10_ev: None,
                sharpness: Some(0.01),
            }],
            inference_errors: Vec::new(),
            policy_adjustments: Vec::new(),
            sky_highlight_adjustment: None,
            sky_chroma_adjustment: None,
            masks,
        }
    }

    #[test]
    fn landscape_proxy_is_letterboxed_to_exactly_512_square() {
        let proxy = build_proxy(&image(400, 200, [0.18; 3]), None).unwrap();
        assert_eq!((proxy.width, proxy.height), (512, 512));
        assert_eq!((proxy.content_width, proxy.content_height), (512, 256));
        assert_eq!((proxy.content_x, proxy.content_y), (0, 128));
        assert!(proxy.rgb[..128 * 512 * 3].iter().all(|value| *value == 0));
    }

    #[test]
    fn portrait_proxy_preserves_aspect_ratio_and_centres_content() {
        let proxy = build_proxy(&image(200, 400, [0.18; 3]), None).unwrap();
        assert_eq!((proxy.content_width, proxy.content_height), (256, 512));
        assert_eq!((proxy.content_x, proxy.content_y), (128, 0));
    }

    #[test]
    fn neutral_map_is_fixed_and_does_not_clip_highlights() {
        assert!((neutral_map(MID_GRAY) - 0.5).abs() < 1.0e-6);
        assert!(neutral_map(100.0) < 1.0);
        assert!(neutral_map(100.0) > neutral_map(1.0));
    }

    #[test]
    fn sky_policy_inverts_letterbox_and_uses_the_spatial_mask() {
        let source = image(8, 4, [1.5, 0.75, 0.375]);
        let proxy = build_proxy(&source, None).unwrap();
        let mut evidence = eligible_evidence(&proxy);
        let sky = evidence.masks.get_mut(RegionKind::Sky);
        // Deliberately poison the padding. Correct inverse sampling must never
        // allow it to affect a source pixel.
        sky.confidence.fill(255);
        for y in proxy.content_y..proxy.content_y + proxy.content_height {
            for x in proxy.content_x..proxy.content_x + proxy.content_width {
                sky.confidence[y * PROXY_SIZE + x] =
                    if x < proxy.content_x + proxy.content_width / 2 {
                        255
                    } else {
                        0
                    };
            }
        }

        let map = build_sky_highlight_map(&source, None, None, &proxy, &evidence, 1.0).unwrap();
        for y in 0..source.height {
            for x in 0..source.width {
                let correction = map.corrections_ev()[y * source.width + x];
                if x < source.width / 2 {
                    assert!(
                        correction < 0.0,
                        "left sky pixel ({x}, {y}) was not adjusted"
                    );
                } else {
                    assert_eq!(correction, 0.0, "right non-sky pixel ({x}, {y}) moved");
                }
            }
        }
    }

    #[test]
    fn sky_policy_requires_pixel_confidence_and_highlight_evidence() {
        let mut source = image(3, 1, [1.5, 1.0, 0.5]);
        source.pixels[2] = [0.2, 0.15, 0.1];
        let proxy = build_proxy(&source, None).unwrap();
        let mut evidence = eligible_evidence(&proxy);
        let sky = evidence.masks.get_mut(RegionKind::Sky);
        for y in proxy.content_y..proxy.content_y + proxy.content_height {
            for x in proxy.content_x..proxy.content_x + proxy.content_width {
                let source_band = (x - proxy.content_x) * 3 / proxy.content_width;
                sky.confidence[y * PROXY_SIZE + x] = if source_band == 1 { 180 } else { 255 };
            }
        }
        let map = build_sky_highlight_map(&source, None, None, &proxy, &evidence, 1.0).unwrap();
        assert!(map.corrections_ev()[0] < 0.0);
        assert_eq!(map.corrections_ev()[1], 0.0);
        assert_eq!(map.corrections_ev()[2], 0.0);
        assert_eq!(map.report().affected_pixels, 1);
    }

    #[test]
    fn sky_policy_is_bounded_deterministic_and_preserves_chromaticity() {
        let source = image(4, 2, [1.5, 0.75, 0.375]);
        let proxy = build_proxy(&source, None).unwrap();
        let evidence = eligible_evidence(&proxy);
        let first = build_sky_highlight_map(&source, None, None, &proxy, &evidence, 1.0).unwrap();
        let second = build_sky_highlight_map(&source, None, None, &proxy, &evidence, 1.0).unwrap();
        assert_eq!(first.corrections_ev(), second.corrections_ev());
        assert!(
            first
                .corrections_ev()
                .iter()
                .all(|value| (-SKY_MAX_COMPRESSION_EV..=0.0).contains(value))
        );

        let before = source.pixels[0];
        let mut adjusted = source;
        first.apply(&mut adjusted).unwrap();
        let after = adjusted.pixels[0];
        assert!(after[0] < before[0]);
        assert!((before[0] * after[1] - before[1] * after[0]).abs() < 1.0e-6);
        assert!((before[0] * after[2] - before[2] * after[0]).abs() < 1.0e-6);
    }

    #[test]
    fn sky_chroma_policy_uses_magenta_evidence_without_neutralizing_blue() {
        let source = image(4, 2, [1.0, 0.45, 1.0]);
        let proxy = build_proxy(&source, None).unwrap();
        let evidence = eligible_evidence(&proxy);
        let map = build_sky_chroma_map(&source, None, None, &proxy, &evidence, 1.0).unwrap();
        assert!(
            map.weights()
                .iter()
                .all(|weight| (0.0..=1.0).contains(weight))
        );
        assert!(map.report().affected_pixels > 0);

        let before = source.pixels[0];
        let before_y = luminance(before);
        let before_lab = crate::oklab::from_linear_srgb(before);
        let mut adjusted = source;
        map.apply(&mut adjusted).unwrap();
        let after = adjusted.pixels[0];
        let after_lab = crate::oklab::from_linear_srgb(after);
        assert!(after_lab.a < before_lab.a);
        assert!((luminance(after) - before_y).abs() < 1.0e-5);
        assert!((after_lab.b / after_lab.l - before_lab.b / before_lab.l).abs() < 1.0e-5);
    }

    #[test]
    fn sky_chroma_policy_leaves_non_sky_pixels_exactly_untouched() {
        let source = image(4, 2, [1.0, 0.45, 1.0]);
        let proxy = build_proxy(&source, None).unwrap();
        let mut evidence = eligible_evidence(&proxy);
        evidence.masks.get_mut(RegionKind::Sky).confidence.fill(0);
        let map = build_sky_chroma_map(&source, None, None, &proxy, &evidence, 1.0).unwrap();
        let mut adjusted = source.clone();
        map.apply(&mut adjusted).unwrap();
        assert_eq!(adjusted.pixels, source.pixels);
    }
}
