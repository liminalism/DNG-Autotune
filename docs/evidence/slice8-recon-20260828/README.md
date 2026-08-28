# Slice 8 reconstruction ground truth — 2026-08-28

Pairs from `raw/raw_backlit2`. Less-exposed frame is the unclipped ground
truth; more-exposed frame is the reconstruction candidate.

| Pair | Bright (clipped) | Dark (GT) | EXIF ΔEV |
|---|---|---|---|
| Hand to sky | `_DSC1345` 1/250 f/10 ISO 100 | `_DSC1346` 1/3200 f/10 ISO 100 | 3.68 |
| Window | `_DSC1349` 1/30 f/2.8 ISO 250 | `_DSC1348` 1/60 f/4 ISO 100 | 3.35 |

`examples/probe-highlight-gt.rs` develops both frames to scene-linear
(production path) and compares OKLab chroma of pixels the harmonic
estimator touched to the dark frame scaled by `2^ΔEV`. Display-stage
dumps cannot do this: `render_baseline` clips at 1.0, so a 12× lift of
the dark sun compared reconstructed luma to encoded white.

## Results

| Pair | n | ON chroma RMSE | OFF chroma RMSE | ON chroma mean | OFF chroma mean | GT chroma mean |
|---|---|---|---|---|---|---|
| Hand/sky | 27 717 | 0.032 | 0.119 | 0.032 | 0.112 | 0.042 |
| Window | 462 590 | 0.039 | 0.177 | 0.048 | 0.159 | 0.034 |

Reconstruction is closer to the dark-frame chroma than reconstruction
disabled. Disabled invents chroma (window 0.159 vs GT 0.034). Pixel-wise
hue p90 is large for **both** arms (~140–170°): these are handheld
brackets (hand pose and camera both moved), and the integer-pixel search
locked at (0, 0). Hue is not a usable gate until a real warp
registration exists.

## Decision

`--local-tone` and `--hdr` stay opt-in. Colour continuity vs GT is
encouraging on chroma and not established on hue. The gate does not
pass for promoting either operator.

JSON: `hand-sky.json`, `window.json`.
