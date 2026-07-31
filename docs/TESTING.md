# Field testing protocol

## Build check

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

## Small acceptance batch

Start with 10–20 files and one worker:

```bash
raw-autotune test-raws \
  --output test-results \
  --preset auto \
  --format tiff \
  --emit-baseline
```

For every image, review:

1. automatic render;
2. baseline render;
3. JSON sidecar;
4. Lightroom/Darktable Auto reference, when available.

## Local-tone acceptance

The local operator is opt-in. Compare the same representative files at zero,
half, and full strength:

```bash
raw-autotune test-raws --output local-off  --format jpeg --summary local-off.json
raw-autotune test-raws --output local-half --format jpeg --summary local-half.json --local-tone 0.5
raw-autotune test-raws --output local-full --format jpeg --summary local-full.json --local-tone 1
```

`--jobs` sizes itself; a large linear DNG at full local-tone strength is inside
the per-image budget `memory.rs` already sets from the chroma stage.
Inspect hard skyline/building edges for halos, smooth skies for banding, faces
for uneven patches, and high-ISO shadows for amplified noise. In the summaries,
compare `near_white_fraction`, `luminance_entropy`, and `average_gradient`.
Every local-tone report must stay inside -0.75 to +1.0 EV. Also verify that an
omitted flag and `--local-tone 0` produce byte-identical images.

## Paired-JPEG acceptance

Whenever the corpus contains RAW+JPEG pairs, grade against them rather than by
eye. This is the only test that answers the question the program exists to
answer — "is this at least as good as the camera's own JPEG?" — with a number.

```bash
raw-autotune pairs --output out --format jpeg --reference --summary out/summary.json
tools/contact-sheet.py out/summary.json --output out/sheet.html
```

Read, in the batch summary:

- `reference_saturation_ratio.median` — ours over the camera's. 1.0 is a match.
  Unlike colourfulness this does not move with exposure, so it is the measure
  to tune chroma on.
- `reference_subject_ev_delta` — where our curve puts the subject against where
  the camera put it. This one is also produced under `--dry-run`.
- `reference_colourfulness_delta`, `reference_mean_level_delta`.

Then open the sheet, which is sorted worst-first, and work down it.

**Do not read a rise in `near_white_fraction` as blown highlights.** It counts
pixels with *any* channel at or above 98% of full scale, so a saturated sky
trips it with nothing lost. Confirm against `clipped_fraction`, and against
`luminance_entropy` and `average_gradient`, which fall if gradation is really
being flattened.

**Do not read a `subject_display_ev` delta as an error without looking at the
frame.** It assumes the camera's own JPEG is the right answer. On night and
very-high-ISO scenes it is not: on the corpus's ISO 2500 waterfall the camera
renders nearly black, our render is 3.17 EV brighter, and ours is the usable
one. Read a large delta as "we and the camera disagree", then open the pair and
decide who is right. The 0.10 EV acceptance figure below applies to daylight.

**Do not read a high `mean_saturation` ratio at high ISO as extra colour.**
Chroma noise reads as saturation. Before 0.1.14 the ISO 8000+ band measured
2.2x the camera; the excess was red and green speckle, not colour. Check the
`chroma_denoise` block and a 1:1 crop.

## Record failures precisely

Use these labels:

```text
DECODE
DEMOSAIC
WHITE_BALANCE
CAMERA_COLOR
EXPOSURE
BLACK_POINT
HIGHLIGHT_COMPRESSION
HIGHLIGHT_CLIPPING
SHADOW_COMPRESSION
CONTRAST
SATURATION
GAMUT
NOISE
SHARPNESS
LOCAL_TONE
SEMANTIC_SUBJECT
OUTPUT_METADATA
```

Do not call every poor result "bad auto." A wrong camera matrix and a wrong
exposure estimate need different fixes.

## Useful sidecar comparisons

- `p50_ev` versus `center_median_ev`: large difference often indicates
  backlighting or a bright/dark border.
- `key_score`: large positive/negative values flag high-/low-key assumptions.
- `p005_ev` and `p995_ev`: show the range used to establish curve endpoints.
- `near_white_fraction`: helps identify extremely bright images, but see the
  caveat above — it is not a clipping measure.
- `clipped_fraction`: a channel at full scale, i.e. information definitively
  gone. This is the clipping measure.
- `mean_saturation`: brightness-invariant chroma, comparable between two
  renderings of the same scene at different exposure.
- `exposure_ev`: repeated extreme corrections may indicate a calibration issue.
