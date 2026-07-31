# Candidate-render planning reports (0.1.15 session, 2026-07-29)

Three planning agents were run for `docs/PLAN.md` §5.2 (candidate render +
sanity check) while 0.1.15 was being implemented; the session ended before they
were reported. Recovered verbatim from the agent transcripts. The adversarial
review (third report) recommends rejecting §5.2 as written; §4b item 3 in the
plan records the summary verdict.


---

# Report 1

## 1. Where it goes in `process_job_inner`

Insert one block **after** `parameters.saturation *= options.saturation_scale` (line ~344) and **before** the `reference.compare(...)` block (line ~350) and the dry-run early return. Reasons:

- Everything upstream (BaselineExposure, orientation, chroma denoise, local WB) already happened, so the proxy is built from exactly the buffer the final render sees.
- The reference block predicts subject EV from `parameters`; it must see the *winner*, not the base.
- Placing it before the `dry_run` return means `--dry-run --summary` — the project's main analysis tool — reports the decision for a whole corpus without writing images. That is worth more than saving the proxy render cost on dry runs.

The full-res `tone::render(&linear, &parameters, local_tone.as_ref())` call is unchanged; `parameters` is simply rebound to the winner's `ToneParams`. Local tone is built *after* selection today; keep it there and give the proxy its own `localtone::build` over the proxy image when `options.local_tone > 0.0` (it is off by default, so normally `None`).

## 2. Candidate generation — new module `src/candidates.rs`

Do not mutate `ToneParams` fields directly: `shadow_power`/`highlight_power`/`black_output_ev` are coupled to the input range and to the noise-floor guard inside `analyze::derive_params`. Instead make `derive_params` re-derivable:

```rust
// analyze.rs — identity default must reproduce today's arithmetic exactly
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct CandidateOffset {
    pub exposure_ev: f32,        // 0.0
    pub black_margin_ev: f32,    // 0.0, added to the preset safety margin, shadow side
    pub white_margin_ev: f32,    // 0.0, highlight side
    pub contrast_scale: f32,     // 1.0
    pub highlight_norm: Option<f32>,
}
pub(crate) fn derive_params_with(
    stats: &AnalysisStats, preset: Preset, exposure_ev: f32,
    noise_floor_ev: Option<f32>, offset: &CandidateOffset) -> ToneParams;
```

Each offset field is applied behind an `if offset.x != 0.0` branch so the identity path is instruction-for-instruction the existing code (the byte-identical-at-strength-0 rule in CLAUDE.md).

Axes, one per reference-free metric in PLAN §4b — clipped, crushed, entropy:

| label | offset | targets |
|---|---|---|
| `base` | identity | always index 0 |
| `protect_highlights` | `white_margin_ev: +0.5`, `highlight_norm: Some(1.0)` | near-white/clipped |
| `protect_shadows` | `black_margin_ev: +0.5` | crushed |
| `open_shadows` | `black_margin_ev: -0.4` | entropy on low-key frames |
| `expand_range` | `contrast_scale: 1.08` | entropy on flat frames |

**Gate, don't grid.** `propose(&stats, &base, preset, limit) -> Vec<Candidate>` emits `base` plus only those variants whose failure mode the analysis says is live: `protect_highlights` only when `stats.near_white_fraction` exceeds a threshold, `protect_shadows` only when `near_black_fraction` does or a noise floor bound the black point, `expand_range` only for `TonalClass::Flat`, `open_shadows` only for `LowKey`/`HighDynamicRange`. Typical frames yield 1–2 candidates and cost nothing; the hard frames get 3–5. This matches how `chroma::apply` earns its keep (inert on 309/368 files). Exposure itself is deliberately *not* an axis when the preview oracle fired — the oracle already owns that decision, and perturbing it would re-open the thing §4b says to borrow.

## 3. Proxy: decimate by striding, not by averaging

```rust
pub struct Proxy { pub image: LinearImage, pub stride: usize }
pub fn build_proxy(image: &LinearImage, target_pixels: usize) -> Proxy; // stride = ceil(sqrt(total/target))
```

Take **point samples on an integer stride in both axes** (the same estimator `analyze` already uses), not a box average. `OutputStats::measure` already computes 7 of its 8 fields from a strided sample of ~250k pixels, so a strided proxy render of ~250–500k pixels yields statistically equivalent `colourfulness`, `clipped_fraction`, `near_white_fraction`, `crushed_fraction`, `mean_level`, `mean_saturation`, `luminance_entropy`. Box averaging would *not*: it destroys isolated clipped speculars and would systematically under-report the one metric that means "information definitively lost".

**The one metric the proxy invalidates is `average_gradient`**, because decimated neighbours are not neighbours. Flag it to the scoring agent as unusable, or optionally also render a contiguous centre crop (`min(1024, w) x min(1024, h)`) for it. Memory: 500k px x 3 x u16 = 3 MB per candidate; render, measure, drop, one at a time — the peak stays under 10 MB against a 50 MP full-res render's 300 MB. Time: 5 x 500k pixels is ~1% of one full-res render.

## 4. Determinism

- `Vec<Candidate>` in fixed generation order; **no `HashMap` anywhere**.
- Selection is a serial fold with a strict `>` on `f64` scores, so ties go to the earlier index and `base` (index 0) wins any tie. Never `partial_cmp().unwrap()`; treat non-finite scores as losing.
- `build_proxy` must write via indexed `par_chunks_exact_mut` over output rows — **no rayon float reduction** (`par_iter().sum()` associates by work-stealing and is not reproducible).
- Stride depends only on `width`/`height`/a constant. No dependence on `--jobs`, batch composition, or wall time.
- The winner's `ToneParams` is used *verbatim*; do not re-derive after selection (a re-derive could differ in the last ulp).
- When `propose` returns one candidate, skip the proxy entirely and take the existing path — that is what makes the feature-off output byte-identical.
- Tests: `repeated_selection_is_identical`, `identity_offset_reproduces_derive_params` (exact `==` on every field), `ties_prefer_the_base`.

## 5. Reporting

Bump `REPORT_SCHEMA_VERSION` 5 → **6**. Add to `Sidecar`, `ProcessReport`, `SummaryEntry` (all `#[serde(skip_serializing_if = "Option::is_none")]`):

```rust
pub struct CandidateSelection {
    pub proxy_width: usize, pub proxy_height: usize, pub proxy_stride: usize,
    pub chosen: usize, pub chosen_label: String,
    pub margin: f32,                       // winner score minus runner-up
    pub candidates: Vec<CandidateRecord>,  // generation order
}
pub struct CandidateRecord {
    pub label: String, pub offset: CandidateOffset,
    pub params: ToneParams, pub measured: OutputStats, pub score: f32,
}
```

In `BatchSummary` add `candidate_margin: Option<Distribution>` and `candidate_choice_counts: BTreeMap<String, usize>` (built in `main.rs` alongside `tonal_class_counts`). That answers the corpus question directly: how often does `base` lose, to what, and by how much — and a near-zero margin distribution is the signal that the whole feature is noise.

## 6. CLI

One flag, in the `--chroma-denoise` family: `--candidates <N>` (`default_value_t = 0`, validated `0..=5`), where 0 is off and byte-identical. Ship opt-in, measure on the paired corpus, then flip the default to automatic gating and keep the flag only as a sweep knob — exactly the path `--local-tone` and `--chroma-denoise` took. No per-file flag, ever, and no user-facing scoring weights.

**Risks:** proxy/full-res disagreement on `clipped_fraction` for sub-stride speculars (mitigate: strided sampling, and pin a test that proxy and full-res stats agree within tolerance on a corpus frame); candidate churn producing visible inconsistency between adjacent burst frames (§5.3's sequence consistency will have to see this block); and metric-chasing renders that score well and look worse — mitigate by requiring the margin distribution plus a contact-sheet pass before the default flips.

### Critical Files for Implementation
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/pipeline.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/analyze.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/types.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/metrics.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/main.rs (new `src/candidates.rs`, registered in `src/lib.rs`)

---

# Report 2

## The shape: incumbent-defends, gates first, saturating score, margin

Not a weighted sum over all eight metrics — that is exactly the failure `--local-tone 1.0` demonstrates. Structure it as **veto gates → bounded score on one-directional axes only → margin against the incumbent → index tie-break**. Taste axes appear *only* as gates; information axes appear *only* as saturating gains. That makes "drift toward a flat, over-saturated look" unrepresentable rather than merely down-weighted.

Candidate 0 is always the controller's current `ToneParams` (the incumbent), rendered at the same proxy size so `sampled_pixels` matches. All comparisons are relative to its stats `b`; the incumbent's own score is 0 by definition.

## 1 + 3. Gates (hard) versus score (soft)

Reject candidate `s` outright if any holds:

- `s.clipped_fraction > b.clipped_fraction + 0.0010` **and** `> 0.0020` (our median is 0.0000%; never trade away the axis we already win 86/98).
- `s.crushed_fraction > b.crushed_fraction + 0.0020` **and** `> 0.0050`.
- `s.near_white_fraction > b.near_white_fraction + 0.020`. Gate, never a score term: STATUS records it now counts saturated-but-unblown pixels.
- `s.mean_level` outside `b.mean_level ± 6.0` (0–255; ≈ ±0.2 EV). Exposure was settled by the oracle at 0.03 EV MAE; candidates may not relitigate it.
- `s.mean_saturation` outside `[0.85, 1.15] × b.mean_saturation`, upper bound tightened to `1.05` when `trust < 1` (below) because chroma noise inflates it.
- `|s.colourfulness − b.colourfulness| > 0.15 × b.colourfulness`. Gate only — `auto` sits at 1.017 of the camera and must stay there.
- `s.luminance_entropy < b.luminance_entropy − 0.10` bits. This is the anti-flattening gate, sized from measurement: local-tone 0.5 cost 0.21 bits and 1.0 cost 0.45, so both are vetoed with margin.
- any non-finite value, or `sampled_pixels != b.sampled_pixels`.

Score for survivors, with `sat(x) = x.clamp(-1.0, 1.0)`:

```
g_c = sat((b.clipped  - s.clipped)  / 0.0020)
g_k = sat((b.crushed  - s.crushed)  / 0.0020)
g_e = sat((dead(s.entropy - b.entropy, 0.02)) / 0.15)
g_g = sat((s.average_gradient / b.average_gradient - 1.0) / 0.10)
score = 1.00*g_c + 0.60*trust*g_k + 0.80*g_e + 0.70*trust*g_g
```

`dead(x, d)` zeroes |x| < d (measurement noise). Every term saturates at ±1: recovering 0.2 pp of clipping earns full credit and recovering 5 pp earns no more, and **entropy is capped at +0.15 bits**, so histogram-equalisation buys nothing beyond the first sliver. That cap is the structural answer to "maximising entropy is wrong".

`trust = 1 − chroma::automatic_strength(snr10_ev)` (1 at `snr10_ev ≤ -3.5`, 0 at `≥ -0.5`). `average_gradient` cannot distinguish texture from grain, and lifting shadows on a noisy frame only reveals noise, so both terms fade out on exactly the frames the fitted noise model calls dirty.

## 2. Preventing drift from the tuned default

The incumbent scores 0 and a challenger must reach **`score ≥ MARGIN = 0.25`** to displace it. Sub-margin improvements are indistinguishable from measurement wobble, and the asymmetry is deliberate: the default is the artefact 98 pairs were used to tune, a challenger is a hypothesis. Combined with the gates, a challenger can only win by keeping information the default threw away, while staying inside the default's tuned exposure and colour band.

## 4. Scene class and noise

- **Night / `LowKey`** (or `stats.target_median_ev < -1.5`): crushed gate tightens to `b.crushed + 0.0005`; `mean_level` upper gate tightens to `+3.0` (criterion "no night rendered as day"); `trust` is usually low, so gradient drops out on its own.
- **`HighKey`**: `near_white` gate tightens to `+0.010` and the `mean_level` *lower* gate to `−3.0`, so a candidate cannot "improve" the histogram by darkening a legitimately bright scene — the same failure `ORACLE_TARGET_CEILING_EV` is suspected of.
- **`HighDynamicRange`** (`measured_dynamic_range_ev ≥ 11`): the class candidates exist for. Widen `mean_level` to `±10`, set `w_c = 1.30`, `w_e = 1.00`, `MARGIN = 0.20`.
- **`Flat`** (`DR ≤ 4.25`): `MARGIN = 0.35`; there is little to win and much to over-stretch.

## 5. Determinism

Candidates come from a fixed `const` array, index is identity. Render into an indexed `Vec` (`par_iter().map().collect()`), never a channel or `HashMap`. Quantise before comparing: `key = (score * 4096.0).round() as i64`, compare `i64`. Select by `max_by_key` semantics implemented as a fold that replaces only on strict `>` — first index wins ties, and index 0 is the incumbent, so an exact tie always returns the default. No `partial_cmp` anywhere; NaN is gated out before scoring. Record chosen index and every candidate's score in the sidecar.

## 6. Acceptance test on the 98 pairs

1. **Scorecard non-regression** (JPEGs used only for grading): rerun the four-row table. Each row's loss count may not rise by more than 1, and total wins must strictly exceed 163.
2. **Negative control, the strongest test available**: add the known-bad `--local-tone 1.0` render as a candidate. The rule must veto or not-select it on ≥95/98. We already know the answer, so this is falsifiable today.
3. **Incumbent retention ≥ 70%.** A rule that overrides the default on most frames has replaced the default, not selected among candidates.
4. **Harm bound on taste axes**: on every frame where a challenger won, saturation ratio and colourfulness ratio to the camera must not move away from 1.0 by >0.05, nor subject EV by >0.10 EV.
5. **Hold-out**: fit thresholds on Sony+ProShot (58), verify unchanged on Samsung (40).
6. **Determinism**: `--jobs 1` vs `--jobs 8` byte-identical, identical chosen indices, two consecutive runs identical.
7. **Blind visual pass** over the ~20–30 challenger-won frames, worst-first via `tools/contact-sheet.py`: zero ruined, none graded worse.

### Critical Files for Implementation
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/metrics.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/analyze.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/types.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/chroma.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/pipeline.rs

---

# Report 3

## Verdict: reject §5.2 as written. It is the only item in the plan with no measurement behind it, and the repo has already run the experiment and lost.

### 1. The premise is falsified by the project's own history

PLAN §4b asserts "clipped, crushed, entropy and gradient are one-directional: information kept is better, and no opinion is required." Two of those four are not one-directional, and the repo proves it.

`luminance_entropy` is a 256-bin histogram of the *rendered* 8-bit luma (`src/metrics.rs:128,156`). Any monotone contrast stretch spreads 16-bit values across more bins and raises it while adding nothing; its maximiser is histogram equalisation. `average_gradient` is a strided adjacent-pixel difference (`metrics.rs:179-191`) — contrast × noise. Its maximiser is an unfiltered, over-contrasted frame.

The decisive evidence is the `--local-tone` sweep in STATUS.md: gradient rose 3.71 → 3.84 → 3.93 while entropy fell 7.5204 → 7.3143 → 7.0716, and the images got visibly worse. **That was a 3-candidate selection problem. The metrics disagreed with each other and a human broke the tie by looking.** §5.2 proposes automating a decision this corpus has already demonstrated the metrics cannot make.

Worse, the tonal-detail row of the scorecard is statistical noise: median ours **7.5204** vs camera **7.5203**, scored W46/T9/L43. A 10⁻⁴-bit difference on a 250 000-sample histogram is a coin flip. `clipped_fraction` has a sampling quantum of 4×10⁻⁶ and a corpus median of 0.0000% with p90 0.004% — one to ten sampled pixels. Three of four axes have no headroom or no resolution; ranking candidates on them is selecting on quantisation noise.

### 2. Goodhart, with the signs already documented backwards

The three scene classes where the controller is weakest are the three where `OutputStats` has the **wrong sign**:

- **Night.** "Renders night as day" scores *better* on every axis: higher `mean_level`, higher entropy (fuller histogram), higher gradient (lifted noise), lower `crushed_fraction`. The thing that fixed night was `MAX_ORACLE_DEVIATION_EV` — a prior, refusing the metric, which reported a 3.17 EV "error" on the render that was correct.
- **Extreme ISO.** STATUS: "Chroma noise reads as saturation"; the magenta veil measures 1.40× the camera. A scorer using `mean_saturation` or `colourfulness` **prefers the veiled candidate**.
- **Shadows.** `average_gradient` is maximised by lowering the black point into the noise, which directly reverses the validated SNR=1 floor in `derive_params` (binds on 12-15% of frames).

The metrics that would exclude these are `near_white_fraction` — documented as meaning "has a saturated sky", not "is blown" (52 of 271 frames >10%, nothing blown) — and `mean_saturation`, documented as unable to distinguish speckle from colour. **Both proposed guard rails are the two statistics the repo has already retired as unreliable.**

Note also the two metric families pull in opposite degenerate directions (entropy/gradient → harsh and noisy; clipped/crushed → flat and grey). Any weighting between them is a hidden aesthetic constant, fitted by hand, uninspectable.

### 3. No validation set, and a closed loop

Nowhere in the repo is there a single identified frame where `derive_params` chose wrong and a different `ToneParams` would have been better. The evidence base is zero frames. Every other change here landed with a number (`highlight_norm` 1.00 sweep, saturation 1.20 → 0.981 on a second sensor, `MIN_PREVIEW_PIXELS` 250 000).

And the acceptance test *is* the objective function. The win/tie/lose scorecard is computed from the same `OutputStats` the search would maximise, on the same 98 pairs every constant was already fitted on — one photographer, one body for 313 of 368 files, three sessions in one place. The scorecard would go W86 → W98 and mean nothing. §5.2 is unfalsifiable by construction.

### 4. Cost, and the proxy is unsound

`LinearImage` is 12 B/px, `Rgb16Image` 6 B/px: 600 MB + 300 MB on a 50 MP Expert RAW. `--jobs 8` already OOMs at 31 GiB. Holding N renders to compare adds ~600 MB per worker at N=3 and moves the OOM point to `--jobs 3`. `measure` is two full passes over a 300 MB buffer plus a strided gradient pass — memory-bandwidth bound, and it runs N times.

The reduced-resolution proxy fails on exactly the metrics proposed. Clipping fractions are not scale-invariant — downsampling averages a clipped pixel with its neighbour and they fall monotonically; at proxy scale every candidate reads 0.000% and the axis dies. `average_gradient` is per-adjacent-pixel and rises with downsampling while noise disappears, so proxy ranking ≠ full-res ranking. `chroma::MAX_RADIUS` is **6 pixels**, so a proxy is a physically different filter. The only scale-robust metric is entropy — the one with no discriminating power.

### 5. Bursts

An argmax over a candidate ladder is discontinuous by construction. Two frames 200 ms apart that straddle a boundary render 0.3-0.5 EV apart — a visible step between adjacent library thumbnails, worse than either candidate applied consistently. §5.3 (burst consistency) is scheduled *after* §5.2, so 0.3 ships the instability then tries to smooth a discontinuous function it cannot interpolate without re-rendering.

### 6. Opportunity cost

Candidate search **cannot touch the largest measured gap**: no `ToneParams` field sharpens, and local detail is lost on 64/98 at 3.71 vs 4.59. It has no headroom on highlights (W86, 0.0000% median) or shadows (T68). Sharpening and multi-scale chroma are one-parameter fixes with a measured deficit, a known sign, and a known implementation. EXIF/ICC — "not polish; it is the product" — is still undone.

## What to build instead

**Drop §5.2 as a per-image quality search.** Reorder 0.3 to: output sharpening (promote from §4 item 6), edge-aware/multi-scale chroma, EXIF/ICC. Spend the desktop budget on *more expensive operators with a monotone quality direction* — guided-filter chroma, iterative highlight reconstruction, a better demosaic — not on more guesses scored by statistics that have been wrong three documented times.

**The minimal surviving version is a guard, not a selector.** Render once. If the render trips a hard, one-sided, physically unambiguous bound — `clipped_fraction` above the camera's, or above ~0.5% absolute (worst corpus frame: 0.923%); or `crushed_fraction` risen against a no-oracle baseline — re-render *once* with a bounded signed correction in the single direction that fixes it, and accept the second render only if it strictly improves that axis without worsening the other. Two renders worst case, on the ~1-5% of frames that trip, sequential so peak memory is unchanged, full resolution, no proxy, no weighting, no entropy, no gradient, no saturation, no colourfulness, no tie-breaking. That serves usable-criterion 3 ("no frame is ruined"), which is a floor, not a search for a maximum — and a floor is the only thing these metrics are strong enough to enforce.

**Precondition on any selection landing:** hold out a source. Fit the rule on ProShot + Expert RAW, validate on the 42 Sony pairs untouched. That is the repo's own strongest validation precedent (0.1.12's constant → 0.981 on Sony) and §5.2 currently has nothing like it.

### Critical Files for Implementation
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/metrics.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/analyze.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/pipeline.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/src/tone.rs
- /mnt/Samsung980_1TB/Rust-projects/raw-autotune/docs/PLAN.md
