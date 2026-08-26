# Roadmap

Reconciled against the code on 2026-08-26, at 0.1.19 / `archive-auto-v6`. This
file had been left as it was written after the first image batch, so most of
what it listed as future work had in fact shipped. Every line below is marked
**done**, **partial** or **open** against what is actually in `src/`, and the
authority for a "done" is `CHANGELOG.md` and `docs/STATUS.md`, not this file.

## 0.1.x: make the prototype dependable

- **done** — Compile and test on Linux and Windows. macOS remains unverified
  (`docs/BUILD_STATUS.md`).
- **open** — Redistributable regression RAW samples. The corpus under `raw/` is
  still not distributable, so every corpus test skips when it is absent.
- **done** — EXIF and an embedded sRGB ICC profile, 0.1.17 (`src/metadata.rs`),
  on all three output formats and, since 2026-08-26, on the in-memory API too.
- **partial** — Collision reporting exists; user-selectable output naming
  templates do not.
- **open** — Scene-linear EXR or floating-point TIFF output. `OutputFormat` is
  still `{Tiff, Png, Jpeg}` and all three are integer.
- **done** — Memory profiled on 12, 24 and 50 MP files; `--jobs auto` plans the
  worker count from free memory and the largest input (`src/memory.rs`, 0.1.19).

## 0.2: photographic corrections

- **done** — Clipped-highlight reconstruction before demosaic
  (`src/raw_highlight.rs`), through the Slice 5 joint raw-domain
  log-chromaticity estimator.
- **done** — Camera/ISO-aware noise estimate and conservative denoising:
  `src/noise.rs`, `src/chroma.rs` (0.1.14/0.1.15) and `src/luma.rs`
  (2026-08-13). All automatic and noise-model driven.
- **done** — Hot/dead pixel suppression (`src/hotpixels.rs`).
- **partial** — DNG `WarpRectilinear` and `FixVignetteRadial` are implemented
  (`src/lens.rs`). `WarpRectilinear2`, fisheye and gain-map opcodes are not.
  An exact Lensfun profile path exists but stays opt-in.
- **done** — Output-size-aware sharpening (`src/sharpen.rs`, 0.1.15).
- **partial** — Gamut compression and saturated-highlight handling exist in
  `src/tone.rs` and `src/color.rs`; the perceptual gamut compressor
  (`raw-autotune.requirement.slice-7-perceptual-gamut-compressor-phase-c-7`)
  is still a proposed requirement.

## 0.3: stronger non-neural automatic controller

This is where the real work now is. Almost none of it has been done.

- **partial** — Separate centre and highlight statistics exist in
  `src/analyze.rs`; edge and probable-sky statistics do not, outside the opt-in
  `--semantic` path.
- **partial** — Of the five promised scene policies, **high-key** and
  **low-key** exist as `TonalClass` classifications, and **night** exists as a
  real automatic policy (`analysis.low_light_score` driving `--night-tone`,
  2026-08-13). **Backlit and flat-scene policies do not exist.** There is no
  backlit detector at all — the only centre-weighting is a weak composition
  prior — and no indoor, tungsten or mixed-illuminant detection: white balance
  is as-shot unless `--local-white-balance` is passed.
- **open** — Candidate render generation and objective sanity checks.
- **open** — Sequence consistency for bursts and time-adjacent photographs.
- **open** — User-tunable policy file.

## 0.4: stronger local adaptation

- **open** — The 0.1.11 Gaussian first pass is still what `--local-tone` uses.
  An edge-aware operator exists for the automatic night path
  (`localtone::build_hdr`), but the general base/detail decomposition does not,
  and `raw-autotune.work.slice-8-local-tone-sky-ground-hdr-evaluation` gates
  any promotion behind a highlight-colour-continuity check that has no evidence
  yet.
- **open** — Corpus-driven halo checks beyond the synthetic edge tests.
- **open** — Face/skin-safe detail treatment.

## Deferred ONNX analysis

Partly landed, and the results are mixed. Models live under `models/` (not in
the tree — `models/artifacts/` is gitignored; `tools/scene_models/` rebuilds
them) and run through `lege-gpu` with a CPU fallback. The whole path is behind
`--semantic`, off by default, and observational by construction: it writes
masks and region evidence to the sidecar and does not touch output pixels.

1. **landed, gated off** — small face detector (YuNet). Zero true positives
   over the 74-image corpus and one low-confidence false-positive person
   response, so no face or person policy is supported. Since 2026-08-26 it does
   not run unless `--semantic-faces` asks for it: nothing consumed the mask, so
   `--semantic` was spending an ONNX session per frame to write an empty plane.
   The `Face` mask stays present and blank either way, so the sidecar shape is
   unchanged. See `raw-autotune.assessment.scene-evidence-usefulness` and
   `raw-autotune.decision.face-detection-is-off-by-default`.
2. **open** — lightweight scene classifier. The MobileOne backbone in
   `models/manifest.json` is explicitly *not* a scene-scores model.
3. **open** — salient-subject mask.
4. **partial** — semantic sky/person/vegetation segmentation via Cityscapes
   LR-ASPP. Sky is validated (detected in 42 of 74 files, masks visually
   aligned); vegetation and building are useful measurement strata only; water,
   snow/sand, text and salient-foreground have no evidence at all.
5. **open** — depth.

The rule still holds and has been kept: model outputs become
`AnalysisFeatures`-style evidence, not finished pixels. The deterministic
renderer remains the execution layer.

## Learned parameter controller

**open**, and unchanged: once a substantial reference set exists, train a
compact model to predict the existing explicit parameter vector.

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
