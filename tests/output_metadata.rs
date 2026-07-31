//! The writer boundary: `output::save_image_with_metadata` is what the pipeline
//! calls, so it gets tested through the crate's public surface rather than only
//! through `metadata`'s internals.
//!
//! Two things are checked here that the unit tests cannot:
//!
//! 1. **The off path is byte-identical.** `save_image` and
//!    `save_image_with_metadata(.., None)` must produce the same bytes for all
//!    three formats, because the regression gate requires that output not move
//!    when a new feature is switched off.
//! 2. **Real source metadata reaches every format.** Read from an actual RAW
//!    under `raw/`, written through all three encoders, and parsed back.
//!
//! `raw/` is not distributable, so the real-source cases skip when it is absent.

use raw_autotune::metadata::{ORIENTATION_NORMAL, SourceMetadata, srgb_icc_profile};
use raw_autotune::output::{save_image, save_image_with_metadata};
use raw_autotune::tone::Rgb16Image;
use raw_autotune::types::{JpegSettings, OutputFormat};
use rawler::exif::Exif;
use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, Rational};
use std::io::Cursor;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("raw-autotune-output-metadata-tests");
    std::fs::create_dir_all(&dir).expect("scratch directory");
    dir.join(name)
}

fn gradient(width: u32, height: u32) -> Rgb16Image {
    Rgb16Image::from_fn(width, height, |x, y| {
        let value = ((x * 613 + y * 271) % 65536) as u16;
        image::Rgb([value, value.wrapping_add(2048), value / 3])
    })
}

const FORMATS: [(OutputFormat, &str); 3] = [
    (OutputFormat::Jpeg, "jpg"),
    (OutputFormat::Tiff, "tif"),
    (OutputFormat::Png, "png"),
];

/// The whole "features off" guarantee, in one assertion per format.
#[test]
fn no_metadata_is_byte_identical_to_the_legacy_writer() {
    let image = gradient(40, 90);
    for (format, extension) in FORMATS {
        let legacy = scratch(&format!("legacy.{extension}"));
        let explicit = scratch(&format!("explicit-none.{extension}"));
        save_image(&legacy, image.clone(), format, 90).expect("legacy write");
        save_image_with_metadata(
            &explicit,
            image.clone(),
            format,
            &JpegSettings::from_quality(90),
            None,
        )
        .expect("write");
        assert_eq!(
            std::fs::read(&legacy).expect("readable"),
            std::fs::read(&explicit).expect("readable"),
            "{extension}: passing None must not change a single byte"
        );
    }
}

/// EXIF must survive every format, and the orientation override must hold in all
/// three. A synthetic source so the expected values are exact.
#[test]
fn every_format_carries_exif_and_reports_upright() {
    let metadata = SourceMetadata::from_exif(Exif {
        orientation: Some(8), // Rotate 270: must not reach the output.
        date_time_original: Some("2026:03:04 09:08:07".to_string()),
        exposure_time: Some(Rational::new(1, 60)),
        fnumber: Some(Rational::new(4, 1)),
        iso_speed_ratings: Some(800),
        focal_length: Some(Rational::new(85, 1)),
        ..Default::default()
    })
    .with_camera("TESTMAKE", "TESTMODEL");

    let image = gradient(40, 90);
    for (format, extension) in FORMATS {
        let path = scratch(&format!("tagged.{extension}"));
        save_image_with_metadata(
            &path,
            image.clone(),
            format,
            &JpegSettings::from_quality(90),
            Some(&metadata),
        )
        .expect("write");
        let bytes = std::fs::read(&path).expect("readable");

        let exif_block = match format {
            // A TIFF is its own EXIF container.
            OutputFormat::Tiff => bytes.clone(),
            OutputFormat::Jpeg => {
                jpeg_app1(&bytes).unwrap_or_else(|| panic!("{extension}: no APP1 Exif segment"))
            }
            OutputFormat::Png => {
                png_chunk(&bytes, b"eXIf").unwrap_or_else(|| panic!("{extension}: no eXIf chunk"))
            }
        };

        let mut cursor = Cursor::new(&exif_block);
        let tiff = GenericTiffReader::new(&mut cursor, 0, 0, None, &[])
            .unwrap_or_else(|error| panic!("{extension}: EXIF does not parse: {error}"));
        let exif = Exif::new(tiff.root_ifd())
            .unwrap_or_else(|error| panic!("{extension}: EXIF does not decode: {error}"));

        assert_eq!(
            exif.orientation,
            Some(ORIENTATION_NORMAL),
            "{extension}: the pixels are already rotated, so the tag must say 1"
        );
        assert_eq!(
            exif.date_time_original.as_deref(),
            Some("2026:03:04 09:08:07")
        );
        assert_eq!(exif.exposure_time, Some(Rational::new(1, 60)));
        assert_eq!(exif.fnumber, Some(Rational::new(4, 1)));
        assert_eq!(exif.iso_speed_ratings, Some(800));
        assert_eq!(exif.focal_length, Some(Rational::new(85, 1)));
        assert_eq!(exif.color_space, Some(1), "{extension}: must declare sRGB");

        // And every format must be decodable afterwards, with the dimensions the
        // renderer produced.
        let decoded = image::open(&path)
            .unwrap_or_else(|error| panic!("{extension}: written file does not decode: {error}"));
        assert_eq!((decoded.width(), decoded.height()), (40, 90));
    }
}

/// The ICC profile must be embedded, byte-identical, wherever it is stored
/// uncompressed. PNG's `iCCP` is deflated, so only presence is checked there.
#[test]
fn every_format_embeds_the_srgb_profile() {
    let image = gradient(16, 16);
    let metadata = SourceMetadata::default();

    let jpeg = scratch("icc.jpg");
    save_image_with_metadata(
        &jpeg,
        image.clone(),
        OutputFormat::Jpeg,
        &JpegSettings::from_quality(90),
        Some(&metadata),
    )
    .expect("write");
    let bytes = std::fs::read(&jpeg).expect("readable");
    assert_eq!(
        jpeg_app2(&bytes).expect("APP2 ICC_PROFILE segment"),
        srgb_icc_profile()
    );

    let tiff = scratch("icc.tif");
    save_image_with_metadata(
        &tiff,
        image.clone(),
        OutputFormat::Tiff,
        &JpegSettings::from_quality(90),
        Some(&metadata),
    )
    .expect("write");
    let bytes = std::fs::read(&tiff).expect("readable");
    assert!(
        find(&bytes, srgb_icc_profile()).is_some(),
        "the TIFF must contain the profile verbatim"
    );

    let png = scratch("icc.png");
    save_image_with_metadata(
        &png,
        image,
        OutputFormat::Png,
        &JpegSettings::from_quality(90),
        Some(&metadata),
    )
    .expect("write");
    let bytes = std::fs::read(&png).expect("readable");
    assert!(
        png_chunk(&bytes, b"iCCP").is_some(),
        "the PNG must carry an iCCP chunk"
    );
}

/// The same input written twice must produce the same bytes, for every format.
#[test]
fn writes_are_reproducible() {
    let metadata = SourceMetadata::from_exif(Exif {
        date_time_original: Some("2026:03:04 09:08:07".to_string()),
        ..Default::default()
    })
    .with_camera("TESTMAKE", "TESTMODEL");
    let image = gradient(31, 71);

    for (format, extension) in FORMATS {
        let first = scratch(&format!("repeat-a.{extension}"));
        let second = scratch(&format!("repeat-b.{extension}"));
        save_image_with_metadata(
            &first,
            image.clone(),
            format,
            &JpegSettings::from_quality(90),
            Some(&metadata),
        )
        .expect("write");
        save_image_with_metadata(
            &second,
            image.clone(),
            format,
            &JpegSettings::from_quality(90),
            Some(&metadata),
        )
        .expect("write");
        assert_eq!(
            std::fs::read(&first).expect("readable"),
            std::fs::read(&second).expect("readable"),
            "{extension}: repeated writes must be byte-identical"
        );
    }
}

/// End to end from a real camera file: read its metadata, write all three
/// formats, read the capture time back.
#[test]
fn real_raw_reaches_every_format() {
    let Some(source) = find_source() else {
        eprintln!("skipping real_raw_reaches_every_format: no RAW corpus under raw/");
        return;
    };

    let metadata = SourceMetadata::read(&source);
    let captured = metadata
        .date_time_original()
        .unwrap_or_else(|| panic!("{} has no DateTimeOriginal", source.display()))
        .to_string();

    let image = gradient(48, 32);
    for (format, extension) in FORMATS {
        let path = scratch(&format!("real.{extension}"));
        save_image_with_metadata(
            &path,
            image.clone(),
            format,
            &JpegSettings::from_quality(90),
            Some(&metadata),
        )
        .expect("write");
        let bytes = std::fs::read(&path).expect("readable");
        let block = match format {
            OutputFormat::Tiff => bytes.clone(),
            OutputFormat::Jpeg => jpeg_app1(&bytes).expect("APP1"),
            OutputFormat::Png => png_chunk(&bytes, b"eXIf").expect("eXIf"),
        };
        let mut cursor = Cursor::new(&block);
        let tiff = GenericTiffReader::new(&mut cursor, 0, 0, None, &[]).expect("EXIF parses");
        let exif = Exif::new(tiff.root_ifd()).expect("EXIF decodes");
        assert_eq!(
            exif.date_time_original.as_deref(),
            Some(captured.as_str()),
            "{extension}: capture time lost"
        );
        assert_eq!(exif.orientation, Some(ORIENTATION_NORMAL));
    }
}

// --- helpers ---

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn jpeg_segments(bytes: &[u8]) -> Vec<(u8, &[u8])> {
    let mut segments = Vec::new();
    let mut index = 2;
    while index + 4 <= bytes.len() && bytes[index] == 0xFF {
        let marker = bytes[index + 1];
        // SOI, SOS, EOI: nothing past here is a metadata segment.
        if matches!(marker, 0xD8..=0xDA) {
            break;
        }
        let length = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
        if length < 2 || index + 2 + length > bytes.len() {
            break;
        }
        segments.push((marker, &bytes[index + 4..index + 2 + length]));
        index += 2 + length;
    }
    segments
}

fn jpeg_app1(bytes: &[u8]) -> Option<Vec<u8>> {
    jpeg_segments(bytes)
        .into_iter()
        .find(|(marker, payload)| *marker == 0xE1 && payload.starts_with(b"Exif\0\0"))
        .map(|(_, payload)| payload[6..].to_vec())
}

fn jpeg_app2(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut profile = Vec::new();
    for (marker, payload) in jpeg_segments(bytes) {
        if marker == 0xE2 && payload.starts_with(b"ICC_PROFILE\0") {
            profile.extend_from_slice(&payload[14..]);
        }
    }
    (!profile.is_empty()).then_some(profile)
}

fn png_chunk(bytes: &[u8], name: &[u8; 4]) -> Option<Vec<u8>> {
    let mut index = 8;
    while index + 12 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[index..index + 4].try_into().unwrap()) as usize;
        let kind = &bytes[index + 4..index + 8];
        if kind == name {
            return Some(bytes[index + 8..index + 8 + length].to_vec());
        }
        if kind == b"IEND" {
            break;
        }
        index += 12 + length;
    }
    None
}

fn find_source() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("raw");
    if !root.is_dir() {
        return None;
    }
    let mut found: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    value.eq_ignore_ascii_case("arw") || value.eq_ignore_ascii_case("dng")
                })
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found.into_iter().next()
}
