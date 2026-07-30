# Known limitations

## RAW coverage

Rawler supports many containers and cameras, but its convenient `RawDevelop`
helper does not yet guarantee development of every decoded sensor arrangement.
The CLI catches panics per file and reports unsupported cases.

The first version is expected to work best with ordinary Bayer DNG/ARW/CR2/CR3/
NEF/RW2/ORF files. X-Trans currently uses Rawler's basic bilinear path in this
helper and will not match a mature editor's detail rendering.

## Color

The program uses Rawler's available camera matrix and as-shot white balance.
The convenient first-version development path calibrates directly into linear
sRGB rather than a wider scene-referred working space. It does not yet
implement:

- dual-illuminant matrix interpolation selected from estimated scene light;
- DCP hue/saturation maps;
- vendor picture styles;
- ICC output embedding;
- mixed-light white-balance analysis;
- a trained camera-look transform.

Sunsets, colored stage light, LEDs, and underwater images are intentionally not
"neutralized" by a speculative white-balance algorithm.

Because that path is colorimetric rather than a picture style, its chroma sits
below what a camera's own JPEG engine produces. Measured in 0.1.12 against 29
Samsung RAW+JPEG pairs, the `auto` preset rendered at 0.847 of the camera's
saturation, and the shortfall was flat: the same 0.85 on both ProShot and
Expert RAW files, and roughly constant across luminance and across the chroma
range. `auto`'s `saturation` now carries a 1.20 correction that closes it. Two
things this does *not* fix, both of which need the missing pieces listed above:
the correction is one global scalar, so it cannot reproduce a per-hue picture
style; and it corrects a level, not a rendering intent.

0.1.13 validated it on a second sensor: on 13 Sony A7C pairs the `auto` preset
renders at a 0.981 saturation ratio against the camera's own JPEG, having been
fitted entirely on Samsung. The per-frame spread is wider on Sony (0.73 to 1.37)
than on the phones, which is what a single global scalar standing in for a
per-hue table looks like — right on average, approximate per scene.

Since 0.1.7 an opt-in local multi-illuminant correction exists
(`--local-white-balance`, `src/whitebalance.rs`, after fierro2009). It stays off
by default precisely because of the paragraph above: run unguarded it renders a
campfire green. It acts only on frames with two or more chromatically distinct
illuminants, and refuses lights more than 0.22 from neutral in chromaticity on
the grounds that they are the subject, not a cast. It has been validated on 7
phone DNGs only.

## Highlights

The program compresses bright scene-linear values but does not reconstruct
sensor channels that were already clipped. It can retain highlight gradation
that exists in the developed f32 buffer; it cannot invent missing channel data.

The tone curve's highlight segment is driven by a blend of luminance and the
brightest channel (`highlight_norm`), because a luminance-only curve clips
saturated highlights in a single channel before luminance reaches white. Above
half the highlight range a pixel at `highlight_norm == 1.0` cannot clip at all.
The `neutral` and `auto` presets use 1.0; `punchy` remains below it and permits
some clipping in exchange for contrast.

One interaction remains: pulling a scene highlight down into the midtone range
increases its adaptive vibrance weight, which can push a very saturated pixel
back out of gamut. It was measurably worse on 1 of 14 test frames (2.3% -> 8.7%
of pixels clipped) while the batch average more than halved.

0.1.12's saturation correction acts after the curve, so it works through the
same gamut boundary and widens that interaction: on the 29-pair corpus the
worst frame went from 0.000% to 1.445% of pixels with a channel at full scale.
That frame is a wall of saturated brick, and the camera's own JPEG of it clips
1.422% — so the ceiling being hit is the sRGB gamut, not a defect in the curve.
Every other frame in the corpus clips well below its camera JPEG. The bound is
what stopped the correction at 1.20: at 1.30 the median frame clipped 0.308%
against 0.001%, which is a real loss rather than a gamut coincidence.

## Noise and detail

Chroma noise reduction exists since 0.1.14 (`src/chroma.rs`), automatic and
driven by the frame's own fitted noise model. It is inert on clean frames — 309
of 368 corpus files — and cannot alter luminance at all, by construction.

Not done: **luma denoising**, hot-pixel suppression, chromatic-aberration
correction, output sharpening, local texture control.

Two limits of the chroma filter specifically:

- It is a fixed-radius box on the colour difference, so it removes speckle but
  not **low-frequency chroma blotching**, which at extreme ISO leaves the frame
  at about 1.40x the camera's saturation. A wider box would reach it only by
  smearing colour across real edges; a multi-scale or edge-aware filter is the
  actual fix.
- Luma grain is untouched by design, so a high-ISO frame still looks grainier
  than the camera's own JPEG — though the camera's is also visibly mushier, and
  which is preferable for an archive is a taste call this project has not made.

High-ISO shadow lifting can still expose luma noise.

Since 0.1.6 a per-image sensor noise model (`src/noise.rs`) bounds the black
point: it is never placed below the level where SNR falls to 1, so shadow range
is not spent stretching pure noise. This binds on 12% of the Sony batch.

The model is validated against EXIF ISO in aggregate: `shot_slope` tracks ISO
at +1.039 in log2 against a theoretical 1.000, R^2 0.976. It is not validated
per frame, where dense texture still inflates the fit, so the floor is capped at
the frame's 5th percentile and a bad estimate cannot crush more than about 5% of
an image. 257 of 258 frames yield an estimate; the one refusal has too little
dynamic range to determine a slope. See `docs/RESEARCH_NOTES.md`.

## Automatic decisions

The controller does not identify people, faces, sky, foreground, night scenes,
snow, documents, products, or intended subjects. Center weighting is only a
weak composition prior.

Failures on high-key, low-key, strongly backlit, or unusual photographs are
expected and should be retained as test cases for the next controller.

Since 0.1.13 the controller's exposure decision is usually not the one that
ships. Where the file carries a real embedded preview — every source here except
ProShot — the target is taken from the camera's own rendering instead, because
measured against 21 paired camera JPEGs that reduces the mean subject error from
0.4-0.6 EV to under 0.05 EV. Two consequences worth being explicit about:

- **On those files the program largely inherits the camera's judgement,
  including its mistakes**, and is not making an independent exposure decision.
  That is the right trade for an archiving tool whose bar is "at least as good
  as the camera JPEG", and the wrong one for a tool trying to beat it.
- **But not unconditionally.** `MAX_ORACLE_DEVIATION_EV` bounds how far the
  oracle may pull the target from the controller's own, and 0.1.14 found five
  night frames where that bound is what saves them: Sony's own JPEG renders an
  ISO 2500 waterfall nearly black, and the clamp leaves us 3.17 EV brighter and
  usable. So the program *can* beat the camera at exposure, on exactly the
  scenes where the camera is worst — but only by the width of that guard, and
  by accident of its design rather than by any judgement of its own.
- **On ProShot there is no preview to fall back on**, so the aim-the-median rule
  runs undiluted there. Its failures are visible on that source alone, which
  makes ProShot the place to test controller changes.

The `+1.0 EV` ceiling on an oracle-derived target (`ORACLE_TARGET_CEILING_EV`)
binds on 16 of 287 files with previews and is probably too tight — most of them
are independently classified `high_key`, i.e. genuinely bright rather than
badly previewed. No paired JPEG exists for any of the 16, and 0.1.14's beach and
high-variance pairs did not reach that regime either (brightest camera subject
+0.99 EV), so it has not been changed. See `docs/STATUS.md`.

## Output metadata

EXIF, XMP, GPS, camera profile, and thumbnails are not copied. TIFF and PNG are
written as 16-bit RGB pixel files, but v0.1 does not embed an ICC profile.
Consumers should currently interpret them as sRGB-encoded RGB.

JPEG is necessarily 8-bit.

## Local contrast

An opt-in first pass exists as `--local-tone`. It chooses a Gaussian surround
per pixel and applies bounded local exposure before the global view transform.
It is deterministic, noise-aware, and tested against synthetic ramps and hard
edges, but it is not edge-aware in the bilateral/guided-filter sense. Strong
edges can therefore still show broad adaptation transitions, particularly at
maximum strength.

It has no semantic knowledge: it cannot recognize a face, sky, skin, snow, or
the intended subject. It also does not reproduce a phone pipeline's denoising,
sharpening, segmentation, or multi-frame fusion. The full-resolution working
buffers are memory intensive, so `--jobs 1` is recommended for large files.
The feature remains off by default pending a more varied RAW corpus.

## Decoder corrections applied on top of Rawler

Rawler 0.7.2 mishandles some valid DNGs. raw-autotune corrects two cases before
development and records what it did in the sidecar's `level_normalization`
field, so every adjustment is auditable per file.

### Lossless JPEG restart intervals

Rawler's lossless JPEG decoder ignores the `DRI` marker and its bit pump never
resynchronizes on the `RSTn` markers in the entropy stream. Affected files
decode to a smooth diagonal ramp with no scene content — and Rawler reports
success, so nothing downstream can tell. Samsung Galaxy linear DNGs are written
this way (a restart every 16 rows).

`src/ljpeg.rs` implements the T.81 Annex H lossless process with restart
handling, and `src/redecode.rs` substitutes its output. The substitution is
deliberately narrow: it requires the raw IFD to be lossless-JPEG compressed
*and* its first stream to declare a nonzero restart interval. Files Rawler
decodes correctly are untouched.

Restrictions of the replacement decoder:

- subsampled scans (any component with `H` or `V` other than 1) are rejected
  rather than guessed at;
- multi-scan streams are rejected;
- tile reassembly is implemented but has not been tested against a real tiled
  file.

### Sensor level layout and domain

Linear DNGs that record a repeating black-level pattern (for example 2x2 by
`cpp`, giving 12 levels) alongside one white level per component make Rawler
panic. A repeat pattern only carries information for CFA data, so for linear RGB
the levels are collapsed to one per component.

A second correction handles decoders that emit samples outside the range
`BitsPerSample` allows, which happens when a decompressor scales its output into
the full 16-bit storage container and leaves the recorded levels behind. It
triggers only on that hard inconsistency — a sample above `2^bps - 1` — and it
therefore depends on the frame containing at least one such sample. A frame
underexposed by more than `16 - bps` stops across its entire area would not
trip it.

## Build verification

See `docs/BUILD_STATUS.md`. The code builds, tests, and lints clean on Windows
with Rust 1.89.0 and has been run over a real DNG batch. Linux and macOS builds
have not been verified.
