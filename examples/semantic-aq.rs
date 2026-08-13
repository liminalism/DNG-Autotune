//! Inspect codec-neutral semantic AQ evidence on an encoder-selected grid.
//!
//! ```text
//! cargo run --release --example semantic-aq -- image.ARW 8
//! ```
//! Use cell size 8 for JPEG XL atoms, or 16/32 for bpg-rs HEVC QGs.

use raw_autotune::api::{RenderOptions, render_file_rgb16};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: semantic-aq <RAW> [cell-size]"))?;
    let cell_size = args
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(8);

    let mut options = RenderOptions::automatic();
    options.semantic = true;
    let rendered = render_file_rgb16(input, &options)?;
    let map = rendered
        .report
        .encoder_spatial_aq_map(cell_size, cell_size)?;

    println!(
        "source={}x{} cell={}x{} grid={}x{} schema={}",
        map.grid.source_width,
        map.grid.source_height,
        map.grid.cell_width,
        map.grid.cell_height,
        map.grid.grid_width,
        map.grid.grid_height,
        map.schema_version
    );
    for layer in &map.layers {
        let count = layer.confidence.len();
        let mean = layer.confidence.iter().sum::<f32>() / count.max(1) as f32;
        let peak = layer.confidence.iter().copied().fold(0.0f32, f32::max);
        let confident = layer
            .confidence
            .iter()
            .filter(|&&confidence| confidence >= 0.5)
            .count();
        println!(
            "{:?}: uses={:?} mean={mean:.4} peak={peak:.4} confident_cells={confident}/{count}",
            layer.kind, layer.uses
        );
    }
    Ok(())
}
