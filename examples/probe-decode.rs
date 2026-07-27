//! Debug helper: write a downsampled PNG from the decoded sample buffer, after
//! raw-autotune's decode corrections but before any tone processing. If this
//! looks like the scene, decoding is fine and the fault is downstream.
//!
//! Pass `--raw` to bypass the corrections and see what Rawler alone produced.
//!
//! cargo run --release --example probe-decode -- <file.dng> <out.png>

use rawler::RawImageData;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: probe-decode <file.dng> <out.png>");
    let out = args
        .next()
        .expect("usage: probe-decode <file.dng> <out.png>");

    let uncorrected = std::env::args().any(|a| a == "--raw");
    let raw = if uncorrected {
        rawler::decode_file(&path)?
    } else {
        raw_autotune::decode_corrected(std::path::Path::new(&path))?.0
    };
    let cpp = raw.cpp;
    // `RawImage::new_with_data` stores `sample_width / cpp`, so `raw.width` is
    // the pixel width and a sample row is `width * cpp` long.
    let pixels_per_row = raw.width;
    let row_stride = raw.width * cpp;
    let height = raw.height;

    let data: Vec<f32> = match &raw.data {
        RawImageData::Integer(samples) => samples.iter().map(|v| *v as f32).collect(),
        RawImageData::Float(samples) => samples.clone(),
    };
    let maximum = data.iter().copied().fold(0.0_f32, f32::max).max(1.0);

    let step = (pixels_per_row / 800).max(1);
    let out_w = pixels_per_row / step;
    let out_h = height / step;
    println!("{path}: {pixels_per_row}x{height} cpp={cpp} max={maximum} -> {out_w}x{out_h}");

    let mut buffer = Vec::with_capacity(out_w * out_h * 3);
    for y in 0..out_h {
        for x in 0..out_w {
            let base = (y * step) * row_stride + (x * step) * cpp;
            let sample = |c: usize| {
                let value = data.get(base + c.min(cpp - 1)).copied().unwrap_or(0.0) / maximum;
                // Rough gamma so shadow detail is visible in the dump.
                (value.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8
            };
            buffer.extend_from_slice(&[sample(0), sample(1), sample(2)]);
        }
    }

    image::save_buffer(
        &out,
        &buffer,
        out_w as u32,
        out_h as u32,
        image::ColorType::Rgb8,
    )?;
    println!("wrote {out}");
    Ok(())
}
