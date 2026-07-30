# Plan: from 0.1.11 to a usable 0.3

Written 2026-07-28. This document plans the next few development passes so
that, by roughly version 0.3, the program is usable **in the way actually
intended**, which is narrower and more specific than the general roadmap:

> A fully automatic RAW developer that replaces the camera's internal
> RAW-to-JPEG processing, but with the computation, memory, and time budget of
> a desktop. Shoot RAW-only on a Samsung S24+ and a Sony A7C, run one command
> over the card/folder, and get archive-ready images for a photo library.
> No per-image adjustment, no editing workflow, no professional ambitions.

This reframing changes priorities relative to `docs/ROADMAP.md`. The roadmap
is organized by photographic subsystem maturity; this plan is organized by
"what blocks the archival use case." Some roadmap items turn out to be
blockers (EXIF), some turn out to be optional (policy files, EXR), and one
item not on the roadmap at all is a blocker (zero-flag operation).

## 1. Definition of "usable" for the intended purpose

The tool is usable when all of the following hold:

1. **One command, no per-source flags.** `raw-autotune <folder> --output
   <folder>` produces correct output for A7C ARW, S24+ Expert RAW, S24+ Pro
   mode, and ProShot DNG without the user knowing which is which. Today Expert
   RAW silently renders ~2.6 EV too bright unless the user remembers
   `--preview-exposure 1.0`. A tool for archiving cannot require per-file
   forensics.
2. **Output is library-grade.** The files go into a photo library and must
   behave like camera JPEGs there: correct EXIF (date/time, camera, lens, GPS,
   orientation), an embedded color profile, and a thumbnail-friendly format.
   Today output carries no metadata at all — the library would sort every
   photo by file modification date and show no capture info. For an archiving
   tool this is not polish; it is the product.
3. **No frame is ruined.** The bar is not "matches Lightroom Auto"; it is
   "never clearly worse than the camera's own JPEG, usually at least as good."
   Concretely: no blown highlights the camera JPEG kept, no crushed shadows,
   no visibly wrong exposure class (night rendered as day), no green campfires.
   A small number of merely *bland* results is acceptable; ruined ones are not.
4. **High-ISO frames are presentable.** The A7C corpus runs to ISO 10000 and
   phone sensors are noisy at base ISO. Without denoising, the "larger
   computation budget" promise is unmet — noise handling is the single biggest
   quality gap between this tool and any camera's internal engine, because the
   camera always denoises.
5. **Batch runs are unattended and idempotent.** Point it at a year of RAWs
   overnight: it skips already-processed files, survives individual failures,
   and reports what it did. Re-running produces byte-identical output
   (determinism is already a stated product property — keep it).

Explicitly **not** required for 0.3: a GUI, semantic/ONNX analysis, lens
corrections beyond DNG opcodes, X-Trans excellence, camera looks/DCP,
interactive anything.

## 2. The corpus comes first

Development is paused until a larger corpus exists. That is the right order:
the STATUS doc records that Expert RAW support rests on **one file**, the
preview oracle's guard rails on ten, and the mixed-light guard on two data
points. Every constant tuned now would be re-tuned later.

### What to gather

Aim for breadth of *scene class* and *source*, not raw count. 258 more sunny
A7C frames add little; the controller's failures live in the classes it cannot
see (`docs/KNOWN_LIMITATIONS.md`, "Automatic decisions").

Per source — **S24+ Expert RAW** (highest priority, n=1 today; target 30+),
**S24+ Pro mode** (n=7; target 20+), **ProShot** (n=2; target 20+), **A7C**
(n=258 but one photographer, mostly ordinary scenes; target: fill the missing
classes below, and if possible a second body or lens):

Scene classes to cover on *each* source, roughly matching the README's test
list because each maps to a distinct subsystem:

- ordinary daylight (control group);
- indoor tungsten/LED, and mixed window+tungsten (white balance);
- night street and near-dark (exposure class, noise, preview oracle);
- high ISO of a normal scene (denoiser, noise floor);
- backlit person, and a person filling the frame (future semantic work needs
  ground truth *now*);
- snow / bright wall / high-key (exposure class);
- low-key / stage-light / concert (exposure class, saturated light guard);
- sunset, neon, campfire — strongly colored light that must NOT be neutralized;
- deliberately clipped highlights (reconstruction);
- deliberately 2–3 EV underexposed (shadow lift + noise);
- bursts / time-adjacent sequences of one scene (0.3 sequence consistency).

### Capture the reference alongside

**Critical and cheap: shoot RAW+JPEG, not RAW-only, while building the
corpus.** The camera JPEG is the exact baseline this tool is trying to beat;
having it per-frame turns "is our render better?" from an opinion into a
side-by-side. For Expert RAW, also keep Samsung's rendered output. Store as
`corpus/<source>/<class>/name.{arw|dng,jpg}` so the survey tooling can pair
them mechanically.

Also record, once per source: a gray-card or white-wall frame at each whole
ISO stop (feeds the pooled noise profile with content-free data), and a dark
frame (lens cap) at high ISO if convenient (hot-pixel map).

### Corpus infrastructure — **done in 0.1.12**

- ~~Extend `--dry-run --summary` to also ingest the paired camera JPEG and emit
  comparison metrics.~~ `--reference` / `--reference-dir`, `src/reference.rs`.
  Emits mean level, colourfulness, near-white, crushed and saturation deltas
  per file plus their distributions across the batch, and a subject-EV delta
  that works under `--dry-run` because the controller's target and curve are
  both known before anything is rendered.
- ~~A tiny HTML contact-sheet generator.~~ `tools/contact-sheet.py`, sorted
  worst-first.
- Added along the way: `mean_saturation` in the output metrics, because
  colourfulness moves with exposure as well as chroma and so cannot compare two
  renderings of one scene at different brightness; and `--saturation-scale`, so
  the chroma path can be swept the way `--exposure-bias` offsets exposure.

It paid for itself immediately. 29 pairs showed exposure was already within
0.02 EV of the camera at the median, and colour was not: `auto` rendered at
0.847 of the camera's saturation. See `CHANGELOG.md` 0.1.12.

13 Sony A7C pairs followed in 0.1.13 and did the same job twice over: they
confirmed the chroma constant transfers to a second sensor untouched (0.981
against a target of 1.0, fitted entirely on Samsung), and they made the oracle
decision measurable. Both findings below came out of them.

- **The trigger for auto-enabling the preview oracle is preview size, not file
  class**, which is what §3's first bullet had assumed. Acted on in 0.1.13.
- **The "Expert RAW renders 2.6 EV too bright" defect never generalised.**
  Eight paired daylight Expert RAWs sit at -0.14 EV median with the oracle off.
  The original file is a night scene; the failure is the aim-the-median rule on
  night scenes, not a property of the source. The Sony pairs then showed the
  same rule running +0.49 EV bright at the median and +1.66 EV on a low-key
  frame — so this was never a phone problem at all, it is the controller's, and
  it was invisible for as long as the main corpus had no references.

58 more pairs in 0.1.14 (42 Sony spanning ISO 100-12800 with night frames, 16
ProShot) brought the corpus to 98 pairs and settled the noise question — and
delivered the first case where the pairs' *numbers* pointed the wrong way. See
`docs/STATUS.md`, "What the night and high-ISO pairs settled".

**Two lessons worth keeping.** First: every conclusion that survived contact
with the pairs was about the controller, and every conclusion the project had
drawn from a single file was wrong in some detail. Gather pairs for a scene
class before tuning for it, not after.

Second, from 0.1.14: **the camera JPEG is a reference, not ground truth.** On
night frames Sony's own rendering is badly underexposed, and the subject-EV
metric therefore reported a 3.17 EV "error" on frames where our render was the
better one. The scene classes where the controller is weakest are the same ones
where the vendor is weakest, so the metric is least trustworthy exactly where it
is most needed. Read the number, then open the pair.

## 3. Pass 0.1.13 — remove the flags (autodetect pass)

Goal: the zero-flag invariant (usable-criterion 1), using the new corpus to
validate. No new imaging science; this is plumbing and policy.

- ~~**Auto-enable the preview oracle per file class.**~~ **Done in 0.1.13, but
  not by file class.** The 42 pairs showed the trigger has to be the preview
  itself: `preview::MIN_PREVIEW_PIXELS` rose 30 000 -> 250 000, so a file
  carrying a real rendering gets the oracle and one carrying only an index
  thumbnail does not. Subject EV mean absolute error against the camera's own
  JPEG went 0.64 -> 0.03 EV on Sony and 0.40 -> 0.00 EV on Expert RAW, with
  ProShot correctly inert; 0 frames worse anywhere. The `LinearRaw` +
  `BaselineExposure` signature this bullet proposed would have been both
  narrower and wrong — it identifies a vendor, not a usable preview.
- **Revisit `ORACLE_TARGET_CEILING_EV`.** Now that the oracle is automatic, the
  +1.0 ceiling binds on 16 of 287 files with previews, most of them
  independently classified `high_key`. It is probably refusing legitimate bright
  scenes. None of the 16 has a paired JPEG; see `docs/STATUS.md`. Needs a
  high-key pair set before it is touched.
- **Auto-select `--jobs`** from available RAM and file dimensions instead of
  documenting "use `--jobs 1` for large files." The tool knows both numbers.
- **Skip-existing / resume** for unattended batch runs, plus a end-of-run
  summary (n rendered, n skipped, n failed and why).
- ~~Decide the fate of `--local-tone` default-on/off *after* the corpus visual
  pass.~~ **Decided: it stays off.** Measured against the 98 pairs, full
  strength takes tonal-detail wins from 46 to 7 out of 98 (median entropy
  7.5204 -> 7.0716) to buy a gradient rise of 3.71 -> 3.93 against a camera at
  4.59. It buys a little local contrast with a lot of global tonal
  distribution. See `docs/STATUS.md`. `--local-white-balance` stays off too, on
  the unchanged grounds in the limitations doc.

Exit test: run the full mixed corpus with **no flags** and diff the survey
against per-source flagged runs — they must match where a flag was previously
required, and be byte-identical where it wasn't (the existing gate test).

**The oracle half of this test passed in 0.1.13**: all 310 files render from one
no-flag invocation with zero failures, the 287 that changed all carry a real
preview, and the 23 that do not are byte-identical to 0.1.12. The zero-flag
invariant now holds for exposure across all three target sources; what remains
in this pass is `--jobs` and resume, neither of which affects rendering.

## 4. Pass 0.2 — the quality gap (noise, highlights, metadata)

This is the roadmap's 0.2 pruned and reordered for the archival use case.

Priority order:

1. **EXIF copy + ICC embed** (usable-criterion 2). Copy date/time, camera,
   lens, exposure, GPS, orientation from the RAW into JPEG/TIFF output; embed
   sRGB ICC. Verify a target photo library (whatever will actually host the
   archive — decide which, and test against it) sorts and displays them
   correctly. This is first because it is finite, testable, and blocks the
   entire use case regardless of image quality.
2. **Profiled denoising** (usable-criterion 4). **Chroma half done in 0.1.14**
   (`src/chroma.rs`), driven by the fitted `snr10_ev` rather than by ISO, with
   an automatic strength and no flag. It takes the ISO 8000+ saturation ratio
   against the camera from 2.17 to 1.37 and is inert on 309 of 368 corpus files.
   Two things remained after 0.1.14:
   - ~~**Low-frequency chroma blotching.**~~ **Done in 0.1.15**: a
     luminance-guided second stage (fast guided filter, 128 px support) takes
     the ISO 8000+ saturation ratio 1.37 → 1.27 and removes the magenta veil
     visibly; the residual is regularisation-limited, not support-limited.
   - **Luma noise**, still entirely untouched, and deliberately so — it is the
     part that destroys texture. The camera's own high-ISO JPEGs are visibly
     mushier than ours, so this is not obviously a gap to close all the way.
3. **Hot/dead pixel suppression.** Cheap, mechanical, and phone sensors need
   it. The dark frames help but a median-based detector should not require
   them.
4. **Clipped-highlight reconstruction** (pre-demosaic, as roadmapped). The
   deliberately-clipped corpus class is the test set. Even the simple
   "rebuild the clipped channel from the surviving ones" method beats the
   current compress-only behavior on skies.
5. **Lens corrections via DNG opcodes only.** Phone DNGs carry their opcode
   lists (distortion, vignetting, gain maps — note STATUS's warning about the
   Samsung gain map); applying them closes most of the gap to the phone's own
   render. A general lens database is out of scope for 0.3.
6. ~~**Output sharpening**, output-size-aware, mild, automatic.~~ **Done in
   0.1.15** (`src/sharpen.rs`): luma-only unsharp at 1 px, amount 0.75,
   noise-faded by the same snr10_ev ramp as the denoiser, per-channel headroom
   guard. Local-detail wins went 31 → 56 of 98 with the highlight rout intact.

Explicitly deferred from roadmap-0.2: nothing else moves up.

## 4b. Beating the camera, not matching it

The reference corpus creates an obvious trap: the program borrows the camera's
exposure target and is graded against the camera's JPEG, so it could converge on
being a slower copy of the camera. That is not the goal, and the corpus already
shows it is not what is happening — but the strategy should be written down.

**Borrow the judgement, beat the rendering.** A camera JPEG contains two things:

1. *A judgement about the scene* — that this is a night shot and should look
   like night, where the subject is, what the faces are. That needs having been
   there, plus detection this program deliberately defers. Borrowing it via the
   preview oracle is free and does not cap anything.
2. *A rendering of that judgement* — one global curve, NR and sharpening in
   about 100 ms on a battery, single pass, tuned to be safe for a stranger.
   Every one of those constraints is absent on a desktop.

The oracle borrows only (1): it sets the display EV the subject lands on. How
the remaining twelve stops are distributed around it is entirely ours, which is
why the program can sit 0.03 EV from the camera's exposure and still keep more
highlight information on 86 of 98 frames.

Two bounds on the borrowing, both already in place and both now evidenced: the
oracle applies only where the preview is a real rendering, and
`MAX_ORACLE_DEVIATION_EV` stops it following a vendor rendering that is itself
wrong — which on the night frames is what makes our output the better one.

**Grade on axes where "better" needs no reference.** Clipped, crushed, entropy
and gradient are one-directional: information kept is better, and no opinion is
required to say so. Those are where a win can be claimed. Subject placement and
colour level are taste, need a reference, and are where matching *is* the right
answer — in ordinary conditions. Night and extreme ISO are where the camera is
untrustworthy on both counts.

So the acceptance metric is the win/tie/lose scorecard in `docs/STATUS.md`, not
distance to the camera. Ranked by what that scorecard says is actually missing:

1. ~~**Local detail.**~~ **Won in 0.1.15.** Was lost on 64 of 98 frames, median
   gradient 3.71 against 4.59; output sharpening flipped it to won-on-56 with a
   median of 4.83. `--local-tone` was *not* the answer and had been measured out.
2. ~~**Chroma veil at extreme ISO.**~~ **Closed in 0.1.15** by the
   luminance-guided chroma stage.
3. **Candidate render and sanity check** (§5.2) — **but see the 0.1.15 planning
   verdict below before building it.** Three planning agents were run: an
   implementation plan (a `CandidateOffset` re-derivation through
   `derive_params`, identity candidate always index 0, placed before the
   dry-run return so surveys report the decision), a scoring design
   (veto gates on the taste axes, saturating gains on the information axes,
   incumbent-defends with a margin), and an adversarial review. The review's
   verdict: **reject §5.2 as written.** Its case: `luminance_entropy` and
   `average_gradient` are not one-directional (their maximisers are histogram
   equalisation and amplified noise respectively — the `--local-tone` sweep
   already demonstrated the metrics preferring a worse image); the tonal-detail
   row is statistical noise (medians 10^-4 bits apart); `clipped_fraction` has
   a corpus median of 0.0000% with nothing left to recover; and on the three
   scene classes where the controller is weakest (night, extreme ISO, deep
   shadow) the metrics carry the *wrong sign*, so an automated selector would
   systematically pick the frame a human rejects. If §5.2 happens at all it
   should wait for scene-class policies (§5.1) and metrics with actual
   resolution on the axes that remain contested.

## 5. Pass 0.3 — the controller earns its keep

With metadata, denoising, and zero-flag operation done, 0.3 is where "never
clearly worse than the camera JPEG" gets systematically true. From the
roadmap's 0.3 list, in this order:

1. **Scene-class policies** (high-key, low-key, night, backlit, flat), driven
   by the statistics split (center/edge/highlight/probable-sky). The corpus
   classes map one-to-one onto these policies; each policy lands only with
   its corpus class as regression evidence. Night is first — it is the
   documented failure ("renders night as day") and the preview oracle already
   half-solves it.
2. **Candidate render + sanity check.** Render cheap proxies under 2–3
   parameter candidates, score with the existing output metrics (entropy,
   near-white, crushed, colourfulness), pick deterministically. This is the
   "spend desktop compute on quality" promise in its simplest honest form.
3. **Sequence consistency** for bursts: time-adjacent frames of one scene get
   consistent exposure/WB decisions. Matters for a library where bursts sit
   side by side; the corpus burst class is the test set.
4. **Skip the user-tunable policy file** unless a concrete need appears — it
   serves users who want to adjust, which is the workflow this tool exists to
   avoid. A `--preset` is enough.

Exit criterion for 0.3 = the usable-criteria in §1, measured as: a blind
side-by-side pass over the paired corpus (ours vs camera JPEG) with every
frame graded better / equal / worse / **ruined**. Target: zero ruined, and
"worse" confined to classes with a documented follow-up. That grading session
is the acceptance test for the whole plan.

## 6. What stays deferred past 0.3

Unchanged from the roadmap, restated so this document is self-contained: ONNX
and all semantic analysis (faces, sky, subjects) wait until the non-neural
controller's failures are *classified* on the corpus — the corpus person/
backlit classes are being gathered now precisely so that classification is
possible later. GUI, camera looks/DCP, X-Trans quality, EXR, GPU: all out.

The learned parameter controller (roadmap end-state) becomes plausible only
after §5's grading sessions accumulate — graded corpus passes *are* the
future training data. Keep the grades.

## 7. Order of operations, summarized

```text
done     paired-comparison survey + contact-sheet tooling      (0.1.12)
         chroma matched to the camera JPEG on 29 pairs         (0.1.12)
         chroma confirmed on 13 Sony pairs, no change needed   (0.1.13)
         preview oracle automatic; no per-source flags left    (0.1.13)
         chroma denoising, driven by the fitted noise model    (0.1.14)
         98 pairs across ISO 25-12800; daylight is done        (0.1.14)
         guided chroma stage: the extreme-ISO veil is gone     (0.1.15)
         output sharpening: local detail flipped to a win      (0.1.15)
now      one gap left worth shooting: a genuinely high-key pair
0.1.16   remaining autodetection: --jobs from RAM; batch resume/skip-existing
0.2      EXIF/ICC → hot pixels → highlight reconstruction
         → DNG-opcode lens corrections
0.3      scene-class policies → burst consistency → candidate renders
         (candidate renders demoted; see §4b item 3's planning verdict)
gate     blind grading vs camera JPEGs: zero ruined frames
```

Each pass ends with the standing verification from `docs/STATUS.md` (gate
survey diff, `cargo test --release`, clippy, determinism check) plus the new
paired-corpus grading.
