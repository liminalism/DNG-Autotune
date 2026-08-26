# Scene & Lighting Detection Plan (backlit / indoor / mixed light / macro)

Status: **proposed plan, rev 2** — written 2026-08-26, revised the same day
after reading the Mobile AI 2021 challenge report (`research/2105.08819v1.pdf`)
in full. Not yet dispositioned into AKR; the next planning session should
`akr ingest` this document or supersede it with proper `work`/`decision`
records.

Rev 2 changes: the challenge report's training recipes are digested into a
concrete plan to **train our own desktop-class model** on the CamSDD data
instead of adopting a published phone model; the multi-label head is replaced
by the paper-faithful 30-way softmax with inference-time mapping; Burn is
assessed as the training stack and found feasible.

## 1. Problem and constraints

raw-autotune's measured detection today is: a five-way tonal class, a
validated night score (`low_light_score`), and observational semantic masks
(sky validated; vegetation/building weak; faces useless). The classes that
gate real render-policy differences on phones — **backlit / contre-jour,
indoor artificial light (tungsten / mixed illuminant), and macro** — are not
detected at all, and the project's own history shows why: extensive testing
found purely algorithmic detection of photo makeup **inconclusive**. Every
phone vendor ships a lightweight neural scene classifier for exactly this
job; that is the design to follow — but *not* the deployment constraints,
because dng-autotune is a desktop tool: heavier than on-camera processing,
lighter than a full Lightroom-style developer. We have orders of magnitude
more compute and memory per frame than a phone NPU budget.

Constraints established by prior decisions:

- Face detection is **out** (user decision 2026-08-26; YuNet produced zero
  true positives on the 74-image corpus).
- Macro detection is **maybe in** — cheap, it rides the same classifier.
- Weights are fetched reproducibly via `models/manifest.json` +
  `tools/scene_models/`, never committed (the 1.1 GB untracked-artifacts
  problem must not grow).
- Statistics stay the arbiter where they are already validated (night); the
  NN adds only the classes statistics cannot do. Observational-first
  rollout, exactly like the sky work: no render change without corpus
  evidence.
- raw-autotune is AGPL; dataset/model licence only needs to permit use +
  redistribution of weights alongside an AGPL tool. **Training our own
  weights does not bypass the dataset licence** — weights derived from
  CamSDD inherit whatever terms CamSDD imposes on derived works, so the
  licence check stays phase 1 regardless of who trains.

## 2. What exists publicly (researched 2026-08-26)

**A ready public "lighting detection model" effectively does not exist** as a
maintained, permissively-licensed artifact. What does exist:

1. **CamSDD** — the Camera Scene Detection Dataset (ETH Zurich, Mobile AI
   2021; arXiv:2105.07869): 30 categories that are *precisely the vendor
   taxonomy*, including **Backlight/Contre-jour**, **Indoor (mediocre or
   artificial lighting)**, **Night Shot**, **Macro/Close-up < 0.3 m**,
   **Candlelight**, Neon Lights, Stage/Concert, Overcast, Blue Sky,
   Sunrise/Sunset, Snow, Text/Document, Monitor Screen. Facts from the
   challenge report (local copy `research/2105.08819v1.pdf`, read
   2026-08-26): **9,897 official training images**, ~350 per class,
   balanced, single-label, crawled from Flickr and hand-cleaned, distributed
   at **576×384 px** (that resolution is the ceiling on model input — no
   larger pixels exist in the dataset). Winner top-1 95.0%, top-3 99.5%.
   **Licence: still unverified** — the project page
   (people.ee.ethz.ch/~ihnatova/camsdd.html) was unreachable when checked;
   this group's datasets are typically research-use. **Verifying the CamSDD
   dataset terms is step one of this plan.**
2. **C5 — Cross-Camera Convolutional Color Constancy** (ICCV 2021,
   github.com/mahmoudnafifi/C5): **Apache-2.0**, pretrained weights in-repo,
   input is a log-chroma *histogram* (not pixels), no metadata needed at
   test time, generalizes to unseen cameras. This is the missing
   **illuminant estimator**: raw-autotune already has the pre-white-balance
   raw data that CCC-family methods want.
3. The challenge's published TFLite models (INT8, phone-shaped) exist but
   are now the *fallback*, not the plan: they were optimized for constraints
   we do not have (see §3).

## 3. What the challenge report teaches about training (read 2026-08-26)

The paper is a compendium of ten training recipes on exactly our dataset.
The load-bearing findings:

- **Nobody competitive trained from random initialization.** Every team in
  the top eight fine-tuned an **ImageNet-pretrained** backbone (MobileNet-V2/
  V3, EfficientNet-Lite4/B0). The single from-scratch entry (Sidiki, 60K
  params, 8 conv layers) reached **78% top-1 vs 95% for the winner** despite
  a heroic training schedule. With ~330 images/class, pretraining is where
  almost all of the accuracy comes from. So "develop a new model from
  scratch" should mean **our own model, trained by us from a pretrained
  backbone** — not random init.
- **Bigger models win when runtime doesn't bind.** ByteScene's *teacher*
  (BiT-style ResNet101x3, frozen backbone, only the classifier head trained
  for 10 epochs) hit **97.83% top-1** on validation — nearly 3 points above
  the best phone model. The teams then spent their effort distilling that
  accuracy *down* into phone models. On desktop we simply keep the big
  model. Their phone constraints — INT8 quantization, TFLite op set,
  ReLU6/HardSigmoid-only activations, 128 px inputs — all evaporate.
- **Resolution matters.** EVAI's ablation: MobileNetV2 at 192×288 beat the
  same net at 96×144 by ~5 points top-1. Phone teams downsized to 128 px
  for FPS; we should train and infer at or near the dataset-native 576×384.
- **Extra pseudo-labeled data is worth +2%.** ByteScene added 2,577 images
  pseudo-labeled by their big model; ALONG crawled 50K images and kept
  high-confidence self-distilled labels (+2%), re-weighted misclassified
  samples (+1%). ALONG's ablation (their Table 5): augmentation +1%, extra
  data +2%, all tricks → 97.0% val on EfficientNet-Lite4.
- **The recipes themselves are modest and reproducible on our budget.**
  Representative: AdamW, lr 1.5e-3, weight decay 4e-5, batch 256, label
  smoothing 0.1–0.2, freeze-backbone → unfreeze → re-freeze staged
  fine-tuning, standard augmentation (flips, crops, rotations,
  brightness/contrast, blur, colour jitter), SWA and 5-seed teacher
  ensembles at the fancy end (PyImageSearch's Noisy-Student distillation).
  Total optimization steps are small: these are 10–200 epoch runs over
  <10K images at ≤384 px. **On any recent desktop GPU one run is minutes
  to ~an hour; even CPU-only overnight is viable. The user's "a day or two
  of training" estimate is right, and most of it goes to experiments
  (class mapping, augmentation, own-corpus pseudo-labeling), not to a
  single long run.**

### Design consequence — Model A becomes "our model"

- **Backbone**: ImageNet-pretrained **ResNet-50** class (see Burn note
  below for why ResNet), fine-tuned end-to-end at **576×384** (or 384×384
  letterbox), FP32. No quantization, no op-set restrictions. Expected
  accuracy from the paper's evidence: ≥97% top-1-equivalent (between
  EfficientNet-Lite4+tricks at 97.0 and ResNet101x3 at 97.83). Inference
  ~10–30 ms GPU / ~100–300 ms CPU per frame — noise inside a pipeline that
  spends seconds developing a RAW, so it can run **always-on**.
- **Head**: **30-way softmax, exactly the paper's taxonomy**, so our
  validation numbers are directly comparable to the published table (a
  free sanity check on our training). The ~11 policy classes (`backlit,
  night, indoor_artificial, tungsten_candle, overcast, sunny_sky, sunset,
  snow, document_screen, macro, neutral`) are produced by an
  **inference-time mapping** over the calibrated softmax/top-k output.
  Rev 1 proposed a multi-label sigmoid head; rev 2 drops it: CamSDD is
  single-label, so multi-label co-occurrence (backlit+indoor) cannot be
  supervised from this data anyway, and the paper's 99.5% top-3 shows the
  softmax's top-k already carries the co-occurring classes. Fusion (§4)
  handles co-occurrence with measured corroboration instead.
- **Domain adaptation, the step the paper validates twice**: CamSDD is
  Flickr JPEGs; our input is raw-autotune's 512-px proxy render. Close the
  gap the way ByteScene/ALONG did — **pseudo-label our own corpus** (the
  74-image paired corpus plus the user's archive renders) with the trained
  model, hand-review the labels (hundreds of images, an evening), and run
  a short final fine-tune on the mix. This also directly tunes the model
  to *our* renderer's colour and tone.
- **Optional later distillation**: if always-on cost ever matters (batch
  archive runs), distill into MobileNetV2 exactly as the paper does. Not
  scheduled; desktop numbers say we won't need it.

### Burn as the training stack — feasible, with a bounded backbone menu

The user asked whether the **Burn** Rust crate can do the training. Yes,
with one constraint:

- [tracel-ai/models](https://github.com/tracel-ai/models) provides
  **ResNet-family and MobileNetV2 implementations with ImageNet
  pretrained-weight download/import** (see also the
  [ResNet-Burn write-up](https://burn.dev/blog/resnet-burn/)); burn-train
  supplies the loop, AdamW, LR schedules; CUDA/wgpu backends cover the
  GPU. Fine-tuning ResNet-50 on <10K images is squarely inside what the
  framework demonstrably does. **Verify the pinned versions and weight
  sources at phase start** (the ecosystem moves fast).
- The constraint: the pretrained-backbone menu in Burn is ResNet /
  MobileNetV2 / SqueezeNet class — no EfficientNet or ConvNeXt ports with
  weights today. That is fine here: ResNet-50 at full resolution with the
  paper's tricks is comfortably in the 97% band this task needs.
- The augmentation pipeline (flips, crops, rotation, colour jitter, blur)
  must be hand-rolled over the `image` crate — the paper's augmentations
  are all simple affine/photometric ops, a day of work, reusable.
- **Bonus that may retire ONNX for this model**: Burn runs inference
  natively in Rust. Model A could ship inside raw-autotune as a Burn
  module + weights record fetched via `models/manifest.json` — pure Rust,
  AGPL-clean, no ONNX runtime / lege-gpu dependency for the classifier.
  Rev 1's "must stay in the scene.rs lege-gpu ONNX path" constraint is
  relaxed to a **phase-3 decision**: Burn-native vs ONNX export-from-
  PyTorch. (Burn imports ONNX but does not export it, so choosing Burn
  for training implies Burn for inference of this model. C5, §4, stays
  ONNX regardless — its weights are ported, not trained.)
- Fallback if Burn fights back mid-phase: the identical recipe in
  PyTorch → ONNX export → existing lege-gpu path. The recipe, classes,
  and gates are stack-independent; only the tooling swaps.

## 4. Model B — illuminant estimator (C5) — unchanged from rev 1

- Port C5 to ONNX; run on the raw pre-WB chroma histogram raw-autotune
  already computes territory for. Output: illuminant RGB → CCT + tint.
- Consumers: (a) an **indoor/tungsten/mixed-light verdict** (CCT bands +
  classifier corroboration); (b) a confidence gate for the existing
  `--local-white-balance` corrector, whose known failure ("campfire green
  when unguarded") is exactly an unguarded-illuminant problem.
- Cost: histogram input → sub-5 ms; weights ~a few MB, Apache-2.0.

### Fusion — typed verdict, statistics remain the arbiter

A `SceneLighting` struct produced per frame:

```
struct SceneLighting {
    backlit: Confidence,        // classifier + centre/surround EV split corroboration
    night: Confidence,          // existing low_light_score REMAINS authoritative;
                                // classifier only corroborates
    indoor: Confidence,         // classifier + C5 CCT band
    mixed_light: Confidence,    // C5 spatial disagreement (stretch goal: per-tile C5)
    macro_shot: Confidence,     // classifier + EXIF focus-distance corroboration
    // plus pass-through of the classes render policy may want later:
    // snow, sunset, document_screen, overcast, sunny_sky
}
```

Corroboration rules (each class promotes from `observed` to `actionable`
only when the cheap measured signal agrees):

- **backlit**: classifier ∧ (centre-vs-surround EV split — already computed
  as `center_median_ev` vs `p50_ev` — exceeds a corpus-derived margin).
- **indoor/tungsten**: classifier ∧ C5 CCT < threshold (or C5 alone at high
  confidence — C5 is the physically grounded one here).
- **macro**: classifier ∧ EXIF (FocusDistance / lens magnification when the
  maker notes carry it); classifier alone otherwise, at reduced confidence.
- **night**: `low_light_score` stays authoritative — it is validated; the
  classifier must NOT override it.

## 5. Rollout phases (each gated like the sky work)

1. **Licence + fetch** — verify CamSDD terms (they govern our trained
   weights too); wire the CamSDD download and C5 weights into
   `tools/scene_models/fetch.py` + `models/manifest.json` with licence
   notes; decide the MobileOne-S0 retirement. If the licence blocks
   derived-weight redistribution: fall back to assembling an equivalent
   30-class training set from CC-licensed Flickr/Open Images imagery
   (costs curation time, not architecture), or to research-use-only local
   training with weights never redistributed.
2. **C5 observational** — ONNX port, run over the paired corpus, record CCT
   per frame as `RenderReport` evidence only. Acceptance: CCT separates the
   known tungsten/daylight frames; no regressions in runtime budget.
3. **Train Model A** (was "classifier observational") —
   a. reproduce the paper's baseline: fine-tune the pretrained backbone,
      30-way softmax, their augmentation + label smoothing; gate on
      matching the published accuracy band (≥94% top-1 on a held-out
      split — below that, the training setup is wrong, stop and fix);
   b. resolution/backbone ablation (384 vs 576-wide input; ResNet-50 vs
      MobileNetV2) — pick on accuracy, we have the runtime headroom;
   c. pseudo-label + hand-review our own corpus renders, short final
      fine-tune (domain adaptation);
   d. decide Burn-native vs ONNX inference shipping (see §3);
   e. run observationally over the corpus, record per-class scores as
      evidence only. The docs name backlit and indoor as corpus classes
      with **no paired evidence** — capturing those pairs is part of this
      phase.
4. **Policy wiring, one class at a time** — backlit first (exposure/tone
   policy for silhouette vs fill), then indoor/tungsten (WB corrector gate),
   each behind its own A/B against the camera-JPEG scorecard, exactly like
   `docs/STATUS.md`'s existing highlight/shadow/local-detail methodology.
5. **Macro** — policy is mild (sharpening/denoise bias, AQ hint for the
   encoder); ship last.

## 6. What this plan deliberately does not do

- No face/person path (decision above).
- No semantic segmentation growth — the LR-ASPP sky mask stays as-is.
- No training-infrastructure sprawl: one training crate/script under
  `tools/scene_models/` (plus the existing fetch/prepare/export/pack),
  seeded from a pretrained backbone — never from random init, which the
  challenge data shows costs ~17 points top-1 at this dataset size.
- No claim that the classifier replaces measured statistics — it fills the
  classes where measurement was shown inconclusive, and everything it
  asserts must be corroborated before it moves pixels.

## Sources

- Mobile AI 2021 challenge report (training recipes; local copy read in
  full 2026-08-26): `research/2105.08819v1.pdf`, https://arxiv.org/abs/2105.08819
- CamSDD dataset paper: https://arxiv.org/abs/2105.07869
- CamSDD project page (licence check pending, unreachable 2026-08-26):
  https://people.ee.ethz.ch/~ihnatova/camsdd.html
- C5 (Apache-2.0): https://github.com/mahmoudnafifi/C5
- Burn model zoo with pretrained-weight import: https://github.com/tracel-ai/models
  and https://burn.dev/blog/resnet-burn/

## 7. Shot list — paired evidence for phase-4 policy classes (added 2026-08-26)

Phase 4's A/B methodology grades our render against the camera JPEG, and the
docs note backlit and indoor have **no paired evidence** in the corpus. These
shots fill that gap. On every scene: **RAW+JPEG in one press** (the camera
JPEG is the scorecard reference — RAW-only shots can't be used), camera on
auto WB/exposure as usual, 3–5 frames per scene with small angle/framing
variation. 10–20 keepers per class is plenty; these are observational
fixtures, not training data.

### Needs daylight (tomorrow)

1. **Backlit** — the phase-4 priority (wired first):
   - a person or object between camera and the sun, subject's lit side
     *away* from camera (face/front in shade), sky bright behind;
   - the same setup against bright sky without direct sun;
   - indoors by day: subject in front of a bright window, shot from inside
     (window-backlit is the classic indoor backlight the classifier and the
     centre-vs-surround EV split must both catch).
   Vary how deep the silhouette goes — from mild rim-light to full
   silhouette. The policy question is "silhouette vs fill", so both
   intents on the same scene are the most valuable pairs.
2. **Mixed light** — daytime indoor with the room lights **on** and daylight
   coming through a window; subject positioned so both illuminants hit it.
   A few frames with the lights off for the same scene make a clean A/B on
   the illuminant estimator.

### Nighttime indoor cuts it — for these classes it's actually *better*

3. **Indoor artificial / tungsten** — evening/night rooms lit only by warm
   bulbs (no daylight contamination through windows, which is exactly why
   night beats day for this class): 2–3 different rooms/light types. Add
   one cool-LED or fluorescent-lit room for CCT contrast.
4. **Candle light** (policy class, quick win while you're at it): a scene
   dominated by candle/flame light, lights off.
5. **Macro** (optional, mild policy): close-focus small objects — indoor at
   night is fine; a daylight variant is a bonus, not a requirement.

So: tonight's indoor shooting covers classes 3–5 perfectly; tomorrow's
daylight session is needed only for backlit and mixed-light. Night outdoor
already exists in the corpus (`raw_at_night`).
