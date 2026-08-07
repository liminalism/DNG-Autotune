# Plan: White-Blowout Diagnostic Apparatus and Continuous Reconstruction

## Goal

Implement the testing apparatus from `docs/white-blowout-advice.md` (diagnostic dumps, clip-state map, synthetic constant-chromaticity ramps, ablation harness, new metrics) and then execute the advice's production fix sequence — continuous, neighbourhood-guided highlight chromaticity reconstruction with raw-domain clip confidence and perceptual gamut mapping — ending with a controlled local-tone evaluation. No HDR output work until the reconstruction and gamut defects are closed.

## Success Criteria

- Stage diagnostics can prove whether the cyan→violet sky gradient in `_DSC1289`/`_DSC1290` (and `raw/raw_3rd_batch` cohort) originates in `highlight::reconstruct` vs `tone::compress_gamut`: pre/post-reconstruction and pre/post-gamut buffers plus clip-state map, all written under one frozen `ToneParams` so analysis differences do not confound the comparison. The red→magenta boundary test in the advice is runnable on one RAW.
- Synthetic ramp suite exists, runs in `cargo test --release` without corpus files, and fails on the current `highlight.rs:107` (`CLIP_THRESHOLD=0.98` + `NEAR_WHITE_CLIPPED_CHANNELS=2` regime-switch) while passing after the continuous fix:
  - constant-chromaticity (neutral gray, blue-sky, blue→neutral cloud) ramps monotonic in luminance, continuous in hue, no discontinuity at the 1→2 clipped-channel threshold
  - broad smooth gradient crossing 0/1/2/3-channel saturation, fully-clipped interior stays smooth/no invented texture
  - bright sky next to dark foliage (edge-bleed check)
- Ablation sweep is one CLI invocation or a `tools/` script: `--highlight-reconstruction 0`, `{0.25,0.75,1.0}`, `--working-space rec2020`, `--highlight-contrast 1.2`, `recon off + --local-tone 0.35`, each writing lossless PNG so the gradient's stage can be attributed.
- Metrics replace the overloaded "near-white" semantic: per-frame 1/2/3-clipped fractions, reconstruction uncertainty distribution, hue/OKLab discontinuity across clip boundaries, low-frequency hue total variation in bright low-texture regions, pre/post-gamut hue delta, fully-clipped connected-component sizes — exposed in `metrics.rs`/`types.rs` and, where per-file, in the sidecar.
- Production fix lands behind `highlight_reconstruction == 0` byte-identical guarantee (per `CLAUDE.md` regression gate and `highlight.rs:85` contract) and passes the regression gate in `docs/STATUS.md`: `--dry-run --summary` before/after byte-identical when disabled, `cargo test --release` + `cargo clippy --lib --bins --release`, determinism with `--jobs 8`.
- Final gamut stage is hue-preserving (OKLab chroma-reduction toward in-gamut sRGB, per `src/oklab.rs` + `src/tone.rs:102`) rather than the current `compress_gamut` scale-around-anchor + clamp.

## Context And Current Facts

- **Advice diagnosis** (`docs/white-blowout-advice.md:8`): two problems in order — (1) autotuner manufactures false sky chroma during highlight reconstruction, (2) scene may still benefit from single-frame local tone mapping. Current code's three structural hazards: hard `0.98` demosaiced threshold, rule switch at 1→2 clipped channels, and `two-or-three clipped → near-white at strength 1.0` regardless of `0.75`. Output `clipped_fraction` on the defect frames is 0.0004% (`_DSC1289`) / 0.0008% (`_DSC1290`) so final clamp is not the cause.
- **Measured cohort** (`docs/HDR_EVALUATION.md:24`, `docs/white-blowout-advice.md:39`): `_DSC1289` 958,928 pixels ≥1 clipped, 344,861 with ≥2, 0 fully clipped; `_DSC1290` 801,061 clipped, 682,786 multi-channel, 5,828 fully clipped. `src/highlight.rs` already reports `near_white_pixels` (schema v14) but the pipeline still counts `near_white_fraction` coarsely in `src/analyze.rs:332` (`maximum >= 0.995`). White-blowout advice §"Metrics worth adding" explicitly calls that fraction inadequate.
- **Current reconstruction** (`src/highlight.rs:30`): runs on demosaiced camera RGB, threshold `CLIP_THRESHOLD=0.98`, `NEAR_WHITE_CLIPPED_CHANNELS=2`, survivor-anchored blend at `strength` for 1-clipped vs widened anchor over all channels at `1.0` for ≥2-clipped, never darkens. Tests pin `two_clipped_channels_land_on_one_white_balanced_level` and `near_white_reconstruction_is_independent_of_strength`. The advice says this regime switch is exactly what creates a processing-regime gradient following luminance contours.
- **Gamut stage** (`src/tone.rs:102` `compress_gamut`): scales RGB around `mapped_luminance` then per-channel clamp. Comments in `src/color.rs:33` and `src/tone.rs:335` describe the hand-off; advice §"Fix gamut mapping" says this is not perceptually hue-constant and can turn a small upstream error into a visible sky band. `src/oklab.rs` already provides `from_linear_srgb`, `hue`, `chroma`, `hue_difference` for measurement but not for rendering.
- **Pipeline ordering** (`src/pipeline.rs:58` `develop`, `src/color.rs` owned path, `src/rescale.rs:1`): decode → `apply_corrections` → rescale/normalize → demosaic (`src/demosaic.rs` select) → color matrix → `highlight::reconstruct` (inside `color::develop`) → `chroma.rs` → `whitebalance.rs` → `analyze.rs` → optional `localtone.rs` → `tone.rs:432` render. `dump_stages` today (`src/pipeline.rs:139`) only dumps scene-linear `LinearImage` after `tone::render_baseline` (transfer-function-only), and starts at `01-scene-linear` after `develop()`. Advice §"First step" requires dumps *before/after* reconstruction and before/after `compress_gamut` on the owned path, plus a clip-state map — none exist.
- **Existing invariants**: `docs/STATUS.md` regression gate, `CLAUDE.md` determinism (batch vs single, `--jobs` independence), `--highlight-reconstruction 0` / `--local-tone 0` byte-identical contracts, `REPORT_SCHEMA_VERSION` bump discipline in `src/types.rs:13`, `src/rescale.rs` invariants about not mutating `RawImage` for `src/noise.rs`. `docs/HDR_EVALUATION.md` already concluded HDR output does not address this defect (unclipped headroom <1 EV on all five blown-sky frames).
- **Chroma path coupling** (`src/analyze.rs:121`, `src/tone.rs:348`): `saturation=1.22`, `highlight_desaturation=0.16` → net highlight chroma factor 1.0248 at the shoulder, per advice §"Why the current code creates the gradient". There is a second amplifier where `highlight_norm` and chroma capping interact (`src/tone.rs:378`).

## Constraints And Non-goals

- Do not add an earlier clamp or reintroduce `clip_euclidean_norm_avg` semantics; `docs/white-blowout-advice.md:13` explicitly forbids it (owned path corrected the Rawler clip).
- Keep the regression gate from `docs/STATUS.md:1-3` intact: new opt-in features byte-identical at strength 0, `--dry-run --summary` diff ignores only `elapsed_ms`, determinism under `--jobs 8`. `src/rescale.rs` and `src/color.rs` invariants must not be broken (no `&mut RawImage` normalization, `SubBlack` semantics preserved).
- Do not enable `--local-tone` by default and do not build HDR output (PQ/gain-map) inside this plan; HDR belongs after reconstruction+gamut fixes per advice §§"Where HDR belongs" and "Recommended implementation order:8".
- `--highlight-contrast` stays a diagnostic/user preference (advice step 9), not the correction.
- Non-goal: semantic sky/face/subject detection, multi-frame HDR, luma denoising, DCP creative tables.

## Key Decisions

1. **Where the diagnostics live.** Advice says "inside the owned color path." Decision: add hooks in `src/color.rs:develop` (which calls `highlight::reconstruct` and owns the working→display matrix) to emit dumps before/after `highlight::reconstruct` and before/after `compress_gamut` in `src/tone.rs:282`. Alternative rejected: extending only `pipeline::dump_stage` (which sees `LinearImage` after develop) — it cannot see the pre-reconstruction camera-RGB buffer the advice specifies (items 1-4). Use a `--dump-stages` extension (or new `--dump-highlight-stages DIR`) that writes both the existing `LinearImage` baseline dumps and the new camera-RGB + gamut-stage PNGs, reusing `tone::render_baseline` with/without the working→display matrix consistently. Keep the "best effort, never fails the file" contract (`src/pipeline.rs:137`).

2. **Frozen ToneParams for the decisive test.** Advice caveat: `--highlight-reconstruction 0` also changes `analyze::analyze`'s view of the image and thus `ToneParams`. Decision: the ablation harness's definitive comparison renders the pre- and post-reconstruction buffers through the *same* `ToneParams` (snapshot params from one analysis pass or inject `ToneParams` directly into the dump renderer). Simple CLI ablation (`raw-autotune --highlight-reconstruction X --dry-run` over the corpus) remains the preliminary triage; the frozen-params image pair is the court of record. Alternative rejected: pure CLI ablation — confounds exposure and curve changes with reconstruction changes, precisely what the advice warns against.

3. **Raw-domain clip confidence vs demosaiced threshold.** Advice §1 mandates per-CFA-site confidence from normalized raw samples and the actual per-channel white level, preserved through hot-pixel correction, with `smoothstep(t0,t1,x)` rather than boolean `>=0.98`. Decision: compute confidence in `src/rescale.rs:206` / `src/hotpixels.rs` / `src/levels.rs` where `RawImage` white/black levels are still available, store as a companion `Vec<f32>` mask aligned with the mosaic, and propagate/demosaic it alongside camera RGB (bilinear-averaged or nearest-trusted, not interpolated through saturated sites). `t0/t1` start tied to `white_level` per channel and per-camera (A7C white 15360, ProShot white from DNG), not a global 0.98; expose as constants in `src/highlight.rs` for tests. Alternative rejected: keep demosaiced `>=0.98` — it conflates demosaic averaging, algorithm-dependent spread, and channel-dependent white levels.

4. **Chromaticity estimator.** Advice recommends log-ratios `u=log((R+eps)/(G+eps))`, `v=log((B+eps)/(G+eps))` filtered with confidence weighting and edge-aware guided filter using the frame's luminance as guide (cf. `src/chroma.rs` second stage already uses a guided filter). Decision: reuse `src/chroma.rs` guided-filter primitive if it can operate on log-ratio planes; otherwise add a small `src/guided.rs` utility shared by both. Solve `s* = argmin Σ w_i (s q_i - x_i)^2` over trusted channels only, reconstruct clipped channels toward `s* q`, constrained `x'_i >= x_i` (never contradict saturation lower bound). Fallback ladder: 1-clip → two survivors strongly constrain `s` + validate prior; 2-clip → one survivor gives intensity, spatial prior gives ratios; 3-clip → extend low-frequency color/luminance from component boundary, suppress detail. Suppress foliage→sky bleed with the edge-aware filter (advice §2). Alternative rejected: global "near-white → desaturate" — it is what the current code does and what produced the regime boundary.

5. **Continuous blend vs hard branches.** Replace `if near_white {1.0} else {strength}` (`src/highlight.rs:232`) with `(1-c_i) x_i + c_i x̂_i` per channel, where `c_i = smoothstep(t0,t1,x_i)` is per-channel clip confidence and a second confidence describes local prior reliability (fraction of trusted neighbours, distance to component boundary, connected-component size from §"Metrics worth adding"). This eliminates the 1→2-channel discontinuity that the synthetic ramp test is built to catch. Keep a single `strength` knob that scales the blend uniformly (so `0` remains byte-identical: `c_i * strength` or early return), not two different policies.

6. **Reconstruction uncertainty propagation.** Add per-pixel `u ∈ [0,1]` carried into `src/tone.rs:348` (`chroma_scale` / `highlight_desaturation` region). Apply `C_gain = C_normal * (1 - u * d_uncertain)` rather than globally raising `highlight_desaturation` (advice §"Carry reconstruction uncertainty"). Initial mapping: reliable prior → retain chroma, 1-clip weak prior → mild cap, 2-clip → stronger, 3-clip interior → strong smooth neutralization toward neutral. `u` also gates local-tone detail enhancement per advice §"Where HDR belongs" constraints (no independent R/G/B local curves, median correction ≈0, shadow lift +0.4–0.6 EV, highlight compression −0.6–0.9 EV, already close to `src/localtone.rs` bounds of +1.0/−0.75 EV).

7. **Gamut compressor.** Replace `compress_gamut`'s scale-around-anchor + clamp with OKLab chroma reduction preserving lightness & hue: display-linear RGB → `src/oklab.rs:68` → reduce chroma by binary search until `cbrt→XYZ→sRGB` is in `[0,1]` with smooth onset before the boundary. Optimise with cusp approximation only after correctness. Must run on display-linear RGB (advice colour path §144: "what actually clips is a *display* channel"), consistent with existing `working_to_display` placement (`src/pipeline.rs:432`, `src/tone.rs:289`). Verify on the five blown-sky frames that pre/post-gamut hue delta drops, and that `compress_gamut` unit tests still hold or are replaced by perceptual invariants.

8. **Naming and metrics.** Advice step 3: rename "near white" internally to "multi-channel clipped". Decision: keep sidecar field names for schema compatibility but add `fraction_1/2/3_clipped`, `reconstruction_uncertainty` histogram, `hue_discontinuity_across_clip_boundary`, `low_freq_hue_total_variation`, `reconstruction_chroma/luminance_delta`, `pre_vs_post_gamut_hue_delta`, `fully_clipped_connected_component_sizes`. Bump `REPORT_SCHEMA_VERSION` (currently 14 in `src/types.rs:35`) exactly once for the batch. Keep `near_white_fraction` in `analyze::AnalysisStats` as deprecated alias for compatibility with existing `survey.json` tooling, but drive policy from the new fractions.

9. **Synthetic test location.** New `src/synthetic.rs` (or `tests/synthetic_ramps.rs`) that constructs `Image<CameraRgb>` / `LinearImage` without files, feeds them through `highlight::reconstruct` and through `tone::render_pixel_linear` with a frozen `ToneParams`. Tests assert monotonicity, hue continuity (via `src/oklab.rs:119`), and absence of threshold discontinuities. This satisfies "more rigorous control than any vendor JPEG" and runs on CI without `raw/` data.

## Recommended Approach

Phase A — apparatus (no production rendering change) — is the whole point of this plan; phase B uses it to land the fix; phase C closes gamut + HDR comparison.

- **Phase A (this plan's deliverable):**
  1. Extend `--dump-stages` in `src/cli.rs:178` / `src/pipeline.rs:139` / `src/color.rs:derive` to emit the six advice buffers: camera RGB pre-reconstruction, clip-state map (black/red/magenta/white encoding `0/1/2/3 clipped`), camera RGB post-reconstruction, luminance + OKLab chroma/hue deltas, display-linear RGB pre-`compress_gamut`, display-linear RGB post-`compress_gamut`. All under one frozen `ToneParams`. Add `tools/grade_sky.py` replacement or `tools/dump_highlight_stages.py` + `tools/ablation.sh` that drives the table in `docs/white-blowout-advice.md:84` and writes PNGs for the decisive overlay (gradient vs clip-map).
  2. Add `src/synthetic.rs` with the five synthetic fixtures from advice §"Correctness reference" (neutral ramp, constant blue-sky ramp, blue→neutral cloud, sky|foliage edge, broad 0→1→2→3 saturation sweep). Helper to build camera-RGB ramps with known `white_balance`/`white_levels` so thresholds are not magic numbers.
  3. Add metrics in `src/metrics.rs:13` / `src/highlight.rs:119` / `src/types.rs`: per-frame `fraction_1/2/3_clipped`, uncertainty distribution, hue discontinuity, etc., and a library function to compute them on any `Image<CameraRgb>` for use by both synthetic tests and sidecar reporting.

- **Phase B (production correction, uses Phase A as gate):**
  4. Plumbing for raw-domain confidence: `src/rescale.rs` → `src/hotpixels.rs` → `src/demosaic.rs` → `src/color.rs` (mask type, threading, determinism — chunks fixed at 64 rows, not `rayon` thread count, per existing invariant).
  5. Neighbourhood-guided log-chromaticity reconstruction (`src/highlight.rs` rewrite) with continuous blending and connected-component falloff for 3-clipped interiors.
  6. Uncertainty chroma limiting in `src/tone.rs:377` (threaded through `AnalysisStats` → `ToneParams` or via a companion `ReconstructionUncertainty` image passed to `tone::render` alongside `LocalToneMap`).

- **Phase C (close the loop):**
  7. Perceptual gamut compressor in `src/tone.rs:102` using `src/oklab.rs`, with smooth onset.
  8. Controlled `--local-tone 0.35` evaluation on the same five frames plus the HDR-shaped checklists (halo around foliage/ridge, sky/ground EV separation, `docs/HDR_EVALUATION.md` headroom figure).

Each phase bumps `REPORT_SCHEMA_VERSION` in `src/types.rs` once and keeps `--highlight-reconstruction 0` / `--local-tone 0` byte-identical (existing tests in `src/highlight.rs:278` enforce all `reconstruction_never_darkens_a_channel`-class invariants; add counterparts for the new estimator).

## Work Plan

### Slice 1 — Diagnostic dumps and clip-state map (Phase A.1)

- **Owns:** `src/color.rs` (owned develop path, highlight call site), `src/rescale.rs` + `src/hotpixels.rs` (confidence source), `src/tone.rs` (pre/post gamut hooks), `src/pipeline.rs` + `src/cli.rs` (dump wiring), `src/highlight.rs` (minor: expose clip-state classifier).
- **Does:** define `ClipConfidence` / `ClipMap` type; compute pre-reconstruction camera-RGB buffer; classify 0/1/2/3 clipped with both current threshold and (behind flag) smoothstep confidence; emit PNGs: `*-highlight-pre.png`, `*-highlight-clipmap.png` (black/red/magenta/white, `docs/white-blowout-advice.md:72`), `*-highlight-post.png`, `*-highlight-delta-luma.png`, `*-highlight-delta-oklab.png`, `*-gamut-pre.png`, `*-gamut-post.png` plus a JSON sidecar with per-channel clipped counts matching `docs/HDR_EVALUATION.md:24` table. Implement frozen-`ToneParams` render for the ablation comparison (reuse `ToneLut::new` path).
- **Depends on:** nothing.
- **Parallelisable with:** Slice 2.

### Slice 2 — Synthetic ramp suite and ablation harness (Phase A.2 + A.3 harness)

- **Owns:** new `src/synthetic.rs` (or `tests/synthetic_ramps.rs`), `src/highlight.rs` tests, `src/oklab.rs` helpers, `tools/ablation.py` (or `tools/grade_sky.py` restoration — advice notes it is missing; `CHANGELOG.md` references it, `docs/white-blowout-advice.md:325` says neither `HDR_EVALUATION.md` nor `grade_sky.py` were on master at review time; now `HDR_EVALUATION.md` exists but the script still does not).
- **Does:** fixtures: neutral, constant blue-sky, blue→neutral, sky|foliage, 0→1→2→3 sweep. Each constructs `Image<CameraRgb>` with calibrated `white_balance` (e.g. A7C ~[2.219,1,1.773] per `src/highlight.rs:358`) and known chromaticity; runs `highlight::reconstruct` at `{0,0.25,0.75,1.0}`; asserts luminance monotonicity, OKLab hue continuity, no discontinuity at clip thresholds (finite-difference of hue across the 1→2 boundary < epsilon), constant-hue preservation until evidence gone, 3-clipped smoothness. Harness script runs `cargo run --release -- <raw> --dump-highlight-stages /tmp/dumps` plus the CLI ablation table (advice table, p.84) over a single `_DSC1289.ARW` and diffs the frozen-params pair.
- **Depends on:** Slice 1's `ClipMap` type only; can start with a stub classifier.

### Slice 3 — New metrics (Phase A.3, parallel with Slice 2)

- **Owns:** `src/metrics.rs`, `src/highlight.rs:119` `HighlightReport`, `src/types.rs` sidecar schema, `src/analyze.rs:330` near-white accounting.
- **Does:** replace single `near_white_fraction` policy with `fraction_{1,2,3}_clipped`, expose in `AnalysisStats` + `OutputStats` where appropriate; compute `reconstruction_uncertainty` distribution, hue discontinuity across clip-state boundaries (OKLab `hue_difference` on adjacent pixels straddling the boundary), low-freq hue total variation (blurred hue field over bright low-texture ROI), reconstruction chroma/luma delta, pre/post-gamut hue delta, fully-clipped component sizes (connected-component label, 4- or 8-connectivity, deterministic scan). Wire into `pipeline::process_job` sidecar and into `--dry-run --summary` JSON.
- **Depends on:** Slice 1 clip-state; otherwise independent.

### Slice 4 — Raw-domain confidence plumbing (Phase B.4)

- **Owns:** `src/rescale.rs:131` `NormalizedSamples::Mosaic` + `Levels`, `src/hotpixels.rs`, `src/demosaic.rs:222` green estimators, `src/color.rs:329` `ColorTransform`.
- **Does:** build per-CFA-site `c_i = smoothstep(t0,t1,x_i)` from `NormalizedSamples` (raw code space near `white_level`, per-channel), preserve through `hotpixels` suppression, demosaic/propagate companion mask (not via `CFAColor`-averaged demosaic that collapses saturated sites), expose to `highlight::reconstruct` as `&[f32]` alongside `Image<CameraRgb>`. Keep `CLIP_THRESHOLD` as deprecated fallback when confidence not yet plumbed; tests assert mask determinism under `--jobs 8` and `normalize_does_not_mutate_the_raw_image` still holds.
- **Depends on:** Slice 1 design locked.

### Slice 5 — Continuous log-chromaticity reconstruction (Phase B.5)

- **Owns:** `src/highlight.rs` (major), optionally new `src/guided.rs`.
- **Does:** implement `u=log(R+eps/G+eps)`, `v=log(B+eps/G+eps)` confidence-weighted edge-aware filtered `q`, solve `s*` over trusted channels, synthesize clipped channels toward `s* q` with `x'_i >= x_i` lower bound, continuous per-channel blend `(1-c_i)x_i + c_i x̂_i`, second confidence for prior reliability with smooth neutralization toward neutral deep inside 3-clipped components. Collapse the two `near_white` special cases in current `highlight.rs:194` into the one continuous path. Preserve `strength` as uniform scale, `0` = skip module.
- **Depends on:** Slice 4; validated by Slice 2 synthetic suite (must turn the synthetic ramp from fail to pass).

### Slice 6 — Uncertainty-gated chroma limiting (Phase B.6)

- **Owns:** `src/tone.rs:348` chroma/vibrance block, `src/types.rs:169` `ToneParams`, `src/highlight.rs` uncertainty image.
- **Does:** propagate per-pixel `u` into `render_pixel_linear` (new `Option<&UncertaintyMap>` param akin to `LocalToneMap`), apply `C_gain = C_normal * (1 - u * d_uncertain)` with the 4-tier interpolation described in advice (reliable → retain, 1-clip weak → mild cap, 2-clip → stronger, 3-clip → strong). Do not raise global `highlight_desaturation`.
- **Depends on:** Slice 5; small enough to ship together if desired — keep separate for reviewability.

### Slice 7 — Perceptual gamut compressor (Phase C.7)

- **Owns:** `src/tone.rs:102` `compress_gamut`, `src/oklab.rs`.
- **Does:** OKLab path: display-linear → OKLab → preserve L,h, binary-search chroma to in-gamut sRGB with smooth onset (no hard channel clamp). Remove the current scale-around-`mapped_luminance` + clamp. Add cusp-approximation optimisation only after correctness. Tests on five blown-sky frames: pre/post-gamut hue delta bound; `crushed_fraction` regression check from `docs/STATUS.md:378` still 108/108 tied.
- **Depends on:** Slice 5-6 (upstream hue must be correct first; advice says gamut fix "will not recover a wrong hue created upstream").

### Slice 8 — Local-tone sky/ground HDR evaluation (Phase C.8, no code or gated experiment)

- **Owns:** `src/localtone.rs`, `src/tone.rs` exposure-gain path, `tools/` evaluation.
- **Does:** sweep existing Gaussian-surround operator at `--local-tone 0.35` (advice) with the new uncertainty gating; measure sky-band vs ground-band EV separation, halo around foliage/ridge, `HIGHLIGHT_NORM` interaction, and `docs/HDR_EVALUATION.md` unclipped-headroom figure. If warranted, implement the guided-filter log-luminance `B=guided(L), D=L-B, ΔEV = B_compressed + kD - L` design from advice §"Where HDR belongs" with median-correction≈0, shadow +0.4-0.6 EV, highlight −0.6-0.9 EV, single scalar RGB gain. Gate: do not ship default-on until halo test passes on leaf/branch/ridge crops.
- **Depends on:** Slice 7.

## Validation Plan

- **Slice 1:** run `raw-autotune raw/raw_3rd_batch/_DSC1289.ARW --output /tmp/dumps --dump-highlight-stages /tmp/dumps/stages --overwrite` and verify 7 PNGs written, `clipmap` pixels count matches `HighlightReport {clipped_pixels, near_white_pixels, fully_clipped_pixels}` (advice numbers: `_DSC1289` 958,928 / 344,861 / 0, `_DSC1290` 801,061 / 682,786 / 5,828). Visual check: cyan→violet boundary aligns with red→magenta boundary. Command evidence: `ls /tmp/dumps/stages/*.{png,json}` + `cargo test --release highlight`.
- **Slice 2:** `cargo test --release synthetic` — expect FAIL on current code (discontinuity at 1→2 threshold), PASS after Slice 5. Ablation harness: `tools/ablation.py --raw raw/raw_3rd_batch/_DSC1289.ARW --out /tmp/ablation` reproduces the advice table (recon 0 / 0.25/0.75/1.0 / rec2020 / highlight-contrast 1.2 / recon off + local-tone 0.35) with frozen params and reports byte differences.
- **Slice 3:** `cargo test --release metrics` + manual `raw-autotune raw/raw_3rd_batch --dry-run --summary /tmp/survey.json && jq '.files[] | {input, c1: .analysis.fraction_1_clipped, c2: .analysis.fraction_2_clipped, c3: .analysis.fraction_3_clipped}' /tmp/survey.json` matches the `docs/HDR_EVALUATION.md:24` percentages. Sidecar schema version bump verified: `jq .schema_version /tmp/survey.json` == new constant.
- **Slice 4:** determinism gate from `docs/STATUS.md:1`: two runs with `--jobs 8` byte-identical stage dumps despite `HashMap` IFD ordering trap (`src/preview.rs` note). `cargo test --release rescale -- --nocapture` includes `parallel_normalization_equals_the_sequential_reference` and `normalize_does_not_mutate_the_raw_image`.
- **Slices 5-6:** synthetic suite flips to PASS; `raw/raw_3rd_batch` before/after `--dry-run --summary` with `--highlight-reconstruction 0` byte-identical (ignore `elapsed_ms`), with default strength `fraction_1/2/3_clipped` distribution unchanged but hue discontinuity metric → ~0, low-freq hue TV ↓, green deficit on `_DSC1283` near-white cohort 37.5%→~0% as in `CHANGELOG.md` table.
- **Slice 7:** `cargo test --release oklab tone` plus corpus check: `cargo test --release -- --test-threads=1` then `raw-autotune raw --dry-run --summary /tmp/after.json` and `tools/compare_summaries.py /tmp/before.json /tmp/after.json` — gamut pre/post hue delta small, `crushed_fraction` tie holds (108/108 per `docs/STATUS.md:378`).
- **All slices:** `cargo test --release` (currently ~83 tests) and `cargo clippy --lib --bins --release` per `CLAUDE.md`. No `cargo fmt` or ad-hoc linter substitutes. OOM budget respected: `tools/` harnesses need no `--jobs` override (per `src/localtone.rs` note, local-tone memory peaks below chroma-denoise which `--jobs auto` already budgets).

## Risks / Rollback

- **Risk:** raw-domain confidence plumbing touches `rescale→hotpixels→demosaic` threading and determinism. **Mitigation:** keep the old demosaiced `CLIP_THRESHOLD` path behind `--highlight-reconstruction 0` / flag, instrument `RescaleReport::notes` like the existing `(b)-(e)` notes; rollback is reverting the call site in `src/color.rs` to the previous `highlight::reconstruct(&mut image, wb, strength)` without the mask param — one hunk.
- **Risk:** guided-filter chromaticity estimator smears foliage green into sky if edge-awareness too weak (advice §2 warns). **Mitigation:** synthetic sky|foliage fixture catches it; keep radius/guide weights conservative, reuse `src/chroma.rs`'s existing 128-px guided filter which already handles this tradeoff. Rollback: cap `d_uncertain` → 0 retains new blend but restores sky hue from the mild-cap path.
- **Risk:** OKLab binary-search gamut compressor costs per-pixel `cbrt` + matrix multiply. **Mitigation:** per-frame is bounded by pixel count, not corpus size; LUT design in `src/tone.rs:194` shows ~40 KiB tables make this class of cost negligible — but delay cusp optimisation until correctness proven, and keep old `compress_gamut` behind `cfg(test)` for comparison.
- **Risk:** metric churn breaks downstream `tools/contact-sheet.py` and `CHANGELOG.md` corpus numbers. **Mitigation:** single `REPORT_SCHEMA_VERSION` bump (currently `14` in `src/types.rs:35`) for the whole apparatus; keep deprecated `near_white_fraction` field populated for one release; update `tools/contact-sheet.py` and `docs/STATUS.md` tables atomically.
- **Risk:** `--highlight-reconstruction 0` byte-identical contract violated by new plumbing even when disabled. **Mitigation:** gate is `cargo test --release` + before/after `--dry-run --summary` diff with `strength=0`; CI fails if any pixel or `ToneParams` moves.

## Open Questions

- None blocking. Assumptions: `t0/t1` for `smoothstep` start near per-channel `white_level` (A7C 15360, ProShot DNG varies) with values to be tuned against the synthetic ramp, not fixed at `0.98`; `eps` in `log((R+eps)/(G+eps))` ~1e-4–1e-5 scene-linear; guided-filter radius for `q` likely 8–32 px (distinct from `src/chroma.rs`'s 128 px veil filter). All to be settled by the apparatus, not by this plan.
- Confirm: synthetic suite lives in `src/synthetic.rs` unit tests (file-free, CI-simple) vs `tests/` integration — prefer the former for `cargo test --release <name>` ergonomics. Either is acceptable.

---
*Plan file: `.agents/plans/2026-08-07-white-blowout-testing-apparatus.md` — no code changed.*
