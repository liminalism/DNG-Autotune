//! The camera's own JPEG, shipped alongside the RAW, used as a yardstick.
//!
//! `docs/PLAN.md` sets the bar for this program as "never clearly worse than
//! the camera's own JPEG, usually at least as good". That is only a testable
//! claim if the camera's JPEG is on disk next to the RAW and both are measured
//! the same way, which is why the plan asks for RAW+JPEG pairs while the corpus
//! is being gathered.
//!
//! This module is measurement only. Nothing here feeds the render: the
//! reference is read, reduced to a handful of numbers, and reported. The
//! embedded preview in [`crate::preview`] is the one vendor rendering that
//! *does* steer a decision, and it is deliberately a separate path — a
//! reference JPEG is an answer sheet, and grading against an answer sheet the
//! renderer has already read would prove nothing.
//!
//! # What is comparable, and when
//!
//! Two of the numbers are available without rendering anything, so they work
//! under `--dry-run` over a whole corpus:
//!
//! - `subject_display_ev`, measured with exactly the estimator
//!   [`crate::preview`] uses, against the display EV this program's curve will
//!   place its own subject at. That difference is the exposure error.
//!
//! The rest — colourfulness, saturation, blown highlights, crushed shadows —
//! need our render to exist, so they appear only in a full run.

use crate::metrics::OutputStats;
use crate::tone::map_ev;
use crate::types::ToneParams;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Extensions tried, in order, when pairing a RAW with a rendering.
const REFERENCE_EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "JPG", "JPEG"];

/// Where to look for the camera's rendering of a RAW.
#[derive(Debug, Clone)]
pub enum ReferenceSource {
    /// Do not look. The default: most runs are not corpus work.
    Disabled,
    /// Alongside the RAW itself, which is how a camera writes RAW+JPEG.
    Sibling,
    /// In one directory, for corpora whose renderings were collected separately.
    Directory(PathBuf),
}

impl ReferenceSource {
    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    /// The camera JPEG paired with `raw`, if one exists.
    ///
    /// Pairing is by file stem, which is what every camera and every phone
    /// camera app does. Extensions are tried in a fixed order so that a
    /// directory holding both `name.jpg` and `name.JPEG` resolves the same way
    /// on every run and every filesystem.
    pub fn locate(&self, raw: &Path) -> Option<PathBuf> {
        let directory = match self {
            Self::Disabled => return None,
            Self::Sibling => raw.parent()?.to_path_buf(),
            Self::Directory(directory) => directory.clone(),
        };
        let stem = raw.file_stem()?;

        REFERENCE_EXTENSIONS
            .iter()
            .map(|extension| directory.join(stem).with_extension(extension))
            .find(|candidate| candidate.is_file())
    }
}

/// The camera's rendering, measured.
#[derive(Debug, Clone, Serialize)]
pub struct ReferenceReport {
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// Subject brightness in display EV relative to middle grey, measured with
    /// the same centre weighting [`crate::preview`] and [`crate::analyze`] use.
    pub subject_display_ev: f32,
    pub p05_display_ev: f32,
    pub p50_display_ev: f32,
    pub p95_display_ev: f32,
    pub measured: OutputStats,
    /// This program's rendering minus the camera's. Positive means brighter,
    /// more colourful, or more clipped than the camera.
    pub delta: ReferenceDelta,
}

/// Ours minus the camera's, for each measure the two share.
///
/// Every field but the first is `None` under `--dry-run`, where our own
/// rendering was never produced.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ReferenceDelta {
    /// Where our curve places the subject, minus where the camera placed it.
    /// Available without rendering, because the controller's target and the
    /// tone curve are both known before a single pixel is mapped.
    pub subject_display_ev: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colourfulness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_saturation: Option<f32>,
    /// Ours divided by the camera's, which is the form the chroma path is
    /// tuned in: 1.0 means the two renderings are equally saturated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturation_ratio: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_level: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub near_white_fraction: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crushed_fraction: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub luminance_entropy: Option<f32>,
}

/// Read and measure the camera's rendering.
///
/// Best effort, like the preview reader: a corrupt or unreadable reference is
/// reported as absent, never as a failure of the file it was paired with.
pub fn read(path: &Path) -> Option<ReferenceReport> {
    let rgb = image::open(path).ok()?.into_rgb8();
    let display = crate::preview::display_ev(&rgb)?;

    Some(ReferenceReport {
        path: path.to_string_lossy().into_owned(),
        width: rgb.width(),
        height: rgb.height(),
        subject_display_ev: display.subject_ev,
        p05_display_ev: display.p05_ev,
        p50_display_ev: display.p50_ev,
        p95_display_ev: display.p95_ev,
        measured: OutputStats::measure_rgb8(&rgb),
        delta: ReferenceDelta::default(),
    })
}

/// The display EV this program's curve will place the subject at.
///
/// `target_median_ev` is the *curve-input* EV the controller aimed the subject
/// at, so it has to go through the curve before it can be compared with a
/// measurement of somebody else's finished rendering. This is the forward
/// direction of the inversion the preview oracle performs.
pub fn predicted_subject_display_ev(target_median_ev: f32, params: &ToneParams) -> f32 {
    map_ev(target_median_ev, params)
}

impl ReferenceReport {
    /// Fill in the difference against our own analysis, and against our own
    /// rendering when one was produced.
    pub fn compare(&mut self, predicted_subject_display_ev: f32, ours: Option<&OutputStats>) {
        self.delta = ReferenceDelta {
            subject_display_ev: predicted_subject_display_ev - self.subject_display_ev,
            ..Default::default()
        };

        let Some(ours) = ours else {
            return;
        };
        let reference = &self.measured;

        self.delta.colourfulness = Some(ours.colourfulness - reference.colourfulness);
        self.delta.mean_saturation = Some(ours.mean_saturation - reference.mean_saturation);
        self.delta.saturation_ratio = (reference.mean_saturation > 1.0e-6)
            .then(|| ours.mean_saturation / reference.mean_saturation);
        self.delta.mean_level = Some(ours.mean_level - reference.mean_level);
        self.delta.near_white_fraction =
            Some(ours.near_white_fraction - reference.near_white_fraction);
        self.delta.crushed_fraction = Some(ours.crushed_fraction - reference.crushed_fraction);
        self.delta.luminance_entropy = Some(ours.luminance_entropy - reference.luminance_entropy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn write_jpeg(path: &Path, image: &RgbImage) {
        image
            .save_with_format(path, image::ImageFormat::Jpeg)
            .unwrap();
    }

    fn temporary_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("raw-autotune-reference-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn textured(level: u8) -> RgbImage {
        RgbImage::from_fn(128, 96, |x, y| {
            let wobble = ((x % 8) as i16 - (y % 5) as i16) * 6;
            let channel = |base: i16| (base + wobble).clamp(0, 255) as u8;
            Rgb([
                channel(level as i16 + 30),
                channel(level as i16),
                channel(level as i16 - 25),
            ])
        })
    }

    #[test]
    fn a_disabled_source_never_pairs() {
        let directory = temporary_directory("disabled");
        let raw = directory.join("frame.dng");
        write_jpeg(&directory.join("frame.jpg"), &textured(120));
        assert!(ReferenceSource::Disabled.locate(&raw).is_none());
    }

    #[test]
    fn a_sibling_jpeg_is_found_by_stem() {
        let directory = temporary_directory("sibling");
        let raw = directory.join("frame.dng");
        let jpeg = directory.join("frame.jpg");
        write_jpeg(&jpeg, &textured(120));
        assert_eq!(ReferenceSource::Sibling.locate(&raw), Some(jpeg));
    }

    #[test]
    fn an_unpaired_raw_reports_no_reference() {
        let directory = temporary_directory("unpaired");
        write_jpeg(&directory.join("other.jpg"), &textured(120));
        assert!(
            ReferenceSource::Sibling
                .locate(&directory.join("frame.dng"))
                .is_none()
        );
    }

    #[test]
    fn a_separate_directory_is_searched_instead_of_the_siblings() {
        let raws = temporary_directory("split-raw");
        let renderings = temporary_directory("split-jpeg");
        let raw = raws.join("frame.dng");
        let jpeg = renderings.join("frame.jpg");
        write_jpeg(&jpeg, &textured(120));

        assert_eq!(
            ReferenceSource::Directory(renderings).locate(&raw),
            Some(jpeg)
        );
        assert!(ReferenceSource::Sibling.locate(&raw).is_none());
    }

    /// A brighter reference must read as a higher subject EV, since that
    /// difference is the whole exposure signal.
    #[test]
    fn a_brighter_reference_measures_higher() {
        let directory = temporary_directory("brightness");
        let dark = directory.join("dark.jpg");
        let bright = directory.join("bright.jpg");
        write_jpeg(&dark, &textured(70));
        write_jpeg(&bright, &textured(190));

        let dark = read(&dark).unwrap();
        let bright = read(&bright).unwrap();
        assert!(bright.subject_display_ev > dark.subject_display_ev + 1.0);
    }

    #[test]
    fn an_unreadable_reference_is_absent_rather_than_fatal() {
        assert!(read(Path::new("no-such-file.jpg")).is_none());
    }

    /// Under `--dry-run` only the exposure difference is knowable; claiming a
    /// colour difference we never measured would be worse than reporting none.
    #[test]
    fn without_a_render_only_the_exposure_delta_is_reported() {
        let directory = temporary_directory("dry-run");
        let path = directory.join("frame.jpg");
        write_jpeg(&path, &textured(120));

        let mut report = read(&path).unwrap();
        report.compare(report.subject_display_ev + 0.75, None);

        assert!((report.delta.subject_display_ev - 0.75).abs() < 1.0e-4);
        assert!(report.delta.colourfulness.is_none());
        assert!(report.delta.saturation_ratio.is_none());
    }

    /// The ratio is the form the chroma path is tuned in, so it has to be 1.0
    /// when a rendering is compared with itself.
    #[test]
    fn comparing_a_rendering_with_itself_gives_a_unit_saturation_ratio() {
        let directory = temporary_directory("identity");
        let path = directory.join("frame.jpg");
        write_jpeg(&path, &textured(120));

        let mut report = read(&path).unwrap();
        let ours = report.measured.clone();
        report.compare(report.subject_display_ev, Some(&ours));

        assert!((report.delta.saturation_ratio.unwrap() - 1.0).abs() < 1.0e-5);
        assert_eq!(report.delta.colourfulness, Some(0.0));
        assert_eq!(report.delta.mean_level, Some(0.0));
    }
}
