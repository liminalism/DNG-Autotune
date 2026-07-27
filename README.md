# raw-autotune 0.1.0 prototype

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
7. applies a hue-preserving, middle-gray-anchored global view transform;
8. writes 16-bit TIFF/PNG or 8-bit JPEG;
9. writes a JSON sidecar containing all measured statistics and selected
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

Correct mixed lighting (off by default, see below):

```bash
raw-autotune photo.dng --local-white-balance 0.7
```

Apply a manual bias on top of automatic exposure:

```bash
raw-autotune photo.dng --exposure-bias -0.35
```

Create JPEG previews:

```bash
raw-autotune raw-folder --format jpeg --jpeg-quality 94
```

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

## Preview-driven exposure

`--preview-exposure <0..1>` takes the exposure target from the camera's own
embedded preview instead of aiming the median at middle grey.

Off by default, because it changes the exposure of every file that has a
preview. Use it when the controller's "middle grey always" rule is wrong for
your material — night scenes especially, which it otherwise renders as day.

The preview is located through the TIFF directory rather than by scanning for
JPEG markers, which would find auxiliary gain and depth maps instead. Its
brightness is inverted back through the tone curve before use, since the preview
is a display-referred rendering and the controller's target is not. Guard rails
cap how far it may move the target and refuse previews that are blown, flat or
too small.

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

## Presets

### `neutral`

Conservative contrast, wider highlight/shadow safety margins, and no vibrance
boost. It still estimates exposure.

### `auto`

Default general-purpose rendering. It retains a larger tonal range than
`punchy`, applies moderate contrast, and makes a small adaptive colorfulness
adjustment.

### `punchy`

Tighter endpoints, stronger midtone contrast, and a larger colorfulness boost.
This is useful for learning whether the default controller is simply too
restrained, but it is more likely to overprocess difficult files.

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
- local tone mapping;
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
