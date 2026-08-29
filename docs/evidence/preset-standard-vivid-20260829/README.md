# 2026-08-29 — when to choose `--preset standard` and when `--preset vivid`

The last open Candidate D question: `vivid` ships (standard's tone grade plus
the bundled public-domain ART/RawTherapee `SONY ILCE-7C` HueSatMap), but
nothing said who should pick it, and nothing had checked the failure the
table could plausibly cause — a hue/saturation LUT can flatten a legitimately
saturated blue or sunset as easily as it can lift a dull one.

## Method

102 RAW+JPEG pairs, every Sony and Samsung frame in the corpus that has the
camera's own JPEG beside it: `raw/raw_3rd_batch` (42 ARW + 16 Samsung DNG,
paired through `raw/jpeg`), `raw/arw_better` (13), `raw/raw_backlit2` (14),
`raw/raw_extremely_bright` (8), `raw/indoor_tungsten` (9). `raw_3rd_batch` is
the sky batch the purple-sky work was tuned on; the other four Sony sets are
held out from it.

Three arms of the same binary, archive-auto-v8 otherwise unchanged:
`--preset auto` (the default), `--preset standard`, `--preset vivid`.

`tools/compare_presets.py` measures each render against its camera JPEG in
CIELAB. `tools/grade_sky.py` could not answer this: it measures sky brightness
and a green deficit, and the whole standard/vivid difference lives in hue and
chroma. The new tool reports

- `C ratio` — our mean chroma over the camera's, whole frame
- `C sat` — the same ratio restricted to the camera's own `C* > 40` pixels,
  which is a different number from the frame average and has to be, because
  `standard` overshoots the frame while falling short exactly where the camera
  is boldest
- `hue err` / `hue bias` — chroma-weighted mean absolute and signed hue error
- per-family chroma ratios and hue biases, families assigned by the *camera's*
  hue so a preset cannot move a pixel into a flattering bucket
- `neutralised` / `overshot` — share of the camera's `C* > 40` pixels we drop
  more than 25% below, or push more than 25% above

`--against` repeats the chroma and hue measures directly between two of our
own renders. Those pairs are perfectly registered, so `desat` is exact in a
way the camera comparison is not.

Per-frame data: `sony-per-frame.json`, `samsung-per-frame.json`. Per-set
medians: `medians.json`. Pictures: `standard-vs-vivid-sheet.jpg`.

## Result 1 — vivid does not neutralise saturated colour. It never did.

Over all 86 Sony frames, the share of `standard`'s `C* > 40` pixels that
`vivid` drops more than 25% below is **0.00% median, 0.9% worst frame**. The
per-family chroma ratio of vivid over standard is above 1.0 on every frame for
blue (min 1.30) and on all but a handful for warm (min 0.83) and green
(min 0.68). The table lifts; it does not flatten. The check the work item
asked for passes, and it is not close.

## Result 2 — the choice is a real trade, and both sides are measured

Sony, 86 frames, medians against the camera JPEG:

| preset | C ratio | C sat | hue err | C blue | C warm | h blue | h warm | neutralised | overshot |
|---|---|---|---|---|---|---|---|---|---|
| `auto` | 1.115 | 0.652 | 15.36° | 1.005 | 0.905 | −7.23° | +3.17° | 60.2% | 1.4% |
| `standard` | 1.172 | 0.698 | 15.45° | 1.026 | 0.949 | −6.84° | +3.08° | 53.5% | 2.9% |
| `vivid` | 1.709 | 0.956 | 16.78° | 1.564 | 1.288 | +5.36° | +8.87° | 26.7% | 25.5% |

And directly, vivid over standard on the same pixels: chroma ×1.46 median
(×1.51 blue, ×1.38 warm, ×1.51 green), blue hue **+15.2°**, warm hue **+9.6°**.

So:

- **`vivid` is the only preset that reaches the camera's chroma where the
  camera is boldest.** `auto` and `standard` sit 30–35% under it on `C* > 40`
  pixels; vivid lands within 5%. That is a genuine gap it closes.
- **It closes it by lifting the entire frame,** not just the bold pixels, so
  the frame average goes from 17% over the camera to 71% over, and a quarter
  of the camera's saturated pixels end up more than 25% *above* it.
- **It rotates hue, and the rotation is visible.** +15° on blue is toward
  violet — the same direction as the sky the faithful-blue-violet decision was
  taken on, another 15° along it. +10° on warm is toward yellow. Hue error
  against the camera goes *up*, not down, on four of the five sets.

`standard-vs-vivid-sheet.jpg` is the same conclusion at a glance: on `_DSC1266`
and `_DSC1289` the sky turns lavender, and on `_DSC1284` and the Samsung frame
the foliage turns yellow-green.

## Result 3 — vivid is worst where its table is least applicable

Per set, vivid minus standard and vivid against the camera:

| set | n | vivid `hue err` vs standard's | vivid h blue | note |
|---|---|---|---|---|
| `raw_3rd_batch` (sky) | 42 | 15.75° vs 16.16° | +1.58° | the only set where vivid's hue error improves |
| `arw_better` | 13 | 16.63° vs 13.20° | +8.50° | |
| `raw_backlit2` | 14 | 23.65° vs 17.61° | +6.82° | worst absolute hue error |
| `raw_extremely_bright` | 8 | 12.07° vs 12.08° | +1.03° | |
| `indoor_tungsten` | 9 | 14.81° vs 11.38° | +24.52° | blue hue error more than doubles |
| Samsung DNG | 14 | 14.23° vs 13.74° | +4.32° | warm hue error +24.15°, warm chroma 1.66 |

Two structural reasons, both confirmed from the sidecars
(`huesatmap-table-selection.json`):

**The profile's tungsten table never runs on the camera it was made for.**
The HueSatMap interpolates between its two calibration illuminants
(`CalibrationIlluminant1 = 17` Std A, `CalibrationIlluminant2 = 21` D65) only
when a CCT is available, and the CCT comes from the full DNG colour report.
`dngcolor::camera_to_working_any` needs a DNG `ColorMatrix1` tag, which an ARW
does not have. So: **86/86 Sony frames used the D65 table alone, and 16/16
Samsung frames interpolated** — the interpolation runs only on the camera the
profile is not calibrated for. This is why `indoor_tungsten` is the worst Sony
set.

**Nothing checks the profile against the file.** The DCP's
`UniqueCameraModel` is read and recorded in the sidecar, but no code compares
it to the RAW being developed, and no warning is printed. `--preset vivid` on
a Samsung DNG silently applies a Sony A7C table; that is the 1.66 warm chroma
ratio and the +24° warm hue error in the last row.

**The table is also not paired with its own matrix.** The DCP carries
`ColorMatrix1/2` and `ForwardMatrix1/2`; we use none of them, applying the
HueSatMap on top of the transform derived from the RAW's own calibration. A
HueSatMap encodes the residual of the profile's *own* forward matrix, so
applying it after a different one is a look, not a calibration — even on the
A7C. `ProfileHueSatMapEncoding` is absent (linear, the default), so the
application space itself is right; it is the pairing that is not.

## What this settles

`vivid` stays opt-in and stays out of the archive path, as decided. The
guidance now in `README.md` follows from the three results above: reach for it
on saturated daylight Sony landscapes where the camera-JPEG boldness is
wanted, and stay on `standard` for anything where hue placement is the point —
indoor and mixed light, reproduction, and every camera that is not the A7C.
