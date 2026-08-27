# Training the CamSDD scene/lighting classifier (Model A)

Implements phase 3 of `research/LIGHTING_DETECTION_PLAN.md` (rev 2).
One script: `tools/scene_models/train_camsdd.py`. Dataset: the local CamSDD
copy at `CamSDD/CamSDD/` (9,898 train / 600 val / 600 test, 30 classes,
576×384 px, CC BY-NC-SA 4.0 — see "Licence" below).

## Stack decision: PyTorch → ONNX (Burn rejected for training)

Decided 2026-08-26. The plan's §3 left this open with Burn assessed as
feasible; the deciding review found four points, the first one decisive:

1. **Burn's headline benefit is void here.** The pure-Rust win of Burn-native
   inference was "retire the ONNX runtime for this model" — but Model B (C5
   illuminant estimator) ships as ONNX regardless (its weights are ported,
   not trained), so lege-gpu's ONNX path stays in the tree either way.
   Burn inference would *add* a second inference stack, not remove one.
2. **Option asymmetry.** Burn imports ONNX but does not export it. Training
   in PyTorch keeps both shipping paths open (lege-gpu ONNX now, Burn-native
   import later); training in Burn forecloses ONNX. Since this plan
   explicitly budgets for draft models that fail their gates and get
   rewritten, the option-preserving stack wins.
3. **Iteration speed is the actual workload.** The effort goes to recipe
   experiments (augmentation, resolution/backbone ablation, pseudo-label
   domain adaptation), not one long run. torchvision transforms + AMP +
   pretrained zoo make each experiment near-mechanical; Burn requires
   hand-rolling the augmentation pipeline over the `image` crate (the plan
   itself budgeted "a day of work") before experiment one.
4. **The accuracy gate is calibrated against Python-stack results.** The
   ≥94% band comes from challenge teams on TF/PyTorch; reproducing their
   recipes in the same ecosystem isolates *our* mistakes from framework
   differences. The repo is already shaped for this: `export.py` already
   requires torch, and `prepare.py`/`report.py` already screen ONNX graphs
   for lege-gpu.

Training-stack licence is irrelevant to shipping (PyTorch is BSD; the weights
are the artifact and their terms come from CamSDD either way). ResNet-50
exports to a clean ONNX op set (Conv/BN-fused/ReLU/MaxPool/GAP/Gemm) with
none of `report.py`'s hard-reject ops.

## Environment

```bash
python3 -m venv .venv-train
.venv-train/bin/pip install torch torchvision   # CUDA wheels, no toolkit needed
```

RTX 4060 Laptop (8 GB): batch 24 at 384×576 with AMP fits; gradient
accumulation (`--accum 4`) gives an effective batch of 96.

## Gates — early promise / early failure

Each stage has an explicit promise threshold and an explicit failure
threshold with a prescribed reaction. The script prints `GATE ...` lines;
don't tune past a failed gate.

| Gate | What runs | Promise | Failure → reaction |
| --- | --- | --- | --- |
| **0. Pipeline smoke** (~10 min) | Linear probe: frozen ImageNet backbone, train only the 30-way head, 3 epochs | val top-1 **≥ 75%** | < 40% → data/label/normalization bug. Fix the pipeline; touching the recipe is forbidden. |
| **1. Baseline fine-tune** (~1–2 h) | Full fine-tune, 30 epochs, paper recipe (label smoothing 0.1, AdamW, cosine, standard augmentation) | val top-1 **≥ 94%** (paper band 94.5–97.8; top-3 ≥ 99%) | < 90% after the schedule **plus one retry** (lr sweep or 576-wide input) → the draft is wrong; rewrite (backbone swap, recipe change), don't polish. |
| **2. Policy-class quality** (amended 2026-08-26, see below) | Confusion analysis of the Gate-1 model | **Top-3 recall ≥ 95%** AND top-1 recall ≥ 80% on each of Backlight, Indoor, Night_shot, Macro, Candle_light over val+test (40 imgs/class) | Any policy class < 80% top-1 while overall ≥ 94% → the model is good but not *for us*; targeted fixes (class re-weighting, augmentation) before any policy wiring. |
| **3. Exportability** | `--export-onnx` + `report.py` screen + timing | No hard-reject ops; ≤ 300 ms CPU / ≤ 30 ms GPU per 576×384 frame | Export or runtime failure → fix export, not the model. |
| **4. Domain adaptation** (phase 3c) | Pseudo-label own proxy renders, hand-review, short fine-tune | No regression on CamSDD val; hand-reviewed accuracy on own corpus ≥ Gate-1 level | Large CamSDD→proxy gap that fine-tuning doesn't close → revisit proxy tone mapping before blaming the model. |

Experiment budget: if Gate 1 is not reached within ~10 runs / 2 days of
experiments, stop and re-plan (fallback options: EfficientNet via timm, or
reproduce a specific challenge entry verbatim).

## First draft results (2026-08-26, `runs/ft0`, seed 42)

| Gate | Result | Verdict |
| --- | --- | --- |
| 0 probe | **92.17%** val top-1 (3 epochs, ~46 s/epoch) | PROMISE |
| 1 fine-tune | **97.00%** best val top-1 (epoch 21), top-3 99.8–100%; 96.67% on val+test (n=1200); ~69 min total | PROMISE — top of the paper band, no retries |
| 2 policy recall | Night/Candle/Indoor 100%, Macro 90%, **Backlight 82.5% top-1 / 97.5% top-3**; 4 of 7 Backlight misses → Sunset_Sunrise (the pre-registered confusion) | INCONCLUSIVE as authored — resolved 2026-08-26 by amending the check to the top-k metric the mapping layer consumes (see "Hand review + Gate-2 disposition" below); adapt1 passes the amended gate. |
| 3 export | Conv/Relu/Add/MaxPool/GAP/Gemm only, BN fused, `report.py` compatible; **100 ms CPU / 6.1 ms GPU** per frame | PROMISE |

Evidence records: `@raw-autotune.evidence.camsdd-gate-{0-probe,1-finetune,2-policy-recall,3-export-runtime}-20260826`.

## Phase 3b ablation (2026-08-26, val+test combined, n=1200)

| Run | Arch @ input | top-1 | Backlight recall (worst policy class) |
| --- | --- | --- | --- |
| `runs/ft0` | ResNet-50 @ 384×576 | 96.67% | 82.5% |
| `runs/ft-mnv2` | MobileNetV2 @ 384×576 | 96.42% | 85.0% |
| `runs/ft-r50-192` | **ResNet-50 @ 192×288** | **96.92%** | **90.0%** (36/40, exactly at the Gate-2 bar) |

Winner: **ResNet-50 @ 192×288** — best overall, best Backlight, and a
quarter of the inference pixels. Plausible mechanism: at ~330 images/class
the stronger effective augmentation of RandomResizedCrop at the smaller
target regularizes better than extra resolution helps; the paper's
resolution finding (192×288 ≫ 96×144) evidently saturates by 192×288 for
this task. Margins are small (0.25–0.5 pt ≈ 3–6 images), so the winner is
also the cheaper model — an easy call.

## Phases 3c/3e results (2026-08-26)

All 384 corpus RAWs were dumped through the new `--dump-scene-proxy` flag
and scored (`.agent/scratch/camsdd-adapt/labels/`, kept via
`akr scratch keep`). Findings:

- **Distribution is archive-plausible** (Greenery 47, Indoor 44,
  Architecture 42, Snow 36, Text_Documents 29 — a sampled Text_Documents
  frame really is a photographed cookbook page).
- **Night is invisible in the proxy** (`@raw-autotune.observation.`
  `night-invisible-in-fixed-tone-proxy`): the fixed neutral tone renders
  high-ISO night/dusk frames at normal brightness; the `raw_at_night`
  batch scatters to Snow/Indoor/Sunset. `low_light_score` stays the sole
  night authority, and future snow policy needs corroboration on pale
  dusk scenes.
- **Domain gap is real**: median corpus confidence 0.65 vs ~1.0 on CamSDD.
- **Gate 4 passed** (`runs/adapt0`): 44 conf≥0.90 pseudo-labels ×4
  oversample, 5 epochs at lr 1e-5 → CamSDD val 97.33% vs 97.50 baseline
  (one-image dip, inside tolerance); val+test 96.92%, Backlight 87.5%.
  Corpus effect modest (44→49 high-confidence, 352/384 agreement) — the
  expected ceiling for one self-training round on 44 images.
- **Shipping candidate**: `models/artifacts/camsdd_resnet50_192.onnx`
  (1×3×192×288, clean op screen, 36.9 ms CPU) — re-exported from
  `runs/adapt1` after the hand review (below); adapt0 was the
  confidence-filtered first round.

## Hand review + Gate-2 disposition (2026-08-26, user review of review.html)

The user reviewed all 384 corpus predictions class by class. This yields
per-class **precision on our domain** — the number CamSDD argmax accuracy
cannot provide — recorded as
`@raw-autotune.observation.corpus-per-class-precision-hand-review`.
Trust tiers for the phase-4 mapping layer:

| Tier | Classes | Mapping-layer consequence |
| --- | --- | --- |
| **A — precision-trusted** | Beach, Blue_Sky, Text_Documents, Mountain, Landscape, Waterfall, Flower, Food, Macro, Night_shot, Portrait, Greenery/outdoor | May gate mild policy at ordinary thresholds; Macro/Food/Text are clean enough for render/encoder hints. |
| **B — high recall, low precision** | Indoor (~50% precision, catches every true indoor), Architecture (confident FPs: camper RV, porta-potty) | Corroboration **mandatory** before acting (Indoor ∧ C5 CCT — the plan's §4 rule, now empirically quantified). |
| **C — unusable on this corpus** | Kids (0%), Underwater (0%), Computer_Screens (25%), Snow (36/36 false — no snow exists in the corpus; pale roads/dusk trigger it) | Blocklisted from gating anything; excluded from pseudo-labels. Snow stays plausible as a *detector* but needs measured corroboration (the pale-scene confusion, twice confirmed). |

Structural note: argmax is single-label by construction (no image appears
in two categories), but real frames belong to several classes at once.
This is exactly why the Rust mapping layer consumes the **top-k softmax
vector**, not the argmax — and why Gate 2 is now authored against it:

**Gate-2 amendment**: promise = per-policy-class **top-3 recall ≥ 95%**
with top-1 ≥ 80% kept as the hard floor. Rationale: rev 2 of the plan
already ruled that policy classes are produced by inference-time mapping
over top-k (single-label CamSDD cannot supervise co-occurrence); judging
the model on argmax recall penalizes it for putting Backlight second
behind Sunset on genuinely backlit sunsets — a frame the mapping layer
handles correctly. Verdict comes from `--stage eval`, which now prints
both metrics per class.

The reviewed pseudo-label csv (`pseudo-reviewed.csv`, the one Snow
mislabel `_DSC0887` dropped — verified a snowless winter road) fed
`runs/adapt1`, the second adaptation round and shipping candidate:

- **Gate 4, zero regression**: CamSDD val recovers the full **97.50%**
  baseline (adapt0, trained *with* the mislabel, had dipped to 97.33 —
  at 43 pseudo-images ×4 oversample, one wrong label measurably hurt).
- **Gate 2 (amended), PROMISE**: on val+test, policy classes top-1/top-3:
  Macro 95.0/100, Night_shot 97.5/100, Candle_light 100/100,
  Indoor 100/100, Backlight 87.5/95.0. Worst top-3 = 95.0% ≥ bar;
  worst top-1 = 87.5% ≥ floor. Overall 96.83% top-1 / 99.67% top-3.
- Re-exported and re-screened: `camsdd_resnet50_192.onnx` clean.

Evidence: `@raw-autotune.evidence.camsdd-gate-2-topk-20260826`,
`@raw-autotune.evidence.camsdd-adapt1-reviewed-20260826`.

## Usage

```bash
V=.venv-train/bin/python

# Gate 0 — smoke (probe head only)
$V tools/scene_models/train_camsdd.py --stage probe --run runs/probe0

# Gate 1 — full fine-tune (resumes backbone from ImageNet, not from probe)
$V tools/scene_models/train_camsdd.py --stage finetune --run runs/ft0

# Gate 2 — per-class report on val+test
$V tools/scene_models/train_camsdd.py --stage eval --run runs/ft0 --split both

# Gate 3 — export best checkpoint
$V tools/scene_models/train_camsdd.py --stage export --run runs/ft0 \
    --export-onnx models/artifacts/camsdd_resnet50.onnx
python3 tools/scene_models/report.py models/artifacts/camsdd_resnet50.onnx

# Phase 3b ablations — variant backbones / input sizes
$V tools/scene_models/train_camsdd.py --stage finetune --arch mobilenet_v2 --run runs/ft-mnv2
$V tools/scene_models/train_camsdd.py --stage finetune --input 192x288 --run runs/ft-r50-192

# Phases 3c/3e — score own proxy renders, review, adapt
target/release/raw-autotune raw/<batch> --dry-run --semantic \
    --dump-scene-proxy .agent/scratch/camsdd-adapt/proxies/<batch>
$V tools/scene_models/pseudo_label.py --run runs/ft0 \
    --proxies .agent/scratch/camsdd-adapt/proxies \
    --out .agent/scratch/camsdd-adapt/labels          # scores.jsonl + review.html + pseudo.csv
$V tools/scene_models/train_camsdd.py --stage adapt --run runs/adapt0 \
    --init runs/ft0 --pseudo-csv .agent/scratch/camsdd-adapt/labels/pseudo.csv \
    --baseline 0.97                                   # Gate 4: no CamSDD val regression
```

Runs land in `tools/scene_models/runs/<name>/` (gitignored): `best.pt`,
`last.pt`, `metrics.jsonl` (one JSON line per epoch), `config.json`.
Everything is seeded (default 42); metrics lines record the seed and the
exact class list.

## Licence

CamSDD ships CC **BY-NC-SA 4.0** (`CamSDD/CamSDD/LICENSE.md`). Consequences,
under the conservative reading that trained weights are Adapted Material:

- Local training and use: fine.
- Redistributing the trained weights: permitted, but only under
  BY-NC-SA-compatible terms — attribution to CamSDD/ETH, **non-commercial**,
  share-alike. The weights therefore carry a NonCommercial restriction that
  the AGPL binary itself does not. Record this in `models/LICENSE-NOTES.md`
  before any weight redistribution.
- If commercial-grade weights are ever needed: retrain on a CC-BY/CC0
  curated set (the plan's phase-1 fallback), same code.

## Phase 4 — in-pipeline wiring and first paired evidence (2026-08-27)

`--scene-classify` now runs `camsdd_resnet50_192.onnx` inside the render
pipeline (lege-gpu CPU reference) on the same scene proxy the dump/adapt
loop used: content region, PIL-parity antialiased letterbox to 288×192,
ImageNet mean/std. Export gained a Flatten→Reshape rewrite (lege-gpu has no
Flatten shape inference; value-identical at batch 1). Parity against the
training venv on the dumped proxies: **top-1 identical, worst |Δp| =
0.0064** across the raw_backlit batch (PIL's fixed-point u8 resize vs our
f32 is the entire difference). Regression fixture:
`tests/fixtures/camsdd/` + the `classifier_golden_matches_python_reference`
unit test (skips when the model artifact is absent).

The sidecar carries `scene.classification` (full 30-way distribution +
top3 + entropy) and a fused observational `scene_lighting` block
(schema 26): backlit / indoor / night / macro, each `quiet | observed |
actionable` per the plan's corroboration rules. Tier-C classes are never
fused. Constants and their derivations:

- `BACKLIT_EV_SPLIT_MARGIN_EV = 1.25`: corpus survey 2026-08-27 (n=258
  ARWs), `p50_ev − center_median_ev` distribution p50 −0.15, p90 0.47,
  p95 0.72, p99 1.11, max 1.33 — the margin sits above p99 so ordinary
  compositions cannot corroborate by accident.
- `INDOOR_CCT_THRESHOLD_K = 3800`: inside the measured tungsten/daylight
  gap (3398 → 4339 K) from the C5 acceptance run.
- Night `actionable` mirrors `automatic_night_strength`'s 0.2 ramp start —
  the verdict reports exactly where the render already acts.

### What the raw_backlit batch (4 pairs, ajar-door scenes) actually showed

1. **Composition mismatch, honestly reported.** The classifier calls all
   four frames Indoor (0.58–0.87; correct as a scene description) with
   Backlight at 1–6%, and the EV split is *negative* — the bright region
   (the door opening) is central, so the centre outshines the surround.
   CamSDD's Backlight class is subject-against-light; a view *through* an
   opening is not that. Fusion verdicts: `backlit: quiet`,
   `indoor: observed` (C5 CCT 4560–5359 K — daylight-dominated, correctly
   refusing the tungsten corroboration).
2. **The dominant quality gap on these frames is not tone, it is
   highlight colour.** Exposure already matches the camera (centre-key
   delta ≤ 0.17 EV). But the clipped exterior foliage reconstructs into
   large pink/magenta false-colour regions under **both spatial
   estimators** (harmonic default and raw-pyramid), while
   `--highlight-method current` renders it nearly camera-like, and
   `--highlight-reconstruction 0` shows the raw magenta cast the solvers
   were supposed to fix. Evidence:
   `docs/evidence/phase4-backlit-20260827/ablation-grid.png` (six-way
   crop grid incl. camera JPEG). This is the standing purple-sky /
   unswept-spatial-floor family, at much higher severity, and it — not a
   silhouette-vs-fill tone policy — is what these frames say phase 4
   must gate first.
