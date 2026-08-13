//! Render-neutral guidance for a downstream image encoder.
//!
//! This module deliberately knows nothing about codecs. It translates RAW
//! analysis and optional scene evidence into a small, versioned contract that
//! an encoder crate can consume later. Deriving hints never changes pixels.

use crate::scene::{RegionKind, SceneEvidence};
use crate::types::AnalysisStats;
use serde::Serialize;

pub const ENCODER_HINTS_SCHEMA_VERSION: u32 = 1;

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
    HighlightHeadroom,
    TextureRetention,
}

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
                RegionKind::Sky
                    if region.mean_confidence >= 0.75
                        && region.clipped_fraction.unwrap_or(0.0) >= 0.10 =>
                {
                    hints.minimum_bit_depth = Some(10);
                    if !hints.reasons.contains(&"confident_clipped_sky") {
                        hints.reasons.push("confident_clipped_sky");
                    }
                    vec![RegionEncodingUse::HighlightHeadroom]
                }
                RegionKind::Vegetation | RegionKind::BuildingOrInterior => {
                    vec![RegionEncodingUse::TextureRetention]
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
                rendering: "test".into(),
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

    #[test]
    fn semantic_and_raw_signals_produce_profile_and_spatial_hints() {
        let scene = scene();
        let hints = EncoderProfileHints::derive(&analysis(0.8), Some(&scene));
        assert_eq!(hints.schema_version, 1);
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
        assert_eq!(document["schema_version"], 1);
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
