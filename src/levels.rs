//! Sensor black/white level reconciliation.
//!
//! Rawler 0.7.2 normalizes a `LinearRaw` (already-demosaiced) image with
//! `imgop::raw::correct_blacklevel`, which walks the interleaved pixel data in
//! `blacklevel.len()`-sized chunks and requires `blacklevel.len() ==
//! whitelevel.len()`. It panics otherwise.
//!
//! Several real linear DNGs — Samsung Galaxy phone DNGs among them — store
//! `BlackLevelRepeatDim` as 2x2 with `cpp` samples per position, i.e. 12 black
//! levels, alongside 3 white levels. That is valid DNG but Rawler cannot
//! consume it, and even a coincidental count match would be wrong: a 12-wide
//! chunk stride does not line up with 3-component interleaved pixels.
//!
//! **The collapse is a Rawler-compatibility workaround, not a correct
//! reduction.** An earlier version of this comment argued the opposite — that
//! once a file is linear RGB every repeat position holds the same component set,
//! so averaging the pattern loses nothing. That argument is false. A
//! `BlackLevelRepeatDim` on linear data is a meaningful repeating per-position
//! offset: DNG defines `BlackLevel` positionally, and a demosaicing pipeline
//! upstream of the file may well have left a pattern behind, which is precisely
//! why the tag is allowed to have a repeat on `LinearRaw` at all. Averaging it
//! discards that offset. It happens to cost nothing on this corpus, where every
//! linear file records the same level at every position (Samsung's Expert RAW
//! stores `[0 x 12]`), but "inert on the files we have" is not "correct".
//!
//! The collapse stays because `--raw-color-path rawler` still needs it:
//! `correct_blacklevel` panics unless `blacklevel.len() == whitelevel.len()`, and
//! a 12-wide chunk stride would not line up with 3-component interleaved pixels
//! even if the counts matched. [`crate::rescale`] needs none of this — it indexes
//! the real `width x height x cpp` layout — so the owned colour path is not
//! limited by the reduction, and leaving this code untouched is what makes "the
//! fitted noise model must not move" true by construction rather than by
//! measurement.

use anyhow::{Result, bail};
use rawler::formats::tiff::Rational;
use rawler::rawimage::{BlackLevel, RawPhotometricInterpretation, WhiteLevel};
use rawler::{RawImage, RawImageData};

/// Denominator used when re-encoding an averaged black level as a `Rational`.
/// Black levels are small magnitudes, so four decimal places is lossless in
/// practice for the values cameras actually record.
const RATIONAL_DENOMINATOR: u32 = 10_000;

fn to_rational(value: f32) -> Rational {
    let scaled = (value.max(0.0) * RATIONAL_DENOMINATOR as f32).round();
    let numerator = if scaled.is_finite() {
        scaled.clamp(0.0, u32::MAX as f32) as u32
    } else {
        0
    };
    Rational::new(numerator, RATIONAL_DENOMINATOR)
}

/// Collapse a `width x height x cpp` black-level pattern to one level per
/// component by averaging each component over its repeat positions.
fn collapse_black_levels(levels: &[f32], components: usize) -> Vec<f32> {
    debug_assert!(components > 0);

    if levels.is_empty() {
        return vec![0.0; components];
    }

    // The pattern is laid out as `y * width * cpp + x * cpp + component`, so a
    // component's samples are every `pattern_cpp`-th entry. When the stored cpp
    // does not divide evenly into the image cpp, fall back to a flat average so
    // the result is still a defensible single level rather than a mislabeled one.
    if !levels.len().is_multiple_of(components) {
        let mean = levels.iter().sum::<f32>() / levels.len() as f32;
        return vec![mean; components];
    }

    (0..components)
        .map(|component| {
            let samples: Vec<f32> = levels
                .iter()
                .skip(component)
                .step_by(components)
                .copied()
                .collect();
            samples.iter().sum::<f32>() / samples.len() as f32
        })
        .collect()
}

/// Resize a white-level list to `components` entries, repeating the last known
/// value when the file recorded fewer than one per component.
fn fit_white_levels(levels: &[f32], components: usize) -> Vec<f32> {
    debug_assert!(components > 0);

    let fallback = levels.last().copied().unwrap_or(u16::MAX as f32);
    (0..components)
        .map(|index| levels.get(index).copied().unwrap_or(fallback))
        .collect()
}

/// Reconcile `raw`'s black and white levels so Rawler can develop the file and
/// so the levels describe the same domain as the decoded samples.
///
/// Returns a human-readable note per adjustment, so the caller can record that
/// the file needed correcting. An empty list means the levels were used exactly
/// as the file recorded them.
pub fn prepare_levels(raw: &mut RawImage) -> Result<Vec<String>> {
    let mut notes = Vec::new();

    // Layout first: this reduces the levels to one entry per component, which
    // the domain rescale can then scale uniformly.
    if let Some(note) = normalize_sensor_levels(raw)? {
        notes.push(note);
    }
    if let Some(note) = reconcile_sample_domain(raw) {
        notes.push(note);
    }

    Ok(notes)
}

/// Reconcile `raw`'s black and white levels into a layout Rawler can develop.
fn normalize_sensor_levels(raw: &mut RawImage) -> Result<Option<String>> {
    match raw.photometric {
        RawPhotometricInterpretation::BlackIsZero => {
            bail!(
                "Rawler 0.7.2 cannot develop BlackIsZero raw data (this file reports {} component(s) per pixel)",
                raw.cpp
            );
        }
        // CFA data goes through `as_bayer_array`, which tolerates any length by
        // broadcasting the first level. Nothing to reconcile.
        RawPhotometricInterpretation::Cfa(_) => Ok(None),
        RawPhotometricInterpretation::LinearRaw => {
            let components = raw.cpp;
            if components == 0 {
                bail!("linear raw image reports zero components per pixel");
            }

            let black = raw.blacklevel.as_vec();
            let white = raw.whitelevel.as_vec();

            if black.len() == components && white.len() == components {
                return Ok(None);
            }

            let original = format!(
                "black levels {} ({}x{} cpp={}), white levels {}",
                black.len(),
                raw.blacklevel.width,
                raw.blacklevel.height,
                raw.blacklevel.cpp,
                white.len()
            );

            let collapsed_black = collapse_black_levels(&black, components);
            let fitted_white = fit_white_levels(&white, components);

            for (index, (black, white)) in
                collapsed_black.iter().zip(fitted_white.iter()).enumerate()
            {
                if !(white - black).is_finite() || white <= black {
                    bail!(
                        "component {index} has an unusable level range (black {black}, white {white})"
                    );
                }
            }

            raw.blacklevel = BlackLevel {
                levels: collapsed_black.iter().copied().map(to_rational).collect(),
                width: 1,
                height: 1,
                cpp: components,
            };
            raw.whitelevel = WhiteLevel::new(
                fitted_white
                    .iter()
                    .map(|value| value.max(0.0).round() as u32)
                    .collect::<Vec<u32>>(),
            );

            Ok(Some(format!(
                "linear raw levels normalized to {components} component(s); file recorded {original}"
            )))
        }
    }
}

/// Largest value the u16 sample container can hold.
const CONTAINER_MAX: f32 = u16::MAX as f32;

/// Highest decoded sample value, or `None` for float data (already normalized).
fn observed_maximum(data: &RawImageData) -> Option<u16> {
    match data {
        RawImageData::Integer(samples) => samples.iter().copied().max(),
        RawImageData::Float(_) => None,
    }
}

/// Reconcile the level domain with the domain the decoded samples actually occupy.
///
/// `BitsPerSample` defines the legal sample range: nothing may exceed
/// `2^bps - 1`. When the decoder emits values above that ceiling it did not
/// honor `bps` — the samples were scaled into the full u16 storage container
/// instead. Rawler documents exactly this failure for its JPEG-XL path
/// ("jxl_oxide scales the pixels into full range of storage type ... This
/// breaks blacklevel scaling"), and the same mismatch appears with lossless
/// JPEG streams whose SOF precision exceeds the TIFF's `BitsPerSample` — as in
/// Samsung's 12-bit linear DNGs, which decode to a maximum near 65535 while
/// recording `WhiteLevel` 4095.
///
/// Because black/white correction is `(sample - black) / (white - black)`,
/// scaling both levels into the sample domain is exactly equivalent to scaling
/// every sample back down, and costs one pass over the levels rather than one
/// over the pixels.
///
/// The trigger is a hard inconsistency rather than a tuned threshold, but it
/// does depend on the image containing at least one sample above `2^bps - 1`.
/// A frame underexposed by more than `16 - bps` stops everywhere would not trip
/// it; see `docs/KNOWN_LIMITATIONS.md`.
fn reconcile_sample_domain(raw: &mut RawImage) -> Option<String> {
    if raw.bps == 0 || raw.bps >= 16 {
        return None;
    }

    let container_max = ((1_u32 << raw.bps) - 1) as f32;
    let observed = observed_maximum(&raw.data)? as f32;
    if observed <= container_max {
        return None;
    }

    let scale = CONTAINER_MAX / container_max;
    let white = raw.whitelevel.as_vec();
    // Only act when the recorded levels are the ones out of domain. If the file
    // already recorded a white level covering the decoded data, trust it.
    if white.iter().cloned().fold(0.0_f32, f32::max) >= observed {
        return None;
    }

    raw.blacklevel.levels.iter_mut().for_each(|level| {
        *level = to_rational(level.as_f32() * scale);
    });
    raw.whitelevel
        .0
        .iter_mut()
        .for_each(|level| *level = ((*level as f32 * scale).round() as u32).min(u16::MAX as u32));

    Some(format!(
        "sample domain rescaled by {scale:.4}: {}-bit levels but decoded samples reach {} (max legal {})",
        raw.bps, observed as u32, container_max as u32
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_pattern_collapses_per_component() {
        // 2x2 repeat, cpp=3, second component offset by 1.0 at every position.
        let levels = vec![
            0.0, 1.0, 2.0, //
            0.0, 1.0, 2.0, //
            0.0, 1.0, 2.0, //
            0.0, 1.0, 2.0,
        ];
        assert_eq!(collapse_black_levels(&levels, 3), vec![0.0, 1.0, 2.0]);
    }

    #[test]
    fn differing_repeat_positions_average() {
        // 1x2 repeat, cpp=2: component 0 is 0 then 4, component 1 is 2 then 6.
        let levels = vec![0.0, 2.0, 4.0, 6.0];
        assert_eq!(collapse_black_levels(&levels, 2), vec![2.0, 4.0]);
    }

    #[test]
    fn indivisible_pattern_falls_back_to_flat_average() {
        let levels = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(collapse_black_levels(&levels, 3), vec![3.0; 3]);
    }

    #[test]
    fn white_levels_are_repeated_and_truncated() {
        assert_eq!(fit_white_levels(&[4095.0], 3), vec![4095.0; 3]);
        assert_eq!(fit_white_levels(&[1.0, 2.0, 3.0, 4.0], 2), vec![1.0, 2.0]);
        assert_eq!(fit_white_levels(&[], 1), vec![u16::MAX as f32]);
    }

    #[test]
    fn rational_roundtrip_keeps_fractional_black_levels() {
        assert!((to_rational(256.28).as_f32() - 256.28).abs() < 0.001);
        assert_eq!(to_rational(-5.0).as_f32(), 0.0);
    }

    /// Black/white correction is `(sample - black) / (white - black)`, so
    /// scaling the levels into the sample domain must normalize a full-scale
    /// sample to 1.0 exactly as correcting the samples themselves would.
    #[test]
    fn rescaled_levels_normalize_the_same_as_rescaled_samples() {
        let bps = 12_u32;
        let container_max = ((1_u32 << bps) - 1) as f32; // 4095
        let scale = CONTAINER_MAX / container_max;

        let recorded_black = 64.0_f32;
        let recorded_white = container_max;

        // A 12-bit sample the decoder emitted in the 16-bit container.
        let sample_12bit = 2048.0_f32;
        let sample_16bit = sample_12bit * scale;

        let via_samples = (sample_12bit - recorded_black) / (recorded_white - recorded_black);
        let via_levels = (sample_16bit - recorded_black * scale)
            / (recorded_white * scale - recorded_black * scale);

        assert!((via_samples - via_levels).abs() < 1.0e-5);
    }

    #[test]
    fn full_scale_sample_maps_to_one_after_rescale() {
        let scale = CONTAINER_MAX / 4095.0;
        let white = 4095.0 * scale;
        assert!((CONTAINER_MAX / white - 1.0).abs() < 1.0e-5);
    }
}
