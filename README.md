# raw-autotune 0.1.15

`raw-autotune` is a small Rust command-line RAW developer intended for testing a
standalone, batch-oriented equivalent of the useful part of a photo editor's
automatic adjustment workflow.

This is a **workable first version**, not a Lightroom or darktable replacement.

It currently:

1. decodes DNG and other camera RAW formats through Rawler;
2. normalizes sensor black/white levels;
3. demosaics Bayer and supported X-Trans inputs;
4. applies the camera's as-shot white balance and available color matrix into linear sRGB;
5. samples scene-linear RGB statistics;
6. estimates exposure, usable black/white EV limits, contrast, saturation, and
   vibrance;
7. optionally builds a full-resolution, multi-scale local exposure map;
8. applies a hue-preserving, middle-gray-anchored view transform;
9. writes 16-bit TIFF/PNG or 8-bit JPEG;
10. writes a JSON sidecar containing all measured statistics and selected
   parameters.

No ONNX model is required. Neural pre-analysis is deliberately deferred.

## What to expect

The current version should be useful for comparing its automatic decisions on a
real batch of photographs and for identifying where semantic analysis, better
highlight reconstruction, denoising, lens correction, or camera-profile work
will matter.

It will not consistently match Lightroom Auto. The controller only sees global
and center-weighted tonal statistics. It does not know that a region is a face,
sky, snow, stage lighting, sunset, or the intended subject.

## Build

### Windows

Install a current Rust toolchain with `rustup`, then run:

```bat
build-release.bat
```

The executable will be:

```text
target\release\raw-autotune.exe
```

The script also creates a redistributable folder and ZIP under `dist\`.

A Visual Studio C++ build environment may be required by Rust's Windows MSVC
toolchain.

### Linux/macOS

```bash
chmod +x build-release.sh
./build-release.sh
```

Or directly:

```bash
cargo build --release
cargo test
```

Rawler 0.7.2 requires Rust 1.89 or newer. The included
`rust-toolchain.toml` selects Rust 1.89.0.

## First commands

Single image, default automatic preset, 16-bit TIFF:

```bash
raw-autotune photo.dng
```

Choose output directory:

```bash
raw-autotune photo.dng --output processed
```

Batch a directory recursively:

```bash
raw-autotune raw-folder --output processed --format tiff --preset auto
```

Allow two full-resolution images in flight at once (each image may still use the shared Rayon CPU pool):

```bash
raw-autotune raw-folder --output processed --jobs 2
```

Create an intentionally simple baseline render beside every automatic render:

```bash
raw-autotune raw-folder --output processed --emit-baseline
```

Analyze without writing files:

```bash
raw-autotune photo.dng --dry-run
```

Survey a large batch quickly — measures every file, writes no images:

```bash
raw-autotune raw-folder --dry-run --summary survey.json
```

`survey.json` holds per-file analysis and chosen parameters, plus batch tonal
class counts and an exposure distribution.

Grade a batch against the camera's own JPEGs (see below):

```bash
raw-autotune raw-folder --output processed --format jpeg --reference \
  --summary processed/summary.json
```

Correct mixed lighting (off by default, see below):

```bash
raw-autotune photo.dng --local-white-balance 0.7
```

Apply a manual bias on top of automatic exposure:

```bash
raw-autotune photo.dng --exposure-bias -0.35
```

Apply local tone adaptation at half strength:

```bash
raw-autotune photo.dng --local-tone 0.5 --jobs 1
```

Create JPEG previews:

```bash
raw-autotune raw-folder --format jpeg --jpeg-quality 94
```

## Chroma noise reduction

Automatic since 0.1.14, driven by the frame's own fitted noise model. Colour is
averaged over a small window whose radius rises with measured noise; luminance
is **not** touched.

That separation is exact, not approximate. Each pixel is split into its
luminance and a colour difference, the colour difference is blurred, and the two
are recombined — and since the colour difference has zero luminance by
construction and blurring is linear, the result has exactly the luminance it
started with. The filter can dull a colour edge; it cannot soften detail, shift
exposure, or disturb the tone controller's statistics.

Measured against paired camera JPEGs, `mean_saturation` relative to the camera:

| Sony A7C | without | with |
|---|---:|---:|
| ISO 100–200 | 0.94 | 0.94 (filter inert) |
| ISO 8000–12800 | 2.17 | **1.37** |
| ISO 65535 | 2.27 | **1.40** |

A ratio far above 1.0 at high ISO is not extra colour, it is coloured speckle;
a 1:1 crop shows red and green confetti where the camera has none.

The filter is inert on clean frames — 309 of 368 corpus files, including every
base-ISO frame — and those render byte-identically to 0.1.13, allocating
nothing. `--chroma-denoise 0` disables it; values above 1 scale the automatic
strength up.

On frames noisy enough to need it, a second **luminance-guided** stage follows,
new in 0.1.15. No linear low-pass filter can remove low-frequency chroma noise:
it occupies the same spatial band as real colour, so any kernel wide enough to
average it away destroys the colour too. Separating them needs a prior, and
luminance is the right one — a real colour boundary almost always coincides with
a luminance boundary, and sensor chroma noise never does. The filter (after He,
Sun and Tang, computed at reduced resolution after He and Sun) fits a local
linear model of colour against luminance: where luminance has structure, colour
follows the edge; where it is flat, colour collapses to the window mean. Its cost
does not grow with its radius, which is 128 pixels.

At ISO 65535 that takes the saturation ratio against the camera from 1.40 to
1.27, and at 1:1 the coloured confetti is essentially gone, leaving monochrome
grain — chroma removed, luminance untouched, as designed. Widening the support
past 128 px changes nothing further, so what remains is not blotching.

## Output sharpening

Automatic since 0.1.15. An unsharp mask at a one-pixel radius on **luminance
only** — capture sharpening, not a creative effect. The correction is added
equally to all three channels, so it moves luminance and leaves the colour
difference between channels exactly as it was: sharpening cannot shift a hue.
That is the mirror of the chroma filter above, which moves colour and cannot
touch luminance; between them the two partition the signal.

It exists because the 98-pair scorecard made it the largest measured gap — this
program lost `average_gradient` to the camera on 64 of 98 frames, for the simple
reason that cameras sharpen their JPEGs and this one did not sharpen at all.

Three guards: the amount fades to zero on frames the fitted noise model calls
dirty (sharpening cannot tell grain from texture); a soft shrinkage suppresses
corrections below the noise floor so flat areas stay flat; and the correction
may spend only half the remaining distance to black or white, so it can never be
what clips a highlight or crushes a shadow.

`--sharpen 0` disables it; values above 1 scale the amount up. The default was
set by sweeping against the corpus and stopping short of the camera's own
acutance, which is higher than necessary.

## Local white balance

`--local-white-balance <0..1>` enables a local, multi-illuminant correction
after fierro2009: the brightest regions of the frame are treated as the lights
illuminating it, and each pixel is corrected by a blend of their white points
weighted by spatial and chromatic proximity.

It is **off by default and should stay off for most work.** It only acts on
frames where two or more chromatically distinct illuminants are found, since
the camera's as-shot white balance already handles a single one. It also
refuses to neutralize strongly coloured lights, because a campfire, a sunset or
a neon sign is the subject rather than a cast to be removed — without that
guard the method renders fire green.

Use it for mixed-lighting scenes: window light against tungsten, or a night
street under sodium and LED together.

## Local tone adaptation

`--local-tone <0..1>` enables the first-pass local operator. It uses
full-resolution Gaussian surrounds to choose an adaptive scale at every pixel,
then adds a median-anchored local exposure correction before the existing
global tone curve. Shadow lift is capped at +1 EV, highlight compression at
-0.75 EV, and a 0.15 EV dead band avoids changing already-even regions.

The method follows the Reinhard scale selection reproduced by Zhang and Feng,
with corrected equations from the original paper. It does not implement their
later curvelet fusion. Noise-floor and bright-detail guards reduce shadow-noise
amplification and avoid lifting small bright features.

It is **off by default** while it receives broader visual testing. The map is
full resolution and memory intensive; use `--jobs 1` for large RAW files. The
sidecar records its scale histogram, correction range, guarded fraction, and
strength. `--local-tone 0` takes the unchanged rendering path and produces
byte-identical output to omitting the option.

## Preview-driven exposure

The exposure target comes from the camera's own embedded preview rather than
from aiming the median at middle grey. **This is automatic since 0.1.13** and
needs no flag.

The controller's "middle grey always" rule is right on ordinary scenes and
wrong on the ones where the photographer's intent was not middle grey — night
and low-key scenes above all, which it otherwise renders as day. The camera
already made that judgement, and it wrote the result into the file.

The decision of whether the oracle applies is made by the preview itself: a
file carrying a real rendering gets it, a file carrying only an index thumbnail
does not, because a thumbnail is not a rendering and steering by one measurably
made exposure worse. No camera-model list is involved. Measured against 42
RAW+JPEG pairs:

| source | embedded preview | mean subject-EV error before | after |
|---|---|---:|---:|
| Sony A7C ARW | 1616x1080 | 0.64 EV | **0.03 EV** |
| Samsung Expert RAW | up to 8160x6120 | 0.40 EV | **0.00 EV** |
| ProShot DNG | 256x191 thumbnail | 0.31 EV | 0.31 EV (inert) |

Pass `--preview-exposure 0` to switch it off, or a value in `0..1` to use a
partial blend toward the camera's target.

The preview is located through the TIFF directory rather than by scanning for
JPEG markers, which would find auxiliary gain and depth maps instead. Its
brightness is inverted back through the tone curve before use, since the preview
is a display-referred rendering and the controller's target is not. Guard rails
cap how far it may move the target and refuse previews that are blown, flat or
too small.

Cost: every file now decodes its preview. That is negligible for a 1.7 MP Sony
preview and about a second for a 50 MP Samsung one. It happens before the RAW
decode and is reduced to a handful of floats immediately, so it does not raise
peak memory.

## Pooled sensor noise

The per-frame noise fit is content-dependent. Frames from the same camera at the
same ISO have the same gain, so their estimates can be pooled:

```bash
raw-autotune raw-folder --noise-scan sony.json     # decode and fit only, then stop
raw-autotune raw-folder --noise-profile sony.json  # use it
```

The scan is far cheaper than a full pass (13 seconds against about 4 minutes for
258 files here) because it stops after fitting. Using a profile keeps output
reproducible: a file develops identically alone or in a batch.

`--pool-noise` builds the profile from the current batch instead. It costs an
extra decode per file and makes output depend on which files were passed
together, so prefer a profile file.

## Grading against the camera's own JPEG

The program's stated bar is "never clearly worse than the camera's own JPEG,
usually at least as good". That is only testable if the camera's JPEG is on
disk, so shoot RAW+JPEG while building a test corpus. Given a directory of
pairs, `--reference` measures both renderings the same way and reports the
difference:

```bash
raw-autotune pairs --output out --format jpeg --reference --summary out/summary.json
```

Pairing is by filename stem. Use `--reference-dir DIR` when the JPEGs were
collected somewhere else. This is **measurement only**: it changes nothing
about what is rendered, and the renderer never sees the reference. Each file's
sidecar gains a `reference` block holding the camera's own statistics and the
signed difference; the batch summary gains the distribution of each difference
across the corpus.

The measure to steer colour by is `saturation_ratio` — ours divided by the
camera's, where 1.0 is a match. It is a ratio of ratios, so unlike
colourfulness it does not move when the two renderings differ in brightness.

For visual triage, `tools/contact-sheet.py` turns a summary into an HTML page
of side-by-side pairs with the numbers under each, worst-first:

```bash
tools/contact-sheet.py out/summary.json --output out/sheet.html
```

## Presets

### `neutral`

Conservative contrast, wider highlight/shadow safety margins, and neither a
vibrance nor a saturation boost: chroma is left as the camera's colour matrix
delivered it. It still estimates exposure.

### `auto`

Default general-purpose rendering. It retains a larger tonal range than
`punchy`, applies moderate contrast, and makes a small adaptive colorfulness
adjustment. Its saturation is set so that the render matches the saturation of
the camera's own JPEG on the paired test corpus.

### `punchy`

Tighter endpoints, stronger midtone contrast, and a larger colorfulness boost.
This is useful for learning whether the default controller is simply too
restrained, but it is more likely to overprocess difficult files.

`--saturation-scale` multiplies whichever preset is in use. It exists so the
chroma path can be swept against a corpus of RAW+JPEG pairs, the way
`--exposure-bias` offsets the automatic exposure; 1.0 is the preset as tuned.

## Output layout

Directory structure is retained below the output directory:

```text
processed/
  raw-folder/
    trip/
      DSC01234_auto.tif
      DSC01234_auto.json
      DSC01234_baseline.tif    # only with --emit-baseline
```

The sidecar contains:

- camera identity and sensor metadata;
- sampled EV percentiles;
- center and global median measurements;
- estimated tonal class;
- selected exposure and curve endpoints;
- curve powers and color parameters;
- local-tone parameters and correction statistics, when enabled;
- output entropy and average-gradient measurements;
- explicit version limitations.

## Recommended first test

Use 20–50 images rather than one ideal photograph. Include:

- ordinary daylight;
- an indoor image;
- a high-ISO image;
- a backlit person;
- snow or a bright wall;
- a night photograph;
- a sunset;
- a strongly saturated sign or LED;
- a photograph with clipped highlights;
- an intentionally underexposed image.

Run:

```bash
raw-autotune test-raws --output results --emit-baseline --jobs 1
```

Inspect the automatic image, the baseline image, and the JSON sidecar together.
Record failures as one of:

- exposure placement;
- white balance/color;
- highlight handling;
- shadow handling;
- excessive or weak contrast;
- saturation/gamut;
- noise;
- demosaic detail;
- unsupported camera/file.

That distinction matters because each failure belongs to a different subsystem.

## Deliberate omissions in 0.1

- ONNX inference and semantic pre-analysis;
- face/subject/sky detection;
- semantic or face-aware local tone control;
- profiled RAW denoising;
- robust clipped-highlight reconstruction;
- lens distortion/vignetting/chromatic-aberration correction;
- DCP hue/saturation maps and camera looks;
- embedded ICC profile and copied EXIF metadata;
- scene-linear EXR export;
- GPU processing;
- GUI.

See `docs/ROADMAP.md`, `docs/KNOWN_LIMITATIONS.md`, and `docs/BUILD_STATUS.md`.

## Licensing

The project source is MIT-licensed. It depends on Rawler, which is LGPL-2.1.
Read `THIRD_PARTY.md` before distributing compiled binaries.
