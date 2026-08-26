# raw-autotune 0.1.19

`raw-autotune` is a small Rust command-line RAW developer intended for testing a
standalone, batch-oriented equivalent of the useful part of a photo editor's
automatic adjustment workflow.

This is a **workable first version**, not a Lightroom or darktable replacement.

It currently:

1. decodes DNG and other camera RAW formats through Rawler;
2. normalizes sensor black/white levels itself, keeping sub-black samples;
3. suppresses isolated hot/dead CFA sites and adaptively chooses mature PPG or
   guarded owned RCD/AMaZE-class interpolation for ordinary Bayer data;
4. reconstructs partially clipped RGB highlights and applies the DNG matrix
   model (or the decoder camera matrix fallback) into scene-linear RGB without
   destructive gamut clipping;
5. applies standardized DNG distortion, lateral chromatic-aberration and
   vignetting instructions when the file supplies them;
6. samples scene-linear RGB statistics;
7. estimates exposure, usable black/white EV limits, contrast, saturation, and
   vibrance;
8. reduces chroma noise and sharpens, both scaled by the frame's own fitted
   sensor-noise model;
9. optionally builds a full-resolution, multi-scale local exposure map;
10. applies a hue-preserving, middle-gray-anchored view transform;
11. writes 16-bit TIFF/PNG or 8-bit JPEG (the latter through the
    `jpeg-encoder` crate), with the source EXIF and an sRGB ICC profile;
12. writes one JSON batch summary by default; per-image sidecars remain
    available with `--sidecar`.

Everything from the black level onward is this program's own for ordinary Bayer
files; Rawler supplies container parsing, camera data and decompression. Its PPG
demosaic remains available as an explicit diagnostic control, and its bilinear
four-colour path covers uncommon RGBE mosaics.

No ONNX model is required. Neural pre-analysis is deliberately deferred.

## What to expect

The current version should be useful for comparing its automatic decisions on a
real batch of photographs and for identifying where semantic analysis, luma
denoising, unprofiled lenses, or creative camera-profile work will matter.

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

## Interactive mode

Run the executable with **no arguments** for the minimal automatic front-end:

```bash
raw-autotune
```

It asks only for one or more RAW files/folders and an output directory, then
starts. The versioned `archive-auto-v6` profile chooses colour, harmonic
highlight reconstruction,
corrections, tone, metadata, JPEG settings and safe concurrency. Passing any
argument uses the non-interactive CLI, with the same defaults and optional
expert overrides.

## First commands

Single image, automatic archive JPEG:

```bash
raw-autotune photo.dng
```

Choose output directory:

```bash
raw-autotune photo.dng --output processed
```

Batch a directory recursively (the default):

```bash
raw-autotune raw-folder --output processed
```

How many full-resolution images are held in flight at once is decided from the
memory the machine reports free and the size of the largest input, and the run
header says what it chose and why:

```
raw-autotune v0.1.19 | 376 file(s) | profile=archive-auto-v6 | preset=auto | concurrent images=5 \
  (auto: 22.76 GiB available, 2.67 GiB per image at 49.9 MP)
```

Request a lower ceiling when something else on the machine needs the memory, or
to pin a measurement (each image still uses the shared Rayon CPU pool whatever
this is). A numeric request is an upper bound: the safety planner lowers it when
necessary, and refuses to decode if its conservative half-`MemAvailable` budget
cannot hold one frame:

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

## In-memory library API

An embedding crate does not need to invoke the executable or exchange temporary
files. `api::render_file` returns an owned, self-describing packed RGB buffer
plus the analysis and processing report:

```rust
use raw_autotune::api::{PixelFormat, RenderOptions, render_file};

let mut options = RenderOptions::automatic();
options.pixel_format = PixelFormat::Rgb8Srgb;
let image = render_file("photo.dng", &options)?;

assert_eq!(image.row_stride, image.width as usize * 3);
sender.send(image)?; // move the pixels and report to another crate/thread
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Rgb8Srgb` is packed RGB and is usually the best IPC/network payload before
compression. `Rgb16SrgbBe` preserves the 16-bit render and gives every channel
an explicit network-friendly byte order. The result owns its `Vec<u8>`, is
`Send`, and performs no output writes. See `examples/in-memory.rs`.

In-process high-bit-depth encoders should use `api::render_file_rgb16` instead.
It returns the tone renderer's native-endian `Vec<u16>` allocation directly,
without byte packing, an image-file intermediate, or another RGB conversion.

It also returns everything such an encoder needs to write a faithful archive
file without reopening the source RAW:

```rust,no_run
use raw_autotune::api::{RenderOptions, render_file_rgb16};

let image = render_file_rgb16("photo.ARW", &RenderOptions::automatic())?;

// The colour space is stated rather than assumed. `--working-space` selects the
// *intermediate* space and is converted back before the tone curve, so this is
// `Srgb` in every current configuration — read the field, do not hardcode it.
let space = image.color_space;

// A bare EXIF TIFF structure: no `Exif\0\0` header, no APP1 framing, all
// offsets relative to the start of the buffer. Byte-identical to what the file
// writers embed, with orientation already normalized. This is what a JPEG XL
// encoder's `with_exif` expects.
if let Some(exif) = image.exif.as_deref() { /* encoder.with_exif(exif) */ }

// The generated matrix-shaper ICC profile for `space`.
if let Some(icc) = image.icc.as_deref() { /* encoder.with_icc(icc) */ }
# let _ = (space,);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`RenderOptions::metadata = false` leaves both `None` and skips the one extra
metadata parse; it never changes a pixel. `render_file` carries the same three
fields.

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

The DNG matrix model is automatic. Disable it only for a controlled comparison:

```bash
raw-autotune photo.dng --no-dng-color
```

Apply a manual bias on top of automatic exposure:

```bash
raw-autotune photo.dng --exposure-bias -0.35
```

Apply local tone adaptation at half strength:

```bash
raw-autotune photo.dng --local-tone 0.5
```

Run a pre-demosaic highlight experiment at its required full strength:

```bash
raw-autotune photo.ARW --highlight-method raw-pyramid \
  --highlight-reconstruction 1
raw-autotune photo.ARW --highlight-method harmonic \
  --highlight-reconstruction 1
```

**`harmonic` is the default** — this paragraph used to say `current` was, which
stopped being true when the harmonic estimator was promoted. `current` remains
available and is what a frame falls back to when the spatial solve is declined.
The spatial methods keep trusted Bayer sites exact and preserve the original raw
clip map for downstream uncertainty. A partial reconstruction strength is
rejected because blending two estimators can recreate the very clip-boundary
contour these methods are meant to measure.

The spatial solvers cost roughly 6 to 46 seconds and 1.3 GB on a 24 MP frame,
and most of that is paid whether or not anything in the frame is clipped. So the
archive profile does not run one on a frame that has essentially no clipped
sites:

```bash
# The default: skip the solve below 0.001% clipped CFA sites.
raw-autotune photo.ARW

# Always solve, whatever the frame looks like.
raw-autotune photo.ARW --spatial-highlight-floor 0
```

A declined frame is byte-identical to `--highlight-method current` on the same
file. `clipped_cfa_sites` in the sidecar is the statistic the floor is compared
against, so a batch survey shows exactly which frames would be affected. See
`docs/KNOWN_LIMITATIONS.md` for what the floor does and does not buy.

Override the archive JPEG quality:

```bash
raw-autotune raw-folder --format jpeg --jpeg-quality 94
```

JPEG is written with the [`jpeg-encoder`](https://crates.io/crates/jpeg-encoder)
crate. The automatic archive settings are quality 95, baseline, optimized
Huffman tables and 4:4:4 chroma; the flag CLI exposes overrides:

```bash
raw-autotune raw-folder --format jpeg --jpeg-quality 90 \
  --jpeg-subsampling 420 --jpeg-progressive --no-jpeg-optimize
```

* `--jpeg-quality <1..100>`
* `--jpeg-subsampling <auto|444|422|420>` — `auto` is 4:4:4 at quality ≥ 90 and
  4:2:0 below it
* `--jpeg-progressive` — progressive rather than baseline
* `--no-jpeg-optimize` — skip the default optimized Huffman pass

The source EXIF (`APP1`) and the sRGB ICC profile (`APP2`) are embedded exactly
as they are for TIFF and PNG.

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
full resolution and memory intensive, though it peaks below the chroma stage
that `--jobs auto` already budgets for, so it needs no separate allowance. The
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

Two companions to it, added in 0.1.18 because the whole-frame figure turns out to
be blind to things that matter:

- **`highlight_saturation_ratio`** restricts the same comparison to pixels above
  0.7 luminance. On the current corpus the whole-frame ratio is 1.05 — nearly a
  perfect match — while the highlight band is **1.53**. The camera desaturates its
  shoulder hard and this program does not. That is a difference of intent, not an
  error, so it is reported rather than corrected.
- **`hue`** compares the two renderings *per pixel* in Oklab, on a canonical
  512-pixel grid that both are box-averaged onto. Aggregate statistics cannot see
  a hue shift, because a frame's mean hue is whatever colour covers most of it.
  The comparison declines when the two renderings are different crops, and says so
  rather than reporting a zero.

Read the hue numbers with `mean_degrees`, not `median_degrees`. A colour change
usually affects a minority of pixels, so a real improvement can leave the median
untouched: between the two colour paths the corpus median hue change is 0.00° while
the best single frame improves by 26°.

For visual triage, `tools/contact-sheet.py` turns a summary into an HTML page
of side-by-side pairs with the numbers under each, worst-first:

```bash
tools/contact-sheet.py out/summary.json --output out/sheet.html
```

### Two scorecards, not one

The preview oracle borrows the camera's judgement about a scene, which is free
and worth having — but it means a pooled median cannot distinguish "the
controller got better at judging scenes" from "more files happened to carry a
usable preview". So every summary splits its reference measures by
`guidance_mode`, and each sidecar records which mode its file used:

- `independent` — the controller chose the exposure target by itself, because the
  file carries no usable preview or the oracle was switched off;
- `preview_guided` — the camera's embedded preview supplied the target.

`--no-preview` renders the independent arm deliberately, and
`tools/contact-sheet.py --guidance independent` sheets it alone. Report both.

## Owning the colour conversion

Since 0.1.18 the program owns everything from black-level normalization onward,
and the current release adds the ordinary-Bayer demosaic. That was not always
so: it used to develop through
Rawler's `Rescale` and `Calibrate`, and both destroyed data before this program saw
it. `Calibrate`'s per-pixel tail clips out-of-gamut channels to zero and rewrites
every pixel with a channel above 1.0 — which, after as-shot white balance, is much
of the highlight range of many frames. `Rescale` clips sensor samples below the
black level, rectifying the noise floor so that a black region cannot render black.

`--raw-color-path` selects between them, and **`owned` is the default**:

```bash
# The default. Same matrix composition Rawler uses, nothing discarded.
raw-autotune folder --output out --format jpeg

# The control arm, and how to reproduce pre-0.1.18 output.
raw-autotune folder --output out --format jpeg --raw-color-path rawler
```

The default was flipped on measurement, not preference — see below.

`--working-space` chooses the linear RGB space the owned path converts into:
`srgb` (the default) or `rec2020`, wide enough to hold nearly every real camera
colour. It has no effect on the Rawler path, which is hardcoded to sRGB primaries.
BT.2020 was measured and is *not* clearly better: it wins on near-white but gives
back local detail for no additional hue accuracy, so it stays opt-in.

The default DNG path implements the DNG 1.7 matrix model: one, two or three
numbered `ColorMatrix`/`ForwardMatrix` sets, signature-gated
`CameraCalibration`, `AnalogBalance`, custom `IlluminantData`, Bradford
adaptation, and the no-ForwardMatrix route. Three- and four-camera-channel
profiles are accepted; four-channel profiles can use `ReductionMatrix`.
`AsShotNeutral` and `AsShotWhiteXY` are both supported. Reports record the
estimated CCT, interpolation weights, camera-channel count, custom illuminants,
white-balance source and matrices used. Incomplete profiles safely fall back to
the decoder camera matrix.

The automatic lens path is similarly metadata-driven. DNG `OpcodeList3`
`WarpRectilinear` instructions are applied after demosaic and before
`DefaultCrop`, in the raw IFD's coordinate system; they can correct radial and
tangential distortion plus per-channel lateral chromatic aberration.
`FixVignetteRadial` gain instructions are also supported. The local ProShot
phone DNG applies its embedded warp with a measured maximum displacement of
about 10 pixels. `--lens-correction embedded` is the default and
`--no-lens-correction` remains an alias for `off`.

`--lens-correction profile-exact` opts into the pinned Lensfun database for
non-DNG files. It requires exactly one canonical camera/lens match, applies
distortion and transverse chromatic aberration but not vignetting, and reports
the database version, identities, components, and per-channel displacement.
Missing or ambiguous metadata leaves the frame unchanged; no fuzzy candidate is
ever selected unattended.

### What the missing DCP tables are

DCP means **DNG Camera Profile**. The matrices above answer a colorimetric
question: “what real colour does this sensor response represent?” A DCP can
also contain hue/saturation/value lookup tables (`ProfileHueSatMap*` and
`ProfileLookTable*`) and a profile tone curve. Those answer an aesthetic
question: “how should this camera render foliage, skin, sky, and contrast?”
They are closer to a camera picture style or film look than to basic RAW
compatibility. raw-autotune currently performs the calibrated matrix conversion
and its own tone rendering, but does not reproduce those optional creative
tables.

On the owned path each sidecar's `color` block reports what the clipping would
have cost: the fraction of pixels Rawler's operator would have moved, split by
cause, and how far. Over the current corpus that is a median of 0.215% of pixels
but a maximum of 66% — most frames barely care, and a minority are wrecked.

The owned path lost on crushed shadows in 0.1.17, and the reason turned out to be a
real defect downstream rather than a scoring quirk: `luminance` is a signed sum, so
a pixel with a large negative channel has negative luminance, and the tone curve's
chroma anchor clamped that to 0 — which made the gamut compressor's scale exactly 0
and zeroed every channel, including the positive ones. 0.1.18 floors that anchor at
the curve's own black point, and the regression is gone: `crushed_fraction` is now a
108-way tie where it was 21 losses, with the local-detail win intact, and the owned
path is measurably **closer to the camera's hue** than rawler on the frames where
the clip acts (median −0.905° over the 30 most out-of-gamut frames).
`clip_cost.negative_luminance_fraction` reports the population that was affected and
predicted the regression exactly.

With that fixed, `owned` beats `rawler` on the scorecard, which is what earned it the
default: 84 frames better and 24 worse on local detail, a 108-way tie on crushed
shadows, and closer hue. Its only loss is `luminance_entropy` — by a median of four
millionths of a bit against a scale of 7.5, on a metric whose maximiser is histogram
equalisation. See `CHANGELOG.md` 0.1.18 and `docs/STATUS.md`.

Sub-black sensor samples are also kept rather than clipped, which is what lets a
black region render black. That one was decided by looking, against the metrics:
clipping rectifies the noise floor, and since white balance then multiplies red by
about 2.3 and blue by 1.6 against green at 1.0, the residue is a **magenta cast
across the shadows** — obvious on a night frame, and invisible to every axis on the
scorecard. Three of the four axes actually got *worse* when it was fixed, because
what turns black is noise that used to turn into coloured haze. `--sub-black clip`
restores the old behaviour if you need it.

`--dump-stages DIR` writes the scene-linear intermediate at each pipeline stage
as plain sRGB-encoded PNGs with no tone curve applied, which is how to compare
the two paths' highlights by eye.

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

## Metadata in the output

Output is written to be a drop-in replacement for the camera's own JPEG in a
photo library, so it carries the capture metadata: date and time with its
sub-second and time-zone companions, make, model, the lens group, exposure time,
aperture, ISO, focal length, the metering/flash/exposure-program group, artist
and copyright, and the whole GPS block when the source has one. `Software` is set
to `raw-autotune <version>`, and a generated sRGB v2 ICC profile is embedded.
JPEG gets APP1/APP2, TIFF an ExifIFD plus GPSInfo, PNG `eXIf` plus `iCCP`.

Three things to know:

- **Orientation is written as 1, deliberately.** The pixels are already rotated
  upright by the time they are encoded, so copying the source's orientation tag
  would rotate the image a second time in every viewer.
- **PNG's EXIF is widely ignored.** `eXIf` is a 2017 addition and reader support
  is thin — Windows' own property handlers do not read it. The ICC profile in
  `iCCP` is universally supported. Prefer JPEG or TIFF if a library has to see
  the tags.
- **MakerNotes are not copied.** Vendor note blobs contain absolute file offsets,
  so relocating them into a different container corrupts them.

`--no-metadata` writes no EXIF and no profile, which restores the exact bytes
earlier versions produced. The pixels are identical either way.

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
raw-autotune test-raws --output results --emit-baseline
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
- default-on profiles for files without standardized DNG opcodes, profile
  vignetting, and newer `WarpRectilinear2`/fisheye opcode variants;
- DCP creative hue/saturation maps and camera looks;
- scene-linear EXR export;
- GPU processing;
- GUI.

See `docs/ROADMAP.md`, `docs/KNOWN_LIMITATIONS.md`, and `docs/BUILD_STATUS.md`.

## Licensing

The project source is licensed **AGPL-3.0-or-later** (see `LICENSE`). Its RAW
decoder and optional lens-profile engine are LGPL-family dependencies; the
bundled Lensfun calibration database is CC-BY-SA 3.0. See `THIRD_PARTY.md` for
the versions, attribution, and redistribution notes.

It was MIT until 0.1.18. The change is deliberate: this program interfaces with a
codebase of GPL-family RAW tooling, and matching their licence removes a standing
constraint rather than adding one. Concretely, it means the demosaic quality work
that `docs/REVIEW-2026-07-30.md` rejected on licence grounds — RawTherapee's RCD
and AMaZE, and darktable's algorithms, all GPL-3 — is now an engineering decision
instead of a legal one. Versions already distributed remain available under MIT to
whoever received them; that cannot be and is not being retracted.

One clause worth knowing rather than discovering: AGPL §13 requires offering source
to users who interact with the program *over a network*. This is a local batch CLI,
so it never triggers, and in practice the licence behaves as GPL-3 here. It would
start to matter if the program were ever put behind a service.

Read `THIRD_PARTY.md` before distributing compiled binaries.
