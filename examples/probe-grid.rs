//! Debug helper: print a coarse grid of normalized signal levels.
//!
//! Running this on two exposures of the same scene and plotting one against the
//! other shows whether they are related linearly. A straight line means both
//! files are scene-linear and differ only by gain; a curve means one of them has
//! a tone curve baked in.
//!
//! Blocks are large so that small framing differences between files do not
//! matter. CFA files are block-averaged across the mosaic, which mixes the color
//! planes into a rough luminance proxy; RGB files average their channels. That
//! is close enough to compare curve shape.
//!
//! cargo run --release --example probe-grid -- <file> [grid_columns]

use rawler::RawImageData;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: probe-grid <file> [grid_columns]");
    let columns: usize = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(32);

    // Use the crate's corrected decode, not Rawler's raw output: several files
    // are only decoded correctly after the repairs in `redecode`.
    let (raw, report) = raw_autotune::decode_corrected(std::path::Path::new(&path))?;
    for note in &report.notes {
        eprintln!("  note: {note}");
    }
    let cpp = raw.cpp;
    let stride = raw.width * cpp;

    let black = raw.blacklevel.as_vec();
    let white = raw.whitelevel.as_vec();
    let black_mean = black.iter().sum::<f32>() / black.len().max(1) as f32;
    let white_mean = white.iter().sum::<f32>() / white.len().max(1) as f32;
    let range = (white_mean - black_mean).max(1.0);

    let samples: Vec<f32> = match &raw.data {
        RawImageData::Integer(values) => values.iter().map(|v| *v as f32).collect(),
        RawImageData::Float(values) => values.clone(),
    };

    let rows = (columns * raw.height / raw.width).max(1);
    let block_w = raw.width / columns;
    let block_h = raw.height / rows;

    eprintln!(
        "{path}: {}x{} cpp={} black={black_mean} white={white_mean} grid={columns}x{rows}",
        raw.width, raw.height, cpp
    );

    for row in 0..rows {
        for column in 0..columns {
            let mut total = 0.0f64;
            let mut count = 0u64;
            for y in row * block_h..(row + 1) * block_h {
                for x in column * block_w..(column + 1) * block_w {
                    for c in 0..cpp {
                        if let Some(value) = samples.get(y * stride + x * cpp + c) {
                            total += ((*value - black_mean) / range) as f64;
                            count += 1;
                        }
                    }
                }
            }
            let mean = if count > 0 { total / count as f64 } else { 0.0 };
            println!("{row},{column},{mean:.6}");
        }
    }

    Ok(())
}
