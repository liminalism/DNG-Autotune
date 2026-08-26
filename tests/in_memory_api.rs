use raw_autotune::api::{RenderOptions, render_file_rgb16};
use raw_autotune::metadata::{
    ORIENTATION_NORMAL, OutputColorSpace, SourceMetadata, srgb_icc_profile,
};
use rawler::exif::Exif;
use rawler::formats::tiff::GenericTiffReader;
use rawler::formats::tiff::reader::TiffReader;
use std::io::Cursor;
use std::path::Path;

const FIXTURE: &str = "raw/raw_old/files_2026-07-27_16-48-01/proshot.dng";

#[test]
fn automatic_render_returns_a_direct_owned_rgb16_buffer() {
    let path = Path::new(FIXTURE);
    if !path.exists() {
        eprintln!("skipping in-memory render integration test: local RAW corpus is absent");
        return;
    }

    let image =
        render_file_rgb16(path, &RenderOptions::automatic()).expect("ProShot DNG should render");
    assert_eq!((image.width, image.height), (4064, 3044));
    assert_eq!(image.row_stride, image.width as usize * 3);
    assert_eq!(image.data.len(), image.row_stride * image.height as usize);
    let lens = image
        .report
        .color
        .lens_correction
        .expect("the fixture carries OpcodeList3");
    assert_eq!(lens.rectilinear_warps, 1);
    assert_eq!(lens.opcodes_applied, 1);
    assert!(lens.max_displacement_pixels > 1.0);
}

/// The cross-repo handoff contract: an encoder must be able to write a faithful
/// archive file from what this API returns, without re-opening the source RAW
/// and without assuming a colour space.
#[test]
fn the_in_memory_render_carries_exif_icc_and_an_explicit_colour_space() {
    let path = Path::new(FIXTURE);
    if !path.exists() {
        eprintln!("skipping in-memory metadata test: local RAW corpus is absent");
        return;
    }

    let image = render_file_rgb16(path, &RenderOptions::automatic()).expect("ProShot DNG renders");

    // The colour space is stated, not implied.
    assert_eq!(image.color_space, OutputColorSpace::Srgb);
    assert_eq!(
        image.icc.as_deref(),
        Some(srgb_icc_profile()),
        "the ICC bytes must be the profile the CLI embeds"
    );

    // The EXIF blob is the same builder the file writers use, at the rendered
    // dimensions. Comparing against `exif_payload` directly is what makes
    // "byte-identical to what the CLI writers embed" a checked claim rather
    // than a comment: `output.rs` has no EXIF construction of its own.
    let expected = SourceMetadata::read(path)
        .exif_payload(image.width, image.height)
        .expect("the fixture's EXIF serializes");
    let exif = image.exif.as_deref().expect("the fixture carries EXIF");
    assert_eq!(exif, expected.as_slice());

    // And it is a bare TIFF structure — no `Exif\0\0`, no APP1 framing — that
    // parses on its own, with the orientation already normalized because the
    // pixels are upright.
    assert!(
        !exif.starts_with(b"Exif\0\0"),
        "the blob must not carry the JPEG APP1 header"
    );
    let mut cursor = Cursor::new(exif);
    let tiff =
        GenericTiffReader::new(&mut cursor, 0, 0, None, &[]).expect("EXIF parses standalone");
    let parsed = Exif::new(tiff.root_ifd()).expect("EXIF decodes");
    assert_eq!(parsed.orientation, Some(ORIENTATION_NORMAL));

    // Opting out costs nothing and returns nothing.
    let mut bare = RenderOptions::automatic();
    bare.metadata = false;
    let plain = render_file_rgb16(path, &bare).expect("ProShot DNG renders");
    assert!(plain.exif.is_none() && plain.icc.is_none());
    assert_eq!(
        plain.data, image.data,
        "metadata must never change a single pixel"
    );
}
