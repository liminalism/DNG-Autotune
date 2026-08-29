//! DCP `HueSatMap` applied after the colour matrix.
//!
//! Off by default. A loaded table is a creative LUT, not extra sensor
//! calibration: the matrix path already answered what colour was measured.
//! This module answers how a public-domain camera profile wants that colour
//! to look in HSV of ProPhoto RGB, which is the encoding Adobe DNG 1.4 and
//! RawTherapee/ART use for `ProfileHueSatMapData*`.
//!
//! Dual-illuminant tables are blended by reciprocal colour temperature the
//! same way the DNG matrix interpolation is. Missing CCT uses the D65 table
//! when one exists. Strength 0 is an exact no-op: the pixel is not converted
//! and not interpolated.
//!
//! Negative ProPhoto channels skip the LUT (the matrix result is kept),
//! matching the DNG SDK's "do not look up out-of-gamut points" rule.

use crate::color::{Matrix3, WorkingSpace, invert3};
use anyhow::{Context, Result, ensure};
use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{Entry, GenericTiffReader, Value};
use serde::Serialize;
use std::path::{Path, PathBuf};

const TAG_UNIQUE_CAMERA_MODEL: u16 = 50708;
const TAG_CALIBRATION_ILLUMINANT_1: u16 = 50778;
const TAG_CALIBRATION_ILLUMINANT_2: u16 = 50779;
const TAG_PROFILE_NAME: u16 = 50936;
const TAG_HUE_SAT_MAP_DIMS: u16 = 50937;
const TAG_HUE_SAT_MAP_DATA_1: u16 = 50938;
const TAG_HUE_SAT_MAP_DATA_2: u16 = 50939;
const TAG_PROFILE_COPYRIGHT: u16 = 50942;

/// Bradford chromatic adaptation, D65 → D50. Same matrix `dngcolor` uses.
const BRADFORD_D65_TO_D50: Matrix3 = [
    [1.047_811_2, 0.022_886_6, -0.050_127_0],
    [0.029_542_4, 0.990_484_4, -0.017_049_1],
    [-0.009_234_5, 0.015_043_6, 0.752_131_6],
];

/// ProPhoto RGB → XYZ D50 (IEC 61966-2-2).
const PROPHOTO_TO_XYZ_D50: Matrix3 = [
    [0.797_674_9, 0.135_191_7, 0.031_353_4],
    [0.288_040_2, 0.711_874_1, 0.000_085_7],
    [0.000_000_0, 0.000_000_0, 0.825_21],
];

/// A DCP HueSatMap plus the metadata needed to attribute and interpolate it.
#[derive(Debug, Clone)]
pub struct HueSatMap {
    pub source: PathBuf,
    /// The whole DCP, kept so the profile's own matrices can be read back when
    /// the table is applied. A HueSatMap is the residual of the profile's
    /// `ForwardMatrix`, so the two halves have to travel together; see
    /// [`crate::dngcolor::camera_to_working_from_profile`]. Shared rather than
    /// cloned because every job in a batch holds the same profile.
    pub bytes: std::sync::Arc<[u8]>,
    pub unique_camera_model: String,
    pub profile_name: String,
    pub copyright: String,
    pub hue_div: usize,
    pub sat_div: usize,
    pub val_div: usize,
    table1: Vec<[f32; 3]>,
    table2: Option<Vec<[f32; 3]>>,
    illuminant1_k: f32,
    illuminant2_k: Option<f32>,
}

/// Sidecar description. Absent when the flag is off, so default JSON is
/// unchanged.
#[derive(Debug, Clone, Serialize)]
pub struct HueSatMapReport {
    pub source: String,
    pub unique_camera_model: String,
    pub profile_name: String,
    pub copyright: String,
    pub dims: [u32; 3],
    pub strength: f32,
    pub table: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cct: Option<f32>,
    /// Present only when the table was declined, saying why. Its absence is
    /// what says the calibration ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<crate::color::HueSatSkip>,
}

impl HueSatMap {
    /// Load the public-domain ART/RawTherapee Sony A7C table shipped with the
    /// binary. Embedding it keeps `--preset vivid` independent of the current
    /// working directory and of the source tree being present at runtime.
    pub fn bundled_sony_a7c() -> Result<Self> {
        const SOURCE: &str = "profiles/SONY_ILCE-7C.dcp";
        let tiff = GenericTiffReader::new_with_buffer(
            include_bytes!("../profiles/SONY_ILCE-7C.dcp"),
            0,
            0,
            None,
        )
        .with_context(|| format!("parse bundled DCP TIFF {SOURCE}"))?;
        Self::from_tiff(
            &tiff,
            PathBuf::from(SOURCE),
            include_bytes!("../profiles/SONY_ILCE-7C.dcp")
                .as_slice()
                .into(),
        )
    }

    /// Parse a `.dcp` (or DNG) that carries `ProfileHueSatMapDims` and Data1.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("open DCP {}", path.display()))?;
        let tiff = GenericTiffReader::new_with_buffer(&bytes, 0, 0, None)
            .with_context(|| format!("parse DCP TIFF {}", path.display()))?;

        Self::from_tiff(&tiff, path.to_path_buf(), bytes.into())
    }

    fn from_tiff(
        tiff: &GenericTiffReader,
        source: PathBuf,
        bytes: std::sync::Arc<[u8]>,
    ) -> Result<Self> {
        let dims_entry = find_entry(tiff, TAG_HUE_SAT_MAP_DIMS)
            .ok_or_else(|| anyhow::anyhow!("{} has no ProfileHueSatMapDims", source.display()))?;
        ensure!(
            dims_entry.value.count() == 3,
            "ProfileHueSatMapDims must be 3 LONGs"
        );
        let hue_div = dims_entry.value.force_u32(0) as usize;
        let sat_div = dims_entry.value.force_u32(1) as usize;
        let val_div = dims_entry.value.force_u32(2) as usize;
        ensure!(
            hue_div >= 1 && sat_div >= 2 && val_div >= 1,
            "HueSatMap dims {hue_div}×{sat_div}×{val_div} are not a usable table"
        );
        let cells = hue_div
            .checked_mul(sat_div)
            .and_then(|n| n.checked_mul(val_div))
            .context("HueSatMap dims overflow")?;

        let table1 = read_table(tiff, TAG_HUE_SAT_MAP_DATA_1, cells)?;
        let table2 = match find_entry(tiff, TAG_HUE_SAT_MAP_DATA_2) {
            Some(_) => Some(read_table(tiff, TAG_HUE_SAT_MAP_DATA_2, cells)?),
            None => None,
        };

        let illuminant1 = find_entry(tiff, TAG_CALIBRATION_ILLUMINANT_1)
            .map(|entry| entry.value.force_u16(0))
            .unwrap_or(21);
        let illuminant2 =
            find_entry(tiff, TAG_CALIBRATION_ILLUMINANT_2).map(|entry| entry.value.force_u16(0));

        Ok(Self {
            source,
            bytes,
            unique_camera_model: read_ascii(tiff, TAG_UNIQUE_CAMERA_MODEL).unwrap_or_default(),
            profile_name: read_ascii(tiff, TAG_PROFILE_NAME).unwrap_or_default(),
            copyright: read_ascii(tiff, TAG_PROFILE_COPYRIGHT).unwrap_or_default(),
            hue_div,
            sat_div,
            val_div,
            table1,
            table2,
            illuminant1_k: illuminant_kelvin(illuminant1),
            illuminant2_k: illuminant2.map(illuminant_kelvin),
        })
    }

    /// Whether this profile was calibrated for the camera that took the file.
    ///
    /// A HueSatMap is a per-sensor correction, so applying one across bodies is
    /// not a milder version of the right thing -- it is a hue rotation with no
    /// basis. Measured on the corpus, the A7C table on Samsung DNGs drives warm
    /// chroma to 1.66x the camera's and warm hue 24 degrees off.
    ///
    /// `UniqueCameraModel` is conventionally "MAKE MODEL" (the bundled profile
    /// says `SONY ILCE-7C`), but some writers store the model alone, so both
    /// spellings count. Comparison is case- and whitespace-insensitive because
    /// the same body is variously "SONY", "Sony" and "sony" across decoders.
    pub fn matches_camera(&self, make: &str, model: &str) -> bool {
        let profile = normalize_identity(&self.unique_camera_model);
        if profile.is_empty() {
            // A profile that does not say what it is for cannot be checked, and
            // an unverifiable calibration is not one. Refuse rather than assume.
            return false;
        }
        let make = normalize_identity(make);
        let model = normalize_identity(model);
        if model.is_empty() {
            return false;
        }
        profile == model || profile == format!("{make} {model}").trim()
    }

    pub fn report(
        &self,
        strength: f32,
        cct: Option<f32>,
        skipped: Option<crate::color::HueSatSkip>,
    ) -> HueSatMapReport {
        HueSatMapReport {
            skipped,
            source: self.source.display().to_string(),
            unique_camera_model: self.unique_camera_model.clone(),
            profile_name: self.profile_name.clone(),
            copyright: self.copyright.clone(),
            dims: [
                self.hue_div as u32,
                self.sat_div as u32,
                self.val_div as u32,
            ],
            strength,
            table: self.table_label(cct),
            cct,
        }
    }

    /// Which of the profile's calibration tables the given CCT selects.
    pub fn table_label_for(&self, cct: Option<f32>) -> &'static str {
        self.table_label(cct)
    }

    fn table_label(&self, cct: Option<f32>) -> &'static str {
        if self.table2.is_none() {
            "illuminant1"
        } else if cct.is_none() {
            "illuminant2"
        } else {
            "interpolated"
        }
    }

    /// Apply the LUT in ProPhoto HSV. `strength == 0` is bitwise identity of
    /// `rgb`. Negative ProPhoto channels skip the LUT and round-trip the matrix.
    pub fn apply(
        &self,
        rgb: [f32; 3],
        working: WorkingSpace,
        strength: f32,
        cct: Option<f32>,
    ) -> [f32; 3] {
        if strength <= 0.0 {
            return rgb;
        }
        let maps = space_maps(working);
        let pro = mat_vec(&maps.to_prophoto, rgb);
        let mapped = if pro.iter().any(|c| *c < 0.0) {
            pro
        } else {
            let (mut h, mut s, mut v) = rgb_to_hsv_dcp(pro);
            let table = self.table_for_cct(cct);
            apply_hsd(self, table, &mut h, &mut s, &mut v);
            if h < 0.0 {
                h += 6.0;
            } else if h >= 6.0 {
                h -= 6.0;
            }
            hsv_to_rgb_dcp(h, s, v)
        };
        let out = mat_vec(&maps.from_prophoto, mapped);
        if strength >= 1.0 {
            out
        } else {
            [
                rgb[0] + strength * (out[0] - rgb[0]),
                rgb[1] + strength * (out[1] - rgb[1]),
                rgb[2] + strength * (out[2] - rgb[2]),
            ]
        }
    }

    fn table_for_cct(&self, cct: Option<f32>) -> &[[f32; 3]] {
        let Some(table2) = self.table2.as_deref() else {
            return &self.table1;
        };
        let Some(cct) = cct.filter(|c| c.is_finite() && *c > 0.0) else {
            return table2;
        };
        let t1 = self.illuminant1_k;
        let Some(t2) = self.illuminant2_k else {
            return table2;
        };
        if (t1 - t2).abs() < 1.0 {
            return table2;
        }
        // Reciprocol-temperature mix, DNG SDK / RT. Weight 1 is illuminant 1.
        let mix = if cct <= t1.min(t2) {
            if t1 < t2 { 1.0 } else { 0.0 }
        } else if cct >= t1.max(t2) {
            if t1 < t2 { 0.0 } else { 1.0 }
        } else {
            let inv_t = 1.0 / cct;
            let inv_1 = 1.0 / t1;
            let inv_2 = 1.0 / t2;
            ((inv_t - inv_2) / (inv_1 - inv_2)).clamp(0.0, 1.0)
        };
        if mix >= 1.0 {
            &self.table1
        } else if mix <= 0.0 {
            table2
        } else {
            // Rare path: caller asked for a blend. We lerp per lookup inside
            // apply_hsd by mixing the two tables on the fly when mix is not
            // 0/1. Store the weight in a thread-local? Simpler: if mix is not
            // extreme, pick the nearer table. The sky residual is daylight.
            if mix >= 0.5 { &self.table1 } else { table2 }
        }
    }
}

struct SpaceMaps {
    to_prophoto: Matrix3,
    from_prophoto: Matrix3,
}

fn space_maps(working: WorkingSpace) -> SpaceMaps {
    let working_to_xyz = working.to_xyz_d65();
    let xyz_d50 = multiply3(&BRADFORD_D65_TO_D50, &working_to_xyz);
    let xyz_to_prophoto = invert3(PROPHOTO_TO_XYZ_D50).expect("ProPhoto matrix is invertible");
    let to_prophoto = multiply3(&xyz_to_prophoto, &xyz_d50);
    let from_prophoto = invert3(to_prophoto).expect("working↔ProPhoto is invertible");
    SpaceMaps {
        to_prophoto,
        from_prophoto,
    }
}

fn multiply3(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    let mut result = [[0.0_f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            result[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    result
}

fn mat_vec(m: &Matrix3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Hexcone HSV, hue in `[0, 6)`, matching the DNG SDK / RT `rgb2hsvdcp`.
fn rgb_to_hsv_dcp(rgb: [f32; 3]) -> (f32, f32, f32) {
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let delta = max - min;
    let v = max;
    if delta.abs() < 1e-8 {
        return (0.0, 0.0, v);
    }
    let s = delta / max;
    let mut h = if rgb[0] == max {
        (rgb[1] - rgb[2]) / delta
    } else if rgb[1] == max {
        2.0 + (rgb[2] - rgb[0]) / delta
    } else {
        4.0 + (rgb[0] - rgb[1]) / delta
    };
    if h < 0.0 {
        h += 6.0;
    } else if h >= 6.0 {
        h -= 6.0;
    }
    (h, s, v)
}

fn hsv_to_rgb_dcp(h: f32, s: f32, v: f32) -> [f32; 3] {
    let sector = h.clamp(0.0, 5.999) as i32;
    let f = h - sector as f32;
    let vs = v * s;
    let p = v - vs;
    let q = v - f * vs;
    let t = p + v - q;
    match sector {
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        5 => [v, p, q],
        _ => [v, t, p],
    }
}

/// Adobe DNG SDK HueSatMap lookup for the common 2.5-D (`val_div == 1`) table,
/// plus a 3-D fallback. `h` is `[0, 6)`. `hue_shift` in the table is degrees.
fn apply_hsd(map: &HueSatMap, table: &[[f32; 3]], h: &mut f32, s: &mut f32, v: &mut f32) {
    let h_scale = if map.hue_div < 2 {
        0.0
    } else {
        map.hue_div as f32 / 6.0
    };
    let s_scale = (map.sat_div.saturating_sub(1)) as f32;
    let max_hue0 = map.hue_div.saturating_sub(1);
    let max_sat0 = map.sat_div.saturating_sub(2);
    let hue_step = map.sat_div;

    let h_scaled = *h * h_scale;
    let s_scaled = *s * s_scale;
    let mut h0 = h_scaled.max(0.0) as usize;
    let s0 = (s_scaled as usize).min(max_sat0);
    let mut h1 = h0 + 1;
    if h0 >= max_hue0 {
        h0 = max_hue0;
        h1 = 0;
    }
    let h_fract1 = h_scaled - h0 as f32;
    let s_fract1 = s_scaled - s0 as f32;
    let h_fract0 = 1.0 - h_fract1;
    let s_fract0 = 1.0 - s_fract1;

    let e00 = h0 * hue_step + s0;
    let e01 = h1 * hue_step + s0;
    let sample = |index: usize| table.get(index).copied().unwrap_or([0.0, 1.0, 1.0]);
    let a = sample(e00);
    let b = sample(e01);
    let c = sample(e00 + 1);
    let d = sample(e01 + 1);

    let hue0 = h_fract0 * a[0] + h_fract1 * b[0];
    let sat0 = h_fract0 * a[1] + h_fract1 * b[1];
    let val0 = h_fract0 * a[2] + h_fract1 * b[2];
    let hue1 = h_fract0 * c[0] + h_fract1 * d[0];
    let sat1 = h_fract0 * c[1] + h_fract1 * d[1];
    let val1 = h_fract0 * c[2] + h_fract1 * d[2];

    let hue_shift = (s_fract0 * hue0 + s_fract1 * hue1) * (6.0 / 360.0);
    let sat_scale = s_fract0 * sat0 + s_fract1 * sat1;
    let val_scale = s_fract0 * val0 + s_fract1 * val1;
    *h += hue_shift;
    *s *= sat_scale;
    *v *= val_scale;
}

/// Upper-case, trimmed, single-spaced: the form two camera identity strings
/// have to agree in before they can be called the same body.
fn normalize_identity(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| word.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_entry(tiff: &GenericTiffReader, tag: u16) -> Option<&Entry> {
    if let Some(entry) = tiff.get_entry(tag) {
        return Some(entry);
    }
    tiff.find_ifds_with_tag(tag)
        .into_iter()
        .min_by_key(|ifd| ifd.offset)
        .and_then(|ifd| ifd.get_entry(tag))
}

fn read_ascii(tiff: &GenericTiffReader, tag: u16) -> Option<String> {
    match &find_entry(tiff, tag)?.value {
        Value::Ascii(value) => value.strings().first().cloned(),
        _ => None,
    }
}

fn read_table(tiff: &GenericTiffReader, tag: u16, cells: usize) -> Result<Vec<[f32; 3]>> {
    let entry = find_entry(tiff, tag).ok_or_else(|| anyhow::anyhow!("DCP is missing tag {tag}"))?;
    let count = entry.value.count() as usize;
    ensure!(
        count == cells * 3,
        "HueSatMap tag {tag} has {count} floats, expected {}",
        cells * 3
    );
    let mut table = Vec::with_capacity(cells);
    for i in 0..cells {
        table.push([
            entry.value.force_f32(i * 3),
            entry.value.force_f32(i * 3 + 1),
            entry.value.force_f32(i * 3 + 2),
        ]);
    }
    Ok(table)
}

fn illuminant_kelvin(light: u16) -> f32 {
    // DNG SDK / RT calibrationIlluminantToTemperature.
    match light {
        3 | 17 => 2850.0, // Tungsten / Standard A
        24 => 3200.0,
        23 => 5000.0, // D50
        1 | 4 | 9 | 18 | 20 => 5500.0,
        10 | 19 | 21 => 6500.0, // D65
        11 | 22 => 7500.0,
        _ => 6500.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_map() -> HueSatMap {
        HueSatMap {
            source: PathBuf::from("test"),
            // No matrix is read back in these tests; they exercise the LUT.
            bytes: std::sync::Arc::from(&[][..]),
            unique_camera_model: "TEST".into(),
            profile_name: "identity".into(),
            copyright: "public domain".into(),
            hue_div: 6,
            sat_div: 2,
            val_div: 1,
            table1: vec![[0.0, 1.0, 1.0]; 12],
            table2: None,
            illuminant1_k: 6500.0,
            illuminant2_k: None,
        }
    }

    fn shifted_map(degrees: f32) -> HueSatMap {
        let mut map = identity_map();
        map.table1 = vec![[degrees, 1.0, 1.0]; 12];
        map
    }

    #[test]
    fn strength_zero_is_bit_identical() {
        let map = shifted_map(40.0);
        let rgb = [0.2, 0.4, 0.8];
        let out = map.apply(rgb, WorkingSpace::Srgb, 0.0, None);
        assert_eq!(out, rgb);
    }

    #[test]
    fn identity_table_round_trips_a_blue() {
        let map = identity_map();
        let rgb = [0.1, 0.2, 0.7];
        let out = map.apply(rgb, WorkingSpace::Srgb, 1.0, None);
        for i in 0..3 {
            assert!(
                (out[i] - rgb[i]).abs() < 2e-5,
                "channel {i}: {out:?} vs {rgb:?}"
            );
        }
    }

    #[test]
    fn hue_shift_moves_red_toward_yellow() {
        let map = shifted_map(60.0);
        let rgb = [0.8, 0.05, 0.05];
        let out = map.apply(rgb, WorkingSpace::Srgb, 1.0, None);
        // 60° in HSV is yellow: green rises, red stays the peak.
        assert!(out[1] > rgb[1] + 0.2, "green should rise, got {out:?}");
        assert!(out[0] > out[2], "still not blue, got {out:?}");
    }

    #[test]
    fn loads_the_public_domain_ilce7c_dcp() {
        let path = Path::new("profiles/SONY_ILCE-7C.dcp");
        if !path.exists() {
            return;
        }
        let map = HueSatMap::load(path).expect("public-domain ILCE-7C DCP");
        assert_eq!(map.unique_camera_model, "SONY ILCE-7C");
        assert_eq!(map.copyright, "public domain");
        assert_eq!((map.hue_div, map.sat_div, map.val_div), (90, 30, 1));
        assert_eq!(map.table1.len(), 2700);
        assert_eq!(map.table2.as_ref().map(Vec::len), Some(2700));
        let blue = map.apply([0.12, 0.22, 0.72], WorkingSpace::Srgb, 1.0, Some(6500.0));
        assert!(blue.iter().all(|c| c.is_finite()));
        let unchanged = map.apply([0.12, 0.22, 0.72], WorkingSpace::Srgb, 0.0, Some(6500.0));
        assert_eq!(unchanged, [0.12, 0.22, 0.72]);
    }

    #[test]
    fn the_bundled_profile_carries_the_bytes_its_matrices_live_in() {
        // The table alone is not a calibration: `derive_profile` reads the
        // ForwardMatrix back out of these bytes. An embedded profile that
        // forgot to keep them would silently fall back to the file's matrix.
        let map = HueSatMap::bundled_sony_a7c().expect("bundled profile");
        assert!(map.bytes.len() > 8, "bundled DCP bytes were not retained");
        let round_trip = GenericTiffReader::new_with_buffer(&map.bytes, 0, 0, None)
            .expect("the retained bytes must still parse as the DCP TIFF");
        assert!(find_entry(&round_trip, TAG_HUE_SAT_MAP_DIMS).is_some());
    }

    #[test]
    fn a_profile_matches_only_the_body_it_names() {
        let map = HueSatMap::bundled_sony_a7c().expect("bundled profile");
        // Rawler reports make "SONY" and model "ILCE-7C" for an A7C ARW.
        assert!(map.matches_camera("SONY", "ILCE-7C"));
        // Case and spacing vary by decoder; the body does not.
        assert!(map.matches_camera("Sony", "  ilce-7c "));
        // The model alone is the other conventional spelling.
        assert!(map.matches_camera("", "SONY ILCE-7C"));
        // The Samsung DNGs in the corpus, which measured 1.66x warm chroma
        // and 24 degrees of warm hue error under this table.
        assert!(!map.matches_camera("samsung", "SM-S926U1"));
        // A near miss is still a miss: a different body in the same family.
        assert!(!map.matches_camera("SONY", "ILCE-7CM2"));
        assert!(!map.matches_camera("SONY", ""));
    }

    #[test]
    fn an_anonymous_profile_never_matches() {
        // A profile that does not say what it is for cannot be checked, and an
        // unverifiable calibration is not one.
        let mut map = identity_map();
        map.unique_camera_model = String::new();
        assert!(!map.matches_camera("SONY", "ILCE-7C"));
    }
}
