# Design of the first testable version

## Goal

The first release answers one practical question:

> Can a deterministic, fast, global automatic controller produce useful batch
> renders from the user's RAW files often enough to justify expanding the
> renderer?

It therefore keeps RAW development, analysis, parameter selection, and
rendering separate even though they currently live in one small crate.

## Pipeline

```text
RAW container
  -> Rawler decode
  -> black/white normalization
  -> demosaic / Fuji rotation
  -> active/default crop
  -> as-shot white balance
  -> camera-matrix calibration
  -> oriented scene-linear RGB (f32)
  -> sparse global + center sampling
  -> automatic parameter estimate
  -> exposure
  -> scene-EV to display-EV curve
  -> hue-ratio restoration
  -> highlight desaturation
  -> adaptive vibrance
  -> soft gamut compression
  -> sRGB transfer function
  -> TIFF / PNG / JPEG
```

## Why preserve an f32 intermediate

The code consumes Rawler's `Intermediate` directly instead of immediately
converting it to a 16-bit `DynamicImage`. This avoids an extra u16
quantization/clamp and retains any remaining f32 headroom produced by Rawler's
linear-sRGB calibration path. Rawler itself already performs nonnegative,
highlight-safe camera-to-sRGB mapping in this early developer.

## Analyzer

The analyzer samples at most approximately `--max-samples` pixels. It measures
luminance in EV relative to 18% middle gray:

```text
EV = log2(Y / 0.18)
```

It calculates whole-frame percentiles plus a center median. The current
"subject" estimate is deliberately weak:

```text
subject EV = 60% center median + 40% whole-frame median
```

That provides a modest backlit-subject correction without falsely pretending to
perform subject detection.

The distribution's asymmetry produces a `key_score`:

- positive: likely high-key;
- negative: likely low-key;
- near zero: roughly balanced.

The target median is shifted only partway in that direction, preserving some
intentional high-key or low-key appearance.

## View transform

The tone curve has two EV-domain segments meeting at scene middle gray and
mapping it to display middle gray. Black and white input endpoints come from
robust percentiles plus a safety margin. Output endpoints remain inside display
black and white.

The shadow and highlight exponents are chosen so both curve segments have
approximately the requested derivative at middle gray. The transform therefore
has explicit, inspectable parameters instead of a hidden LUT.

RGB channels are not independently tone-curved. The renderer maps luminance and
restores RGB ratios, then applies controlled highlight desaturation and gamut
compression.

## Error containment

Some uncommon RAW combinations can still reach unimplemented paths in Rawler's
early development helper. Every file is processed behind `catch_unwind`, so one
unsupported file should be reported as an error rather than terminating a batch.

## Performance model

- File-level memory concurrency is bounded by `--jobs` using scoped worker
  threads.
- The default is `auto`: `memory.rs` probes every input's dimensions from its
  TIFF directory, budgets the measured worst-case peak for the largest of them,
  and takes the share of the operating system's available-memory figure that
  fits — never more than the processor count. A 50 MP f32 RGB image plus the
  chroma stage's working buffers is 2.7 GiB, which is why this cannot be a
  fixed number.
- Rawler and per-pixel rendering may use Rayon's shared global CPU pool inside
  each file job, so `--jobs 1` does not force the image pipeline to one CPU core.
- Analysis samples rather than allocating another proxy.
- ONNX, local filtering, and a second high-quality demosaic pass are absent.

## Intended refactor after field testing

Once representative output has been reviewed, split the crate into:

```text
raw-autotune-core
raw-autotune-decode
raw-autotune-analyze
raw-autotune-render
raw-autotune-cli
```

Doing that before the first image batch would create structure without evidence.
