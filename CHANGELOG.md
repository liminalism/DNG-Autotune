# Changelog

## 0.1.15

The two remaining items from the 0.1.14 scorecard's loss column: the chroma
veil at extreme ISO, and local detail, where the camera won 64 of 98 pairs
because it sharpens its output and this program did not. Both land as automatic
defaults; between them the paired-corpus win count goes **189 → 218** out of
392 contested comparisons, with the highlight rout intact.

### Chroma denoising, stage two: luminance-guided

0.1.14 documented its own residual honestly: what survives the box filter at
extreme ISO is *low-frequency* blotching wider than any safe window, visible as
a magenta veil on the ISO 65535 dusk frames. No linear low-pass can remove it —
it occupies the same spatial band as real colour — so the fix is a prior, and
luminance is the right one: real colour boundaries coincide with luminance
boundaries; noise does not.

`src/chroma.rs` gains a second stage, a fast guided filter (He and Sun's
subsampled variant, `GUIDED_SUBSAMPLE 4`) over the colour-difference planes
with the frame's own luminance as the guide. Support is `GUIDED_RADIUS 128` px
— far beyond the 13x13 box, affordable because a guided filter's cost does not
grow with radius. `GUIDED_EPSILON 1.0e-3` was swept on the high-ISO files; it
took the ISO 8000+ saturation ratio against the camera from **1.37 → 1.27**,
and widening support from 48 to 256 px changed nothing (settled at 128), so
the residual is now regularisation-limited, not support-limited. A 1:1 crop of
the dusk frames shows the chroma confetti and veil essentially gone, leaving
monochrome grain — exactly the design contract: chroma removed, luma untouched.
The stage only runs at `automatic_strength >= 0.35` (`GUIDED_MIN_STRENGTH`), so
mildly noisy frames pay only for the box pass and clean frames still allocate
nothing.

### Output sharpening, automatic

`docs/PLAN.md` §4b ranked this the cheapest real win available: the camera won
local detail (`average_gradient`) 64-31, median 4.59 vs our 3.71, and the whole
explanation is that every camera sharpens its JPEG and we did not, at all.

`src/sharpen.rs`: an unsharp mask on **luminance only**, radius 1 px (capture
sharpening, not an effect), amount 0.75, applied after the tone curve. The
correction is added equally to all three channels, so it cannot shift a hue —
the mirror image of the chroma filter, which moves colour and provably cannot
touch luminance. Three guards:

- **Noise fade.** The amount scales by `1 - chroma::automatic_strength(snr10_ev)`
  — the same measured ramp that drives denoising, so the frames too noisy to
  sharpen are exactly the frames the denoiser is already treating, and there is
  one ramp to re-tune instead of two.
- **Soft shrinkage** (`SHRINK_KNEE 0.004`) suppresses corrections below the
  noise floor, so flat areas stay flat instead of acquiring crawling texture.
- **Per-channel headroom.** The correction may spend at most half the distance
  to black or white, measured on the outermost *channel*, not on luminance.
  The first full-corpus run guarded on luminance and paid for it immediately:
  highlights went W86 L1 → W73 L21, because clipping is per channel — a sky
  pixel at luma 0.9 with blue at 0.99 gets pushed over. Same lesson as
  0.1.12's chroma cap; it is now a comment in the code.

The amount was swept against the paired corpus with the camera's median
gradient (4.59) as the target: `--sharpen 1.0` reaches 94% of it with clipping
at 0.024% vs the camera's 1.25%, and a 1:1 halo check reads clean at 1.0 and
"processed" at 1.4. Shipped at 0.75 as the automatic amount; `--sharpen`
scales it, 0 disables.

### The scorecard, before and after both features

| axis (98 pairs) | 0.1.14 | 0.1.15 |
|---|---|---|
| highlights kept | W86 T11 L1 | **W87 T10 L1** |
| shadows kept | W26 T68 L4 | W26 T68 L4 |
| tonal detail | W46 T9 L43 | **W49 T7 L42** |
| local detail | W31 T3 L64 | **W56 T0 L42** |

Median `average_gradient` 3.71 → **4.83**, now above the camera's 4.59 — the
one axis the camera systematically won is now ours on a majority of pairs —
with corpus clipping back at 0.0000% after the per-channel fix. Highlights and
shadows did not pay for it.

### Verification

`cargo test --release` 120 tests (13 chroma, 7 sharpen among them: the
luminance-invariance, cannot-clip-or-crush, flat-field-untouched and
hue-preservation contracts are each pinned by a test), clippy clean.
Determinism: three 8-worker runs over 36 files spanning snr10_ev -6 to +2.0
(both stages inert / box only / box+guided+faded sharpen), all JPEGs
byte-identical, sidecars identical apart from the self-referential output path.
The scorecard was measured on Linux; tests, clippy and the determinism check
were run on the Windows toolchain (same pinned 1.89, same checkout).

## 0.1.14

58 more pairs (42 Sony A7C, 16 ProShot) covering ISO 100 to 12800, two 0.8-1.0 s
night frames, beach and high-variance scenes. The corpus is now 98 pairs across
368 files. This pass is mostly what they found; one thing they found was a
defect worth fixing, and two were conclusions that would have been wrong.

### Chroma noise reduction, automatic

`docs/PLAN.md` §4's second 0.2 item. At ISO 1600 and above the program was
rendering at 2.2x the camera's `mean_saturation`, and a 1:1 crop showed what
that "saturation" was: red and green speckle the camera removes and we did not.

`src/chroma.rs` splits each pixel into luminance and a colour difference, blurs
the colour difference, and recombines. The colour difference has zero luminance
by construction and blurring is linear, so **the filter cannot change luminance
at all** — it can dull a colour edge, but it cannot soften detail, shift
exposure, or move the tone controller's statistics. That guarantee is what lets
it run by default, and there is a per-pixel test for it.

Strength comes from `snr10_ev`, the frame's own fitted SNR=10 crossing, rather
than from ISO, because it is measured rather than declared and every file has
one. It separates the corpus cleanly:

```text
Sony ISO 100-200      -6.28      ProShot, all       -5.71
Sony ISO 500-1000     -3.63      Sony ISO 8000+     -1.00
Sony ISO 1600-2500    -3.08      Sony ISO 12800+    +1.94
```

Saturation relative to the camera's own JPEG, over the paired files:

| Sony band | before | after | filter |
|---|---:|---:|---|
| ISO 100-200 (n=17) | 0.943 | 0.943 | inert |
| ISO 500-1000 (n=7) | 1.100 | 1.100 | inert |
| ISO 8000-12800 (n=6) | 2.169 | **1.372** | r5, strength 0.92 |
| ISO 65535 (n=9) | 2.274 | **1.403** | r6, strength 1.00 |

Colourfulness against the camera improves with it: +21.7 -> +7.0 and
+54.5 -> +10.8 on those two bands.

Over the whole 368-file corpus the filter is active on 59 files (16%) and inert
on 309, and **every inert file renders byte-identically to 0.1.13** — it
allocates nothing on a clean frame. 368 of 368 render, 0 failures. Clipping
improves (worst frame 1.345% -> 0.923%), crushed is unchanged, determinism holds
byte-for-byte over three 8-worker runs.

`--chroma-denoise` scales the automatic strength; 0 disables it.

It does not remove everything, and the limit is understood rather than guessed:
at ISO 65535 the filter already runs at full radius and scaling it further
changes nothing, because what survives is low-frequency chroma blotching wider
than the window. A box big enough to reach it would smear colour across real
edges. That needs a multi-scale or edge-aware filter, which is future work.

### Two conclusions the pairs overturned

**The `MAX_ORACLE_DEVIATION_EV` guard is validated, not too tight.** It binds on
5 of the 42 Sony pairs, all night or high-ISO, and the numbers alone say it is
costing us up to 3.17 EV of agreement with the camera. The images say the
opposite: on those frames Sony's own JPEG is *badly underexposed* — a waterfall
at ISO 2500 rendered nearly black — and our clamped render shows the scene.
Following the camera there would have made those frames unusable. The guard was
supported by one file before; it is now supported by five, with visual evidence.

**Therefore `subject_display_ev` delta is not an error measure on its own.** It
assumes the camera is right, and on night scenes the camera is not. The
acceptance threshold `docs/STATUS.md` gained in 0.1.13 ("below 0.10 EV") holds
only for scene classes where the vendor's rendering is trustworthy, and that
document now says so. This is the main limitation of grading against camera
JPEGs and it took a night frame to expose it.

### Still open

The +1.0 `ORACLE_TARGET_CEILING_EV` question from 0.1.13 is **untested by this
batch**: the brightest camera subject in it is +0.99 EV, so nothing reached the
regime where the ceiling binds. Snow would have been the test and is not
available. A white wall in direct sun or a bright sand beach at midday would do.

Sidecar and summary schema version 5, adding the `chroma_denoise` block.

## 0.1.13

13 Sony A7C RAW+JPEG pairs were shot for this pass, closing the gap 0.1.12
recorded as its largest unvalidated surface. They confirmed 0.1.12's colour work
transfers to a second sensor, and they settled a question 0.1.12 explicitly
deferred.

### 0.1.12's saturation constant, validated on Sony

The 1.20 factor was tuned entirely on Samsung. On 13 Sony pairs the `auto`
preset now renders at a **0.981** saturation ratio against the camera's own
JPEG, against 1.011 on ProShot and 1.042 on Expert RAW. A constant fitted on one
sensor landing within 2% on another is the strongest available evidence that the
deficit it corrects was in this program's pipeline rather than in one phone's
rendering intent. No change was needed.

Highlights are where the two diverge, in our favour: over the 13 pairs we hard
clip a **0.000%** median (0.001% worst) against the camera's **3.494%** median
and 8.073% worst, at comparable near-white.

### The preview oracle is now automatic

`docs/PLAN.md` §3's first item. `--preview-exposure` was a flag defaulting to
off; it now defaults to an automatic decision and the flag is an override.
Passing `0` disables it.

**The trigger is the preview, not the file class**, which is what the pairs
corrected. `preview::MIN_PREVIEW_PIXELS` rises 30 000 -> 250 000, so a file
carrying a real rendering gets the oracle and a file carrying only an index
thumbnail does not. The corpus separates cleanly, with no threshold-fitting:

```text
Samsung Expert RAW   50.0, 24.5, 12.5, 10.0 Mpx   rendering
Sony A7C ARW                          1.745 Mpx   rendering
ProShot                               0.049 Mpx   thumbnail (256x191)
```

250 000 pixels sits five times above the largest thumbnail and seven times below
the smallest real preview. The old floor of 30 000 admitted ProShot's thumbnail
on the reasoning that it was "still the only rendering that camera provides";
the pairs show it is not a rendering, and steering by it made the median error
worse.

Subject EV against the camera's own JPEG, over all 42 pairs:

| source | preview | mean abs error, off | automatic | frames worse |
|---|---|---:|---:|---:|
| Sony A7C (n=13) | 1616x1080 | 0.64 EV | **0.03 EV** | 0 |
| Samsung Expert RAW (n=8) | up to 8160x6120 | 0.40 EV | **0.00 EV** | 0 |
| ProShot (n=21) | 256x191 | 0.31 EV | 0.31 EV, inert | 0 |

Full strength, not a blend: at 0.5 the error is halved rather than removed and
no frame was better at 0.5 than at 1.0, so there is no evidence for a partial
adoption and the default does not invent one.

Over the whole 310-file corpus, 310 rendered with 0 failures. Every one of the
287 files whose exposure moved has a real preview, and the 23 that do not are
byte-identical to 0.1.12 — the gate for this change. Highlights and shadows both
improve, because the oracle mostly darkens:

| over 310 files | 0.1.12 | 0.1.13 |
|---|---:|---:|
| clipped, max (Sony corpus) | 0.52% | **0.15%** |
| clipped, max (Samsung LinearRaw) | 1.95% | **0.01%** |
| near-white, median (Sony corpus) | 2.08% | **1.51%** |
| crushed, frames newly above 0.1% | — | **0** |

That last row is the one that mattered. 33 frames darken by more than 1 EV and
the night files land where the camera put them — the old Samsung night DNGs go
from a mean level of 88-118 to 31-35, which is the "renders night as day"
failure finally fixed on the files that produced the complaint — yet not one
frame's crushed fraction rose. The noise floor and the fifth-percentile guard in
`analyze::derive_params` are what hold the black point up, and they held.

### Known trade

On backlit high-dynamic-range interiors the oracle recovers the window at the
cost of the room: the worst mover, a doorway scene at -3.50 EV, gains the view
outside and loses the furniture to shadow. That is the camera's own choice being
reproduced faithfully, and it is the case a future local operator or scene-class
policy should improve rather than one the oracle should be blamed for. It
crushes nothing.

## 0.1.12

First pass using a corpus of RAW+JPEG pairs, as `docs/PLAN.md` §2 asks for. 29
Samsung pairs were shot for it: 21 ProShot (4080x3060 true CFA Bayer, 16-bit
uncompressed, `DngCreator`) and 8 Samsung Expert RAW (8160x6120 LinearRaw,
14-bit JPEG-XL, `BaselineExposure` +2.0).

### Grading against the camera's own JPEG

- Add `--reference` and `--reference-dir DIR`, which pair each RAW with the
  camera JPEG of the same stem, measure both the same way, and report the
  signed difference per file and its distribution across the batch. This is
  measurement only; the renderer never sees the reference.
- Add `metrics::OutputStats::mean_saturation`, the mean of `(max - min) / max`
  over pixels above 5% of full scale. Colourfulness moves with exposure as well
  as with chroma, so two renderings of one scene at different brightness are
  not comparable on it; this is a ratio and they are.
- Add `--saturation-scale`, a global multiplier on the preset's saturation, so
  the chroma path can be swept against a corpus the way `--exposure-bias`
  offsets exposure. The default 1.0 multiplies exactly, so it cannot move
  output. Gate: with the measurement machinery added but before the preset and
  cap changes below, all 29 files re-rendered byte-identically to 0.1.11 and no
  pre-existing summary field moved.
- Add `tools/contact-sheet.py`, which turns a summary into an HTML page of
  side-by-side pairs with the numbers under each, worst-first.
- Raise sidecar and summary schema versions to 4.

### The chroma deficit this found, and the fix

Exposure needed no work: over the 29 pairs the subject lands within +0.02 EV of
the camera's own placement at the median. Colour did. The `auto` preset
rendered at **0.847** of the camera's saturation, and the shortfall was flat —
the same 0.85 on both sources, and roughly constant across luminance and across
the chroma range, which is the signature of a pipeline-wide shortfall rather
than a scene-dependent one.

`auto`'s `saturation` is raised 1.02 -> 1.22 and `punchy`'s 1.06 -> 1.27, the
same 1.20 factor. `neutral` keeps 1.00: its documented job is to add no chroma
opinion. Swept over the corpus:

| `--saturation-scale` | saturation ratio, ProShot | colourfulness delta | clipped, median |
|---:|---:|---:|---:|
| 1.00 | 0.847 | -11.1 | 0.000% |
| 1.10 | 0.933 | -5.2 | 0.000% |
| **1.20** | **1.017** | **+0.0** | **0.000%** |
| 1.30 | 1.130 | +3.0 | 0.154% |
| 1.40 | 1.237 | +7.2 | 0.271% |

Two independent measures — the saturation ratio reaching 1.0 and the
colourfulness delta reaching 0 — land on the same value, and 1.20 is the last
step before gamut clipping appears.

### Highlight headroom cap in the chroma step

The Sony corpus, which has no paired JPEGs, caught what the phone corpus could
not. Chroma is expanded around the pixel's own rendered luminance, so a larger
scale drives the outermost channel past the white point the curve just placed
it under. With `highlight_norm == 1.0` the curve deliberately lands the
brightest channel *just* below white, so on a blue sky the boost spends exactly
the headroom that protection created. Raising saturation alone took the worst
Sony frame from 0.000% to 23.316% of pixels with a channel at full scale,
undoing 0.1.10.

`tone::render_pixel_local` now caps the chroma scale at the value that lands the
outermost channel on the curve's own output range. The cap's floor is the scale
the pixel would get with no saturation opinion, not 1.0 — a floor of 1.0
overrules `highlight_desaturation` and pushes chroma up on exactly the pixels
the preset wanted pulled in, which left four frames clipping more than before.

Over all 258 Sony A7C frames:

| | 0.1.11 | saturation only | 0.1.12 |
|---|---:|---:|---:|
| colourfulness, median | 36.19 | 44.47 | **44.37** |
| saturation, median | 0.229 | 0.273 | **0.272** |
| clipped, p90 | 0.011% | 11.161% | **0.008%** |
| clipped, max | 0.689% | 23.316% | **0.524%** |

The cap keeps the colour gain essentially intact while leaving hard clipping
slightly *better* than 0.1.11. On the 29 pairs the ratio lands at 1.011
(ProShot) and 1.042 (Expert RAW), with no frame clipping at all.

**Near-white rose and is not a regression, but the 0.1.10 figures are no longer
comparable.** `near_white_fraction` counts pixels with *any* channel at or above
98% of full scale, so a saturated blue sky whose blue channel sits on the
curve's white point is counted. Across the Sony corpus it goes from a 0.338%
median to 2.080%, and 58 of 258 frames now exceed 10%. Three measurements say
nothing is blown: the fraction of pixels with *all three* channels near white is
0.00% before and after, and luminance entropy (7.4986 -> 7.4989 median) and
average gradient (4.1415 -> 4.1464) are unchanged. The gradation is intact and
carried by the two channels that are not at the ceiling.

### Corrected file-class identification

`docs/STATUS.md` had `proshot.dng` and `expertraw.dng` the right way round, but
the distinction is worth stating in terms that survive a new file: ProShot
writes 4080x3060 CFA Bayer with an Android build fingerprint in `Software` and
DNGVersion 1.4 (the `DngCreator` signature) and carries only a 256x191
thumbnail; Samsung Expert RAW writes LinearRaw with JPEG-XL compression,
DNGVersion 1.7, a non-zero `BaselineExposure`, and a full-resolution JPEG
preview. Only the second is usable by `--preview-exposure`.

## 0.1.11

### Opt-in local tone adaptation

- Add `--local-tone <0..1>`, defaulting to zero. Zero follows the old render
  path exactly; a real ARW rendered byte-identically with the flag omitted and
  with `--local-tone 0`.
- Add a deterministic, full-resolution Reinhard/Zhang adaptive-surround
  implementation. Nine Gaussian scales select a local base per pixel; that
  base drives a median-anchored EV correction while the established global
  curve and hue-preserving RGB gain remain unchanged.
- Bound corrections to +1 EV shadow lift and -0.75 EV highlight compression,
  with a 0.15 EV dead band, sensor-noise-floor fade, and bright-detail guard.
- Record the selected-scale histogram, correction distribution and guard
  activity in sidecars and batch summaries.
- Add luminance entropy and average gradient to rendered-output metrics.
- Raise sidecar and summary schema versions to 3.

The synthetic suite covers constant fields, light/dark minorities, limits,
noise protection, determinism, monotonic ramps and hard edges. A
maximum-strength pass rendered all 268 available files without failure or a
correction-bound violation; it peaked at 2.05 GiB on the largest phone DNG.
Detailed measurements are in `docs/STATUS.md`.

## 0.1.10

### Auto-preset highlight protection

- Raise `auto`'s `highlight_norm` from 0.90 to 1.00. At 1.00 the brightest
  channel is protected from render-created clipping by construction.
- Add a regression test fixing that policy value.
- Correct sidecar and batch-summary `schema_version` to 2, as 0.1.9's
  changelog promised. The 0.1.9 writers accidentally continued to emit 1
  after adding preview and pooled-noise fields.

Measured by rendering all 258 Sony A7C files at 0.90, 0.95, and 1.00:

| `highlight_norm` | near-white median | p90 | max | frames above 10% | colourfulness median |
|---:|---:|---:|---:|---:|---:|
| 0.90 | 1.22% | 11.36% | 27.14% | 35 | 36.70 |
| 0.95 | 0.71% | 4.57% | 16.30% | 3 | 36.68 |
| 1.00 | **0.35%** | **1.86%** | **8.14%** | **0** | 36.66 |

The full protection costs only 0.05 points of median colourfulness and 0.04
level on an 8-bit mean, while removing every frame above the 10% near-white
threshold. All 774 renders completed without failure.

## 0.1.9

Two opt-in features, both default-off. Sidecar `schema_version` is now 2.

### Preview-driven exposure (`--preview-exposure <0..1>`)

Takes the exposure target from the camera's own embedded preview. The
controller aims every frame's median at middle grey, which across 258 Sony
files lands them all within +/-0.9 EV of it — consistent, but it renders night
as day.

- `src/preview.rs` locates previews through the TIFF directory, not by scanning
  for JPEG markers. Scanning finds Samsung's single-channel gain map appended
  after the preview, and the first end-of-image marker in a strip belongs to a
  nested EXIF thumbnail rather than the preview.
- Candidates are sorted explicitly. `find_ifds_with_filter` walks `sub_ifds()`,
  a `HashMap` whose iteration order varies per process, so relying on the order
  it returns would have made output non-deterministic.
- The preview's brightness is inverted back through the tone curve
  (`tone::inverse_map_ev`) before use: it measures display EV while the target
  is curve-input EV, a systematic error of about the curve's slope otherwise.
- Measured with the same centre weighting the analyzer applies to the raw, so
  the two are compared like for like.

Verified on the motivating file: `expertraw.dng` renders at mean level 112.2
with the oracle off, 64.8 with it on, against the vendor preview's own 63.0.
Across 268 files a preview was found for every one; the deviation cap never
binds and the asymmetric ceiling catches 6%.

### Pooled sensor noise (`--noise-scan`, `--noise-profile`, `--pool-noise`)

Frames from one camera at one ISO share a gain, so their noise estimates pool.
`--noise-scan` decodes and fits only, taking 13 seconds over 258 files against
about 4 minutes for a full pass. A profile keeps output reproducible: a file
develops identically alone or in a batch, which in-batch pooling cannot promise.

`read_variance` is deliberately not pooled — it clamps to zero on more than half
of all frames, so a group median of it would look like a measurement while being
an artifact of that clamp.

**Measured effect is small.** 245 of 258 frames pool and some floors move by up
to 5 EV, but the black point changes on only 1 file. The reason is that 0.1.8's
green-plane fix already removed most of the per-frame disagreement this was
built to correct: the ISO 100 within-group spread fell from 128.6x to 3.4x. The
mechanism is correct and validated, but on this corpus it is now nearly a no-op.

## 0.1.8

Fixes a defect in the 0.1.5 noise estimator: it was measuring the wrong colour.

- Sample the **green** CFA phase, chosen from the camera's own layout, instead
  of phase (0,0). On an RGGB sensor that phase is red, which under daylight
  carries roughly half the signal of green, starving the fit of brightness
  range. This was the cause of the 43 frames that produced no estimate.
- Replace fixed-width brightness bins with equal-count bins. Fixed widths left
  most of the range empty on any frame without bright highlights, so the tiles
  piled into the darkest few bins and failed the minimum-bin check.
- Weight the fit by `1 / variance^2`. The sampling error of a variance estimate
  grows with the variance, so unweighted least squares was dominated by the
  brightest bins and dragged the intercept negative.
- Clamp the intercept into `[0, darkest bin variance]` rather than fitting a
  parameter there is no data for.
- Replace the old failure mode with an honest one: refuse when the brightest
  bin is under 4x the darkest, since without that leverage the slope is not
  determined.

Measured over the 258-file Sony corpus:

```text
                                        before    after
estimates obtained                     215/258  257/258
log2(shot_slope) vs log2(ISO) slope     +1.154   +1.039   (theory 1.000)
  R^2                                    0.893    0.976
monotonic ISO ladder inversions         6 of 15  2 of 15
```

The single remaining refusal is a frame with 3.54 EV of dynamic range, where
the leverage guard fires correctly.

The better estimates are also *less* aggressive: the black point moved down by
a median 0.87 EV on the 33 frames that changed, because the red-plane fit had
been overstating noise and crushing more than necessary.

## 0.1.7

- Add `src/whitebalance.rs` and `--local-white-balance <0..1>`: local
  multi-illuminant white balance after fierro2009, correcting each pixel by a
  blend of detected light white points weighted by spatial and chromatic
  proximity. Off by default.
- Clustering is deterministic (fixed chromaticity grid, fixed merge order)
  rather than the paper's randomly seeded K-means, which it states is not
  repeatable.
- White points are normalized to unit luminance so the correction moves colour
  without moving exposure; the paper's version always brightens.
- Only acts when two or more chromatically distinct illuminants are found,
  since as-shot white balance already handles a single one.
- Rejects lights more than 0.22 from neutral in chromaticity. Without this the
  method rendered a campfire green: the fire is the brightest thing in frame, so
  it was read as a cast and neutralized. With it, that frame is left untouched
  while a night street scene still has its magenta cast corrected.

## 0.1.6

- Add `src/shotinfo.rs`: read ISO, exposure time and aperture from EXIF, into
  the sidecar and `--summary`.
- Validate the 0.1.5 noise model against ISO. Across 258 Sony frames the SNR=10
  crossing rises +0.910 EV per stop of ISO against a theoretical 1.00, with
  R^2 0.916 on per-ISO medians. The 11 EV across-batch spread that blocked its
  use in 0.1.5 was real ISO variation, not estimator error.
- Use it: `analyze` now takes a noise floor and will not place the black point
  below the SNR=1 crossing, so shadow range is not spent stretching pure noise.
  Binds on 12% of the batch, raising the black point by a median of 1.11 EV;
  clean frames are untouched.
- Cap that floor at the frame's 5th percentile. The estimate is validated in
  aggregate but not per frame — individual base-ISO frames disagree by up to
  7 EV — so the cap bounds how much of an image a bad estimate can crush. Worst
  case falls from +5.04 EV to +2.80 EV with no change to the median.

## 0.1.5

- Add `src/noise.rs`: per-image Poisson-Gaussian sensor noise estimation
  following kronander2013 §5, reporting the scene EV at which SNR falls to 10
  and to 1. Recorded in the sidecar and `--summary`, and available under
  `--dry-run` since it is estimated from raw samples before development.
- Noise is estimated from second differences along the quieter image axis, not
  from plain tile variance: plain variance is dominated by detail and put the
  SNR=10 crossing above sensor saturation.

The estimate is **measurement only** and does not affect rendering. It finds
that 63% of the 258-file Sony batch have their black point placed below the
SNR=10 floor, which is the effect it was added to detect — but the across-batch
spread is 11 EV and 43 files yield no estimate, so it is not yet trustworthy
enough to bound the black point. See `docs/RESEARCH_NOTES.md` for what would
validate it.

## 0.1.4

- Add `src/metrics.rs`: Hasler & Susstrunk colourfulness, plus near-white,
  hard-clipped and crushed fractions, measured on every rendered image and
  recorded in the sidecar and `--summary`. The highlight work in 0.1.3 was
  steered by an ad-hoc statistic; this replaces it with the measure the
  enhancement literature uses.
- Split "blown" (>=98% of full scale) from "literally clipped" (at full scale).
  After 0.1.3 hard clipping is near zero on the test batch, so it alone cannot
  steer tone work.
- Add `docs/RESEARCH_NOTES.md` recording which parts of the papers in
  `research/` apply and which do not.

Notable from that reading: `tone::map_ev`'s highlight branch is algebraically
identical to logarithmic image processing (LIP) scalar multiplication, and the
`highlight_norm` blend added in 0.1.3 is nnolim2018's intensity-value model.
Both were arrived at independently; neither needed changing.

## 0.1.3

- Drive the tone curve's highlights by the brightest channel rather than by
  luminance alone, controlled by the new `highlight_norm` parameter.

  A saturated blue sky carries roughly 1.7x more signal in its brightest channel
  than in its luminance, so that channel passed 1.0 long before luminance
  reached the white point. `compress_gamut` then desaturated it back into range,
  which is what turned skies flat white. Blending the curve's input toward the
  brightest channel moves that roll-off onto the curve, where it is smooth. The
  blend is inert on neutral pixels, so greys and the exposure anchor are
  unaffected.

  Measured over a 14-file Sony A7C subset: pixels pinned at 250+ fell from 5.36%
  to 2.02% of frame on average (worst file 22.3% -> 1.2%), highlight saturation
  rose from 0.193 to 0.207, and mean image level barely moved (119.5 -> 118.7).

  Preset values: neutral 1.00, auto 0.90, punchy 0.70.

## 0.1.2

- Expose the crate as a library. The `probe-*` examples now decode through
  `raw_autotune::decode_corrected`, the same path the renderer uses. Previously
  they called Rawler directly and so reported on data the program never used —
  which produced one badly wrong diagnosis before it was caught.
- Add `--summary <FILE>`: a machine-readable JSON report for a whole run, with
  per-file analysis and parameters plus batch tonal-class counts and an exposure
  distribution. Works with `--dry-run`, which is the fast way to survey a large
  batch without writing images.
- Apply DNG `BaselineExposure` to the scene-linear data. Samsung Expert RAW
  records +3 EV; without it those files develop three stops dark. Applying it to
  the data rather than folding it into the automatic exposure keeps it outside
  the controller's +/-5 EV clamp.
- Read raw chunk bytes on demand instead of loading the whole file to decide
  whether a repair is needed.

## 0.1.1

First version that actually compiles and runs; 0.1.0 was never built.

- Fix two compile errors against Rawler 0.7.2: `RawDevelop` is constructed from
  its public `steps` field rather than a nonexistent `new_with`, and the
  nonexistent `ProcessingStep::FujiRotate` was dropped.
- Add `src/ljpeg.rs`, a lossless JPEG (SOF3) decoder with restart-marker
  support, and `src/redecode.rs`, which substitutes it for streams Rawler 0.7.2
  decodes incorrectly. Rawler ignores `DRI`/`RSTn` and silently returns a
  diagonal ramp for such files; Samsung Galaxy linear DNGs are affected.
- Add `src/levels.rs`, which collapses repeating black-level patterns to one
  level per component for linear DNGs (Rawler panicked on the mismatch) and
  rescales levels whose domain disagrees with the decoded samples.
- Record every decoder correction in the JSON sidecar's `level_normalization`
  field and on stderr as a `NOTE` line.
- Add four `probe-*` examples for inspecting DNGs that misbehave.

## 0.1.0

- Initial source prototype.
- Batch discovery for Rawler-supported extensions.
- Rawler normalization, demosaic, crop, white balance, and calibration.
- f32 scene-linear intermediate.
- Sparse percentile and center-weighted analysis.
- Neutral, auto, and punchy parameter policies.
- EV-domain global view transform with hue-ratio restoration.
- Adaptive vibrance, highlight desaturation, and gamut compression.
- 16-bit TIFF/PNG and JPEG output.
- JSON decision sidecars.
- Per-file panic containment and bounded file-level concurrency.
- Optional baseline render and analysis-only mode.
