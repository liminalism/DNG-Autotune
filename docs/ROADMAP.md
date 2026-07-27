# Roadmap after the first image batch

## 0.1.x: make the prototype dependable

- Compile and test on Windows, Linux, and macOS.
- Add regression RAW samples that can be redistributed.
- Preserve EXIF and embed an sRGB ICC profile.
- Add clearer collision reporting and user-selectable output naming templates.
- Add a machine-readable batch summary.
- Add optional scene-linear EXR or floating-point TIFF output.
- Profile memory on 12 MP, 24 MP, and 50 MP files.

## 0.2: photographic corrections

- Better clipped-highlight reconstruction before demosaic.
- Camera/ISO-aware noise estimate and conservative denoising.
- Hot/dead pixel suppression.
- Lens correction through DNG opcodes or a lens database.
- Output-size-aware sharpening.
- More deliberate gamut compression and saturated-highlight handling.

## 0.3: stronger non-neural automatic controller

- Separate center, edge, highlight, and probable-sky statistics.
- Explicit high-key, low-key, night, backlit, and flat-scene policies.
- Candidate render generation and objective sanity checks.
- Sequence consistency for bursts and time-adjacent photographs.
- User-tunable policy file.

## 0.4: local adaptation

- Edge-aware log-luminance base/detail decomposition.
- Controlled shadow lift and highlight compression masks.
- Halo checks and strength limits.
- Face/skin-safe detail treatment, initially through heuristics.

## Deferred ONNX analysis

Add models only after the non-neural failures are classified:

1. small face detector;
2. lightweight scene classifier;
3. optional salient-subject mask;
4. semantic sky/person/vegetation segmentation;
5. depth only if it improves real images.

Model outputs should become `AnalysisFeatures`; they should not directly emit
finished pixels. The deterministic renderer remains the execution layer.

## Learned parameter controller

Once a substantial reference set exists, train a compact model to predict the
existing explicit parameter vector:

```text
exposure EV
black/white input EV
contrast
shadow/highlight powers
vibrance/saturation
local tone strength
denoise strength
sharpening strength
```

That preserves debuggability and allows heuristic fallback.
