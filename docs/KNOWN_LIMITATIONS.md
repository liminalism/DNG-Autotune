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
half the highlight range a pixel at `highlight_norm == 1.0` cannot clip at all;
the `auto` and `punchy` presets sit below 1.0 and so still permit some clipping
in exchange for contrast.

One interaction remains: pulling a scene highlight down into the midtone range
increases its adaptive vibrance weight, which can push a very saturated pixel
back out of gamut. It was measurably worse on 1 of 14 test frames (2.3% -> 8.7%
of pixels clipped) while the batch average more than halved.

## Noise and detail

There is no profiled denoising, hot-pixel pass, chromatic-aberration correction,
output sharpening, or local texture control. High-ISO shadow lifting can expose
noise.

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

## Output metadata

EXIF, XMP, GPS, camera profile, and thumbnails are not copied. TIFF and PNG are
written as 16-bit RGB pixel files, but v0.1 does not embed an ICC profile.
Consumers should currently interpret them as sRGB-encoded RGB.

JPEG is necessarily 8-bit.

## Local contrast

All tone processing is global. The program does not lift a face independently
of a bright sky or apply phone-style multi-scale local adaptation.

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
