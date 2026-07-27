//! Capture settings read from the file's EXIF.
//!
//! Added to validate the noise model in [`crate::noise`]: sensor read noise
//! scales with gain, so `shot_slope` must rise with ISO on frames from one
//! camera. Without that check there is no way to tell a real spread in the
//! noise estimate from a broken estimator.
//!
//! Rawler exposes metadata only through the decoder trait, not through a
//! convenience function, so the loader is built once and reused; constructing
//! it parses the bundled camera database and is far too slow to repeat per file.

use rawler::RawLoader;
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;
use serde::Serialize;
use std::path::Path;
use std::sync::OnceLock;

/// Capture settings, as far as the file records them.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ShotInfo {
    /// ISO sensitivity. `iso_speed_ratings` where present, else `iso_speed`.
    pub iso: Option<u32>,
    /// Exposure time in seconds.
    pub exposure_time: Option<f32>,
    /// Aperture as an f-number.
    pub f_number: Option<f32>,
}

impl ShotInfo {
    /// True when nothing at all could be read.
    pub fn is_empty(&self) -> bool {
        self.iso.is_none() && self.exposure_time.is_none() && self.f_number.is_none()
    }
}

fn loader() -> &'static RawLoader {
    static LOADER: OnceLock<RawLoader> = OnceLock::new();
    LOADER.get_or_init(RawLoader::new)
}

/// Read capture settings from `path`, or `None` when they cannot be recovered.
///
/// Best effort throughout: a file that develops fine but whose metadata cannot
/// be parsed must not fail the run.
pub fn read(path: &Path) -> Option<ShotInfo> {
    let source = RawSource::new(path).ok()?;
    let decoder = loader().get_decoder(&source).ok()?;
    let metadata = decoder
        .raw_metadata(&source, &RawDecodeParams::default())
        .ok()?;
    let exif = metadata.exif;

    let rational = |value: Option<rawler::formats::tiff::Rational>| -> Option<f32> {
        let value = value?;
        if value.d == 0 {
            return None;
        }
        let result = value.n as f32 / value.d as f32;
        result.is_finite().then_some(result)
    };

    let info = ShotInfo {
        iso: exif
            .iso_speed_ratings
            .map(u32::from)
            .filter(|iso| *iso > 0)
            .or(exif.iso_speed.filter(|iso| *iso > 0)),
        exposure_time: rational(exif.exposure_time),
        f_number: rational(exif.fnumber),
    };

    (!info.is_empty()).then_some(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_info_is_recognised() {
        assert!(ShotInfo::default().is_empty());
        assert!(
            !ShotInfo {
                iso: Some(100),
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn missing_files_do_not_panic() {
        assert!(read(Path::new("no-such-file.ARW")).is_none());
    }
}
