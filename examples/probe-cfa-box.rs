//! Per-CFA-channel statistics inside a pixel box, from the corrected decode path.
//!
//! cargo run --release --example probe-cfa-box -- file.ARW x0 y0 x1 y1

use rawler::RawImageData;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("file");
    let x0: usize = args.next().unwrap().parse()?;
    let y0: usize = args.next().unwrap().parse()?;
    let x1: usize = args.next().unwrap().parse()?;
    let y1: usize = args.next().unwrap().parse()?;

    let raw = raw_autotune::decode_corrected(std::path::Path::new(&path))?.0;
    let data: Vec<f32> = match &raw.data {
        RawImageData::Integer(s) => s.iter().map(|v| *v as f32).collect(),
        RawImageData::Float(s) => s.clone(),
    };
    let w = raw.width;
    println!(
        "{path}: {}x{} cfa={:?} wb={:?} black={:?} white={:?}",
        raw.width, raw.height, raw.camera.cfa, raw.wb_coeffs, raw.blacklevel, raw.whitelevel
    );
    // Accumulate by CFA colour index.
    let mut sum = [0.0f64; 4];
    let mut n = [0u64; 4];
    let mut maxv = [0.0f32; 4];
    let mut clipped = [0u64; 4];
    let crop = raw.crop_area;
    println!("crop_area={crop:?} active={:?}", raw.active_area);
    for y in y0..y1.min(raw.height) {
        for x in x0..x1.min(w) {
            let c = raw.camera.cfa.color_at(y, x);
            let v = data[y * w + x];
            sum[c] += v as f64;
            n[c] += 1;
            if v > maxv[c] {
                maxv[c] = v;
            }
            if v >= 16000.0 {
                clipped[c] += 1;
            }
        }
    }
    for c in 0..4 {
        if n[c] > 0 {
            println!(
                "cfa {c}: n={} mean={:.1} max={:.0} ge16000={}",
                n[c],
                sum[c] / n[c] as f64,
                maxv[c],
                clipped[c]
            );
        }
    }
    Ok(())
}
