//! Scene-linear reconstruction vs an underexposed EXIF-scaled ground truth.
//!
//! Display-stage dumps clip at 1.0, so they cannot score a sun that the dark
//! frame still holds. This probe develops both frames through the production
//! colour path and compares OKLab chroma of reconstructed pixels to the dark
//! frame scaled by the EXIF EV difference.
//!
//! cargo run --release --example probe-highlight-gt -- \
//!   raw/raw_backlit2/_DSC1345.ARW raw/raw_backlit2/_DSC1346.ARW 3.68

use raw_autotune::color::{DevelopOptions, WorkingSpace};
use raw_autotune::oklab;
use raw_autotune::raw_highlight::HighlightMethod;
use raw_autotune::rescale::SubBlack;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct Stats {
    n: usize,
    chroma_rmse: f64,
    chroma_p50: f64,
    chroma_p90: f64,
    hue_p90_deg: f64,
    chroma_mean: f64,
    chroma_gt_mean: f64,
}

#[derive(Serialize)]
struct Report {
    bright: String,
    dark: String,
    ev_scale: f64,
    shift: [i32; 2],
    reconstructed_pixels: usize,
    usable: usize,
    on: Stats,
    off: Stats,
}

fn develop(
    path: &Path,
    reconstruction: f32,
    method: HighlightMethod,
) -> anyhow::Result<(Vec<[f32; 3]>, Option<Vec<f32>>, usize, usize)> {
    let (raw, _) = raw_autotune::decode_corrected(path)?;
    let (image, _report, uncertainty, _) = raw_autotune::color::develop(
        &raw,
        path,
        DevelopOptions {
            working_space: WorkingSpace::Srgb,
            sub_black: SubBlack::Preserve,
            hot_pixels: 0.5,
            highlight_reconstruction: reconstruction,
            highlight_method: method,
            spatial_highlight_floor: raw_autotune::raw_highlight::DEFAULT_SPATIAL_CLIPPED_FLOOR,
            demosaic: raw_autotune::demosaic::DemosaicMethod::Auto,
            snr10_ev: None,
            full_dng_color: true,
            lens_correction: raw_autotune::lens::LensCorrectionMode::Embedded,
            dump_stages: None,
            illuminant_proxy: false,
            hue_sat_map: None,
            hue_sat_map_strength: 0.0,
        },
    )?;
    Ok((image.pixels, uncertainty, image.width, image.height))
}

fn percentile(mut values: Vec<f64>, p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((p / 100.0) * (values.len() - 1) as f64).round() as usize;
    values[i.min(values.len() - 1)]
}

fn stats(chroma_err: &[f64], hue_err: &[f64], chroma_on: &[f64], chroma_gt: &[f64]) -> Stats {
    let n = chroma_err.len();
    let mse = if n == 0 {
        0.0
    } else {
        chroma_err.iter().map(|v| v * v).sum::<f64>() / n as f64
    };
    Stats {
        n,
        chroma_rmse: mse.sqrt(),
        chroma_p50: percentile(chroma_err.to_vec(), 50.0),
        chroma_p90: percentile(chroma_err.to_vec(), 90.0),
        hue_p90_deg: percentile(hue_err.to_vec(), 90.0) * 180.0 / std::f64::consts::PI,
        chroma_mean: if n == 0 {
            0.0
        } else {
            chroma_on.iter().sum::<f64>() / n as f64
        },
        chroma_gt_mean: if n == 0 {
            0.0
        } else {
            chroma_gt.iter().sum::<f64>() / n as f64
        },
    }
}

fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn align(on: &[[f32; 3]], dark: &[[f32; 3]], w: usize, h: usize, mask: &[bool]) -> (i32, i32) {
    // Search a coarse translation that matches un-reconstructed luma.
    let step = 8;
    let range = 48i32;
    let mut best = (0i32, 0i32);
    let mut best_err = f64::INFINITY;
    for dy in (-range..=range).step_by(step) {
        for dx in (-range..=range).step_by(step) {
            let mut err = 0.0;
            let mut n = 0u64;
            for y in (0..h).step_by(step) {
                for x in (0..w).step_by(step) {
                    let i = y * w + x;
                    if mask[i] {
                        continue;
                    }
                    let xx = x as i32 + dx;
                    let yy = y as i32 + dy;
                    if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                        continue;
                    }
                    let a = luma(on[i]) as f64;
                    let b = luma(dark[yy as usize * w + xx as usize]) as f64;
                    err += (a - b).abs();
                    n += 1;
                }
            }
            if n > 0 {
                let mean = err / n as f64;
                if mean < best_err {
                    best_err = mean;
                    best = (dx, dy);
                }
            }
        }
    }
    best
}

fn gather(
    on: &[[f32; 3]],
    off: &[[f32; 3]],
    dark: &[[f32; 3]],
    mask: &[bool],
    w: usize,
    h: usize,
    shift: (i32, i32),
    scale: f64,
) -> (
    Vec<f64>,
    Vec<f64>,
    Vec<f64>,
    Vec<f64>,
    Vec<f64>,
    Vec<f64>,
    Vec<f64>,
    Vec<f64>,
) {
    let mut on_c_err = Vec::new();
    let mut off_c_err = Vec::new();
    let mut on_h_err = Vec::new();
    let mut off_h_err = Vec::new();
    let mut on_c = Vec::new();
    let mut off_c = Vec::new();
    let mut gt_c_on = Vec::new();
    let mut gt_c_off = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !mask[i] {
                continue;
            }
            let xx = x as i32 + shift.0;
            let yy = y as i32 + shift.1;
            if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                continue;
            }
            let gi = yy as usize * w + xx as usize;
            let gt = [
                (dark[gi][0] as f64 * scale) as f32,
                (dark[gi][1] as f64 * scale) as f32,
                (dark[gi][2] as f64 * scale) as f32,
            ];
            if !gt.iter().all(|c| c.is_finite()) {
                continue;
            }
            let on_lab = oklab::from_linear_srgb(on[i]);
            let off_lab = oklab::from_linear_srgb(off[i]);
            let gt_lab = oklab::from_linear_srgb(gt);
            let on_chroma = on_lab.chroma() as f64;
            let off_chroma = off_lab.chroma() as f64;
            let gt_chroma = gt_lab.chroma() as f64;
            on_c_err.push((on_chroma - gt_chroma).abs());
            off_c_err.push((off_chroma - gt_chroma).abs());
            on_h_err.push(oklab::hue_difference(on_lab, gt_lab).unwrap_or(0.0).abs() as f64);
            off_h_err.push(oklab::hue_difference(off_lab, gt_lab).unwrap_or(0.0).abs() as f64);
            on_c.push(on_chroma);
            off_c.push(off_chroma);
            gt_c_on.push(gt_chroma);
            gt_c_off.push(gt_chroma);
        }
    }
    (
        on_c_err, on_h_err, on_c, gt_c_on, off_c_err, off_h_err, off_c, gt_c_off,
    )
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let bright = args
        .next()
        .expect("usage: probe-highlight-gt <bright.ARW> <dark.ARW> <ev>");
    let dark = args
        .next()
        .expect("usage: probe-highlight-gt <bright> <dark> <ev>");
    let ev: f64 = args
        .next()
        .expect("ev")
        .parse()
        .expect("ev is a float (dark is this many stops down)");
    let scale = 2.0_f64.powf(ev);

    eprintln!("develop ON {}", bright);
    let (on, uncertainty, w, h) = develop(Path::new(&bright), 1.0, HighlightMethod::Harmonic)?;
    eprintln!("develop OFF {}", bright);
    let (off, _, w2, h2) = develop(Path::new(&bright), 0.0, HighlightMethod::Current)?;
    anyhow::ensure!(w == w2 && h == h2, "ON/OFF dimension mismatch");
    eprintln!("develop dark {}", dark);
    let (dark_px, _, w3, h3) = develop(Path::new(&dark), 0.0, HighlightMethod::Current)?;
    anyhow::ensure!(w == w3 && h == h3, "bright/dark dimension mismatch");

    let mask: Vec<bool> = match uncertainty {
        Some(values) => values.iter().map(|v| *v > 0.02).collect(),
        None => vec![false; on.len()],
    };
    let reconstructed = mask.iter().filter(|m| **m).count();
    let shift = align(&on, &dark_px, w, h, &mask);
    eprintln!("shift {shift:?} reconstructed={reconstructed}");

    let (on_c, on_h, on_m, gt_on, off_c, off_h, off_m, gt_off) =
        gather(&on, &off, &dark_px, &mask, w, h, shift, scale);
    let report = Report {
        bright,
        dark,
        ev_scale: ev,
        shift: [shift.0, shift.1],
        reconstructed_pixels: reconstructed,
        usable: on_c.len(),
        on: stats(&on_c, &on_h, &on_m, &gt_on),
        off: stats(&off_c, &off_h, &off_m, &gt_off),
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
