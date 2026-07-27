//! Sensor noise pooled across frames of the same camera and ISO.
//!
//! The per-frame fit in [`crate::noise`] is validated in aggregate — across 258
//! Sony frames `log2(shot_slope)` tracks `log2(ISO)` at slope +1.039 with R^2
//! 0.976 — but not per frame, where dense texture inflates it. Frames at the
//! same ISO from the same body have the same gain by definition, so the median
//! of a group is a far better estimate than any single member.
//!
//! # Why a file, rather than pooling inside the run
//!
//! Pooling across whatever files happen to be in a batch would make a frame's
//! rendering depend on which other frames it was processed with. Order-independent,
//! but still a surprise: the same file would develop differently alone than in
//! company. A profile is written once, inspected if you like, and reused, so a
//! frame renders identically either way.
//!
//! The scan that produces it decodes and fits, then stops — no develop, no tone
//! mapping, no output — so it is much cheaper than a full pass.

use crate::noise::{NoiseEstimate, crossings};
use crate::types::Distribution;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Smallest group whose median is trusted over the per-frame estimate.
const MIN_GROUP: usize = 3;

/// One frame's contribution to a profile.
#[derive(Debug, Clone)]
pub struct NoiseSample {
    pub clean_make: String,
    pub clean_model: String,
    pub iso: Option<u32>,
    pub shot_slope: f32,
}

/// Pooled statistics for one (camera, ISO) group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupEntry {
    pub clean_make: String,
    pub clean_model: String,
    pub iso: u32,
    /// Whole distribution, not just the median: the spread is the direct
    /// evidence for how much the per-frame estimates disagree.
    pub shot_slope: Distribution,
}

/// Pooled sensor noise for one or more cameras.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseProfile {
    pub schema_version: u32,
    pub application_version: String,
    /// Keyed `"make|model|iso"`. A `BTreeMap` so both the JSON key order and
    /// iteration are deterministic.
    pub groups: BTreeMap<String, GroupEntry>,
}

/// Which estimate actually set a frame's noise floor.
#[derive(Debug, Clone, Serialize)]
pub struct NoiseFloor {
    pub snr1_ev: f32,
    pub snr10_ev: f32,
    pub shot_slope: f32,
    pub pooled: bool,
    /// `"frame"` or `"group_median"`.
    pub source: &'static str,
    pub group_key: Option<String>,
    pub group_n: Option<usize>,
}

/// Group key for a camera and ISO.
pub fn group_key(clean_make: &str, clean_model: &str, iso: u32) -> String {
    format!("{clean_make}|{clean_model}|{iso}")
}

impl NoiseProfile {
    /// Build a profile from per-frame samples.
    ///
    /// Frames with no ISO are dropped: gain is proportional to ISO, so pooling
    /// across ISOs would be wrong by construction.
    pub fn build(samples: &[NoiseSample]) -> Self {
        let mut grouped: BTreeMap<String, (String, String, u32, Vec<f32>)> = BTreeMap::new();

        for sample in samples {
            let Some(iso) = sample.iso else { continue };
            if !sample.shot_slope.is_finite() || sample.shot_slope <= 0.0 {
                continue;
            }
            let key = group_key(&sample.clean_make, &sample.clean_model, iso);
            grouped
                .entry(key)
                .or_insert_with(|| {
                    (
                        sample.clean_make.clone(),
                        sample.clean_model.clone(),
                        iso,
                        Vec::new(),
                    )
                })
                .3
                .push(sample.shot_slope);
        }

        let groups = grouped
            .into_iter()
            .filter_map(|(key, (make, model, iso, slopes))| {
                // `Distribution::from_samples` sorts internally, so the result
                // cannot depend on the order frames finished in.
                let distribution = Distribution::from_samples(slopes)?;
                Some((
                    key,
                    GroupEntry {
                        clean_make: make,
                        clean_model: model,
                        iso,
                        shot_slope: distribution,
                    },
                ))
            })
            .collect();

        Self {
            schema_version: 1,
            application_version: env!("CARGO_PKG_VERSION").to_string(),
            groups,
        }
    }

    /// Decide the noise floor for one frame.
    ///
    /// Uses the group median where the group is large enough, otherwise falls
    /// back to the frame's own estimate unchanged. `read_variance` and
    /// `saturation` always come from this frame: `read_variance` clamps to zero
    /// on more than half of all frames, so a pooled median of it would look like
    /// a measurement while being an artifact of that clamp.
    pub fn resolve(
        &self,
        clean_make: &str,
        clean_model: &str,
        iso: Option<u32>,
        estimate: Option<&NoiseEstimate>,
    ) -> Option<NoiseFloor> {
        let estimate = estimate?;
        let key = iso.map(|iso| group_key(clean_make, clean_model, iso));
        let group = key.as_ref().and_then(|key| self.groups.get(key));

        match group.filter(|entry| entry.shot_slope.count >= MIN_GROUP) {
            Some(entry) => {
                let slope = entry.shot_slope.median;
                let (snr1_ev, snr10_ev) =
                    crossings(slope, estimate.read_variance, estimate.saturation);
                Some(NoiseFloor {
                    snr1_ev,
                    snr10_ev,
                    shot_slope: slope,
                    pooled: true,
                    source: "group_median",
                    group_key: key,
                    group_n: Some(entry.shot_slope.count),
                })
            }
            None => Some(NoiseFloor {
                snr1_ev: estimate.snr1_ev,
                snr10_ev: estimate.snr10_ev,
                shot_slope: estimate.shot_slope,
                pooled: false,
                source: "frame",
                group_key: key,
                group_n: group.map(|entry| entry.shot_slope.count),
            }),
        }
    }
}

/// The floor a frame gets with no profile at all: its own estimate.
pub fn frame_floor(estimate: Option<&NoiseEstimate>) -> Option<NoiseFloor> {
    let estimate = estimate?;
    Some(NoiseFloor {
        snr1_ev: estimate.snr1_ev,
        snr10_ev: estimate.snr10_ev,
        shot_slope: estimate.shot_slope,
        pooled: false,
        source: "frame",
        group_key: None,
        group_n: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(iso: Option<u32>, slope: f32) -> NoiseSample {
        NoiseSample {
            clean_make: "Sony".into(),
            clean_model: "ILCE-7C".into(),
            iso,
            shot_slope: slope,
        }
    }

    fn estimate(slope: f32) -> NoiseEstimate {
        NoiseEstimate {
            shot_slope: slope,
            read_variance: 0.0,
            snr1_ev: -1.0,
            snr10_ev: -2.0,
            tiles_used: 100,
            saturation: 14848.0,
        }
    }

    #[test]
    fn groups_are_keyed_by_camera_and_iso() {
        let profile = NoiseProfile::build(&[
            sample(Some(100), 0.10),
            sample(Some(100), 0.12),
            sample(Some(800), 1.00),
        ]);
        assert_eq!(profile.groups.len(), 2);
        assert!(profile.groups.contains_key("Sony|ILCE-7C|100"));
        assert_eq!(profile.groups["Sony|ILCE-7C|100"].shot_slope.count, 2);
    }

    #[test]
    fn frames_without_iso_are_dropped() {
        let profile = NoiseProfile::build(&[sample(None, 0.10), sample(None, 0.20)]);
        assert!(profile.groups.is_empty());
    }

    /// The point of pooling: one wildly wrong frame must not set its own floor
    /// when its group says otherwise.
    #[test]
    fn a_large_group_overrides_an_outlier_frame() {
        let mut samples: Vec<NoiseSample> = (0..10).map(|_| sample(Some(100), 0.12)).collect();
        samples.push(sample(Some(100), 90.0));
        let profile = NoiseProfile::build(&samples);

        let outlier = estimate(90.0);
        let floor = profile
            .resolve("Sony", "ILCE-7C", Some(100), Some(&outlier))
            .unwrap();

        assert!(floor.pooled);
        assert_eq!(floor.source, "group_median");
        assert!((floor.shot_slope - 0.12).abs() < 1.0e-6);
        assert_eq!(floor.group_n, Some(11));
        // A far lower slope means a far lower noise floor.
        assert!(floor.snr1_ev < outlier.snr1_ev);
    }

    #[test]
    fn a_small_group_leaves_the_frame_alone() {
        let profile = NoiseProfile::build(&[sample(Some(400), 1.0), sample(Some(400), 2.0)]);
        let own = estimate(5.0);
        let floor = profile
            .resolve("Sony", "ILCE-7C", Some(400), Some(&own))
            .unwrap();
        assert!(!floor.pooled);
        assert_eq!(floor.source, "frame");
        assert_eq!(floor.snr1_ev, own.snr1_ev);
        assert_eq!(floor.group_n, Some(2));
    }

    #[test]
    fn a_missing_iso_leaves_the_frame_alone() {
        let profile = NoiseProfile::build(&[
            sample(Some(100), 0.1),
            sample(Some(100), 0.1),
            sample(Some(100), 0.1),
        ]);
        let own = estimate(5.0);
        let floor = profile
            .resolve("Sony", "ILCE-7C", None, Some(&own))
            .unwrap();
        assert!(!floor.pooled);
        assert!(floor.group_key.is_none());
    }

    #[test]
    fn no_estimate_yields_no_floor() {
        let profile = NoiseProfile::build(&[]);
        assert!(
            profile
                .resolve("Sony", "ILCE-7C", Some(100), None)
                .is_none()
        );
        assert!(frame_floor(None).is_none());
    }

    /// Building must not depend on the order frames were processed in.
    #[test]
    fn build_is_order_independent() {
        let forward: Vec<NoiseSample> =
            (1..=9).map(|i| sample(Some(100), i as f32 * 0.1)).collect();
        let mut reversed = forward.clone();
        reversed.reverse();

        let a = NoiseProfile::build(&forward);
        let b = NoiseProfile::build(&reversed);
        assert_eq!(
            serde_json::to_string(&a.groups).unwrap(),
            serde_json::to_string(&b.groups).unwrap()
        );
    }
}
