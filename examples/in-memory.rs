//! Minimal embedding example: render a RAW and move packed RGB to a consumer.

use anyhow::{Context, Result};
use raw_autotune::api::{PixelFormat, RenderOptions, render_file};

fn main() -> Result<()> {
    let input = std::env::args_os()
        .nth(1)
        .context("usage: cargo run --example in-memory -- INPUT")?;
    let mut options = RenderOptions::automatic();
    options.pixel_format = PixelFormat::Rgb8Srgb;

    let image = render_file(input, &options)?;
    println!(
        "{}x{} {:?}, stride {}, {} bytes, lens opcodes applied: {}",
        image.width,
        image.height,
        image.pixel_format,
        image.row_stride,
        image.data.len(),
        image
            .report
            .color
            .lens_correction
            .as_ref()
            .map_or(0, |report| report.opcodes_applied),
    );

    // `image` owns the buffer and is Send. A real embedding can now do:
    // sender.send(image)?;
    Ok(())
}
