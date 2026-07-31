//! Debug helper: print the sensor level metadata raw-autotune has to reconcile.
//!
//! cargo run --release --example probe-levels -- <file.dng> ...

fn main() -> anyhow::Result<()> {
    for path in std::env::args().skip(1) {
        let (raw, _) = raw_autotune::decode_corrected(std::path::Path::new(&path))?;
        println!("--- {path}");
        println!("  {} {}", raw.clean_make, raw.clean_model);
        println!(
            "  {}x{} cpp={} bps={}",
            raw.width, raw.height, raw.cpp, raw.bps
        );
        println!("  photometric: {:?}", raw.photometric);
        println!(
            "  blacklevel: {}x{} cpp={} -> {:?}",
            raw.blacklevel.width,
            raw.blacklevel.height,
            raw.blacklevel.cpp,
            raw.blacklevel.as_vec()
        );
        println!("  whitelevel: {:?}", raw.whitelevel.as_vec());
        println!("  wb_coeffs: {:?}", raw.wb_coeffs);
        println!("  orientation: {:?}", raw.orientation);
        println!(
            "  active_area: {:?} crop_area: {:?}",
            raw.active_area, raw.crop_area
        );
        println!(
            "  color_matrix illuminants: {:?}",
            raw.color_matrix.keys().collect::<Vec<_>>()
        );

        let data = raw.data.as_f32();
        let mut sorted: Vec<f32> = data.iter().copied().filter(|v| v.is_finite()).collect();
        sorted.sort_by(f32::total_cmp);
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
        println!(
            "  sample range: min={} p50={} p99={} p999={} max={} (n={})",
            sorted[0],
            at(0.5),
            at(0.99),
            at(0.999),
            sorted[sorted.len() - 1],
            sorted.len()
        );
        println!(
            "  storage: {}",
            match &raw.data {
                rawler::RawImageData::Integer(_) => "Integer(u16)",
                rawler::RawImageData::Float(_) => "Float(f32)",
            }
        );
    }
    Ok(())
}
