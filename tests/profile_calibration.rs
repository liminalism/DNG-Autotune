//! `--preset vivid` develops through the bundled profile, or not at all.
//!
//! The table in `profiles/SONY_ILCE-7C.dcp` is the residual left by that
//! profile's own `ForwardMatrix`. Applied after any other matrix it is a hue
//! rotation with no basis — measured at +15° on blue and +10° on warm before
//! these two rules landed. So the pair travels together, and a file the profile
//! was not made for gets neither half.
//!
//! The RAW corpus is intentionally not distributable; a checkout without it
//! skips rather than weakening the unit tests in `src/huesatmap.rs` and
//! `src/dngcolor.rs`.

use raw_autotune::color::{self, DevelopOptions, WorkingSpace};
use raw_autotune::demosaic::DemosaicMethod;
use raw_autotune::huesatmap::HueSatMap;
use raw_autotune::lens::LensCorrectionMode;
use raw_autotune::raw_highlight::HighlightMethod;
use raw_autotune::rescale::SubBlack;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn corpus(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// The archive defaults, spelled out. `DevelopOptions` deliberately has no
/// `Default`, so that a new stage cannot be silently omitted at a call site;
/// this test honours that by naming every field rather than adding one.
fn options(table: Option<Arc<HueSatMap>>, strength: f32) -> DevelopOptions {
    DevelopOptions {
        working_space: WorkingSpace::Srgb,
        sub_black: SubBlack::Preserve,
        hot_pixels: 0.5,
        highlight_reconstruction: 1.0,
        highlight_method: HighlightMethod::Current,
        spatial_highlight_floor: 0.0,
        demosaic: DemosaicMethod::Auto,
        snr10_ev: None,
        full_dng_color: true,
        lens_correction: LensCorrectionMode::Embedded,
        dump_stages: None,
        illuminant_proxy: false,
        hue_sat_map: table,
        hue_sat_map_strength: strength,
    }
}

fn develop(path: &Path, options: DevelopOptions) -> (Vec<[f32; 3]>, color::ColorReport) {
    let raw = rawler::decode_file(path).expect("corpus file decodes");
    let (image, report, _, _) = color::develop(&raw, path, options).expect("develops");
    (image.pixels, report)
}

/// The Sony half: the profile matches, so both halves apply and the scene CCT
/// interpolates the profile's two calibration illuminants. Before this an ARW
/// carried no DNG `ColorMatrix1`, so no CCT existed and the Std A table was
/// unreachable on the only camera it was calibrated for.
#[test]
fn a_matching_profile_supplies_both_the_matrix_and_the_interpolated_table() {
    let path = corpus("raw/arw_better/_DSC1236.ARW");
    if !path.exists() {
        eprintln!("skipping: corpus RAW not present");
        return;
    }
    let table = Arc::new(HueSatMap::bundled_sony_a7c().expect("bundled profile"));
    let (pixels, report) = develop(&path, options(Some(table), 1.0));

    let dng = report
        .dng_color
        .expect("the profile route reports a calibration");
    assert_eq!(
        dng.profile_source.as_deref(),
        Some("profiles/SONY_ILCE-7C.dcp")
    );
    assert!(
        dng.used_forward_matrix,
        "the profile's own matrix must drive it"
    );
    assert!(
        dng.weight_illuminant1 > 0.0 && dng.weight_illuminant1 < 1.0,
        "both calibrations must contribute, got {:?}",
        dng.interpolation_weights
    );

    let map = report.hue_sat_map.expect("an applied table is reported");
    assert_eq!(map.table, "interpolated");
    assert_eq!(map.strength, 1.0);
    assert!(
        map.skipped.is_none(),
        "a matching profile must not be skipped"
    );
    assert_eq!(map.cct, Some(dng.estimated_cct));

    assert!(pixels.iter().flatten().all(|c| c.is_finite()));
}

/// The other half: a body the profile does not name gets neither the table nor
/// the profile's matrix, and the pixels are exactly the ones it would have had
/// with no profile at all. Equality here is the whole point — "close enough"
/// would mean the render silently depends on a flag that was declined.
#[test]
fn a_mismatched_profile_changes_nothing_and_says_why() {
    let path = corpus("raw/raw_3rd_batch/20260729_144601.dng");
    if !path.exists() {
        eprintln!("skipping: corpus RAW not present");
        return;
    }
    let table = Arc::new(HueSatMap::bundled_sony_a7c().expect("bundled profile"));
    let (with_profile, report) = develop(&path, options(Some(table), 1.0));
    let (without_profile, plain) = develop(&path, options(None, 0.0));

    assert_eq!(
        with_profile, without_profile,
        "a declined profile must leave the frame bit-identical"
    );

    let map = report
        .hue_sat_map
        .expect("a declined table is still reported");
    assert_eq!(map.strength, 0.0);
    assert_eq!(
        map.skipped,
        Some(color::HueSatSkip::CameraMismatch),
        "the sidecar has to say why the calibration did not run"
    );
    // The file's own DNG calibration is what actually developed it, so the
    // report must not claim the profile.
    assert!(
        report
            .dng_color
            .as_ref()
            .and_then(|dng| dng.profile_source.as_ref())
            .is_none(),
        "a declined profile must not be credited with the conversion"
    );
    assert!(
        plain.dng_color.is_some(),
        "the DNG carries its own calibration"
    );
}

/// Strength 0 declines before the profile is ever consulted, so it stays the
/// exact no-op the CLI documents even on the camera the profile matches.
#[test]
fn strength_zero_is_bit_identical_on_a_matching_camera() {
    let path = corpus("raw/arw_better/_DSC1236.ARW");
    if !path.exists() {
        eprintln!("skipping: corpus RAW not present");
        return;
    }
    let table = Arc::new(HueSatMap::bundled_sony_a7c().expect("bundled profile"));
    let (at_zero, report) = develop(&path, options(Some(table), 0.0));
    let (without, _) = develop(&path, options(None, 0.0));

    assert_eq!(at_zero, without);
    assert!(
        report.hue_sat_map.is_none(),
        "strength 0 is off, not a skipped calibration"
    );
}

/// The working space is a free choice at the call site, and the profile route
/// has to honour it the way the file route does.
#[test]
fn the_profile_route_respects_the_requested_working_space() {
    let path = corpus("raw/arw_better/_DSC1236.ARW");
    if !path.exists() {
        eprintln!("skipping: corpus RAW not present");
        return;
    }
    let table = Arc::new(HueSatMap::bundled_sony_a7c().expect("bundled profile"));
    let mut wide = options(Some(table), 1.0);
    wide.working_space = WorkingSpace::Rec2020;
    let (_, report) = develop(&path, wide);
    assert_eq!(report.working_space, WorkingSpace::Rec2020);
    assert!(report.hue_sat_map.is_some_and(|map| map.skipped.is_none()));
}
