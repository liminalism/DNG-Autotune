# 2026-08-29 — the HueSatMap becomes a calibration: paired matrix, interpolated illuminants, camera gate

`--preset vivid` shipped as the bundled ART/RawTherapee `SONY ILCE-7C`
HueSatMap applied on top of the transform derived from the RAW's own
calibration. That is not what a HueSatMap is: it encodes the residual left by
*its own profile's* `ForwardMatrix`, so after a different matrix it is a hue
rotation with no basis. Measured the previous day
(`docs/evidence/preset-standard-vivid-20260829/`) that rotation was **+15° on
blue and +10° on warm**, with chroma lifted ×1.46 — visible as lavender skies
and yellow-green foliage.

Three changes, all confined to paths that load a profile. `auto` and
`standard` are byte-identical throughout.

1. **The matrix comes from the same profile.** `dngcolor::gather` is split so
   its calibration half can be read from a standalone DCP while the scene white
   still comes from the RAW (a profile has no picture in it).
   `camera_to_working_from_profile` composes the DNG 1.7 model from the DCP's
   `ColorMatrix1/2` and `ForwardMatrix1/2`.
2. **The illuminants interpolate.** `solve_white` returns the scene CCT from
   the same solve, so the table's `CalibrationIlluminant1` (Std A) and
   `CalibrationIlluminant2` (D65) finally blend on an ARW. Before, no Sony file
   could produce a CCT at all — `camera_to_working_any` needs a DNG
   `ColorMatrix1` tag an ARW does not carry — so all 86 used the D65 table
   alone.
3. **A mismatched profile is declined.** `UniqueCameraModel` is compared to the
   file's make and model; on a mismatch neither the matrix nor the table
   applies, a warning names both, and the sidecar records
   `hue_sat_map.skipped`. Chosen with the user over failing the file or
   applying anyway.

## Method

The same 102 RAW+JPEG pairs as the previous day (86 Sony, 16 Samsung DNG),
graded by `tools/compare_presets.py` in CIELAB against the camera's own JPEG.
Three arms of one binary: `--preset standard`, `--preset vivid`, and
`--preset vivid --saturation-scale 0.7874` (which returns the preset's global
chroma scalar from 1.27 to 1.00 — see result 3).

## Result 1 — the rotation is gone

Sony, 86 frames, medians, taken directly against `standard` on the same pixels:

| | table alone (before) | paired profile (after) |
|---|---|---|
| chroma × standard | 1.462 | **1.109** |
| chroma-weighted \|Δhue\| | 7.70° | **4.94°** |
| blue hue | **+15.16°** | **+3.38°** |
| warm hue | **+9.60°** | **+0.62°** |

78% of the blue rotation and 94% of the warm rotation were the missing matrix,
not the table. `before-after-sheet.jpg` is the same result at a glance: the
lavender sky on `_DSC1266` and `_DSC1289` and the yellow-green wood on
`_DSC1284` are gone.

Against the camera JPEG, the overshoot follows: the share of the camera's
`C* > 40` pixels that vivid pushes more than 25% *above* falls from **25.5% to
4.7%**, while the share it leaves more than 25% below falls from standard's
53.5% to 44.1%.

| preset | C ratio | C sat | hue err | C blue | C warm | h blue | h warm | neutralised | overshot |
|---|---|---|---|---|---|---|---|---|---|
| `standard` | 1.172 | 0.698 | 15.45° | 1.026 | 0.949 | −6.84° | +3.08° | 53.5% | 2.9% |
| `vivid` (before) | 1.709 | 0.956 | 16.78° | 1.564 | 1.288 | +5.36° | +8.87° | 26.7% | 25.5% |
| `vivid` (after) | 1.270 | 0.753 | 16.19° | 1.101 | 1.036 | −4.52° | +3.53° | 44.1% | 4.7% |

Read honestly: the calibrated table still does not *match* the camera. Its
chroma-weighted absolute hue error stays a little above `standard`'s on all
five Sony sets, because the camera JPEG is a look and the calibration is not
aimed at it. What improved is placement — blue hue bias moves from 5.4°
*past* the camera to 4.5° short of it, closer than `standard`'s 6.8° — and
warm chroma lands at 1.036 of the camera's where `standard` sits at 0.949.

## Result 2 — the indoor set flips from worst to best

`raw/indoor_tungsten` was the set the missing interpolation hurt most, and it
is the set that gained most:

| set | n | blue hue bias, `standard` | before | after |
|---|---|---|---|---|
| `indoor_tungsten` | 9 | +13.65° | **+24.52°** | **+8.49°** |
| `raw_3rd_batch` (sky) | 42 | −9.69° | +1.58° | −7.61° |
| `arw_better` | 13 | −5.54° | +8.50° | −1.99° |
| `raw_backlit2` | 14 | −7.04° | +6.82° | −4.29° |
| `raw_extremely_bright` | 8 | −7.58° | +1.03° | −7.15° |

Indoors, vivid used to be 10.9° *worse* than `standard` on blue and is now
5.2° *better*. That is the Std A table doing its job for the first time:
`huesat-decisions.txt` shows every Sony frame now reporting an interpolated
table with a scene CCT, where before all 86 read `illuminant2` with no CCT.

## Result 3 — `standard`'s global chroma scalar is *not* a double count

The suspicion was that `derive_params` gives `Vivid` the same global
`saturation` 1.27 as `Standard`, a scalar that exists (docs/KNOWN_LIMITATIONS.md)
to close the colorimetric-vs-camera chroma shortfall — the very shortfall a
per-hue calibration also closes. The third arm tests it by returning that
scalar to `neutral`'s 1.00 and letting the table work alone.

It is worse on every axis: chroma on the camera's boldest pixels falls to
**0.585**, below `standard`'s 0.698 and well below vivid's 0.753; the share of
those pixels left more than 25% under the camera rises to **71.3%**; and it
newly desaturates 10.5% of `standard`'s bold pixels by more than a quarter.

So there is no double count, and the reason is result 1: once the table is
paired with its own matrix it is nearly chroma-neutral (×1.11), a correction
rather than a boost. Removing the grade's scalar just under-saturates.
**`standard` needs no change**, and the scalar stays where the midtone work
placed it.

## Result 4 — declining is exact, and it is common

All 16 Samsung frames rendered **byte-identical** to `--preset standard`; all
86 Sony frames differ. A declined calibration is a true no-op, not an
approximate one, which is what makes a mixed batch legible: every frame is
either the profile's rendering or the plain one, never something in between.

One caveat the unpaired table did not have: a calibration corrects in both
directions, so `vivid` can now reduce chroma as well as raise it. Over 86
frames, 4 have more than 1% of `standard`'s `C* > 40` pixels losing more than
a quarter of their chroma, the worst being `_DSC1254` at 4.4% — a frame where
the camera's own saturated population is 0.6% of the image. The median is
still 0.00%.

## Reproducing

```bash
cargo build --release
for arm in "standard" "vivid"; do
  ./target/release/raw-autotune raw/arw_better raw/raw_extremely_bright \
      raw/raw_backlit2 raw/indoor_tungsten raw/raw_3rd_batch \
      --output out/$arm --format jpeg --preset $arm --overwrite --sidecar
done
python3 tools/compare_presets.py out/standard out/vivid \
    --reference raw/jpeg raw/arw_better raw/raw_backlit2 \
                raw/raw_extremely_bright raw/indoor_tungsten \
    --match '^_dsc' --against out/standard
```

`tests/profile_calibration.rs` pins the four rules on the corpus: a matching
profile supplies both halves and interpolates, a mismatched one changes
nothing and says why, strength 0 stays an exact no-op, and the profile route
honours the requested working space.
