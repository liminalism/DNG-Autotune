# Known limitations

## RAW coverage

Rawler supports many containers and cameras, but its convenient `RawDevelop`
helper does not yet guarantee development of every decoded sensor arrangement.
The CLI catches panics per file and reports unsupported cases.

The first version is expected to work best with ordinary Bayer DNG/ARW/CR2/CR3/
NEF/RW2/ORF files. X-Trans currently uses Rawler's basic bilinear path in this
helper and will not match a mature editor's detail rendering.

## Color

The default owned path uses the DNG 1.7 matrix model when a complete profile is
present, converting to linear sRGB or optional Rec.2020 without Rawler's
destructive clipping. It handles one, two or three calibrations, three or four
camera channels, `ForwardMatrix`, signature-gated `CameraCalibration`,
`AnalogBalance`, custom xy/SPD `IlluminantData`, `ReductionMatrix`, and Bradford
adaptation. Incomplete or unsupported profiles fall back to the decoder camera
matrix. It does not implement the creative/profile-rendering layers:

- DCP hue/saturation maps;
- vendor picture styles;
- a trained camera-look transform.

These are not missing sensor calibration. A DCP hue/saturation map or look
table is an optional creative LUT applied after the camera has already been
converted colorimetrically: it can make skin warmer, foliage greener, skies
deeper, or emulate a vendor picture style. The matrix path answers what colour
was measured; these tables choose how that colour should look.

Mixed-light correction exists separately under `--local-white-balance`; it is
off by default. JPEG, TIFF, and PNG outputs carry an sRGB ICC profile.

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

The automatic profile reconstructs a partially clipped RGB channel from the
surviving channel ratios before colour conversion. Fully clipped pixels and
strongly coloured highlights are deliberately left alone, so the program still
cannot invent detail when every channel is gone. Four-colour RGBE data does not
yet use this reconstruction.

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

Not done: **luma denoising** or semantic local texture control. Hot/dead CFA
suppression and noise-aware output sharpening are automatic and can be scaled
or disabled. Lens correction is available only where standardized metadata
supports it, as described below.

## Lens correction

The automatic owned path reads DNG `OpcodeList3` directly. It supports
`WarpRectilinear` (radial/tangential distortion and one- or per-channel lateral
chromatic aberration) and `FixVignetteRadial`. Operations run in their specified
order immediately after demosaic, in full raw-image coordinates, before
`DefaultCrop` and colour conversion. Invalid coefficients and unknown required
opcodes disable the whole list; unknown optional opcodes are reported and
skipped. `--no-lens-correction` disables the stage.

This does not make EXIF lens identity a correction profile. Most proprietary
RAW files provide a lens name, focal length, focus distance, and aperture but
not portable distortion/vignetting coefficients. Those files remain unchanged.
There is deliberately no generic correction guessed from make/model. DNG
`WarpFisheye`, `WarpRectilinear2`, gain-map opcodes, and vendor MakerNote lens
profiles are not yet implemented.

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

Core EXIF and GPS metadata are copied, orientation is normalized, and JPEG,
TIFF and PNG carry the generated sRGB ICC profile. MakerNotes, embedded
thumbnails, vendor-private blobs and complete XMP packets are not copied.

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
buffers are memory intensive, though they peak below the chroma-denoise stage
that `--jobs auto` budgets for, so they need no separate allowance. The feature
remains off by default pending a more varied RAW corpus.

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
