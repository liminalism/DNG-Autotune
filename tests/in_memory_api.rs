use raw_autotune::api::{RenderOptions, render_file_rgb16};
use std::path::Path;

#[test]
fn automatic_render_returns_a_direct_owned_rgb16_buffer() {
    let path = Path::new("raw/raw_old/files_2026-07-27_16-48-01/proshot.dng");
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
