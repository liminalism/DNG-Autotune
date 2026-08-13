//! Render-neutral guidance for a downstream image encoder.
//!
//! This module deliberately knows nothing about codecs. It translates RAW
//! analysis and optional scene evidence into a small, versioned contract that
//! an encoder crate can consume later. Deriving hints never changes pixels.

use crate::scene::{RegionKind, SceneEvidence};
use crate::types::AnalysisStats;
use serde::Serialize;
use std::error::Error;
use std::fmt;

pub const ENCODER_HINTS_SCHEMA_VERSION: u32 = 2;
pub const SPATIAL_AQ_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChromaSamplingHint {
    Yuv444,
    HighQualityYuv422,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GrainHint {
    NoPreference,
    Preserve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionEncodingUse {
    /// Smooth, low-activity gradients where an activity-only AQ model can cause
    /// visible banding by spending too few bits.
    SmoothGradientRetention,
    HighlightHeadroom,
    /// Fine stochastic detail. The encoder decides whether its masking model can
    /// quantize this more strongly or whether the requested quality should retain it.
    TextureRetention,
    /// Persistent edges and man-made detail that should not be treated as noise.
    StructuralDetailRetention,
}

/// Geometry of an encoder-requested raster grid in rendered/source coordinates.
///
/// Cells start at `(0, 0)`. The final row and column may cover fewer source
/// pixels than `cell_width`/`cell_height`. `grid_width` is also the raster stride.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SpatialAqGrid {
    pub source_width: u32,
    pub source_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub grid_width: u32,
    pub grid_height: u32,
}

/// One semantic confidence plane resampled to an encoder-selected grid.
///
/// Confidence is normalized to `0.0..=1.0`, in row-major order. The semantic
/// class and its possible uses remain explicit: raw-autotune provides evidence;
/// the encoder remains responsible for choosing direction, strength, and rate
/// compensation in its own quantizer units.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpatialAqLayer {
    pub kind: RegionKind,
    pub uses: Vec<RegionEncodingUse>,
    pub confidence: Vec<f32>,
}

/// Codec-neutral semantic evidence for spatial adaptive quantization.
///
/// This deliberately contains no QP offsets, lambda multipliers, quantizer-step
/// adjustments, or codec block identifiers. HEVC callers can request their
/// 16/32-pixel QG grid; JPEG XL callers can request its 8-pixel atom grid.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpatialAqMap {
    pub schema_version: u32,
    pub grid: SpatialAqGrid,
    pub layers: Vec<SpatialAqLayer>,
}

impl SpatialAqMap {
    pub fn layer(&self, kind: RegionKind) -> Option<&SpatialAqLayer> {
        self.layers.iter().find(|layer| layer.kind == kind)
    }

    /// Build a scalar evidence plane for one encoder-neutral use by taking the
    /// strongest contributing semantic confidence at each cell.
    pub fn confidence_for_use(&self, usage: RegionEncodingUse) -> Vec<f32> {
        let len = (self.grid.grid_width as usize).saturating_mul(self.grid.grid_height as usize);
        let mut result = vec![0.0f32; len];
        for layer in self
            .layers
            .iter()
            .filter(|layer| layer.uses.contains(&usage))
        {
            for (result, &confidence) in result.iter_mut().zip(&layer.confidence) {
                *result = result.max(confidence);
            }
        }
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialAqError {
    SceneEvidenceUnavailable,
    ZeroCellDimension,
    DimensionOverflow,
    InconsistentSceneGeometry,
}

impl fmt::Display for SpatialAqError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SceneEvidenceUnavailable => f.write_str("semantic scene evidence is unavailable"),
            Self::ZeroCellDimension => f.write_str("spatial AQ cell dimensions must be non-zero"),
            Self::DimensionOverflow => {
                f.write_str("spatial AQ dimensions exceed the supported range")
            }
            Self::InconsistentSceneGeometry => {
                f.write_str("scene confidence planes do not match the semantic proxy")
            }
        }
    }
}

impl Error for SpatialAqError {}

/// A stable reference to a dense confidence plane held by `SceneEvidence`.
///
/// JSON reports contain this descriptor, not the potentially multi-megabyte
/// confidence bytes. In-process encoders obtain those bytes through
/// [`EncoderProfileHints::confidence_mask`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EncoderRegionHint {
    pub kind: RegionKind,
    pub uses: Vec<RegionEncodingUse>,
    pub area_fraction: f32,
    pub mean_confidence: f32,
    pub mask_width: usize,
    pub mask_height: usize,
    pub mask_threshold: f32,
    pub coordinate_space: &'static str,
    pub source_width: usize,
    pub source_height: usize,
    pub content_x: usize,
    pub content_y: usize,
    pub content_width: usize,
    pub content_height: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EncoderProfileHints {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_bit_depth: Option<u8>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub chroma_sampling_preference: Vec<ChromaSamplingHint>,
    pub grain: GrainHint,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<EncoderRegionHint>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<&'static str>,
}

impl EncoderProfileHints {
    /// Derive encoder guidance without changing or borrowing rendered pixels.
    pub fn derive(analysis: &AnalysisStats, scene: Option<&SceneEvidence>) -> Self {
        let mut hints = Self {
            schema_version: ENCODER_HINTS_SCHEMA_VERSION,
            minimum_bit_depth: None,
            chroma_sampling_preference: Vec::new(),
            grain: GrainHint::NoPreference,
            regions: Vec::new(),
            reasons: Vec::new(),
        };

        if analysis.low_light_score >= 0.5 {
            hints.grain = GrainHint::Preserve;
            hints.reasons.push("raw_low_light_signal");
        }

        let Some(scene) = scene else {
            return hints;
        };

        let mut chroma_subject = false;
        for region in &scene.regions {
            if region.area_fraction < 0.05 || region.mean_confidence < 0.70 {
                continue;
            }
            let uses = match region.kind {
                RegionKind::Sky if region.mean_confidence >= 0.75 => {
                    let mut uses = vec![RegionEncodingUse::SmoothGradientRetention];
                    if region.clipped_fraction.unwrap_or(0.0) >= 0.10 {
                        hints.minimum_bit_depth = Some(10);
                        hints.reasons.push("confident_clipped_sky");
                        uses.push(RegionEncodingUse::HighlightHeadroom);
                    }
                    uses
                }
                RegionKind::Vegetation | RegionKind::BuildingOrInterior => {
                    if region.kind == RegionKind::Vegetation {
                        vec![RegionEncodingUse::TextureRetention]
                    } else {
                        vec![RegionEncodingUse::StructuralDetailRetention]
                    }
                }
                // Face/person ROI is intentionally excluded until a validated
                // corpus supports it. The current corpus has no face and one
                // false-positive person response.
                _ => Vec::new(),
            };
            if uses.is_empty() {
                continue;
            }
            let Some(mask) = scene
                .masks
                .masks
                .iter()
                .find(|mask| mask.kind == region.kind)
            else {
                continue;
            };
            hints.regions.push(EncoderRegionHint {
                kind: region.kind,
                uses,
                area_fraction: region.area_fraction,
                mean_confidence: region.mean_confidence,
                mask_width: mask.width,
                mask_height: mask.height,
                mask_threshold: mask.threshold,
                coordinate_space: "semantic_proxy_letterboxed",
                source_width: scene.proxy.source_width,
                source_height: scene.proxy.source_height,
                content_x: scene.proxy.content_x,
                content_y: scene.proxy.content_y,
                content_width: scene.proxy.content_width,
                content_height: scene.proxy.content_height,
            });

            if region.kind != RegionKind::Sky
                && region.mean_chroma.unwrap_or(0.0) >= 0.12
                && analysis.mean_chroma >= 0.08
            {
                chroma_subject = true;
            }
        }

        if chroma_subject {
            hints.chroma_sampling_preference = vec![
                ChromaSamplingHint::Yuv444,
                ChromaSamplingHint::HighQualityYuv422,
            ];
            hints.reasons.push("confident_chromatic_subject");
        }
        hints
    }

    /// Borrow the dense u8 confidence plane described by a region hint.
    /// Values are 0..=255 and use the dimensions/threshold in that hint.
    pub fn confidence_mask<'a>(
        &self,
        scene: &'a SceneEvidence,
        kind: RegionKind,
    ) -> Option<&'a [u8]> {
        self.regions.iter().any(|hint| hint.kind == kind).then(|| {
            scene
                .masks
                .masks
                .iter()
                .find(|mask| mask.kind == kind)
                .map(|mask| mask.confidence.as_slice())
        })?
    }

    /// Bilinearly sample a hinted confidence plane in rendered/source pixel
    /// coordinates, undoing the semantic proxy's letterbox transform.
    pub fn confidence_at_source(
        &self,
        scene: &SceneEvidence,
        kind: RegionKind,
        x: usize,
        y: usize,
    ) -> Option<f32> {
        let hint = self.regions.iter().find(|hint| hint.kind == kind)?;
        if x >= hint.source_width || y >= hint.source_height {
            return None;
        }
        let mask = scene.masks.masks.iter().find(|mask| mask.kind == kind)?;
        let proxy_x = hint.content_x as f32
            + (x as f32 + 0.5) * hint.content_width as f32 / hint.source_width as f32
            - 0.5;
        let proxy_y = hint.content_y as f32
            + (y as f32 + 0.5) * hint.content_height as f32 / hint.source_height as f32
            - 0.5;
        let min_x = hint.content_x as f32;
        let min_y = hint.content_y as f32;
        let max_x = (hint.content_x + hint.content_width - 1) as f32;
        let max_y = (hint.content_y + hint.content_height - 1) as f32;
        let proxy_x = proxy_x.clamp(min_x, max_x);
        let proxy_y = proxy_y.clamp(min_y, max_y);
        let x0 = proxy_x.floor() as usize;
        let y0 = proxy_y.floor() as usize;
        let x1 = (x0 + 1).min(hint.content_x + hint.content_width - 1);
        let y1 = (y0 + 1).min(hint.content_y + hint.content_height - 1);
        let fx = proxy_x - x0 as f32;
        let fy = proxy_y - y0 as f32;
        let sample = |sx: usize, sy: usize| mask.confidence[sy * mask.width + sx] as f32 / 255.0;
        let top = sample(x0, y0) + (sample(x1, y0) - sample(x0, y0)) * fx;
        let bottom = sample(x0, y1) + (sample(x1, y1) - sample(x0, y1)) * fx;
        Some(top + (bottom - top) * fy)
    }

    /// Resample every selected semantic layer to a caller-selected block grid.
    ///
    /// Each output value is the area-weighted mean confidence over that source
    /// cell, evaluated in the model proxy without counting letterbox pixels.
    /// This is preferable to center sampling for small codec blocks near a
    /// semantic boundary and makes partial edge cells deterministic.
    pub fn spatial_aq_map(
        &self,
        scene: &SceneEvidence,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<SpatialAqMap, SpatialAqError> {
        if cell_width == 0 || cell_height == 0 {
            return Err(SpatialAqError::ZeroCellDimension);
        }
        let source_width = u32::try_from(scene.proxy.source_width)
            .map_err(|_| SpatialAqError::DimensionOverflow)?;
        let source_height = u32::try_from(scene.proxy.source_height)
            .map_err(|_| SpatialAqError::DimensionOverflow)?;
        if source_width == 0
            || source_height == 0
            || scene.proxy.content_width == 0
            || scene.proxy.content_height == 0
        {
            return Err(SpatialAqError::InconsistentSceneGeometry);
        }
        let grid_width = source_width.div_ceil(cell_width);
        let grid_height = source_height.div_ceil(cell_height);
        let cell_count = usize::try_from(u64::from(grid_width) * u64::from(grid_height))
            .map_err(|_| SpatialAqError::DimensionOverflow)?;
        let grid = SpatialAqGrid {
            source_width,
            source_height,
            cell_width,
            cell_height,
            grid_width,
            grid_height,
        };

        let mut layers = Vec::with_capacity(self.regions.len());
        for hint in &self.regions {
            let mask = scene
                .masks
                .masks
                .iter()
                .find(|mask| mask.kind == hint.kind)
                .ok_or(SpatialAqError::InconsistentSceneGeometry)?;
            if mask.width != scene.proxy.width
                || mask.height != scene.proxy.height
                || mask.confidence.len() != mask.width.saturating_mul(mask.height)
                || hint.source_width != scene.proxy.source_width
                || hint.source_height != scene.proxy.source_height
            {
                return Err(SpatialAqError::InconsistentSceneGeometry);
            }

            let mut confidence = Vec::with_capacity(cell_count);
            for gy in 0..grid_height {
                let y0 = gy * cell_height;
                let y1 = (y0 + cell_height).min(source_height);
                for gx in 0..grid_width {
                    let x0 = gx * cell_width;
                    let x1 = (x0 + cell_width).min(source_width);
                    confidence.push(mean_confidence_for_source_rect(scene, mask, x0, y0, x1, y1));
                }
            }
            layers.push(SpatialAqLayer {
                kind: hint.kind,
                uses: hint.uses.clone(),
                confidence,
            });
        }

        Ok(SpatialAqMap {
            schema_version: SPATIAL_AQ_SCHEMA_VERSION,
            grid,
            layers,
        })
    }
}

fn mean_confidence_for_source_rect(
    scene: &SceneEvidence,
    mask: &crate::scene::RegionMask,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> f32 {
    let proxy = &scene.proxy;
    let px0 = proxy.content_x as f64
        + f64::from(x0) * proxy.content_width as f64 / proxy.source_width as f64;
    let px1 = proxy.content_x as f64
        + f64::from(x1) * proxy.content_width as f64 / proxy.source_width as f64;
    let py0 = proxy.content_y as f64
        + f64::from(y0) * proxy.content_height as f64 / proxy.source_height as f64;
    let py1 = proxy.content_y as f64
        + f64::from(y1) * proxy.content_height as f64 / proxy.source_height as f64;
    let sx0 = px0.floor().max(proxy.content_x as f64) as usize;
    let sx1 = px1
        .ceil()
        .min((proxy.content_x + proxy.content_width) as f64) as usize;
    let sy0 = py0.floor().max(proxy.content_y as f64) as usize;
    let sy1 = py1
        .ceil()
        .min((proxy.content_y + proxy.content_height) as f64) as usize;
    let mut weighted = 0.0f64;
    let mut area = 0.0f64;
    for sy in sy0..sy1 {
        let overlap_y = (py1.min((sy + 1) as f64) - py0.max(sy as f64)).max(0.0);
        for sx in sx0..sx1 {
            let overlap_x = (px1.min((sx + 1) as f64) - px0.max(sx as f64)).max(0.0);
            let weight = overlap_x * overlap_y;
            weighted += f64::from(mask.confidence[sy * mask.width + sx]) * weight;
            area += weight;
        }
    }
    if area > 0.0 {
        (weighted / area / 255.0) as f32
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{RegionMask, RegionMasks, RegionStats, SemanticProxyInfo};
    use crate::types::TonalClass;
    use std::collections::BTreeMap;

    fn analysis(low_light_score: f32) -> AnalysisStats {
        AnalysisStats {
            sampled_pixels: 1,
            sample_stride: 1,
            p005_ev: -8.0,
            p05_ev: -6.0,
            p50_ev: 0.0,
            p95_ev: 2.0,
            p995_ev: 3.0,
            center_median_ev: 0.0,
            measured_dynamic_range_ev: 11.0,
            key_score: 0.5,
            target_median_ev: 0.0,
            tonal_class: TonalClass::Normal,
            near_black_fraction: 0.0,
            near_white_fraction: 0.1,
            clipped_1_fraction: 0.0,
            clipped_2_fraction: 0.0,
            clipped_3_fraction: 0.0,
            mean_chroma: 0.2,
            capture_ev100: None,
            low_light_score,
        }
    }

    fn scene() -> SceneEvidence {
        let kinds = [RegionKind::Sky, RegionKind::Vegetation, RegionKind::Person];
        SceneEvidence {
            proxy: SemanticProxyInfo {
                width: 2,
                height: 2,
                source_width: 4,
                source_height: 2,
                content_x: 0,
                content_y: 0,
                content_width: 2,
                content_height: 1,
                rendering: "test",
            },
            embedded_preview_semantic_eligible: false,
            models: Vec::new(),
            raw_scores: BTreeMap::new(),
            regions: vec![
                RegionStats {
                    kind: RegionKind::Sky,
                    area_fraction: 0.2,
                    mean_confidence: 0.95,
                    p10_ev: None,
                    p50_ev: None,
                    p90_ev: None,
                    clipped_fraction: Some(0.3),
                    reconstruction_uncertainty: Some(0.2),
                    mean_chroma: Some(0.1),
                    noise_snr10_ev: None,
                    sharpness: None,
                },
                RegionStats {
                    kind: RegionKind::Vegetation,
                    area_fraction: 0.3,
                    mean_confidence: 0.9,
                    p10_ev: None,
                    p50_ev: None,
                    p90_ev: None,
                    clipped_fraction: Some(0.0),
                    reconstruction_uncertainty: Some(0.0),
                    mean_chroma: Some(0.2),
                    noise_snr10_ev: None,
                    sharpness: None,
                },
                RegionStats {
                    kind: RegionKind::Person,
                    area_fraction: 0.2,
                    mean_confidence: 0.99,
                    p10_ev: None,
                    p50_ev: None,
                    p90_ev: None,
                    clipped_fraction: Some(0.0),
                    reconstruction_uncertainty: Some(0.0),
                    mean_chroma: Some(0.2),
                    noise_snr10_ev: None,
                    sharpness: None,
                },
            ],
            inference_errors: Vec::new(),
            policy_adjustments: Vec::new(),
            sky_highlight_adjustment: None,
            sky_chroma_adjustment: None,
            masks: RegionMasks {
                width: 2,
                height: 2,
                masks: kinds
                    .into_iter()
                    .map(|kind| RegionMask {
                        kind,
                        width: 2,
                        height: 2,
                        threshold: 0.5,
                        confidence: vec![255; 4],
                    })
                    .collect(),
            },
        }
    }

    fn grid_scene() -> SceneEvidence {
        let mut scene = scene();
        scene.proxy = SemanticProxyInfo {
            width: 4,
            height: 4,
            source_width: 64,
            source_height: 32,
            content_x: 0,
            content_y: 1,
            content_width: 4,
            content_height: 2,
            rendering: "test",
        };
        scene.regions[0].clipped_fraction = Some(0.0);
        scene.masks = RegionMasks {
            width: 4,
            height: 4,
            masks: [RegionKind::Sky, RegionKind::Vegetation, RegionKind::Person]
                .into_iter()
                .map(|kind| {
                    let row = match kind {
                        RegionKind::Sky => [255, 255, 0, 0],
                        RegionKind::Vegetation => [0, 0, 255, 255],
                        _ => [255; 4],
                    };
                    let mut confidence = vec![0; 16];
                    confidence[4..8].copy_from_slice(&row);
                    confidence[8..12].copy_from_slice(&row);
                    RegionMask {
                        kind,
                        width: 4,
                        height: 4,
                        threshold: 0.5,
                        confidence,
                    }
                })
                .collect(),
        };
        scene
    }

    #[test]
    fn semantic_and_raw_signals_produce_profile_and_spatial_hints() {
        let scene = scene();
        let hints = EncoderProfileHints::derive(&analysis(0.8), Some(&scene));
        assert_eq!(hints.schema_version, 2);
        assert_eq!(hints.minimum_bit_depth, Some(10));
        assert_eq!(hints.grain, GrainHint::Preserve);
        assert_eq!(hints.chroma_sampling_preference.len(), 2);
        assert!(
            hints
                .regions
                .iter()
                .any(|hint| hint.kind == RegionKind::Sky)
        );
        assert!(
            hints
                .regions
                .iter()
                .all(|hint| hint.kind != RegionKind::Person)
        );
        assert_eq!(
            hints.confidence_mask(&scene, RegionKind::Sky),
            Some(&[255, 255, 255, 255][..])
        );
        assert_eq!(
            hints.confidence_at_source(&scene, RegionKind::Sky, 3, 1),
            Some(1.0)
        );
        assert_eq!(
            hints.confidence_at_source(&scene, RegionKind::Sky, 4, 1),
            None
        );
    }

    #[test]
    fn unclipped_sky_still_exports_gradient_evidence() {
        let scene = grid_scene();
        let hints = EncoderProfileHints::derive(&analysis(0.2), Some(&scene));
        let sky = hints
            .regions
            .iter()
            .find(|region| region.kind == RegionKind::Sky)
            .expect("confident sky is useful to AQ even when unclipped");
        assert!(
            sky.uses
                .contains(&RegionEncodingUse::SmoothGradientRetention)
        );
        assert!(!sky.uses.contains(&RegionEncodingUse::HighlightHeadroom));
        assert_eq!(hints.minimum_bit_depth, None);
    }

    #[test]
    fn spatial_aq_map_matches_jpeg_xl_and_hevc_grid_shapes() {
        let scene = grid_scene();
        let hints = EncoderProfileHints::derive(&analysis(0.2), Some(&scene));

        for (cell, expected_width, expected_height) in [(8, 8, 4), (16, 4, 2), (32, 2, 1)] {
            let aq = hints.spatial_aq_map(&scene, cell, cell).unwrap();
            assert_eq!(aq.schema_version, SPATIAL_AQ_SCHEMA_VERSION);
            assert_eq!(aq.grid.grid_width, expected_width);
            assert_eq!(aq.grid.grid_height, expected_height);
            assert_eq!(
                aq.layer(RegionKind::Sky).unwrap().confidence.len(),
                (expected_width * expected_height) as usize
            );
        }

        let hevc = hints.spatial_aq_map(&scene, 32, 32).unwrap();
        assert_eq!(hevc.layer(RegionKind::Sky).unwrap().confidence, [1.0, 0.0]);
        assert_eq!(
            hevc.confidence_for_use(RegionEncodingUse::TextureRetention),
            [0.0, 1.0]
        );
    }

    #[test]
    fn spatial_aq_rejects_zero_sized_cells() {
        let scene = grid_scene();
        let hints = EncoderProfileHints::derive(&analysis(0.2), Some(&scene));
        assert_eq!(
            hints.spatial_aq_map(&scene, 0, 8),
            Err(SpatialAqError::ZeroCellDimension)
        );
    }

    #[test]
    fn analysis_only_never_invents_semantic_roi() {
        let hints = EncoderProfileHints::derive(&analysis(0.2), None);
        assert!(hints.regions.is_empty());
        assert_eq!(hints.minimum_bit_depth, None);
        assert_eq!(hints.grain, GrainHint::NoPreference);
    }

    #[test]
    fn json_is_compact_but_describes_the_in_memory_mask() {
        let scene = scene();
        let hints = EncoderProfileHints::derive(&analysis(0.2), Some(&scene));
        let document = serde_json::to_value(&hints).unwrap();
        assert_eq!(document["schema_version"], 2);
        assert_eq!(document["regions"][0]["mask_width"], 2);
        assert_eq!(
            document["regions"][0]["coordinate_space"],
            "semantic_proxy_letterboxed"
        );
        assert_eq!(document["regions"][0]["source_width"], 4);
        assert_eq!(document["regions"][0]["content_height"], 1);
        let scene_document = serde_json::to_value(&scene).unwrap();
        assert!(
            scene_document["masks"]["masks"][0]
                .get("confidence")
                .is_none()
        );
    }
}
