# Changelog

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
