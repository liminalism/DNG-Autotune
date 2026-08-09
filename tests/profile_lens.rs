use raw_autotune::lens::{ImageGeometry, LensCorrection, LensCorrectionMode};
use std::path::PathBuf;

fn sony_prime_raw() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("raw/raw_3rd_batch/_DSC1289.ARW")
}

#[test]
fn sony_a7c_prime_resolves_one_exact_bundled_profile() {
    let path = sony_prime_raw();
    let correction = LensCorrection::resolve(&path, LensCorrectionMode::ProfileExact)
        .expect("profile-exact always returns a diagnostic result");
    let report = correction.into_report();

    assert_eq!(report.source, "lensfun_profile");
    assert_eq!(report.match_status, Some("exact_match"));
    assert_eq!(report.database_version, Some("lensfun-0.7.0-bundled"));
    assert_eq!(report.matched_camera.as_deref(), Some("Sony ILCE-7C"));
    assert_eq!(report.matched_lens.as_deref(), Some("Sony FE 24mm f/2.8 G"));
}

#[test]
fn exact_profile_resamples_rgb_and_clip_confidence_identically() {
    let mut correction =
        LensCorrection::resolve(&sony_prime_raw(), LensCorrectionMode::ProfileExact)
            .expect("the corpus camera and lens have one exact profile");
    let original: Vec<[f32; 3]> = (0..81)
        .map(|index| {
            let value = index as f32 / 100.0;
            [value, value * 0.75, value * 0.5]
        })
        .collect();
    let mut pixels = original.clone();
    let mut confidence = original.clone();
    correction.apply_three_with_confidence(
        &mut pixels,
        Some(&mut confidence),
        9,
        9,
        ImageGeometry {
            full_width: 9,
            full_height: 9,
            origin_x: 0,
            origin_y: 0,
        },
    );

    assert_eq!(pixels, confidence);
    assert_ne!(pixels, original);
}

#[test]
fn off_never_reads_or_constructs_a_correction() {
    assert!(
        LensCorrection::resolve(
            std::path::Path::new("not-even-a-file.ARW"),
            LensCorrectionMode::Off,
        )
        .is_none()
    );
}
