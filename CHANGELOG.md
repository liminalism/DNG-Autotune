# Changelog

## 0.1.19 — the batch sizes itself

### `--jobs` defaults to `auto`

`src/memory.rs` decides how many files to hold in flight from the memory the
operating system reports free and the size of the largest input, and the run
header says what it decided on:

```text
raw-autotune v0.1.19 | 376 file(s) | preset=auto | concurrent images=5 \
  (auto: 22.76 GiB available, 2.67 GiB per image at 49.9 MP)
```

This closes `docs/PLAN.md` criterion 5, which was down to this one gap: a tool
for unattended archiving cannot ship documentation reading "keep `--jobs` at 3 or
below when the batch contains 50-megapixel files" and "`--jobs 8` over the full
corpus is killed by the OOM killer on a 31 GiB machine". That is a per-source
flag wearing a different name, and the program has every number it needs to
work it out itself.

**What one image costs.** Peak resident set per file, measured with
`/usr/bin/time -v` at 0.1.18 with every optional operator forced on
(`--chroma-denoise 2 --local-tone 1`):

| frame | pixels | peak RSS | bytes/pixel |
|---|---|---|---|
| `_DSC0883.ARW` | 10.5 MP | 417 MiB | 41.6 |
| `20260729_114901.dng` | 12.5 MP | 479 MiB | 40.2 |
| `_DSC1236.ARW` | 24.3 MP | 913 MiB | 39.3 |
| `_DSC1250.ARW` (ISO 12800) | 24.3 MP | 1216 MiB | **52.4** |
| `20260728_114800.dng` | 49.9 MP | 1882 MiB | 39.5 |

The spread is the interesting part, and it is not resolution. `20260728_114335`
and `20260728_114339` are the same camera, the same dimensions and the same
20,137,212 bytes on disk, and they peak at 215 MiB and 426 MiB respectively,
reproducibly. The difference is `chroma::apply`, which allocates a
full-resolution `chroma` (12 B/px), `luma` (4 B/px) and `scratch` (12 B/px) plus
the guided stage's own planes — and which runs only on frames noisy enough to
need it, which is a property of the picture and cannot be known before the file
is decoded. So the budget assumes the noisy path on every frame. `--local-tone`
turns out to need no separate allowance: on the same frame it peaks at
38.4 B/px against chroma's 52.4, so covering chroma covers it.

The constant is 56 B/px against a measured worst case of 52.4, plus 64 MiB of
per-file overhead against a fitted 26 MiB. Both are rounded away from the
measurement on purpose: overestimating costs one worker, underestimating kills a
batch two hours in.

**How the size of each input is known before it is opened.** `memory::raw_pixels`
reads the TIFF directory and takes the largest IFD declaring a CFA or LinearRaw
photometric interpretation — no pixel data, no decode, well under a second for
the whole corpus. `tests/probe_dimensions.rs` checks it against what the decoder
finds on every RAW under `raw/`: **376 of 376, worst overshoot 1.0000x**. The
test asserts the two bounds that matter rather than equality — the probe may
never come in under the decoded frame, and may not overshoot it by more than
10% — because the directory declares the stored frame and a source with a wider
border would legitimately overshoot. A file whose directory cannot be read falls
back to its size on disk at an assumed 1 pixel per byte, roughly twice the
corpus's real 2.0 bytes per pixel, which is the safe direction.

**What it chooses.** The batch is sized on the *largest* input, because workers
pull from a shared queue and the five biggest files can be in flight together.
70% of available memory is the budget; the rest absorbs `MemAvailable` being an
estimate that moves, the batch's own output and page cache, and the assumption
that concurrent workers peak simultaneously. The processor count is a hard
ceiling — per-image stages already use every core. On this 31 GiB machine with
22.8 GiB free, the full 376-file corpus plans 5 workers.

The budget is deliberately looser than what a run actually uses: those 5 workers
are budgeted 13.3 GiB, and rendering all 376 files to JPEG peaked at **5.80 GiB**
in 2 min 32 s. Both gaps are the point — the 50-megapixel frames sized the budget
but are 13 of 376 and rarely in flight together, and the frames that need the
chroma stage are a different subset again. Sizing for the case where they
coincide is what makes the run survivable unattended, which is the criterion this
closes.

Memory is read from `MemAvailable` on Linux and `GlobalMemoryStatusEx` on
Windows (declared inline; `windows-sys` would be a dependency tree for one
struct and one function). A platform that answers neither gets one worker, which
is what this program did for its whole life until now.

`--jobs N` still means exactly N. Nothing here changes what is rendered: the
whole-corpus `--dry-run --summary` over 376 files is identical field-for-field
to a 0.1.18 baseline with `elapsed_ms` ignored, and 24 JPEGs spanning all four
sources are byte-identical between `--jobs 1` and `--jobs auto`.

## 0.1.18 — the scorecard learns to see colour, and the regression is fixed

### Relicensed MIT → AGPL-3.0-or-later

Deliberate, and it removes a constraint rather than adding one. This program lives
among GPL-family RAW tooling, and being permissively licensed meant the best
available implementations of things it needs were legally off limits while being
technically ideal. `docs/REVIEW-2026-07-30.md` rejected RCD/AMaZE demosaic on
exactly those grounds — "porting is a licence decision, not an engineering one".
That objection is now void: RawTherapee's and darktable's GPL-3 algorithms are
usable, and the demosaic question returns to the engineering queue on its merits
(where it is still not the current bottleneck, so it stays deferred). The zenraw
verdict is likewise no longer licence-blocked, though it never rested on the licence
and does not change.

`LICENSE` now carries the verbatim AGPL-3.0 text; `LICENSE-MIT` is removed;
`Cargo.toml` declares `AGPL-3.0-or-later`. Rawler stays LGPL-2.1 and linking it from
a copyleft program is precisely what the LGPL exists to permit, so nothing about the
dependency changes. Versions already distributed remain available under MIT to
whoever received them — that is not retractable and is not being retracted.

Worth knowing rather than discovering: AGPL §13 obliges offering source to users who
interact with the program over a network. This is a local batch CLI, so it never
fires, and the licence behaves as GPL-3 here. It would begin to matter only if this
were put behind a service.

0.1.17 ended on a deliberate negative: the owned colour path lost on
`crushed_fraction`, so `rawler` stayed the default. Two things were wrong, and
they turned out to be the same shape of mistake — the program could not see what
it was doing. One was a real defect in the tone curve. The other was that the
scorecard had no axis capable of rewarding the owned path even when it was right.

Both are now fixed, and the owned path wins on the axis that matters.

### The scorecard can now see hue

The four standing axes are highlights kept, shadows kept, tonal detail and local
detail. None of them can see a *hue* shift — and keeping an out-of-gamut colour's
hue instead of letting a clip rotate it is the entire purpose of the owned colour
path. Worse, `crushed_fraction` was **structurally unable** to favour it:
post-`Calibrate` data has no negatives, so rawler could never crush by that route
while the owned path could only add zeros. That row was decided before a single
frame was rendered.

- **`src/oklab.rs`** — linear sRGB to Oklab, hue as `atan2(b, a)`. Oklab rather
  than CIELAB because CIELAB's hue angle bends for saturated blues, which is
  exactly the out-of-gamut population here: skies, and the blue channel an A7C's
  white point multiplies by about 1.6. The cube root is signed and nothing is
  clamped — clamping would destroy the negatives the module exists to measure.
- **A per-pixel hue comparison on a canonical grid** (`src/reference.rs`).
  Everything in that module was aggregate-only before, and a frame's *mean* hue is
  dominated by whatever colour covers most of it, so aggregates cannot see this.
  The reference's decoded pixels are freed before our render exists — deliberately,
  so a 150 MB phone JPEG and the developer's peak never coexist — so a 512-px-long-
  edge linear-light thumbnail is retained instead, about 2 MB. Our render is
  box-averaged **onto that same grid**, and the comparison refuses to run when the
  two renderings are different crops. That also delivers the review's
  "canonically resize and align paired outputs" item.
- **`measured.mean_saturation_highlight`** — saturation restricted to pixels above
  0.7 luminance, added because the whole-frame figure cannot answer the question
  the review asked about the 1.20 saturation multiplier.

One bug worth recording, because it was invisible until the numbers looked wrong:
the first version derived each image's canonical grid independently, and 35 of 106
pairs were then declined as "different crops" when their aspect ratios agreed to
0.13% — two shapes that round to grids a pixel apart. Resampling the second image
*onto* the first's grid fixed it; all 106 pairs now compare. A rounding artifact
had been wearing a geometry mismatch's costume, and it would have quietly halved
the corpus this axis could reach.

### The `crushed_fraction` regression: found, fixed, verified against a prediction

`analyze::luminance` is a **signed** weighted sum, so a pixel with a large enough
negative channel has negative luminance. `tone::render_pixel_local` clamped its
chroma anchor with `luminance(rgb).clamp(0.0, 1.0)`, so such a pixel anchored at
exactly 0 — and `compress_gamut`'s scale is `anchor / (anchor - min)`, which is
then `0 / |min|` = 0, multiplying **every** channel by zero. The pixel collapsed to
pure black *including its positive channels*, while rawler's per-channel clip keeps
those. On that cohort the owned path was strictly **more** destructive than the clip
it replaced, which is the opposite of what removing a clip is supposed to do.

The fix is one clamp: the anchor is floored at `black_output_linear`, which is what
`mapped_norm` three lines above was already clamped to. The curve and the chroma
anchor had simply disagreed about where black is.

**Measured against the prediction 0.1.17 recorded**, on 108 paired frames, owned
against rawler head to head:

```text
axis                     before A1              after A1
crushed_fraction     0 better,  87 same, 21 worse   0 better, 108 same,  0 worse
luminance_entropy   36 better,   5 same, 67 worse  41 better,   5 same, 62 worse
average_gradient    84 better,   0 same, 24 worse  84 better,   0 same, 24 worse
near_white_fraction 34 better,  35 same, 39 worse  34 better,  35 same, 39 worse
```

The regression is gone entirely, and the local-detail win is untouched.

This moves default-path output, which the byte-identity rule does not cover — it is
a bug fix in shared code, with precedent in 0.1.16. The movement is small and
confined to deep shadows, as the mechanism predicts: **16 of 108 frames** change at
all, with median deltas of -0.00013 on `crushed_fraction`, -0.0016 bits of entropy
and +0.001 of mean level. Determinism holds; images are byte-identical at
`--jobs 8` against `--jobs 1`.

### The owned path wins on hue

With the axis in place and the regression fixed, on 106 pairs:

| | rawler | owned |
|---|---:|---:|
| hue vs camera, median | 12.09° | **11.27°** |
| hue vs camera, mean | 21.04° | **20.61°** |
| highlight saturation ratio | 1.528 | 1.619 |

Read the *mean*, not the median, and read the restricted cohort rather than the
whole corpus. The clip only touches out-of-gamut pixels, a minority of most frames,
so a large improvement there barely moves a corpus median — and indeed the median
change across all frames is 0.0000° while the best frame improves by 26.2° and the
worst worsens by 0.14°. **Restricted to the 30 most out-of-gamut frames — the
population the clip actually acts on — the median mean-hue improvement is -0.905°
with only 13 of 30 worse**, and the six largest improvements are all frames where
rawler's clip would have rewritten 33% to 66% of pixels. That is the owned path
doing precisely the job it was built for, finally visible.

`HueComparison` reports mean alongside median for this reason, with the asymmetry
recorded in its doc comment so the next reader does not summarise it with the
wrong statistic.

### A2 and A3 were declined on evidence, not skipped

The plan queued two further pieces. Both were measured for a target first and
neither has one; that is recorded here rather than left as silent scope loss.

- **A noise-versus-colour split for negatives.** The idea was that a negative
  smaller than the sensor noise at that level is noise, and clipping is right for
  it. After the anchor fix there is nothing to fix: `crushed_fraction` is
  0/108/0, and the residual entropy difference is **uncorrelated with negatives**
  (r = -0.16 against `negative_luminance_fraction`, -0.01 against
  `negative_fraction`) while being mildly *positively* correlated with
  `above_one_fraction` (r = +0.16). Half the worst remaining entropy losses have
  zero negative-luminance pixels, and the corpus median residual is -0.000004 bits.
- **A scene-linear gamut operator.** Its precondition was "only if the hue axis
  says `compress_gamut` is still hue-shifting". The axis says the opposite — see
  above. Also worth writing down: the operator as originally proposed was
  *impossible*. "Desaturate toward the achromatic axis preserving hue **and
  luminance**" cannot work for a luminance ≤ 0 pixel, because every non-negative
  RGB triple has non-negative luminance, so the only legal destination is black.
  Stated that way the fix would have reproduced the bug one stage earlier.

### Owning the rescale step — `src/rescale.rs`

The last place Rawler destroyed data before we saw it. `apply_scaling` →
`correct_blacklevel*` clips sub-black samples to zero, which rectifies the sensor
noise floor: a genuinely black region develops to a small *positive* value and true
black is unreachable. That step, plus the demosaic ROI and the default crop, is now
ours. Rawler's `PPGDemosaic` is the only piece still rented — it is not the quality
bottleneck, and it is called directly rather than through the bundled step list.

`--sub-black` is a hidden three-way control rather than a boolean, because one flag
would have conflated two variables:

- `rawler-compat` reproduces `RawImage::apply_scaling` **bit for bit**, defects
  included. The default, so owning the step moved no output at all.
- `clip` fixes the defects but still clips sub-black.
- `preserve` keeps sub-black negative.

Bit-identity was demanded rather than approximated, and proved two independent
ways: eight unit tests clone the `RawImage`, call rawler's own `apply_scaling()` and
compare `f32::to_bits()` element-wise so `+0.0` cannot pass for `-0.0`; and over the
corpus, 376 of 376 files produce identical `analysis` and `parameters` blocks with
six rendered TIFFs byte-identical to the pre-change binary. Two ordering details are
load-bearing and commented as such: compute `white - black` once and then *divide*
(a reciprocal-multiply differs in the last bit), and test the sign with
`is_sign_negative()` rather than `< 0.0`.

**Four latent defects closed, none of which any real file trips.** Said plainly
because the alternative is an unfalsifiable claim: rawler hardcodes a 2×2
black-level repeat and ignores `BlackLevel::{width,height,cpp}`; it does not anchor
the black-level pattern at `ActiveArea` even though it anchors the CFA there;
`as_bayer_array()` silently broadcasts element `[0]` unless the stored length is
exactly 4; and its `chunks_exact_mut(2)` pairing leaves the **last row unnormalized
when height is odd, and the last column when width is odd**, letting raw DN through
into develop. Over 376 files, **zero** trip any of them — every file records equal
levels on a 2×2 or 1×1 repeat, even dimensions, and an `ActiveArea` origin of
`(0,0)`. `RescaleReport::notes` fires if a future file ever does, so the claim
degrades loudly instead of silently.

That also closes the four open questions the plan could not answer statically. The
A7C's black levels come from Rawler's camera config as four *equal* values (512, or
1024 at higher ISO), its white level count is 1 (15360), its `ActiveArea` origin is
`(0,0)`, and it is 6048×4024 — both even, as is every ARW in the corpus.

**And it is cheaper.** Peak working set, `--jobs 1`, with a real render:

| | before | after |
|---|---:|---:|
| 24.3 MP ARW | 321 MB | **242 MB** (−25%) |
| 24.5 MP ×3 LinearRaw | 722 MB | **640 MB** (−11%) |
| `--raw-color-path rawler` (control) | 1002 MB | 1002 MB |

Rawler's `develop_intermediate` clones the whole `RawImage`, converts it to float
and then builds a second full buffer; writing straight into the output from a
borrowed `&RawImage` skips both. Smaller than the ~600 MB the plan estimated, and
the reason is worth recording: that figure modelled *allocation totals* on a 50 MP
file, while this measures working set, which does not track untouched pages or
allocator retention. Direction and attribution are solid; the plan's number was
optimistic. This bears on the `--jobs 8` OOM `docs/STATUS.md` has carried since
before 0.1.12.

One planning claim was **wrong and is corrected here rather than repeated**: the
plan asserted "all 23 Samsung files must be byte-identical across all three modes".
That invariant belongs to the *`LinearRaw`* files only — the 16 Galaxy S24+ frames,
whose `BlackLevel` is 0, so sub-black is unrepresentable and the clip is a strict
no-op. The other Samsung frames are ProShot CFA captures with a black level of
256.25; they do have sub-black samples and they do move under `preserve`. The sharp
check is "the 16 linear DNGs are invariant", and that holds.

Two smaller findings, both pinned by tests rather than claimed as fixes: `bail!` on
a non-positive `white - black` is a behaviour change for a file Rawler would have
"rendered" as `inf`/`NaN` (no corpus file is affected, so nothing moved); and the
widened no-op-crop guard turns out to be *unreachable* given the bounds check, so
Rawler's narrower test is not a live bug — it was backstopped by an assertion that
would have aborted the batch.

### The default is now `--raw-color-path owned`

`docs/REVIEW-2026-07-30.md` set the gate: "rawler stays the default until the owned
path beats it on the scorecard." It now does, so it does not.

Over 106 pairs, head to head against `rawler` on the same files:

```text
average_gradient     84 better,   0 same,  24 worse
crushed_fraction      0 better, 108 same,   0 worse
near_white_fraction  34 better,  35 same,  39 worse
luminance_entropy    41 better,   5 same,  62 worse
hue vs camera        mean 0.69 deg closer; 0.90 deg closer over the
                     30 most out-of-gamut frames, best frame 14 deg
```

The entropy row is the only loss and it is negligible in magnitude: a median of
**-0.0000039 bits** against a scale of about 7.5, with the worst single frame at
-0.081. `luminance_entropy`'s maximiser is histogram equalisation, which this
project has now recorded twice as making it invalid as a one-directional axis. Set
against a large local-detail win, a tie on shadows and a measured hue improvement on
exactly the cohort the change targets, that is not a reason to keep a
data-destroying default.

**`--working-space` stays `srgb`.** BT.2020 was measured and is not clearly better:
it wins `near_white` handsomely (59 better, 43 worse) but gives back local detail
(65/43 against srgb's 81/27) for no additional hue benefit (mean -2.4377° against
-2.4305°). It also cuts out-of-range pixels from a median 0.179% to 0.056%, which is
real but does not show up as image quality here. Available, not default.

### `--sub-black preserve` is the default too — decided by looking, against the metrics

This one the scorecard got wrong, and it is the most useful thing in the release.

`preserve` was measured first and the numbers were discouraging. It more than
doubles the hue win — a median **2.42°** closer to the camera over the 30 most
out-of-gamut frames, against 0.90° for the clipped path — but `crushed_fraction`
gets *worse* on 18 of 108 frames (0 better, 18 worse), `mean_level` falls by up to
**14**, worst-case entropy by 0.42 bits and gradient by 2.24. On that evidence it
was held behind the flag as "probably correct but visible and unlooked-at".

Then somebody looked, which is what `docs/PLAN.md` means by *read the number, then
open the pair*.

**Clipping sub-black puts a magenta cast in the shadows.** The mechanism is
straightforward once seen: rectifying the noise floor lifts each channel's mean
above true black, and white balance then multiplies that pedestal unequally — on an
A7C, red by about 2.3 and blue by 1.6 against green at 1.0. The residue is magenta.
On `_DSC1253`, a night frame with 28.5% sub-black samples, the entire lower half of
the image is visibly tinted and the darkest crop is a solid field of magenta
speckle. Under `preserve` the noise stays symmetric about zero, averages neutral,
and the cast is simply gone. On `_DSC1277`, an ISO 8000 waterfall, the same effect
shows as a red-brown haze over dark foliage that `preserve` removes, leaving the
foliage green. On `_DSC1276` — same scene, ISO 2500, half the sub-black population
— the two renderings are near-identical, so the effect scales with the cause and
does no harm where there is nothing to fix.

Structure survives in every case: rock texture and leaf detail are unchanged. What
turns black is noise that carried no detail and previously turned into coloured
haze instead. So the three metrics that argued against this were describing the fix,
not a defect — which is precisely the failure mode
`docs/PLAN.md` warns about for `average_gradient`, whose maximiser is amplified
noise.

A magenta cast across the shadows of a night frame is the kind of thing criterion 3
means by *ruined*. It has been there the whole life of the project, inherited from
Rawler's `Rescale`, and no metric on the scorecard could see it.

### `crushed_fraction` and `clipped_fraction` were measured at the wrong precision

Found while chasing the above. Our own render is measured in 16 bits by
`OutputStats::measure` while the camera's JPEG is measured in 8 by `measure_rgb8`,
and the two tests were `channel == 0` and `channel == u16::MAX` respectively — so
our side had to hit exactly 0/65535 where the camera's had to hit 0/255. A pixel at
40/65535 is black in every file this program writes and was counted as not crushed.
Both now use display precision: `<= 128` and `>= 65407`, which are exactly the
values `output.rs`'s `(v + 128) / 257` maps to 0 and 255.

**Being precise about what that changed, because it is less than it sounds:** on
the 106-pair corpus, nothing. `clipped_fraction` stays at 94 wins / 10 ties / 2
losses and `crushed_fraction` at 40/66/0, with identical medians — real renderings
put very few pixels in the affected bands, so the thresholds agree in practice and
no previous conclusion moves. Where it bites is where those bands get populated
deliberately: `preserve`'s shadow cost shows as 18 frames worse under the corrected
threshold against 1 frame at +0.000004 under the old one. The cost was real and the
old metric could not see it.

The existing `the_eight_and_sixteen_bit_paths_agree` test asserted agreement on five
statistics and omitted exactly these two, so it could never have caught this. There
is now a second test that covers them.

### The 1.20 saturation multiplier, finally measured where it lives

`docs/REVIEW-2026-07-30.md` suspected the `auto` preset's 1.20 multiplier of
compensating for Rawler's highlight desaturation, and 0.1.17's whole-frame ratio
appeared to exonerate it (1.046 → 1.050). The new highlight-band figure says the
whole-frame number was simply blind: **our highlights sit at 1.53 (rawler) to 1.62
(owned) times the camera's saturation**, against a whole-frame 1.05.

That is a large, previously unmeasured divergence, and it does **not** support the
review's suspicion — it points the other way. We are not under-saturated in
highlights by comparison with the camera; we are half again over. Whether that is
better or worse is taste, not information: the camera desaturates its shoulder
hard, and this program's whole thesis is keeping what the camera throws away. It
needs eyes on a contact sheet, not another constant. Recorded, not acted on, under
the standing heuristic freeze.

### Verification

- **`--sub-black rawler-compat` is bit-identical to Rawler's `apply_scaling`**, by
  eight `f32::to_bits()` unit tests and by 376 of 376 corpus files producing
  identical `analysis` and `parameters` blocks with byte-identical rendered TIFFs.
  Owning the rescale step, on its own, moved nothing.
- **Phase B moved nothing**: a full 376-file survey diffed field by field against
  the 0.1.17 binary shows **zero** behavioural differences; the new statistics are
  additive.
- **A1 and the default flip do move output, deliberately.** A1 changes 16 of 108
  frames, all in deep shadows at ~1e-3 magnitudes. The default flip is the
  scorecard above.
- **376 of 376 files develop with zero failures** on the new default, with zero
  `rescale.notes` raised anywhere.
- **Determinism**: `--jobs 8` against `--jobs 1` gives byte-identical images and
  sidecars.
- 207 tests (19 new in `rescale`, 9 in `oklab`, plus new `metrics`, `reference`,
  `color` and `tone` cases), clippy clean, `cargo fmt --check` clean.

## 0.1.17 — the correctness boundary

An external source-level review recommended stopping heuristic work for a cycle
and fixing the pipeline's correctness boundary instead. Its checkable claims were
verified against this codebase and against rawler 0.7.2's actual source before
anything was adopted; `docs/REVIEW-2026-07-30.md` records what held, what was
overstated, and what was rejected. This release is that boundary.

The headline is a negative result, and it is the useful kind: **the owned colour
path does not yet beat rawler's on the scorecard, so `rawler` stays the default**
— which is exactly the gate the review specified. What the A/B bought is a
diagnosis precise enough to name the next piece of work.

### Own the colour path — `--raw-color-path {rawler|owned}`

Rawler's `ProcessingStep::Calibrate` ends in `clip_euclidean_norm_avg`, which
clips negative channels to zero unconditionally and, for any pixel with a channel
above 1.0, replaces the pixel with the average of its max-normalized colour and
its Euclidean norm. Everything this program measures and renders happened
downstream of that.

`src/color.rs` stops rawler after `Rescale`/`Demosaic`/`CropActiveArea`/
`CropDefault` — camera RGB out — then applies as-shot white balance and a
camera→working-space matrix here, with nothing discarded. The matrix is composed
the way rawler composes its own (row-normalize `xyz_to_cam * working_to_xyz`,
then pseudo-inverse), so at `--working-space srgb` the owned path is rawler's
result *minus the clipping* and the A/B isolates one variable. A test checks the
composition against rawler's own public `normalize`/`multiply`/`pseudo_inverse`
rather than against a transcription of them.

Two things learned writing it, both now pinned by tests:

- **`clip_euclidean_norm_avg` is not a clip.** It does not bound its output:
  `[2.3, 1.0, 1.6]` — an ordinary A7C white point on a blown pixel — comes out as
  `[1.359, 1.076, 1.207]`, still above the white point. What it destroys is the
  *ratio* between channels; the spread collapses from 1.30 to 0.28. The magnitude
  mostly survives and the colour is what is spent. The first version of the test
  asserted an upper bound of 1.0 and failed, which is how this was found.
- **Rawler's illuminant fallback is not deterministic.** `Calibrate` does
  `find(D65).or_else(|| color_matrix.iter().next())`, and that walks a `HashMap`
  — so a file carrying several calibration matrices and no D65 one develops
  differently from one process to the next. Determinism is a product property
  here, so the owned path sorts explicitly (prefer D65, else lowest illuminant
  code) and the summary now counts affected files and warns. **Zero of the 376
  corpus files are affected** — every one carries a D65 matrix — so this is a
  latent hazard, not a live bug, but it is the same `HashMap`-ordering trap
  `CLAUDE.md` already records for IFD lookups.

`--working-space {srgb|rec2020}` rides along, since the composition is mine now.
BT.2020 needs a working→display conversion in `tone.rs`, applied at the top of
the render so every stage below is the code it already was; `WorkingSpace::Srgb`
returns `None` rather than an identity matrix precisely so the default path does
no extra arithmetic at all.

### What the clipping was actually costing

`--raw-color-path owned` reports, per frame, how far rawler's operator would have
moved each pixel — so one run answers the question without a paired before-and-after.

Over 376 files: a median **0.215%** of pixels rewritten, p90 10.0%, **max 66.2%**.
Heavily skewed — 38 frames above 10%, 18 above 25%. So "a large share of
highlights" is true of a minority of frames, and badly true of those.

**The dominant cause is negatives, not highlights**, which the review had it the
other way round: median `negative_fraction` 0.028% against `above_one_fraction`
0.009%, and on the worst frame 61.1% negative against 5.5% above 1.0. The bigger
loss by pixel count was out-of-gamut *colour* clipped to zero, not blown
highlights.

### The scorecard, 106 pairs

Win/tie/lose against the camera JPEG (lower better for the first two, higher for
the last two):

```text
axis                  rawler/srgb      owned/srgb    owned/rec2020
near_white_fraction   48W  1T 57L     48W  1T 57L     49W  1T 56L
crushed_fraction      37W 65T  4L     25W 64T 17L     25W 64T 17L
luminance_entropy     62W  0T 44L     59W  0T 47L     59W  0T 47L
average_gradient      61W  0T 45L     61W  0T 45L     61W  0T 45L
```

Head to head against rawler on the same 108 files: `average_gradient` **84
better, 24 worse** — a real local-detail win, recovered highlight and colour
structure. `crushed_fraction` **0 better, 87 same, 21 worse** and
`luminance_entropy` 36 better, 67 worse — which turn out to be **one regression
reported twice**, with one cause.

#### The regression: negative *luminance*, not negative channels

The first version of this entry blamed "negatives reaching `compress_gamut`" and
was wrong about the mechanism in a way that mattered — it would have led to a fix
that reproduced the bug. An adversarial review of the analysis caught it. The real
mechanism, traced through `tone::render_pixel_local`:

`luminance` is a *signed* weighted sum, so a pixel with a large enough negative
channel has negative luminance. Line 211 sets the chroma anchor with
`luminance(rgb).clamp(0.0, 1.0)` — so that pixel anchors at **0**. Then
`compress_gamut`'s scale is `anchor / (anchor - min)` = `0 / |min|` = **0**, and
every channel is multiplied by it. The pixel collapses to pure black *including
its positive channels*.

That is **strictly more destructive than rawler's clip**, which zeroes only the
offending channel and leaves a dark saturated colour behind. So removing a clip
made these frames worse, not merely differently scored. `src/color.rs` has a test
rendering both versions of one such pixel to pin it.

A negative channel on its own is harmless: `compress_gamut` anchors on a positive
luminance and lands the offending channel on exactly 0, keeping the rest of the
colour. The populations differ by a factor of four — on the worst frame, 61.1% of
pixels have a negative channel but only 14.7% have negative luminance.

`clip_cost.negative_luminance_fraction` now measures exactly that, and it predicts
the regression **exactly**: of 108 frames, zero have negative luminance without a
crushed rise and zero have a crushed rise without negative luminance. The crushed
delta is a median 0.883× the negative-luminance population — a little under 1.0
because some of those pixels were already at black on the rawler path too.

And the entropy row is the same finding: `r = -0.81` between the entropy delta and
the crushed delta, with the eight worst entropy losses being the eight worst crush
frames. Among the 75 frames with **zero** negative luminance the median entropy
delta is -0.000004 bits (44 lost, 26 gained, worst -0.057). So entropy is not an
independent regression and not "noise" either — calling it noise would have been
explaining away a systematic effect (a 67-vs-36 sign count is p ≈ 0.003). It is
the crush, seen through a second metric, plus a genuinely negligible remainder.

#### Much of the negative population is noise, not exotic colour

Worth knowing before designing the fix. The frames with the largest negative
populations are the *noisiest* in the corpus — `snr10_ev` between +1.7 and +2.0
EV, meaning SNR=10 is not reached until two stops above middle grey — with `p05`
around -5 to -7 EV. Rawler's `correct_blacklevel` clips per channel in *camera*
space, so shadow noise sitting at zero in one channel goes negative in the working
space as soon as it passes through a matrix with negative off-diagonals, which the
A7C's has. Those negatives are noise wearing a gamut costume, not scene colour
outside sRGB.

This matters because the treatments differ: hue-preserving gamut mapping of noise
just turns black speckle grey, whereas a per-channel clip — rawler's behaviour —
is arguably right for it. The fitted per-frame noise model needed to tell the two
apart already exists in `noise.rs`.

#### What the fix is, and what it is not

Not "gamut-compress in scene-linear preserving hue and luminance": every
non-negative RGB triple has non-negative luminance, so no luminance-preserving
operator can map a negative-luminance pixel anywhere legal except `[0,0,0]`. Stated
that way the fix reproduces the collapse, one stage earlier. The content of the
work is a **policy for luminance ≤ 0 pixels**, in this order:

1. Floor `compress_gamut`'s anchor at `black_output_linear` rather than 0. Note
   that `mapped_norm` on line 203 is already clamped to
   `[black_output_linear, white_output_linear]` while `mapped_luminance` on line
   211 is clamped to `[0, 1]` — an inconsistency worth fixing on its own account.
   This is a one-line change, it does move default-path output (deep shadows
   below the black floor), and it therefore needs its own measured pass.
2. Split negatives by magnitude against the fitted noise floor: clip the ones that
   are noise, colour-manage the ones that are coherent.
3. Only then a scene-linear gamut operator, with the luminance ≤ 0 branch
   explicit — and note that a "just enough" per-pixel compressor is C0 but not C1
   at the gamut boundary, so gradients crossing it can band. A smooth
   distance-limited compressor avoids that but moves in-gamut colours near the
   boundary, which breaks the "owned = rawler minus clipping" isolation. That is a
   real trade and should be chosen deliberately.

This still outranks milestone 2's `ForwardMatrix`/`CameraCalibration`/
dual-illuminant work for *mechanism*, since the matrix changes which colours land
out of gamut without changing what happens to them. But any fitted thresholds in
step 3 are matrix-dependent, so build the mechanism now and defer the tuning until
after the colour science.

#### Two things this A/B cannot tell us

**The scorecard is structurally biased against the owned path on `crushed`.**
Post-`Calibrate` data has no negatives at all, so the rawler path can never crush
via this route while the owned path can only add zeros — that row was ≤ before a
single frame was rendered. Meanwhile the owned path's actual benefit, shadow and
out-of-gamut hue fidelity instead of rawler's silent hue shift, is invisible to
all four axes. A hue-angle-delta axis restricted to the out-of-gamut cohort would
be the honest instrument, and the gate will keep favouring rawler for structural
reasons until one exists.

**The saturation-multiplier exoneration is weaker than it first looked.** The
ratio against the camera moves only 1.046 → 1.050 → 1.052 across the three arms,
so nothing needs retuning *globally*. But `mean_saturation` averages over the
whole frame while the clipping's desaturation is concentrated in the above-1.0
cohort, a median 0.009% of pixels — a whole-frame mean is nearly blind to it at
that population size. The claim that survives is "the global constant needs no
change"; whether 1.20 was compensating *in highlights* needs saturation measured
on the above-one cohort alone, which no metric currently reports.

**BT.2020 helps where predicted and not where it cannot.** Out-of-range pixels
drop from a median 0.166% to 0.053% (max 66.2% → 52.2%) and `near_white` improves
head to head to 56 better / 47 worse — the clearest evidence the wide space does
something, and worth reading in preference to the against-camera row, which barely
moves (48W → 49W). `crushed` is unchanged at 0/87/21, because a camera's gamut is
not a triangle: no RGB working space eliminates negatives, and this cohort is
largely noise anyway.

### Typed colour-space images

`Image<CameraRgb>` and `Image<SceneLinear>`, with `LinearImage` now an alias for
the latter, so handing un-white-balanced sensor data to the analyser or the tone
curve does not compile. There is deliberately no escape hatch: a `retag` method
was written first and turned out to have no caller, which is the useful result —
every place the colour space changes also changes the numbers. `DisplayLinear`
and `Encoded` markers were left out because the render path goes scene-linear to
encoded `u16` inside one function without materializing a display-linear buffer,
and `tone::Rgb16Image` is already the encoded type.

### Independent and preview-guided evaluation, kept apart

A pooled median cannot distinguish the controller getting better at judging
scenes from more files happening to carry a usable preview. So: `guidance_mode`
in every sidecar (decided per file, since the oracle is automatic and the answer
differs within one run), a split `independent` / `preview_guided` scorecard in
the summary and on the console, `--no-preview` as the honest name for the
independent arm, and `tools/contact-sheet.py --guidance` to sheet one arm alone.
On the corpus the split is 39 independent / 337 preview-guided, with independent
at a +0.04 EV median key delta against preview-guided's +0.02.

`controller_version` is recorded too, frozen at `v1`. The review's freeze
discipline is adopted as policy: no new exposure heuristics until the colour core
lands. The label is what makes a later `v2` comparison mechanical — the corpus
grades already on disk say which controller produced them.

### The `subject` statistic is renamed to what it measures

`subject_display_ev` → `center_weighted_key_display_ev` throughout, schema 7. It
is `0.60 * centre median + 0.40 * frame median` with no subject detection of any
kind behind it, and the old name invited reading a large delta as "the subject is
misplaced" when the honest reading is "the centre of the frame is brighter or
darker than the camera made it" — different claims on a backlit or off-centre
composition.

### EXIF copy and sRGB ICC embedding

Output now carries its capture metadata, which `docs/PLAN.md` §1 counts as the
product rather than polish: before this, a photo library sorted an archive by file
modification date and showed no capture info at all. `src/metadata.rs` copies the
date/time group and its sub-second and time-zone companions, Make, Model, the
lens group, ExposureTime, FNumber, ISO, FocalLength, the metering/flash/exposure-
program group, Artist, Copyright and the whole GPS IFD, adds
`Software = raw-autotune <version>`, and embeds a generated sRGB v2 ICC profile.
JPEG gets APP1/APP2, TIFF an ExifIFD plus GPSInfo plus tag 34675, PNG `eXIf` plus
`iCCP`. **No new dependency**: `image` 0.25 writes the JPEG and PNG metadata
itself and rawler's MIT-licensed TIFF writer builds the EXIF structure.

Measured: Windows' own property handlers now report Date taken, Camera maker,
Camera model, Program name, Exposure time and F-stop for the JPEG and the TIFF,
where every field was previously blank; 22 tags plus the GPS block read back
across all three formats. The generated profile transforms to a reference sRGB
profile through littleCMS with a maximum channel error of 0/255 over 261 sampled
colours, checked separately in the JPEG segment, the TIFF tag and the PNG chunk.

Two traps recorded. Rawler's `write_exif_tags` copies the source `Orientation`,
but `orientation::apply_orientation` has already rotated the pixels — so the tag
is overridden to 1 explicitly. Copying it would have left every portrait frame in
the archive sideways with nothing in the pixels to show it. And an ICC
matrix-shaper TRC maps device to linear, i.e. it is the sRGB *decoding* curve: the
first version sampled `tone::srgb_encode` instead and produced a structurally
valid profile that littleCMS rendered 34/255 as 170/255. Both are covered by
tests, the second by a round-trip against `tone::srgb_encode` plus a convexity
check on the curve's direction.

`--no-metadata` restores the previous bytes exactly, asserted for all three
formats.

### Local white balance: a false invariant fixed

`whitebalance.rs`'s module doc argued that unit-luminance white points made the
correction exposure-neutral. They do not: the correction *divides* by the white
point per channel, and by Jensen's inequality the reciprocal of a non-neutral
unit-luminance vector has luminance ≥ 1. So corrected regions were being
brightened in proportion to how non-neutral the local light was — an exposure
change smuggled in by a colour operator, and precisely what the doc claimed could
not happen. A luminance-restore rescale now follows the division, Tikhonov-
regularized (`gain = before·after / (after² + ε²)`, ε = 1e-4) so it stays finite
and continuous through zero, which matters now that the owned colour path
produces genuinely negative channels. `--local-white-balance` is off by default,
so no shipped output moved; six new tests pin the invariant, one of them
documenting the old bug numerically.

### Smaller things

- `noise.rs`'s `collect_tiles` no longer materializes a full-length `Vec<f32>`
  copy of the decoded RAW to scan a few hundred KB of tiles — an avoidable ~200 MB
  peak on a 50 MP frame. Bit-identical, proven by pinning `estimate()`'s output on
  a synthetic image before and after the change with exact equality, so
  `snr10_ev`/`snr1_ev` and everything downstream of them are untouched.
- `--dump-stages DIR` writes the scene-linear intermediate at each stage as plain
  sRGB-encoded PNGs with no tone curve, so the two colour paths' highlights can be
  compared directly.
- The sidecar `limitations` list is now conditional on the colour path and on
  `--no-metadata`, because a standing "no wide-gamut working space" caveat is
  simply false under `--working-space rec2020`, and a list that lies in one
  configuration is worse than none.

### Not adopted, deliberately

The review's own source recommended two further milestone-1 items that
`docs/REVIEW-2026-07-30.md` rejects, and the verified document wins: **FujiRotate**
(X-Trans is out of scope, and rawler 0.7.2 has no such step), and **moving
sharpening before sRGB encoding** — real as a fact, but to be re-evaluated
*after* the colour core and against the pairs, not before. RCD/AMaZE demosaic
stays rejected on licence grounds: the quality implementations are GPL-3 and this
crate is MIT. *(Superseded in 0.1.18, which relicensed to AGPL-3.0-or-later and
removed that objection entirely; the demosaic is now deferred on merit, not
refused.)*

### Verification

- **Default output has not moved.** A `--dry-run --summary` over all 376 corpus
  files, diffed field by field against a 0.1.16 baseline build, shows **zero**
  behavioural differences — every analysis statistic, tone parameter, noise fit,
  preview reading and reference delta identical. The only two diffs are the
  renamed summary key itself, carrying identical values.
- **Rendered bytes identical**: 28 mixed Sony/Samsung frames render byte-for-byte
  identically to the 0.1.16 binary under `--no-metadata`, and pixel-for-pixel
  identically with metadata on (only the container gains tags).
- **Determinism**: `--jobs 8` against `--jobs 1` gives byte-identical images and
  sidecars across the 28-frame subset.
- 169 tests (164 lib + 5 integration), clippy clean.

## 0.1.16

Eight new Sony pairs shot for exactly one purpose: the brightest scenes
obtainable around the photographer's house — a white wall in direct sun among
them — because `ORACLE_TARGET_CEILING_EV` was the one guard in the program with
no paired evidence on either side. They are the first camera JPEGs in the
corpus whose subject lands above +1 EV, and they settled it: the ceiling was
wrong, in the direction and for the reason `docs/STATUS.md` had suspected.

### The evidence

Seven of the eight pairs sat within 0.06 EV of the camera untouched — bright
*scenes* were never the problem. `_DSC1291`, the wall in sun, is the first
paired frame the ceiling ever bound on: the camera renders its subject at
+1.63 EV, unclipped (0.000% at p95 +1.89), our own analyzer independently
classifies the raw `high_key` (key score 0.54), and the +1.0 clamp was leaving
the render 0.42 EV darker than the camera. A genuinely bright scene,
corroborated twice over, refused on suspicion of being a blown preview.

### Fix one: the ceiling admits corroboration

The ceiling's reasoning — "a vendor rendering more than a stop above middle
grey is probably an HDR-fused or blown preview" — keeps its force only when
the raw statistics *don't* agree with the bright preview. `analyze` now raises
the ceiling by up to +1 EV as the key score climbs from the high-key threshold
(0.32) to 0.60, smoothstepped like the chroma ramp so near-identical frames
cannot render differently. An uncorroborated bright preview still clamps to
+1.0 exactly; full corroboration still refuses anything past +2.0. The 16
corpus files the old ceiling bound on split exactly along this line: the ten
classified high-key rise, the six that are not stay clamped.

### Fix two: the oracle converges — but only brighter

Freeing the ceiling exposed a second, hidden error: the oracle inverts its
target through the *pre-oracle* curve and re-solves once, and on the shoulder
the re-solved curve no longer places the subject where that inversion promised.
`_DSC1291` still landed 0.24 EV under the oracle's ask with the ceiling out of
the way. The re-solve now iterates (fixed cap of 4, deterministic by
construction), re-inverting through each fresh curve — **but only while the
target is moving brighter.** The first full-corpus run iterated both directions
and moved 131 files, driving the 0.1.14 night frames toward the vendor
renderings the pairs had proved wrong (`_DSC1277`: -2.88 → -3.92, others onto
the -4.0 floor) — the single solve's undershoot had been quietly protective
there. The asymmetry mirrors the floor/ceiling bounds and keeps every
below-middle-grey decision bit-for-bit at its night-pair-validated 0.1.14
behaviour.

### Measured

- The 8 bright pairs: median subject delta 0.00 EV, worst 0.05; `_DSC1291`
  -0.42 → **0.00**, and the rendered pair is visually a match (wall texture
  held in direct sun, nothing clipped).
- Full corpus: 42 of 376 files change, every one moving brighter, none by more
  than the corroborated ceiling allows; 12 are the ceiling-freed high-key
  files, the rest small (≤0.06 EV) shoulder-convergence gains on bright
  normal/flat frames. Zero night, low-key or HDR frames move.
- 124 tests (4 new on the guard: uncorroborated-still-clamped, the `_DSC1291`
  case, bounded full corroboration, ramp continuity), clippy clean,
  determinism three 8-worker runs byte-identical over the bright set plus the
  ISO 65535 files.

The corpus stands at 376 files, 106 pairs. The one guard with no paired
evidence is now the zeroth: every oracle rail — floor, deviation, ceiling —
has pairs behind it.

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
