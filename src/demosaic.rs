//! Deterministic Bayer demosaicing and automatic method selection.
//!
//! The automatic policy is deliberately conservative: RCD is the general
//! purpose path, while AMaZE is reserved for mosaics whose measured noise and
//! two-green disagreement both indicate that its extra fine-detail recovery is
//! useful. PPG remains available as a diagnostic/control implementation.

use clap::ValueEnum;
use rawler::CFA;
use rawler::cfa::CFAColor;
use rawler::imgop::Rect;
use rayon::prelude::*;
use serde::Serialize;

/// Versioned name written into reports whenever the automatic selector runs.
pub const SELECTOR_VERSION: &str = "bayer-auto-v1";

/// Clean frames have an SNR=10 crossing below this scene level.
const AMAZE_MAX_SNR10_EV: f32 = -6.0;
/// Robust normalized disagreement between the two green phases.
const AMAZE_MAX_ALIAS_SCORE: f32 = 0.005;
/// Real-file validation found that PPG has less false colour on dense branches
/// above this point, so the owned interpolators stay behind strict guards.
const RCD_MAX_ALIAS_SCORE: f32 = 0.02;
const RCD_MAX_SNR10_EV: f32 = -4.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DemosaicMethod {
    /// Select from the mosaic's measured noise and alias risk.
    Auto,
    /// Ratio-corrected, edge-directed Bayer interpolation.
    Rcd,
    /// High-detail adaptive Bayer interpolation.
    Amaze,
    /// Rawler's mature PPG implementation and the automatic safe fallback.
    Ppg,
}

impl DemosaicMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Rcd => "rcd",
            Self::Amaze => "amaze",
            Self::Ppg => "ppg",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DemosaicReport {
    pub requested: DemosaicMethod,
    pub resolved: DemosaicMethod,
    pub selector_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snr10_ev: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias_score: Option<f32>,
    pub reason: &'static str,
}

/// Resolve the method before allocating an RGB buffer.
pub fn select(
    requested: DemosaicMethod,
    samples: &[f32],
    width: usize,
    height: usize,
    cfa: &CFA,
    snr10_ev: Option<f32>,
) -> DemosaicReport {
    if requested != DemosaicMethod::Auto {
        return DemosaicReport {
            requested,
            resolved: requested,
            selector_version: SELECTOR_VERSION,
            snr10_ev,
            alias_score: None,
            reason: "explicit",
        };
    }

    let alias_score = green_alias_score(samples, width, height, cfa);
    let (resolved, reason) = match (snr10_ev, alias_score) {
        (Some(noise), Some(alias))
            if noise.is_finite()
                && noise <= AMAZE_MAX_SNR10_EV
                && alias <= AMAZE_MAX_ALIAS_SCORE =>
        {
            (DemosaicMethod::Amaze, "clean_low_alias")
        }
        (Some(noise), Some(alias))
            if noise.is_finite() && noise <= RCD_MAX_SNR10_EV && alias <= RCD_MAX_ALIAS_SCORE =>
        {
            (DemosaicMethod::Rcd, "moderate_low_alias")
        }
        (None, _) => (DemosaicMethod::Ppg, "missing_noise_estimate"),
        (_, None) => (DemosaicMethod::Ppg, "missing_alias_estimate"),
        (Some(noise), _) if !noise.is_finite() || noise > RCD_MAX_SNR10_EV => {
            (DemosaicMethod::Ppg, "noise_guard")
        }
        _ => (DemosaicMethod::Ppg, "alias_guard"),
    };

    DemosaicReport {
        requested,
        resolved,
        selector_version: SELECTOR_VERSION,
        snr10_ev,
        alias_score,
        reason,
    }
}

/// A robust, sampled measure of disagreement between the two Bayer green
/// phases. Large disagreement is a useful warning for near-Nyquist structure,
/// where the conservative RCD result is preferred.
fn green_alias_score(samples: &[f32], width: usize, height: usize, cfa: &CFA) -> Option<f32> {
    if width < 4 || height < 4 || samples.len() != width.checked_mul(height)? {
        return None;
    }
    let mut ratios = Vec::with_capacity((width * height / 256).clamp(64, 16_384));
    // Odd strides alternate Bayer phases. An even stride can sample only red
    // or only blue sites for an entire frame, depending on the CFA origin.
    let row_step = ((height - 2) / 128).max(1) | 1;
    let col_step = ((width - 2) / 128).max(1) | 1;
    for y in (1..height - 1).step_by(row_step) {
        for x in (1..width - 1).step_by(col_step) {
            let center = cfa.cfa_color_at(y, x);
            if center != CFAColor::GREEN {
                continue;
            }
            // Green is the only colour that occurs twice in an ordinary Bayer
            // cell. Its nearest diagonal is the other green phase.
            let candidates = [
                (x - 1, y - 1),
                (x + 1, y - 1),
                (x - 1, y + 1),
                (x + 1, y + 1),
            ];
            let mut best = None::<f32>;
            for (nx, ny) in candidates {
                if cfa.cfa_color_at(ny, nx) != center {
                    continue;
                }
                let a = samples[y * width + x];
                let b = samples[ny * width + nx];
                let scale = a.abs().max(b.abs()).max(0.02);
                best =
                    Some(best.map_or((a - b).abs() / scale, |old| old.min((a - b).abs() / scale)));
            }
            if let Some(value) = best.filter(|value| value.is_finite()) {
                ratios.push(value);
            }
        }
    }
    if ratios.len() < 16 {
        return None;
    }
    ratios.sort_unstable_by(f32::total_cmp);
    Some(ratios[ratios.len() * 3 / 4])
}

#[inline]
fn index(width: usize, x: usize, y: usize) -> usize {
    y * width + x
}

#[inline]
fn value(samples: &[f32], width: usize, x: isize, y: isize) -> f32 {
    let height = samples.len() / width;
    let x = x.clamp(0, width.saturating_sub(1) as isize) as usize;
    let y = y.clamp(0, height.saturating_sub(1) as isize) as usize;
    samples[index(width, x, y)]
}

#[derive(Clone, Copy)]
struct Mosaic<'a> {
    samples: &'a [f32],
    width: usize,
    height: usize,
    cfa: &'a CFA,
}

fn average_colour(mosaic: Mosaic<'_>, x: usize, y: usize, wanted: CFAColor, radius: isize) -> f32 {
    let mut sum = 0.0_f64;
    let mut count = 0usize;
    for dy in -radius..=radius {
        let ny = y as isize + dy;
        if !(0..mosaic.height as isize).contains(&ny) {
            continue;
        }
        for dx in -radius..=radius {
            let nx = x as isize + dx;
            if !(0..mosaic.width as isize).contains(&nx) {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if mosaic.cfa.cfa_color_at(ny, nx) == wanted {
                sum += f64::from(mosaic.samples[index(mosaic.width, nx, ny)]);
                count += 1;
            }
        }
    }
    if count == 0 {
        mosaic.samples[index(mosaic.width, x, y)]
    } else {
        (sum / count as f64) as f32
    }
}

#[inline]
fn ratio_candidate(green: f32, same: f32, center: f32) -> f32 {
    const EPS: f32 = 1.0e-5;
    if green > EPS && same > EPS && center > 0.0 {
        green * center / same
    } else {
        green + 0.5 * (center - same)
    }
}

fn green_rcd(samples: &[f32], width: usize, x: usize, y: usize) -> f32 {
    let x = x as isize;
    let y = y as isize;
    let c = value(samples, width, x, y);
    let gl = value(samples, width, x - 1, y);
    let gr = value(samples, width, x + 1, y);
    let gu = value(samples, width, x, y - 1);
    let gd = value(samples, width, x, y + 1);
    let sl = value(samples, width, x - 2, y);
    let sr = value(samples, width, x + 2, y);
    let su = value(samples, width, x, y - 2);
    let sd = value(samples, width, x, y + 2);

    let horizontal = 0.5 * (ratio_candidate(gl, sl, c) + ratio_candidate(gr, sr, c));
    let vertical = 0.5 * (ratio_candidate(gu, su, c) + ratio_candidate(gd, sd, c));
    let gh = (gl - gr).abs() + (2.0 * c - sl - sr).abs();
    let gv = (gu - gd).abs() + (2.0 * c - su - sd).abs();
    let wh = 1.0 / (1.0e-6 + gh * gh);
    let wv = 1.0 / (1.0e-6 + gv * gv);
    let estimate = (horizontal * wh + vertical * wv) / (wh + wv);
    if estimate.is_finite() {
        estimate
    } else {
        0.25 * (gl + gr + gu + gd)
    }
}

fn green_amaze(samples: &[f32], width: usize, x: usize, y: usize) -> f32 {
    let x = x as isize;
    let y = y as isize;
    let c = value(samples, width, x, y);
    let directions = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    let mut weighted = 0.0_f64;
    let mut weights = 0.0_f64;
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for (dx, dy) in directions {
        let g1 = value(samples, width, x + dx, y + dy);
        let s2 = value(samples, width, x + 2 * dx, y + 2 * dy);
        let g3 = value(samples, width, x + 3 * dx, y + 3 * dy);
        // High-order Hamilton-Adams candidate. The third green sample makes
        // the weight sensitive to a real edge rather than to sensor noise at
        // one photosite.
        let candidate = g1 + 0.5 * (c - s2);
        let gradient = (c - s2).abs() + (g1 - g3).abs() + 0.5 * (candidate - g1).abs();
        let weight = 1.0_f64 / f64::from(1.0e-6 + gradient * gradient);
        weighted += f64::from(candidate) * weight;
        weights += weight;
        lo = lo.min(g1);
        hi = hi.max(g1);
    }
    let estimate = (weighted / weights.max(1.0e-12)) as f32;
    // Permit genuine detail beyond the cardinal green range, but prevent the
    // dark/bright pinholes that unconstrained high-order interpolation creates.
    let slack = (hi - lo).abs() * 0.5 + 0.01;
    estimate.clamp(lo - slack, hi + slack)
}

fn interpolate_difference(
    mosaic: Mosaic<'_>,
    green: &[f32],
    x: usize,
    y: usize,
    wanted: CFAColor,
    offsets: &[(isize, isize)],
    high_detail: bool,
) -> f32 {
    let mut sum = 0.0_f64;
    let mut weights = 0.0_f64;
    for &(dx, dy) in offsets {
        let nx = x as isize + dx;
        let ny = y as isize + dy;
        if !(0..mosaic.width as isize).contains(&nx) || !(0..mosaic.height as isize).contains(&ny) {
            continue;
        }
        let (nx, ny) = (nx as usize, ny as usize);
        if mosaic.cfa.cfa_color_at(ny, nx) != wanted {
            continue;
        }
        let ni = index(mosaic.width, nx, ny);
        let difference = mosaic.samples[ni] - green[ni];
        let gradient = (green[ni] - green[index(mosaic.width, x, y)]).abs();
        let weight = if high_detail {
            1.0_f64 / f64::from(1.0e-6 + gradient * gradient)
        } else {
            1.0_f64 / f64::from(1.0e-4 + gradient)
        };
        sum += f64::from(difference) * weight;
        weights += weight;
    }
    if weights > 0.0 {
        green[index(mosaic.width, x, y)] + (sum / weights) as f32
    } else {
        average_colour(mosaic, x, y, wanted, 2)
    }
}

/// Demosaic an RGB Bayer mosaic through the owned camera-RGB path.
///
/// The input is the full normalized mosaic; `roi` selects the output region so
/// interpolation can still use valid samples immediately outside ActiveArea.
pub fn demosaic_bayer(
    samples: &[f32],
    width: usize,
    height: usize,
    cfa: &CFA,
    roi: Rect,
    method: DemosaicMethod,
) -> Vec<[f32; 3]> {
    debug_assert!(matches!(
        method,
        DemosaicMethod::Rcd | DemosaicMethod::Amaze
    ));
    let high_detail = method == DemosaicMethod::Amaze;
    let mosaic = Mosaic {
        samples,
        width,
        height,
        cfa,
    };

    let mut green = vec![0.0_f32; width * height];
    green
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, output) in row.iter_mut().enumerate() {
                *output = if cfa.cfa_color_at(y, x) == CFAColor::GREEN {
                    samples[index(width, x, y)]
                } else if x < 3 || y < 3 || x + 3 >= width || y + 3 >= height {
                    average_colour(mosaic, x, y, CFAColor::GREEN, 2)
                } else if high_detail {
                    green_amaze(samples, width, x, y)
                } else {
                    green_rcd(samples, width, x, y)
                };
            }
        });

    let row_width = roi.d.w;
    let mut output = vec![[0.0_f32; 3]; roi.d.w * roi.d.h];
    output
        .par_chunks_mut(row_width)
        .enumerate()
        .for_each(|(out_y, row)| {
            let y = roi.p.y + out_y;
            for (out_x, pixel) in row.iter_mut().enumerate() {
                let x = roi.p.x + out_x;
                let i = index(width, x, y);
                let own = cfa.cfa_color_at(y, x);
                let g = green[i];
                let cardinal = [(-2, 0), (2, 0), (0, -2), (0, 2)];
                let diagonal = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
                let axial = [(-1, 0), (1, 0), (0, -1), (0, 1)];

                let r = if own == CFAColor::RED {
                    samples[i]
                } else if own == CFAColor::BLUE {
                    interpolate_difference(
                        mosaic,
                        &green,
                        x,
                        y,
                        CFAColor::RED,
                        &diagonal,
                        high_detail,
                    )
                } else {
                    interpolate_difference(
                        mosaic,
                        &green,
                        x,
                        y,
                        CFAColor::RED,
                        if axial.iter().any(|(dx, dy)| {
                            let nx = x as isize + dx;
                            let ny = y as isize + dy;
                            (0..width as isize).contains(&nx)
                                && (0..height as isize).contains(&ny)
                                && cfa.cfa_color_at(ny as usize, nx as usize) == CFAColor::RED
                        }) {
                            &axial
                        } else {
                            &cardinal
                        },
                        high_detail,
                    )
                };
                let b = if own == CFAColor::BLUE {
                    samples[i]
                } else if own == CFAColor::RED {
                    interpolate_difference(
                        mosaic,
                        &green,
                        x,
                        y,
                        CFAColor::BLUE,
                        &diagonal,
                        high_detail,
                    )
                } else {
                    interpolate_difference(
                        mosaic,
                        &green,
                        x,
                        y,
                        CFAColor::BLUE,
                        if axial.iter().any(|(dx, dy)| {
                            let nx = x as isize + dx;
                            let ny = y as isize + dy;
                            (0..width as isize).contains(&nx)
                                && (0..height as isize).contains(&ny)
                                && cfa.cfa_color_at(ny as usize, nx as usize) == CFAColor::BLUE
                        }) {
                            &axial
                        } else {
                            &cardinal
                        },
                        high_detail,
                    )
                };
                *pixel = [r, g, b];
            }
        });
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mosaic(cfa: &CFA, width: usize, height: usize, rgb: [f32; 3]) -> Vec<f32> {
        (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| match cfa.cfa_color_at(y, x) {
                    CFAColor::RED => rgb[0],
                    CFAColor::GREEN => rgb[1],
                    CFAColor::BLUE => rgb[2],
                    _ => 0.0,
                })
            })
            .collect()
    }

    fn full(width: usize, height: usize) -> Rect {
        use rawler::imgop::{Dim2, Point};
        Rect::new(Point::zero(), Dim2::new(width, height))
    }

    #[test]
    fn flat_fields_are_reconstructed_for_all_bayer_phases() {
        for name in ["RGGB", "BGGR", "GRBG", "GBRG"] {
            let cfa = CFA::new(name);
            let samples = mosaic(&cfa, 20, 18, [0.25, 0.5, 0.75]);
            for method in [DemosaicMethod::Rcd, DemosaicMethod::Amaze] {
                let image = demosaic_bayer(&samples, 20, 18, &cfa, full(20, 18), method);
                for pixel in image {
                    assert!((pixel[0] - 0.25).abs() < 1.0e-5, "{name} {method:?}");
                    assert!((pixel[1] - 0.5).abs() < 1.0e-5, "{name} {method:?}");
                    assert!((pixel[2] - 0.75).abs() < 1.0e-5, "{name} {method:?}");
                }
            }
        }
    }

    #[test]
    fn known_photosites_are_preserved_exactly() {
        let cfa = CFA::new("RGGB");
        let samples: Vec<f32> = (0..24 * 20).map(|i| i as f32 / 479.0).collect();
        let image = demosaic_bayer(&samples, 24, 20, &cfa, full(24, 20), DemosaicMethod::Rcd);
        for y in 0..20 {
            for x in 0..24 {
                let channel = cfa.cfa_color_at(y, x) as usize;
                assert_eq!(image[index(24, x, y)][channel], samples[index(24, x, y)]);
            }
        }
    }

    #[test]
    fn selector_uses_all_three_methods_only_inside_their_guards() {
        let cfa = CFA::new("RGGB");
        let samples = mosaic(&cfa, 32, 32, [0.4; 3]);
        assert_eq!(
            select(DemosaicMethod::Auto, &samples, 32, 32, &cfa, None).resolved,
            DemosaicMethod::Ppg
        );
        assert_eq!(
            select(DemosaicMethod::Auto, &samples, 32, 32, &cfa, Some(-5.0)).resolved,
            DemosaicMethod::Rcd
        );
        assert_eq!(
            select(DemosaicMethod::Auto, &samples, 32, 32, &cfa, Some(-7.0)).resolved,
            DemosaicMethod::Amaze
        );

        let textured: Vec<f32> = (0..32 * 32)
            .map(|index| if (index / 32) % 2 == 0 { 0.2 } else { 0.8 })
            .collect();
        assert_eq!(
            select(DemosaicMethod::Auto, &textured, 32, 32, &cfa, Some(-7.0)).resolved,
            DemosaicMethod::Ppg
        );
    }

    #[test]
    fn repeated_runs_are_identical() {
        let cfa = CFA::new("GBRG");
        let samples: Vec<f32> = (0..64 * 48)
            .map(|i| ((i * 7919 % 65521) as f32 / 65521.0) - 0.05)
            .collect();
        let a = demosaic_bayer(&samples, 64, 48, &cfa, full(64, 48), DemosaicMethod::Amaze);
        let b = demosaic_bayer(&samples, 64, 48, &cfa, full(64, 48), DemosaicMethod::Amaze);
        assert_eq!(a, b);
        assert!(a.iter().flatten().all(|value| value.is_finite()));
    }
}
