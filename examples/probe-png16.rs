//! Decode a PNG to raw 16-bit RGB on stdout, losslessly.
//!
//! `tools/evaluate_sky_stages.py` needs the exact encoded samples of the stage
//! dumps, which are 16-bit RGB. It used to get them by shelling out to
//! ImageMagick (`convert -depth 16 RGB:-`), which is not installed everywhere
//! and is a second image stack to keep working. Pillow is not a substitute: its
//! PNG reader silently narrows 16-bit RGB to 8-bit, so 30000 and 30001 both
//! decode to 117 and every pixel-delta and OKLab number measured from it would
//! be quantized to 8 bits without saying so.
//!
//! This probe uses the same `image` crate the program already writes its output
//! with, so the evaluator reads the stage dumps back through the decoder that
//! matches the encoder.
//!
//! ```text
//! cargo run --release --example probe-png16 -- stage.png            > raw.bin
//! cargo run --release --example probe-png16 -- stage.png 16 32 8 8  > crop.bin
//! ```
//!
//! Output is a self-describing little-endian stream: `u32` width, `u32` height,
//! then `width * height * 3` `u16` samples in RGB order. Sixteen-bit input is
//! passed through untouched; 8-bit input is widened by the `image` crate's
//! standard scaling so the stream is always 16-bit.

use anyhow::{Context, Result, bail};
use std::io::{BufWriter, Write};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = match args.next() {
        Some(path) => path,
        None => bail!(
            "usage: probe-png16 <file.png> [x y width height]\n\
             writes u32 width, u32 height, then width*height*3 u16 samples, all little-endian"
        ),
    };

    let crop = {
        let rest: Vec<String> = args.collect();
        match rest.len() {
            0 => None,
            4 => {
                let mut value = [0u32; 4];
                for (slot, text) in value.iter_mut().zip(&rest) {
                    *slot = text
                        .parse()
                        .with_context(|| format!("crop argument {text} is not a u32"))?;
                }
                if value[2] == 0 || value[3] == 0 {
                    bail!("crop width and height must both be non-zero");
                }
                Some(value)
            }
            other => bail!("expected 0 or 4 crop arguments, got {other}"),
        }
    };

    let decoded =
        image::open(&path).with_context(|| format!("could not decode {path} as an image"))?;

    // `to_rgb16` is a no-op view for a 16-bit RGB PNG and a widening conversion
    // for anything else, so the stream is 16-bit regardless of what was stored.
    let full = decoded.to_rgb16();
    let image = match crop {
        None => full,
        Some([x, y, width, height]) => {
            let (full_width, full_height) = (full.width(), full.height());
            if x + width > full_width || y + height > full_height {
                bail!(
                    "crop {width}x{height}+{x}+{y} does not fit inside {full_width}x{full_height}"
                );
            }
            image::imageops::crop_imm(&full, x, y, width, height).to_image()
        }
    };

    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    out.write_all(&image.width().to_le_bytes())?;
    out.write_all(&image.height().to_le_bytes())?;
    // `ImageBuffer<Rgb<u16>, _>` is tightly packed in RGB order, so the only
    // thing to fix is byte order on a big-endian host.
    for sample in image.as_raw() {
        out.write_all(&sample.to_le_bytes())?;
    }
    out.flush()?;
    Ok(())
}
