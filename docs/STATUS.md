# Status and handoff — raw-autotune 0.1.19

Updated 2026-08-26. Scope as stated: the program has to work on **Samsung S24+
Expert RAW**, **ProShot** output, and **Sony A7C ARW**. This records how far
that is, what to run, and what is left.

**Most of this document was written on 2026-07-31 and its measurements are as of
that date. Read every number here as version-stamped, not current.** Work landed
since then — automatic night tone and luma denoising (2026-08-13), opt-in
semantic scene masks (2026-08-13), the Slice 5 joint raw-domain
log-chromaticity estimator (2026-08-23), and the archive-speed spatial-highlight
floor plus the in-memory EXIF/ICC surface (2026-08-26) — is recorded in
`CHANGELOG.md` and in the AKR ledger, and is reflected here only where a section
below says so explicitly. `REPORT_SCHEMA_VERSION` is now 22 (23/24 for the
semantic sidecars) and `AUTO_PROFILE_VERSION` is `archive-auto-v6`.

Two things below are known to disagree with each other or with the code, and are
called out here rather than silently left:

- the ISO 8000+ chroma residual. The 0.1.15 section says the guided-filter stage
  took the saturation ratio to 1.27; the acceptance table at the end still
  expects 1.35 to 1.45. One of the two is stale and neither has been
  re-measured. Do not quote either as settled.
- `docs/PURPLE_SKY_PROBLEM_SCOPE.md` predates the Slice 5 merge and is now
  marked "needs re-verification" rather than "unresolved". Nothing here should
  be read as saying the lavender-sky defect is closed.

## Short answer

**All three targets work on defaults. There are no per-source flags left.** The
A7C highlight defect in the original handoff was fixed in 0.1.10, the chroma
deficit in 0.1.12, and the Expert RAW exposure flag was removed in 0.1.13 when
the preview oracle became automatic.

| Target | Files on hand | Paired JPEG | State | Invocation |
|---|---|---|---|---|
| A7C ARW | 321 | 63 | **Works.** 321/321 render, 0 failures | defaults |
| ProShot + S24+ Pro mode DNG | 39 | 35 | **Works.** 39/39 render | defaults |
| S24+ Expert RAW | 16 | 8 | **Works.** 16/16 render | defaults |

376 of 376 files render with zero failures on a single no-flag invocation, which
is `docs/PLAN.md`'s first criterion for "usable". 106 of them have a paired
camera JPEG, spanning ISO 25 to 12800. Counts are as of 0.1.18 and were taken
from a `--dry-run --reference --summary` over `raw/`; the two Samsung sources are
merged in the middle row because they share a `Model` string — see the next
section for how to tell them apart.

Since 0.1.17 the output also carries EXIF and an sRGB ICC profile, which is
`docs/PLAN.md`'s second criterion. The third and fourth — no ruined frame, and
presentable high-ISO — remain the open ones.

**Since 0.1.19 `--jobs` defaults to `auto`** and is worked out from the memory
the machine reports free and the size of the largest input, which closes
`docs/PLAN.md`'s fifth criterion. See "What to run" below.

**Since 0.1.18 the default colour path is `owned`, not Rawler's.** The current
implementation owns everything from black-level normalization onward for
ordinary Bayer files except the mature PPG kernel selected for noisy/textured
mosaics. Owned RCD/AMaZE-class interpolation is used only behind strict measured
guards because dense branches still show less false colour through PPG.

**DNG matrix colour and standardized lens correction are automatic.** Colour
composes the ForwardMatrix or ColorMatrix/Bradford route with `AnalogBalance`, signature-matched
`CameraCalibration`, one/two/three illuminants, custom `IlluminantData`, and
three/four camera channels with optional `ReductionMatrix`. Incomplete profiles
fall back to the decoder transform; `--no-dng-color` is the control. DNG
`OpcodeList3` supplies `WarpRectilinear` distortion/lateral-CA and
`FixVignetteRadial` coefficients without a camera database; missing metadata is
a no-op and `--no-lens-correction` is the control.

## Telling the two Samsung sources apart

Both come off the same phone and the file name says nothing, so identify them by
structure. This is the single most confusing thing about this corpus:

| | ProShot | Samsung Expert RAW |
|---|---|---|
| Dimensions | 4080x3060 | 8160x6120 (also 5712x4284) |
| Photometric | CFA Bayer, `cpp=1` | LinearRaw, already demosaiced |
| Compression | none, 16-bit | JPEG-XL (52546), 14- or 16-bit |
| `DNGVersion` | 1.4 | 1.7 |
| `Software` | Android build fingerprint | firmware string |
| `Model` | `SM-S926U1` | `Galaxy S24+` |
| `BaselineExposure` | 0 | +2.0 or +3.0 |
| Preview | 256x191 thumbnail only | full-resolution JPEG |
| File size | ~25 MB | 80-140 MB |

The build fingerprint and `DNGVersion` 1.4 are the `DngCreator` signature, i.e.
any third-party app using the Camera2 API. Only Expert RAW carries a preview
`--preview-exposure` can use.

Their rendering intents differ too, which matters when either is used as a
target. On the 29 pairs the ProShot JPEGs are markedly more saturated and blow
far less:

| camera JPEG, median | ProShot (n=21) | Expert RAW (n=8) |
|---|---:|---:|
| `mean_saturation` | **0.299** | 0.233 |
| `near_white_fraction` | **0.89%** | 8.09% |
| `clipped_fraction` | **0.67%** | 5.51% |

**ProShot's JPEGs are the better reference of the two**, and they are also the
21-file majority, so they carry the corpus. Samsung's own 50 MP Expert RAW
JPEGs are flat and blow their skies; do not treat them as a colour target.

## What to run

```bash
# Every source, one command, no per-source flags. This is the whole corpus.
raw-autotune raw/arw raw/arw_better raw/raw_better raw/raw_old raw/raw_3rd_batch \
  --output out --format jpeg

# Optional, recommended: pool sensor noise per ISO first. The scan takes about
# 13 seconds over 258 files; the profile is reusable and keeps output identical
# whether a file is developed alone or in a batch.
raw-autotune raw/arw --noise-scan sony.json
raw-autotune raw/arw --output out --noise-profile sony.json

# To reproduce pre-0.1.13 exposure, switch the preview oracle back off:
raw-autotune raw/arw --output out --format jpeg --preview-exposure 0
```

`--jobs` no longer needs a value; since 0.1.19 it defaults to `auto` and is
chosen from the memory the machine reports free and the size of the largest
input. The advice this paragraph used to carry — "keep it at 3 or below when the
batch contains 50-megapixel Expert RAW files, because `--jobs 8` over the full
corpus is killed by the OOM killer on a 31 GiB machine" — is now the program's
job. On this machine the full corpus plans 5 workers.

Survey a batch without writing images:

```bash
raw-autotune raw/arw --dry-run --summary survey.json
```

Grade a batch of RAW+JPEG pairs against the camera's own renderings, which is
how every colour number below was produced:

```bash
raw-autotune raw/raw_better --output out --format jpeg --reference \
  --summary out/summary.json
tools/contact-sheet.py out/summary.json --output out/sheet.html
```

`survey.json` carries per-file analysis, chosen tone parameters, sensor noise,
EXIF, and rendered-output metrics. It is the fastest way to test a hypothesis
against the corpus and is how nearly every number in this document was produced.

## How each target actually behaves

### Sony A7C ARW — the strongest case

313 files, true Bayer CFA, uncompressed 14-bit, ISO 100–12800, 55 of them with
a paired camera JPEG. All 313 decode, analyse and render with no failures.
Rendered output, `auto` preset, 0.1.13 (before 0.1.14's denoiser, which touches
only the high-ISO tail):

```text
colourfulness   median 40.9        (Hasler & Susstrunk, 0..~110)
mean saturation median 0.274
clipped         median 0.000%      p90 0.004%   max 0.148%
crushed         median 0.000%      max 0.0004%
near-white      median 1.64%       p90 15.01%   max 33.06%
mean level      median 112.5
```

Against the 13 paired JPEGs the `auto` preset lands at a **0.981** saturation
ratio and a **0.03 EV** mean subject error, while hard clipping a 0.000% median
against the camera's 3.494%. On this corpus the program is at least as good as
the camera on every measure it can be compared on.

**Read `near-white` carefully.** It counts pixels with *any* channel at or above
98% of full scale, so 0.1.12's chroma boost trips it wherever a sky is
saturated: 52 of 271 frames exceed 10%, against 0 before 0.1.12. Nothing is
blown. The fraction of pixels with *all three* channels near white is 0.00%,
luminance entropy and average gradient are unchanged, and the paired JPEGs sit
at a comparable 3.90% near-white median while hard clipping twenty times more
than we do. Read the number as "has a saturated sky", not "is blown". The
0.1.10 near-white figures elsewhere in this document predate the change and do
not compare.

### ProShot — works, and is the most honest phone raw

ProShot files are true CFA Bayer (`cpp=1`, GBRG mosaic), i.e. undemosaiced
sensor data, which is what makes ProShot the best phone source here. All 23 on
hand render on defaults, and the 21 that came with a paired JPEG are the corpus
0.1.12's colour work was measured against.

Caveat: ProShot files carry only a **256x191 uncompressed thumbnail**, no JPEG
preview at all. Since 0.1.13 that thumbnail is deliberately rejected — it is
below `preview::MIN_PREVIEW_PIXELS` — so the automatic preview oracle is inert
on ProShot and these files render from the controller's own estimate alone.
That is the measured right answer, not a limitation: steering exposure by the
thumbnail made the median error worse on all 21 pairs. It does mean ProShot is
the one source with no vendor opinion available to fall back on.

### S24+ Expert RAW — works on defaults since 0.1.13

Expert RAW is `LinearRaw` (already demosaiced by the phone), JPEG-XL compressed,
14- or 16-bit, with a `BaselineExposure` of +2 or +3 EV which the program
applies. It carries a full-resolution JPEG preview, so the automatic oracle
applies and **the flag this source used to need is gone**.

`expertraw.dng` rendered about 2.6 EV brighter than Samsung's own rendering
before 0.1.13. It now lands on it:

```text
mean level, 0.1.12 (oracle off)     112.2
mean level, 0.1.13 (automatic)       64.2
Samsung's own preview                63.0
```

**That file is a night scene, and the eight paired daylight Expert RAWs say the
error never generalised.** With the oracle off their subject lands at a median
of -0.14 EV from where Samsung put it, none worse than 0.9 EV. So the honest
statement was never "Expert RAW renders 2.6 EV too bright"; it is "the
controller's aim-the-median rule fails on night scenes, and Expert RAW happened
to be the source of the one night frame". That is exactly why 0.1.13 triggers
the oracle on preview quality rather than on file class.

The corpus still lacks night frames from any source. That is now the most
valuable gap in it — and note that the one night file drove both the oracle's
original design and this pass's most dramatic correction.

## Highlight issue resolved in 0.1.10

The documented 0.90/0.95/1.00 `highlight_norm` sweep has now been run over all
258 A7C files:

| `highlight_norm` | near-white median | p90 | max | frames above 10% | colourfulness median |
|---:|---:|---:|---:|---:|---:|
| 0.90 | 1.22% | 11.36% | 27.14% | 35 | 36.70 |
| 0.95 | 0.71% | 4.57% | 16.30% | 3 | 36.68 |
| 1.00 | **0.35%** | **1.86%** | **8.14%** | **0** | 36.66 |

`auto` now uses 1.00. The full no-clip guarantee costs only 0.05 points of
median colourfulness and 0.04 level on an 8-bit mean relative to 0.90. The
earlier two-point estimate came from comparing the complete `auto` and
`neutral` presets, so it incorrectly attributed their other saturation and
contrast differences to `highlight_norm`.

Those near-white numbers were measured before 0.1.12 raised `auto`'s saturation
and do not compare with today's; see the A7C section above. The property the
sweep was actually protecting — that the curve does not pin a channel at full
scale — still holds, and 0.1.12 measures marginally better on it.

## Colour matched to the camera JPEG in 0.1.12

`--reference` pairs each RAW with the camera's own JPEG of the same capture and
measures both the same way. Against 29 Samsung pairs, `auto` was rendering at
0.847 of the camera's saturation — a flat deficit, the same on both sources and
roughly constant across luminance and chroma. `auto`'s `saturation` went
1.02 -> 1.22 and `punchy`'s 1.06 -> 1.27; `neutral` keeps 1.00 by design.

| | 0.1.11 | 0.1.12 |
|---|---:|---:|
| saturation ratio vs camera, ProShot (n=21) | 0.847 | **1.011** |
| saturation ratio vs camera, Expert RAW (n=8) | 0.842 | **1.042** |
| colourfulness delta vs camera, ProShot | -11.1 | **-0.2** |
| subject EV delta vs camera, ProShot | +0.02 | +0.02 |
| clipped, median / max over the 29 | 0.000% / 0.000% | 0.000% / 0.000% |

**0.1.13 validated this on a second sensor.** The 1.20 factor was fitted
entirely on Samsung; on 13 Sony A7C pairs `auto` renders at a **0.981**
saturation ratio, within 2% of the target it was never tuned against. A constant
fitted on one sensor transferring to another is the strongest available evidence
that the deficit it corrects lived in this program's pipeline rather than in one
phone's rendering intent. No adjustment was needed.

Exposure needed no work on the Samsung pairs; it was already within 0.02 EV of
the camera's own placement at the median. The Sony pairs told a different story
— see below.

The chroma boost had to be capped inside `tone::render_pixel_local`. Chroma is
expanded around the pixel's rendered luminance, so with `highlight_norm == 1.0`
— which lands the brightest channel *just* under white — the boost spends
exactly the headroom that protection created. Uncapped it took the worst Sony
frame from 0.000% to 23.316% of pixels with a channel at full scale. The cap
limits the scale to what lands the outermost channel on the curve's own output
range, and floors that at the pixel's unboosted scale so it cannot overrule
`highlight_desaturation`. With it, Sony clipping is 0.008% at p90 against
0.011% in 0.1.11.

## The preview oracle became automatic in 0.1.13

`docs/PLAN.md`'s zero-flag criterion, met — and met differently from how the plan
described it. The plan proposed triggering on file class; the 42 pairs say the
trigger must be **the preview itself**.

Subject EV error against the camera's own JPEG, mean absolute:

| | n | preview | oracle off | automatic | frames worse |
|---|---:|---|---:|---:|---:|
| Sony A7C | 13 | 1616x1080 | 0.64 EV | **0.03 EV** | 0 |
| Expert RAW | 8 | up to 8160x6120 | 0.40 EV | **0.00 EV** | 0 |
| ProShot | 21 | 256x191 thumbnail | 0.31 EV | 0.31 EV, inert | 0 |

On files with a real preview the oracle is less an improvement than an
identity: the embedded preview *is* the rendering the vendor also wrote to the
paired JPEG. On ProShot a 256x191 thumbnail is not a rendering, and steering by
it made the median error worse — so `preview::MIN_PREVIEW_PIXELS` went from
30 000 to 250 000, which rejects it. That floor is the whole policy.

Sony is where the oracle earns its keep, and it is the source that had no
paired JPEGs until 0.1.13. Sony's own JPEG engine places the subject *below*
middle grey on most frames (-1.93 to +0.13 EV over the 13), so the
aim-the-median rule ran +0.49 EV bright at the median and +1.66 EV bright on the
low-key frame. That is the documented "renders night as day" failure, visible
for the first time on the main corpus rather than on one phone file.

Effect over the whole 310-file corpus: 287 files moved, every one of them
carrying a real preview; the other 23 are byte-identical to 0.1.12. Clipping and
near-white improve, because the oracle mostly darkens, and **no frame's crushed
fraction rose** — the noise floor and the fifth-percentile guard hold the black
point up even when a frame darkens by 3.5 EV.

## What the night and high-ISO pairs settled in 0.1.14

42 more Sony pairs spanning ISO 100 to 12800, including two 0.8-1.0 s night
frames, plus 16 ProShot. Three results, two of which are warnings about the
grading method itself.

**Daylight is done.** ISO 100-1600: subject EV mean absolute error 0.04-0.06 EV,
saturation ratio 0.94-1.10. There is nothing left to tune there with this corpus.

**Chroma noise was the largest remaining defect, and is now measured.** At ISO
1600+ the program rendered at 2.2x the camera's saturation. That was not colour:
a 1:1 crop shows red and green speckle where the camera has none. `src/chroma.rs`
now removes most of it — see `CHANGELOG.md` 0.1.14.

**The oracle deviation guard is right, and the metric that says otherwise is
wrong.** `MAX_ORACLE_DEVIATION_EV` binds on 5 of the 42 Sony pairs, all night or
high-ISO, holding us up to 3.17 EV brighter than the camera. The numbers call
that an error. The images say the opposite:

| | camera subject EV | ours | who is right |
|---|---:|---:|---|
| `_DSC1276` waterfall, ISO 2500 | -7.57 | -4.40 | **ours** — the camera's is nearly black |
| `_DSC1277` waterfall, ISO 8000 | -6.03 | -3.45 | **ours** |

Sony's metering underexposes these badly. Following it would have made the
frames unusable. **Do not loosen that guard on the strength of the subject-EV
metric.** More generally: the camera JPEG is a reference, not ground truth, and
the one scene class where the controller is known to fail is also the class
where the reference is least trustworthy. Always open the pair before acting on
a large delta.

## Are we actually beating the camera? A scorecard, and where we are not

Matching the camera is not the goal — if the output only matched, the camera's
own JPEG would do. So the corpus question is not "how close are we" but "on how
many frames do we keep more of the picture". Over the 98 pairs:

| axis | win | tie | lose | median ours | median camera |
|---|---:|---:|---:|---:|---:|
| highlights kept (`clipped_fraction`) | **87** | 10 | 1 | **0.0000%** | 0.5386% |
| shadows kept (`crushed_fraction`) | 26 | 68 | 4 | 0.0000% | 0.0000% |
| tonal detail (`luminance_entropy`) | 49 | 7 | 42 | ~7.52 | 7.5203 |
| local detail (`average_gradient`) | **56** | 0 | 42 | **4.83** | **4.59** |

(The entropy medians sit within 10^-4 bits of each other — on a 256-bin
histogram that row's W/T/L is close to a coin flip and should not be read as a
signal on its own.)

**Highlights are a rout in our favour and shadows are a draw**, both of which are
"lighting quality" in the sense that matters: information the camera threw away
and we kept. **Local detail flipped in 0.1.15**: it was the one systematic loss
(W31 L64, median 3.71 against 4.59) for the whole life of the corpus, because
every camera sharpens its JPEG output and this program did not at all.
`src/sharpen.rs` closed it without spending the highlight margin — but only
after a first attempt that guarded headroom on luminance instead of per channel
cost 20 highlight wins and was fixed. See `CHANGELOG.md` 0.1.15.

This table is the acceptance test for any change that claims to beat the camera.
Distance-to-camera is the wrong target for these four rows; win count is.

### 0.1.18: the regression is fixed and the owned path wins on hue

Two additions changed the picture, and both were about the program being able to
*see* what it was doing rather than about doing something new.

**A hue axis exists now.** `src/oklab.rs` plus a per-pixel comparison on a
canonical 512-px grid (`reference::compare_pixels`). The four standing axes cannot
see a hue shift, and `crushed_fraction` was structurally unable to favour the owned
path at all — post-`Calibrate` data has no negatives, so rawler can never crush by
that route while the owned path can only add zeros. Also new:
`measured.mean_saturation_highlight`, saturation restricted to pixels above 0.7
luminance.

**The `crushed_fraction` regression is gone.** One clamp: `tone.rs`'s chroma anchor
is floored at `black_output_linear` rather than 0, matching what `mapped_norm` was
already clamped to. Head to head on 108 pairs, owned against rawler:

| axis | before | after |
|---|---|---|
| `crushed_fraction` | 0 better, 87 same, **21 worse** | 0 better, **108 same**, 0 worse |
| `luminance_entropy` | 36 / 5 / 67 | 41 / 5 / 62 |
| `average_gradient` | **84** / 0 / 24 | **84** / 0 / 24 |
| `near_white_fraction` | 34 / 35 / 39 | 34 / 35 / 39 |

**And the owned path is closer to the camera's hue**: median 11.27° against
rawler's 12.09°, mean 20.61° against 21.04°. Read the mean and the restricted
cohort, not the corpus median — the clip only touches out-of-gamut pixels, so over
all frames the median change is 0.0000° while the best frame improves by 26.2°.
Over the 30 most out-of-gamut frames the median mean-hue improvement is **-0.905°**
with only 13 of 30 worse, and the six largest wins are all frames where the clip
would have rewritten 33-66% of pixels.

Two things that fell out of the measurement rather than being planned:

- **Our highlights are half again more saturated than the camera's** — ratio 1.53
  (rawler) to 1.62 (owned), against a whole-frame 1.05. The whole-frame figure was
  blind to it. This does *not* support the review's suspicion that the 1.20
  saturation multiplier compensated for Rawler's desaturation; it points the other
  way. Taste, not information — needs eyes, not a constant.
- **A noise-vs-colour split for negatives and a scene-linear gamut operator were
  both declined on evidence.** The residual entropy difference is uncorrelated with
  negatives (r = -0.16, -0.01) and mildly *positively* correlated with
  `above_one_fraction` (+0.16), median -0.000004 bits. And the gamut operator as
  originally proposed was impossible: "preserve hue **and luminance**" cannot map a
  luminance ≤ 0 pixel anywhere legal except black, so it would have reproduced the
  bug one stage earlier.

### Owning the rescale step, and where the default landed

**`--raw-color-path owned` is the default since 0.1.18**, on the scorecard above:
84/24 on local detail, a tie on crushed shadows, closer hue, and the only loss
(`luminance_entropy`, 41/62) at a median of -0.0000039 bits against a 7.5-bit scale
on a metric whose maximiser is histogram equalisation. `rawler` remains available as
the A/B control and for reproducing pre-0.1.18 output.

**`src/rescale.rs` owns black/white normalization, the demosaic ROI and the
default crop; `src/demosaic.rs` owns method selection and the guarded
interpolators.** Rawler PPG remains the mature fallback. Peak working set fell 25% on a
24 MP ARW (321 → 242 MB) and 11% on a 24.5 MP linear DNG when the owned path
removed `develop_intermediate`'s full-image clone.

`--sub-black` is a hidden three-way control, defaulting to `rawler-compat`, which is
**bit-identical** to `RawImage::apply_scaling` — proven by eight `f32::to_bits()`
tests and by 376/376 files with unchanged `analysis`/`parameters` and byte-identical
TIFFs. So owning the step moved nothing on its own; that was the point.

Four latent Rawler defects are closed in passing and **no real file trips any of
them** — hardcoded 2×2 black-level repeat, no `ActiveArea` anchoring of that
pattern, `as_bayer_array()` broadcasting `[0]` unless the length is exactly 4, and
odd dimensions leaving the last row or column at raw DN. `RescaleReport::notes`
fires if a future file does. Also settled: the A7C's four black levels are *equal*
(512, or 1024 at higher ISO), white level count 1 (15360), origin `(0,0)`, 6048×4024
— and every ARW in the corpus has even dimensions.

### `--sub-black preserve`: the case where the metrics were wrong

**Decided by looking, and the answer was the opposite of what the scorecard said.**
`preserve` is now the default.

The numbers argued against it: `crushed_fraction` worse on 18 of 108 frames (0
better), `mean_level` down by up to 14, worst-case entropy -0.42 bits and gradient
-2.24. On that evidence it sat behind the flag as "probably right but unlooked-at".

Looking settled it in one frame. **Clipping sub-black puts a magenta cast in the
shadows.** Rectifying the noise floor lifts each channel's mean above true black;
white balance then multiplies that pedestal by roughly 2.3 (red) and 1.6 (blue)
against 1.0 (green), and the residue is magenta. On `_DSC1253` — a night frame with
28.5% sub-black samples — the whole lower half of the image is tinted and the
darkest crop is a field of magenta speckle. `preserve` keeps the noise symmetric
about zero so it averages neutral, and the cast disappears. `_DSC1277` (ISO 8000)
shows the same as a red-brown haze over dark foliage. `_DSC1276` — same scene, ISO
2500, half the sub-black population — is near-identical either way, so the effect
scales with its cause and costs nothing where there is nothing to fix.

Structure is unchanged throughout: rock and leaf texture survive. What goes black is
noise that carried no detail and used to become coloured haze instead. **All three
metrics that argued against the change were describing the fix.** That is the
`average_gradient` trap in `docs/PLAN.md` — its maximiser is amplified noise — and
it is the clearest example this project has of why the grading session cannot be
replaced by a scorecard.

A magenta cast across the shadows of a night frame is what criterion 3 means by
*ruined*. It was inherited from Rawler's `Rescale` and has been present for the
whole life of the project, and **no axis on the scorecard could see it**.

### `crushed_fraction` and `clipped_fraction` were measured at the wrong precision

Found while chasing the above, and worth knowing as a class of bug. Our render is
measured in 16 bits; the camera's JPEG in 8. The tests were `== 0` and
`== u16::MAX` against `== 0` and `== 255` — so our side had to hit exactly 0/65535
where the camera's had to hit 0/255. Both now use display precision (`<= 128`,
`>= 65407`), matching what `output.rs` actually writes.

On this corpus it changed **nothing** — 94W/10T/2L and 40W/66T/0L identically, same
medians — because real renderings put few pixels in the affected bands. It matters
only where those bands are populated deliberately, which is exactly what `preserve`
does: its shadow cost reads as 18 frames under the corrected threshold against 1
under the old one.

The pre-existing agreement test covered five statistics and omitted these two, so it
could never have caught it. A second test now covers them.

### How that regression was diagnosed in 0.1.17 — and did not win then

`--raw-color-path owned` (see `CHANGELOG.md` 0.1.17 and
`docs/REVIEW-2026-07-30.md`) removes rawler's `Calibrate` clipping. Over 106
pairs, head to head against the default path on the same 108 files:

| axis | better | same | worse |
|---|---:|---:|---:|
| local detail (`average_gradient`) | **84** | 0 | 24 |
| highlights kept (`near_white_fraction`) | 34 | 35 | 39 |
| tonal detail (`luminance_entropy`) | 36 | 5 | 67 |
| shadows kept (`crushed_fraction`) | 0 | 87 | **21** |

One real win and **one real regression reported twice**: the crushed and entropy
rows have the same cause, correlated at `r = -0.81`, with the eight worst entropy
losses being the eight worst crush frames. Among the 75 frames with no
negative-luminance pixels the median entropy delta is -0.000004 bits.

**That was the 0.1.17 position. It has since changed** — see the section above; the
regression is fixed and the hue axis exists. The account below is kept because the
mechanism is the useful part and the trap generalises.

### Why removing the clip made shadows worse

Not what it looks like, and the first write-up of it here was wrong. The cause is
negative **luminance**, not negative channels:

`analyze::luminance` is a signed weighted sum, so a pixel with a large enough
negative channel has negative luminance. `tone::render_pixel_local` sets its
chroma anchor with `luminance(rgb).clamp(0.0, 1.0)`, so such a pixel anchors at 0
— and `compress_gamut`'s scale is then `anchor / (anchor - min)` = `0 / |min|` =
0, which multiplies every channel by zero. **The pixel collapses to pure black,
including its positive channels.** Rawler's clip zeroes only the offending channel
and leaves a dark saturated colour, so on this cohort the owned path is strictly
the more destructive of the two. `src/color.rs` has a test rendering both versions
of one such pixel.

A negative channel alone is harmless — the anchor is positive and the offending
channel simply lands on 0. The populations differ fourfold: the worst frame has
61.1% of pixels with a negative channel but 14.7% with negative luminance.
`clip_cost.negative_luminance_fraction` measures the one that matters and predicts
the regression exactly — 0 false positives and 0 false negatives over 108 frames,
crushed delta a median 0.883× the population.

**Most of that population is noise, not exotic colour.** The frames with the
largest negative populations are the noisiest in the corpus (`snr10_ev` +1.7 to
+2.0 EV, `p05` -5 to -7 EV). The sub-black clip happens per channel in *camera*
space, so shadow noise pinned at zero in one channel goes negative in the working
space the moment it passes a matrix with negative off-diagonals. Treat those as
noise, not as colour to be gamut-mapped, or the fix will turn black speckle grey.

(Since 0.1.18 that policy is **this program's own**, in `src/rescale.rs`, not
Rawler's. The automatic profile preserves sub-black samples; `--sub-black`
selects the diagnostic alternatives.)

### What that ruled in and out (all now resolved; kept for the reasoning)

**In**, in this order: (1) floor `compress_gamut`'s anchor at
`black_output_linear` — note `mapped_norm` is already floored there while
`mapped_luminance` is clamped to 0, an inconsistency worth fixing regardless; this
moves default-path output in deep shadows and so needs its own measured pass. (2)
Split negatives by magnitude against the fitted noise floor. (3) Only then a
scene-linear gamut operator with an explicit luminance ≤ 0 branch.

**Not** "gamut-compress preserving hue and luminance": every non-negative RGB
triple has non-negative luminance, so no luminance-preserving operator can map a
negative-luminance pixel anywhere legal except black. Written that way the fix
reproduces the bug one stage earlier.

The DNG matrix mechanism described above is now in. Creative DCP tables remain
out because they are a rendering look rather than the colorimetric transform
needed at this stage.

`--working-space rec2020` cuts out-of-range pixels from a median 0.166% to 0.053%
(max 66.2% → 52.2%) and improves the highlight row head to head to 56 better / 47
worse — read that in preference to its against-camera row, which barely moves. It
leaves `crushed` at 0/87/21: a camera's gamut is not a triangle, and this cohort
is mostly noise anyway.

### Two things this scorecard cannot tell you

**The `crushed` row is structurally biased against the owned path.**
Post-`Calibrate` data has no negatives, so the rawler path can never crush by this
route while the owned path can only add zeros — that row was ≤ before a single
frame was rendered. And the owned path's actual benefit, out-of-gamut and shadow
hue fidelity instead of rawler's silent hue shift, is invisible to all four axes.
A hue-angle delta restricted to the out-of-gamut cohort would be the honest
instrument; until one exists the gate favours rawler for structural reasons.

**The 1.20 saturation multiplier is exonerated only globally.** The ratio against
the camera moves 1.046 → 1.050 → 1.052 across the three arms, so the constant
needs no change — but `mean_saturation` averages the whole frame while the
clipping's desaturation lives in the above-1.0 cohort, a median 0.009% of pixels.
A whole-frame mean is nearly blind to it at that size. Whether 1.20 was
compensating *in highlights* needs saturation measured on that cohort alone, which
nothing currently reports. Exposure decisions did move by a median of 0.0000 EV
over 376 files, which is a clean result.

### Two scorecards from now on, not one

Since 0.1.17 the summary splits every reference measure by `guidance_mode`, and
`--no-preview` renders the independent arm. A pooled median cannot tell the
controller getting better at judging scenes from more files happening to carry a
usable preview. On the current corpus the split is 39 independent / 337
preview-guided; the independent arm sits at a +0.04 EV median key delta against
the preview-guided arm's +0.02. Report both, or the number means less than it
looks like.

### Local tone does not close the gap — it makes things worse

`docs/PLAN.md` left the fate of `--local-tone` to "after the corpus visual
pass". The corpus now exists, and the answer is that it should stay off:

| axis | off | `--local-tone 0.5` | `--local-tone 1.0` |
|---|---|---|---|
| highlights kept | W86 T11 L1 | W88 T10 L0 | W88 T10 L0 |
| tonal detail | W46 T9 L43 | W25 T5 L68 | **W7 T4 L87** |
| local detail | W31 T3 L64 | W34 T2 L62 | W37 T2 L59 |

Median luminance entropy falls 7.5204 -> 7.3143 -> 7.0716 while median gradient
rises only 3.71 -> 3.84 -> 3.93, still far short of the camera's 4.59. It trades
a large, measurable loss of global tonal distribution for a small local gain —
the classic flat "HDR look" — and the 12 EV frames confirm it by eye. The
operator is not broken; it is solving the wrong problem for this corpus.

### The chroma veil at extreme ISO — closed in 0.1.15

The 0.1.14 denoiser's documented residual — low-frequency chroma blotching wider
than its window — read as a magenta veil over the ISO 65535 dusk frames, and
against those the camera's JPEG was plainly better. It was never reachable by a
bigger box (that smears colour across real edges); it needed a prior, and
luminance is the right one. 0.1.15's second stage in `src/chroma.rs` — a fast
guided filter over the colour differences with the frame's own luminance as
guide, 128 px support — takes the ISO 8000+ saturation ratio 1.37 → 1.27 and
removes the veil visibly; what remains at 1:1 is monochrome grain, which is the
design intent (luma noise is deliberately untouched). The residual is now
regularisation-limited, not support-limited: widening 48 → 256 px changes
nothing.

## Local tone first pass in 0.1.11

`--local-tone <0..1>` is now available, off by default. It selects one of nine
full-resolution Gaussian surrounds per pixel using the Reinhard contrast test
reproduced by Zhang and Feng. Rather than replacing the established global
render with `L/(1+V)`, the surround becomes a bounded, median-anchored local EV
offset: +1.0 EV maximum lift, -0.75 EV maximum compression, with a 0.15 EV dead
band. Sensor-noise and isolated-bright-detail guards reduce two predictable
failure modes.

The implementation is deterministic and `--local-tone 0` is byte-identical to
the old path. It is still an experimental photographic choice, not a new
default. The Gaussian pyramid's full-resolution floating-point buffers used to
mean `--jobs 1` on large files; since 0.1.19 the automatic job count budgets for
the chroma stage, which peaks higher still, so it needs no separate allowance.

At maximum strength, 268/268 available files rendered successfully (258 A7C
ARWs and 10 Samsung DNGs). Peak resident memory was 2.05 GiB on the largest
phone DNG; the whole one-worker pass took 5m21s. No correction exceeded its
declared bound. On the Sony subset:

| | local off | `--local-tone 1` |
|---|---:|---:|
| near-white median | 0.35% | **0.0028%** |
| near-white p90 | 1.86% | **0.095%** |
| near-white max | 8.14% | **6.69%** |
| frames above 10% near-white | 0 | 0 |
| colourfulness median | 36.66 | 35.62 |
| mean level median | 118.0 | 116.7 |

The maximum-strength pass is a safety bound, not the recommended look.
`--local-tone 0.5` is the sensible starting point for visual evaluation. The
night phone outlier remains extremely noisy both with the feature off and on;
local tone reduced its near-white/clipped fractions but does not substitute for
the missing denoiser.

## What is deliberately not done

- **No semantic local control _by default_.** Local tone has no face, sky, skin
  or subject awareness and does not reproduce a phone's multi-frame image
  pipeline. Since 2026-08-13 an opt-in `--semantic` path does derive scene masks,
  but it is observational by construction — it writes masks and region evidence
  to the sidecar and leaves output pixels untouched. Its two experimental
  operators, `--semantic-sky-highlights` and `--semantic-sky-chroma`, both
  default to 0 and require `--semantic`.
- ~~**No luma denoising.**~~ **Done 2026-08-13** (`src/luma.rs`). Automatic and
  noise-model driven like the chroma filter, it engages only on frames noisy
  enough to need it — roughly ISO 1600 and up — so clean frames render
  identically either way. `--luma-denoise 0` disables it. This reverses the
  earlier position that luma grain was left alone because it is the part that
  destroys texture: on the ISO 12800 night pairs the measured result was
  markedly reduced grain with cloud and edge detail preserved. The regression
  gate holds — with `--night-tone 0 --luma-denoise 0` the changed binary
  reproduces the pre-change default byte-for-byte.
- **No guessed lens profiles.** Standard DNG lens opcodes, hot/dead CFA
  suppression, chroma denoising and output sharpening are automatic.
  Proprietary RAWs without portable lens coefficients are left geometrically
  unchanged.
- **Night rendering is automatic since 2026-08-13.** `--night-tone` (default on)
  applies an edge-aware single-frame local tone map to frames the raw statistics
  detect as low-light, compressing bright light sources and locally lifting
  shadows without globally raising exposure. It is pixel-inert on daylight
  frames and is ignored when `--hdr` or `--local-tone` is set explicitly.
- ~~**No EXIF/ICC in the output.**~~ **Done in 0.1.17.** All three formats carry
  the copied EXIF and a generated sRGB v2 ICC profile; `--no-metadata` restores
  the old bytes exactly. Two gaps remain deliberately: MakerNotes are not copied
  (vendor blobs carry absolute file offsets, so relocating them corrupts them),
  and PNG's `eXIf` chunk is ignored by many readers including Windows' own — use
  JPEG or TIFF if a library has to see the tags.
- **White balance is as-shot only** unless `--local-white-balance` is passed.
  That flag is off by default for a reason: unguarded, the method renders a
  campfire green, because a fire is the brightest thing in frame and gets read
  as a colour cast to remove.

## Things that will bite the next person

**Rawler 0.7.2 silently mis-decodes some valid DNGs.** It ignores the JPEG `DRI`
marker and never resynchronizes on `RSTn`, so any lossless-JPEG stream using
restart intervals decodes to a smooth diagonal ramp with no scene content — and
reports success. `src/ljpeg.rs` replaces that decoder; `src/redecode.rs` decides
when to substitute it. If a new camera produces garbage that looks like a
gradient, this is why. `cargo run --release --example probe-decode -- file.dng
dump.png --raw` shows what Rawler alone produced.

**Do not locate embedded previews by scanning for JPEG markers.** Samsung
appends a single-channel gain map after the preview inside the same strip, and
the preview embeds a 512x384 EXIF thumbnail whose end-of-image marker comes
*first*. Both traps are documented in `src/preview.rs`; use the TIFF directory.

**`tiff.root_ifd().find_ifds_with_tag(...)` misses IFDs.** `root_ifd()` is only
the first IFD of the chain. The `TiffReader` *trait* method on the reader is the
one that sees chained IFD1 and Sony's preview IFD.

**Sort IFD candidates explicitly.** `find_ifds_with_filter` walks `sub_ifds()`,
a `HashMap` whose iteration order varies per process. Relying on the order it
returns makes output non-deterministic — which this crate promises not to be.

**The same trap is live inside rawler's own `Calibrate`.** It selects its
calibration matrix with `find(D65).or_else(|| color_matrix.iter().next())`, and
that fallback walks a `HashMap` — so a file carrying several calibration matrices
and no D65 one develops differently from one process to the next, on the default
colour path. No corpus file is affected (all 376 carry a D65 matrix) and a
`--dry-run --summary` now counts and warns about any that would be, via
`nondeterministic_illuminant_files`. `--raw-color-path owned` sorts explicitly and
has no such case. If a new camera ever trips the warning, develop it on the owned
path.

**Removing a clip can be worse than keeping it, if a downstream stage was relying
on the clip's guarantee.** 0.1.17's owned colour path stops clipping out-of-gamut
negatives, and `crushed_fraction` got *worse* on 21 frames — because
`tone::render_pixel_local` clamps its chroma anchor to `[0, 1]`, and a
negative-luminance pixel therefore anchors at 0, which makes `compress_gamut`'s
scale exactly 0 and zeroes every channel including the positive ones. Rawler's
per-channel clip had been quietly guaranteeing a non-negative luminance. Whenever a
"stop throwing data away" change scores worse, look for the stage downstream that
was depending on the discarded property — and check the *sign* of any signed sum
it takes.

**`luminance` is signed and several call sites clamp it.** `analyze::luminance` is
`0.2126r + 0.7152g + 0.0722b` with no absolute value, so it goes negative on
out-of-gamut input. Every `.max(1e-8)` and `.clamp(0.0, 1.0)` applied to it hides
that sign rather than handling it, so treat any such clamp as a place the sign
was hidden rather than handled.

The specific inconsistency this entry used to name — `mapped_norm` floored at
`black_output_linear` while `mapped_luminance` was floored at 0 — **has been
fixed**. Both now floor at `black_output_linear`, so the curve and the chroma
anchor agree about where black is; see the comment above the
`mapped_luminance` binding in `src/tone.rs`. The general warning stands, the
worked example no longer does.

**`preview::MIN_PREVIEW_PIXELS` is a policy value, not a sanity check.** Since
0.1.13 it is the sole thing deciding which files the automatic exposure oracle
acts on. Lower it and thumbnails start steering exposure; raise it and real
previews get thrown away. There is a regression test pinning it inside the gap
between the two, and a comment recording the measured sizes.

**A file with no usable preview gets no vendor opinion at all.** ProShot is the
one such source here, so it is also the one where an exposure-controller change
shows up undiluted. Test controller changes on ProShot; test oracle changes on
Sony and Expert RAW.

**Chroma noise reads as saturation.** `mean_saturation` cannot tell coloured
speckle from colour, so a high-ISO frame scores as more saturated than a clean
one of the same scene. That is what made the ISO 8000+ band measure 2.2x the
camera before 0.1.14. When a saturation number looks too good or too bad, check
whether `chroma_denoise` is present in the sidecar and look at a 1:1 crop.

**`near_white_fraction` counts pixels with *any* channel near white**, so a
saturated sky reads as near-white without a single blown pixel. Since 0.1.12
raised saturation, this statistic is no longer a proxy for "blown". Use
`clipped_fraction` for information definitively lost, and check luminance
entropy and average gradient before concluding that highlights were flattened.

**Anything applied after the tone curve has to respect the curve's headroom.**
`highlight_norm == 1.0` works by landing the brightest channel just under the
white point; any later per-pixel operation that expands around luminance —
saturation today, a sharpener or local operator tomorrow — will spend that
margin and reintroduce clipping. 0.1.12's cap in `tone::render_pixel_local` is
the pattern to copy — and the margin must be measured **per channel**, not on
luminance: 0.1.15's sharpener guarded on luminance first and turned a W86 L1
highlight rout into W73 L21 before the per-channel fix restored it.

**Determinism is a product property**, stated in `Cargo.toml`. Anything with a
random seed, or that depends on batch composition or thread scheduling, does not
belong in the render path. `--pool-noise` deliberately breaks batch
independence and says so; `--noise-profile` is the reproducible route.

### Harmonic highlight diagnostics

The Harmonic production path now has one raw-domain joint log-chromaticity
estimator across the complete clipped component. The older post-demosaic
`HCHROMA` / `transport_spatial_chromaticity` path remains available only as an
internal ablation and is not a production-stage diagnostic for Harmonic.

`--dump-stages` writes a post-sharpen `-final.png` checkpoint. Its decoded RGB16
pixels are byte-identical to the normal default output; PNG container metadata
may differ. Use this checkpoint for the final identity comparison because
`gamut-post` precedes output sharpening and therefore is not the encoded result.

## Verification you should re-run after any change

```bash
# 1. Gate: output must not move when new features are off.
raw-autotune raw/arw raw/raw_old --dry-run --summary after.json
# compare against a before.json field by field, ignoring elapsed_ms

# 2. Tests and lints.
cargo fmt --check
cargo test --release
cargo clippy --all-targets --release

# 3. Determinism: same input, many runs, --jobs 8, byte-identical sidecars.
#    Compare images byte for byte; sidecars record their own output path, so
#    normalise that before diffing runs written to different directories.

# 4. Grade against the camera's own JPEGs. Run this whenever anything in the
#    render path or the exposure controller moves.
raw-autotune raw/arw_better raw/raw_better --output out --format jpeg \
  --reference --summary out/summary.json
tools/contact-sheet.py out/summary.json --output out/sheet.html

# 4b. Since 0.1.17, grade the two guidance arms separately — a pooled median
#     cannot tell a better controller from more files carrying a preview.
raw-autotune raw/arw_better raw/raw_better --output out-indep --format jpeg \
  --no-preview --reference --summary out-indep/summary.json
tools/contact-sheet.py out/summary.json --guidance independent \
  --output out/sheet-independent.html

# 5. Re-run the colour-path A/B after anything touching colour, the tone curve
#    or the gamut handling. Since 0.1.18 `owned` should TIE rawler on
#    crushed_fraction (0/108/0), win average_gradient (84/24) and beat it on
#    hue. If crushed_fraction goes negative again, the black_output_linear
#    floor on tone.rs's chroma anchor has been lost.
raw-autotune raw/raw_3rd_batch --output out-owned --format jpeg --reference \
  --raw-color-path owned --summary out-owned/summary.json
```

**Read the hue axis with the right statistic.** The clip only touches out-of-gamut
pixels, a minority of most frames, so a large improvement there barely moves a
corpus median: over all 106 pairs the median hue change between the two colour
paths is 0.0000° while the best frame improves by 26.2° and the worst worsens by
0.14°. Use `hue_mean_degrees`, and restrict to the frames with a large
`clip_cost.altered_fraction` when asking whether a colour change helped.

Step 1's baseline needs a binary from the previous release. `git archive HEAD
Cargo.toml Cargo.lock rust-toolchain.toml src | tar -x -C /d/bl` and build there
— a `git worktree` fails on this repo because the filenames under
`research/original-pdfs-*` exceed Windows' path limit.

Step 4's acceptance thresholds, from the current corpus:

| | expect |
|---|---|
| `reference_saturation_ratio.median`, daylight | 0.94 to 1.10 |
| `reference_saturation_ratio.median`, ISO 8000+ | 1.35 to 1.45 |
| `reference_highlight_saturation_ratio.median` | about 1.53, and **not** a target to drive to 1.0 — see below |
| `reference_hue_median_degrees`, whole corpus | about 12°, lower is closer to the camera |
| centre-weighted key EV mean abs error, daylight, source with a preview | below 0.10 EV |
| centre-weighted key EV mean abs error, ProShot | around 0.27 EV |
| centre-weighted key EV mean abs error, night / ISO 1600+ | about 1.5 EV, **and correct** |
| `clipped_fraction` median | 0.000%, and always below the camera's |
| frames whose `crushed_fraction` rose, default colour path | 0 |

Several of those rows are deliberately not 1.0 or 0.0. The high-ISO saturation
ratio (0.1.14) is residual low-frequency chroma noise the denoiser cannot reach.
The night exposure error is the oracle deviation guard refusing a camera rendering
that is itself wrong — see below. The highlight saturation ratio (0.1.18) is a
divergence in *taste*: the camera desaturates its shoulder hard and this program
deliberately does not, so 1.53 is a fact about two rendering intents, not an error
with a fix. **None of them is a target to drive to zero**, and treating any of them
as one would make output worse. The hue row is the one where lower genuinely is
better, and even there the median is the wrong summary — see the note above.

The gate test caught a real regression during 0.1.9 development and is worth
keeping as the first thing you run. Step 4 is newer and has now caught two: the
headroom interaction in 0.1.12, and the fact that a thumbnail is not a preview
in 0.1.13.

## Corpus reality check

The test material is thinner than the file count suggests:

| Class | Files | Paired JPEG | Notes |
|---|---|---|---|
| A7C ARW | 321 | 63 | one photographer, one body, ISO 100–12800 |
| ProShot | 39 | 35 | true CFA; thumbnail-only preview |
| S24+ Expert RAW | 9 | 8 | JPEG-XL, `BaselineExposure` +2 or +3 |
| S24+ Pro mode | 7 | 0 | `LinearRaw`, lossless JPEG w/ restart intervals |

106 of 376 files now have a paired camera JPEG, across all three target sources
and ISO 25 to 12800. Those pairs have settled five questions the project had
been guessing at — chroma level, when to trust a vendor preview, whether the
oracle guards are too tight (all three rails now have paired evidence, and the
ceiling needed fixing), and how much of high-ISO "saturation" is noise — and
are what every default should be re-checked against from here.

`camera-promode1.dng` is an outlier worth knowing about: p50 at +2.35 EV with
only 3.92 EV of range, i.e. crammed against the top and clipped. It behaves
unlike the other six Pro-mode files, which sit at −1.7 to −5.1 EV like ordinary
scene-linear raw.

The pairs were shot in three sessions in one place, on a tropical island. Night,
near-dark, high-ISO, beach and high-variance classes all arrived in 0.1.14 and
were what drove that pass. Still missing, in rough order of value:

1. ~~**A genuinely high-key frame with a pair.**~~ **Delivered and acted on in
   0.1.16** — eight bright-scene pairs including a white wall in direct sun,
   the first camera subjects above +1 EV. The ceiling was wrong and is now
   corroboration-aware; see below and `CHANGELOG.md` 0.1.16. Snow remains
   unobtainable and would still be worth a pair when travel allows: the
   brightest corroborated camera subject seen so far is +1.63 EV, and the new
   +2.0 corroborated bound has itself never been tested against a real scene.
2. **Backlit and high-dynamic-range interiors.** 0.1.13's one visible trade is
   here — the oracle recovers a window and loses the room — and there is still
   no paired evidence to say which answer is right.
3. **A second body or lens.** Every A7C frame is one photographer, one body.
   Two sensors now agree on the chroma constant, which is the strongest evidence
   available that it is not a per-camera fit, but both are consumer sensors
   rendered by their vendor's own engine.
4. Indoor/mixed light, people, deliberately clipped, deliberately underexposed.

Several tuning constants are set from very few data points and are flagged as
such in `docs/KNOWN_LIMITATIONS.md` and `docs/RESEARCH_NOTES.md`:
`whitebalance::MAX_ILLUMINANT_CAST` (two data points) and `analyze`'s preset
table, of which only `highlight_norm` (0.1.10) and `saturation` (0.1.12) have
been swept.

### The oracle ceiling: suspected wrong, then proven wrong, fixed in 0.1.16

`analyze::ORACLE_TARGET_CEILING_EV` (+1.0) bound on 16 of 287 preview files
with no paired evidence either way; this section used to say the test would be
"a white wall in direct midday sun". 0.1.16 got exactly that pair, and it
proved the suspicion: camera subject +1.63 EV, unclipped, our analyzer calling
the raw `high_key` — and the clamp holding the render 0.42 EV darker than the
camera.

The fix keeps the guard's reasoning where it is sound. A bright preview that
the raw statistics *contradict* is still suspect and still clamps to +1.0. One
the raw statistics corroborate raises the ceiling by up to +1 EV, ramped on the
key score from the high-key threshold (0.32) to 0.60 —
`analyze::oracle_target_ceiling_ev`. The 16 bound files split exactly along
that line: ten high-key files freed, six uncorroborated files still clamped.

Fixing it exposed a second error: the oracle's target inversion runs through
the pre-oracle curve, and one re-solve does not converge on the shoulder
(`_DSC1291` still sat 0.24 EV low with the ceiling gone). The re-solve now
iterates — **brighter only**. Iterating both directions moved 131 corpus files
and drove the night frames toward the vendor renderings the 0.1.14 pairs
proved wrong; the single solve's undershoot had been silently protective
there. Dark-side behaviour is bit-for-bit unchanged.

All three rails now have paired evidence: the floor fired once,
`MAX_ORACLE_DEVIATION_EV` fired on five night frames where it was *correct* to
fire, and the ceiling was wrong for corroborated bright scenes and is fixed.
Net effect measured over the corpus: 42 of 376 files move, all brighter, the
bright pairs land at 0.00 EV median subject delta, and no dark frame moves at
all.

## Where the detail lives

- `docs/RESEARCH_NOTES.md` — which parts of the papers in `research/` apply,
  and which do not. Read before implementing anything from them.
- `docs/KNOWN_LIMITATIONS.md` — honest limits, per subsystem.
- `docs/BUILD_STATUS.md` — what has been built and verified, and on what.
- `CHANGELOG.md` — every change with the measurement that justified it.
