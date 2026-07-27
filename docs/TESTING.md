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
  --emit-baseline \
  --jobs 1
```

For every image, review:

1. automatic render;
2. baseline render;
3. JSON sidecar;
4. Lightroom/Darktable Auto reference, when available.

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
- `near_white_fraction`: helps identify already clipped or extremely bright
  images.
- `exposure_ev`: repeated extreme corrections may indicate a calibration issue.
