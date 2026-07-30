# Status and handoff — raw-autotune 0.1.14

Updated 2026-07-28. Scope as stated: the program has to work on **Samsung S24+
Expert RAW**, **ProShot** output, and **Sony A7C ARW**. This records how far
that is, what to run, and what is left.

## Short answer

**All three targets work on defaults. There are no per-source flags left.** The
A7C highlight defect in the original handoff was fixed in 0.1.10, the chroma
deficit in 0.1.12, and the Expert RAW exposure flag was removed in 0.1.13 when
the preview oracle became automatic.

| Target | Files on hand | Paired JPEG | State | Invocation |
|---|---|---|---|---|
| A7C ARW | 313 | 55 | **Works.** 313/313 render, 0 failures | defaults |
| ProShot DNG | 39 | 35 | **Works.** 39/39 render | defaults |
| S24+ Expert RAW | 9 | 8 | **Works.** 9/9 render | defaults |
| S24+ Pro mode | 7 | 0 | **Works.** 7/7 render | defaults |

368 of 368 files render with zero failures on a single no-flag invocation, which
is `docs/PLAN.md` §1's first criterion for "usable". 98 of them have a paired
camera JPEG, spanning ISO 25 to 12800.

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
  --output out --format jpeg --jobs 3

# Optional, recommended: pool sensor noise per ISO first. The scan takes about
# 13 seconds over 258 files; the profile is reusable and keeps output identical
# whether a file is developed alone or in a batch.
raw-autotune raw/arw --noise-scan sony.json
raw-autotune raw/arw --output out --noise-profile sony.json

# To reproduce pre-0.1.13 exposure, switch the preview oracle back off:
raw-autotune raw/arw --output out --format jpeg --preview-exposure 0
```

Keep `--jobs` at 3 or below when the batch contains 50-megapixel Expert RAW
files. `--jobs 8` over the full corpus is killed by the OOM killer on a 31 GiB
machine, and has been since before 0.1.12 — this is not caused by any recent
feature, and `docs/PLAN.md` §3's automatic `--jobs` item is the fix.

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

`docs/PLAN.md` §3's first item, done — and done differently from how the plan
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

### Local tone does not close the gap — it makes things worse

`docs/PLAN.md` §3 left the fate of `--local-tone` to "after the corpus visual
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
default. Use `--jobs 1` on large files because the Gaussian pyramid uses
multiple full-resolution floating-point buffers.

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

- **No semantic local control.** Local tone has no face, sky, skin or subject
  awareness and does not reproduce a phone's multi-frame image pipeline.
- **No luma denoising, lens correction or hot-pixel pass.** (Chroma denoising
  and output sharpening are automatic since 0.1.14/0.1.15; luma noise is left
  alone deliberately — it is the part that destroys texture, and the camera's
  own high-ISO JPEGs are visibly mushier than ours.)
- **No EXIF/ICC in the output.** TIFF and PNG are 16-bit RGB with no profile;
  consumers should assume sRGB.
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

## Verification you should re-run after any change

```bash
# 1. Gate: output must not move when new features are off.
raw-autotune raw/arw raw/raw_old --dry-run --summary after.json
# compare against a before.json field by field, ignoring elapsed_ms

# 2. Tests and lints.
cargo fmt --check
cargo test --release        # 103 tests
cargo clippy --all-targets --release

# 3. Determinism: same input, many runs, --jobs 8, byte-identical sidecars.
#    Compare images byte for byte; sidecars record their own output path, so
#    normalise that before diffing runs written to different directories.

# 4. Grade against the camera's own JPEGs. Run this whenever anything in the
#    render path or the exposure controller moves.
raw-autotune raw/arw_better raw/raw_better --output out --format jpeg \
  --reference --summary out/summary.json
tools/contact-sheet.py out/summary.json --output out/sheet.html
```

Step 4's acceptance thresholds, from the current corpus:

| | expect |
|---|---|
| `reference_saturation_ratio.median`, daylight | 0.94 to 1.10 |
| `reference_saturation_ratio.median`, ISO 8000+ | 1.35 to 1.45 |
| subject EV mean abs error, daylight, source with a preview | below 0.10 EV |
| subject EV mean abs error, ProShot | around 0.27 EV |
| subject EV mean abs error, night / ISO 1600+ | about 1.5 EV, **and correct** |
| `clipped_fraction` median | 0.000%, and always below the camera's |
| frames whose `crushed_fraction` rose | 0 |

Two of those rows are deliberately not 1.0 or 0.0, and both were established in
0.1.14. The high-ISO saturation ratio is residual low-frequency chroma noise the
denoiser cannot reach. The night exposure error is the oracle deviation guard
refusing a camera rendering that is itself wrong — see below. **Neither is a
target to drive to zero**, and treating them as one would make output worse.

The gate test caught a real regression during 0.1.9 development and is worth
keeping as the first thing you run. Step 4 is newer and has now caught two: the
headroom interaction in 0.1.12, and the fact that a thumbnail is not a preview
in 0.1.13.

## Corpus reality check

The test material is thinner than the file count suggests:

| Class | Files | Paired JPEG | Notes |
|---|---|---|---|
| A7C ARW | 313 | 55 | one photographer, one body, ISO 100–12800 |
| ProShot | 39 | 35 | true CFA; thumbnail-only preview |
| S24+ Expert RAW | 9 | 8 | JPEG-XL, `BaselineExposure` +2 or +3 |
| S24+ Pro mode | 7 | 0 | `LinearRaw`, lossless JPEG w/ restart intervals |

98 of 368 files now have a paired camera JPEG, across all three target sources
and ISO 25 to 12800. Those pairs have settled four questions the project had
been guessing at — chroma level, when to trust a vendor preview, whether the
oracle guards are too tight, and how much of high-ISO "saturation" is noise —
and are what every default should be re-checked against from here.

`camera-promode1.dng` is an outlier worth knowing about: p50 at +2.35 EV with
only 3.92 EV of range, i.e. crammed against the top and clipped. It behaves
unlike the other six Pro-mode files, which sit at −1.7 to −5.1 EV like ordinary
scene-linear raw.

The pairs were shot in three sessions in one place, on a tropical island. Night,
near-dark, high-ISO, beach and high-variance classes all arrived in 0.1.14 and
were what drove that pass. Still missing, in rough order of value:

1. **A genuinely high-key frame with a pair.** The only untested guard left is
   `ORACLE_TARGET_CEILING_EV`; the brightest camera subject in the whole corpus
   is +0.99 EV, and the ceiling does not bind until well past that. Snow is the
   classic test and is not obtainable here — a white wall in direct midday sun,
   or bright dry sand shot to the right, would do as well.
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

### The oracle ceiling is firing, and nothing validates it

`analyze::ORACLE_TARGET_CEILING_EV` is +1.0, on the reasoning that "a vendor
rendering a scene more than a stop above middle grey is far more likely an
HDR-fused or blown preview". Now that the oracle is automatic, that guard binds
on **16 of the 287 files that have a preview** — it is live policy, not a
theoretical backstop, and it makes us render those frames 0.1 to 1.1 EV darker
than the camera did.

The evidence points at it being wrong more often than right. Ten of the sixteen
are independently classified `high_key` by our own analyzer, so a bright preview
is corroborated by the raw statistics rather than contradicted by them; these
look like genuinely bright scenes, not broken previews. **None of the sixteen
has a paired JPEG**, so this is a suspicion, not a finding.

**0.1.14's 58 new pairs did not test it.** They include beach and high-variance
scenes, but the brightest camera subject among them is +0.99 EV, and the ceiling
does not bind until well past that. It remains the one guard with no paired
evidence either way. The test is a white wall in direct midday sun, or bright
sand exposed to the right — anything whose camera JPEG genuinely lands above a
stop over middle grey.

The other two rails now have evidence, and it says leave them alone. The floor
fired once, and `MAX_ORACLE_DEVIATION_EV` fired on five night frames where it
was *correct* to fire — see "What the night and high-ISO pairs settled" above.

## Where the detail lives

- `docs/RESEARCH_NOTES.md` — which parts of the papers in `research/` apply,
  and which do not. Read before implementing anything from them.
- `docs/KNOWN_LIMITATIONS.md` — honest limits, per subsystem.
- `docs/BUILD_STATUS.md` — what has been built and verified, and on what.
- `CHANGELOG.md` — every change with the measurement that justified it.
