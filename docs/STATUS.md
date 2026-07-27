# Status and handoff — raw-autotune 0.1.9

Written 2026-07-27. Scope as stated: the program has to work on **Samsung S24+
Expert RAW**, **ProShot** output, and **Sony A7C ARW**. This records how far
that is, what to run, and what is left.

## Short answer

Usable for A7C ARW and ProShot today. Expert RAW works but needs one flag, and
rests on a single test file.

| Target | Files on hand | State | Invocation |
|---|---|---|---|
| A7C ARW | 258 | **Works.** 258/258 render, 0 failures | defaults |
| ProShot DNG | 2 | **Works.** 2/2 render | defaults |
| S24+ Expert RAW | **1** | **Works, with a flag** | `--preview-exposure 1.0` |

The one real quality issue that remains is highlight blowout on the A7C under
the `auto` preset, and there is strong evidence the fix is a one-constant change
— see "The most valuable next change" below.

## What to run

```bash
# A7C ARW — the main case. Use `neutral` if blown skies matter: on this corpus
# it removes highlight blowout entirely for about 2 points of colourfulness.
raw-autotune raw/arw --output out --format jpeg --jobs 4 --preset neutral

# Optional, recommended: pool sensor noise per ISO first. The scan takes about
# 13 seconds over 258 files; the profile is reusable and keeps output identical
# whether a file is developed alone or in a batch.
raw-autotune raw/arw --noise-scan sony.json
raw-autotune raw/arw --output out --noise-profile sony.json

# ProShot — nothing special needed.
raw-autotune proshot.dng --output out --format jpeg

# Expert RAW — needs the preview oracle or it renders about 2.6 EV too bright.
raw-autotune expertraw.dng --output out --format jpeg --preview-exposure 1.0
```

Survey a batch without writing images:

```bash
raw-autotune raw/arw --dry-run --summary survey.json
```

`survey.json` carries per-file analysis, chosen tone parameters, sensor noise,
EXIF, and rendered-output metrics. It is the fastest way to test a hypothesis
against the corpus and is how nearly every number in this document was produced.

## How each target actually behaves

### Sony A7C ARW — the strongest case

258 files, true Bayer CFA, uncompressed 14-bit, ISO 100–10000. All 258 decode,
analyse and render with no failures. Rendered output, `auto` preset:

```text
colourfulness   median 36.3        (Hasler & Susstrunk, 0..~110)
mean level      median 118.0 / 255
crushed         median 0.000%      max 0.00%
near-white      median 1.21%       p90 11.51%   max 27.14%
```

Nothing is crushed to black anywhere. The near-white tail is the open issue.

### ProShot — works, and is the most honest phone raw

`proshot.dng` and `20260609_121157.dng` are true CFA Bayer (`cpp=1`,
GBRG mosaic), i.e. undemosaiced sensor data, which is what makes ProShot the
best phone source here. Both render on defaults.

Caveat: ProShot files carry only a **256x191 uncompressed thumbnail**, no JPEG
preview at all. `--preview-exposure` therefore has very little to work with on
them — it is accepted (48 896 pixels clears the 30 000 floor) but it is a
6%-linear thumbnail, not a rendering.

### S24+ Expert RAW — works, but thinly evidenced

`expertraw.dng` is `LinearRaw` (already demosaiced by the phone), JPEG-XL
compressed, 16-bit, with `BaselineExposure` +3 EV which the program applies.

Without `--preview-exposure` it renders about 2.6 EV brighter than Samsung's own
rendering of the same capture. With `--preview-exposure 1.0`:

```text
mean level, oracle off      112.2
mean level, oracle on        64.8
Samsung's own preview        63.0
```

**This rests on one file.** Every constant in the preview oracle was set against
it plus nine other DNGs. Treat Expert RAW support as demonstrated, not validated
— shoot 20–30 more before trusting it.

## The most valuable next change

Under `auto`, 35 of 258 A7C frames (14%) render with more than 10% of the frame
at or above 98% of full scale. For those frames the **scene** near-white
fraction is `0.000%` — the raw holds nothing at saturation, so the blowout is
produced entirely by the render, not by the capture.

`--preset neutral` removes it entirely. Both presets rendered over all 258:

| | `auto` | `neutral` |
|---|---|---|
| near-white median | 1.21% | **0.00%** |
| near-white p90 | 11.51% | **0.06%** |
| near-white max | 27.14% | **2.94%** |
| frames above 10% near-white | **35** | **0** |
| colourfulness median | 36.3 | 34.3 |
| mean level median | 118.0 | 116.9 |

The only difference that matters here is `highlight_norm`: 0.90 for `auto`,
1.00 for `neutral` (`src/analyze.rs`, the preset table in `derive_params`). At
1.00 no channel can clip by construction; at 0.90 it can.

So `auto` trades the complete elimination of highlight blowout — 35 frames down
to 0 — for about 2 points of colourfulness, roughly 5%, with brightness
essentially unchanged. On this corpus that is a bad trade.

**Raising `auto`'s `highlight_norm` from 0.90 toward 1.0 is a one-constant
change and is the highest-value next edit.** The presets were never
systematically swept; sweeping `highlight_norm` over 0.90/0.95/1.00 with a full
render and comparing the near-white and colourfulness columns of `--summary`
would settle it in about fifteen minutes.

Until then, **use `--preset neutral` for A7C work** if blown skies matter more
than the last few points of saturation.

## What is deliberately not done

- **No local tone mapping.** All tone processing is global. This is the main
  reason phone renderings look different: they lift a subject while keeping the
  median low. Listed in `docs/ROADMAP.md` for 0.4.
- **No denoising, sharpening, lens correction or hot-pixel pass.**
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

**Determinism is a product property**, stated in `Cargo.toml`. Anything with a
random seed, or that depends on batch composition or thread scheduling, does not
belong in the render path. `--pool-noise` deliberately breaks batch
independence and says so; `--noise-profile` is the reproducible route.

## Verification you should re-run after any change

```bash
# 1. Gate: output must not move when new features are off.
raw-autotune raw/arw raw/dng raw/files_* --dry-run --summary after.json
# compare against a before.json field by field, ignoring elapsed_ms

# 2. Tests and lints.
cargo test --release        # 70 tests
cargo clippy --lib --bins --release

# 3. Determinism: same input, many runs, --jobs 8, byte-identical sidecars.
```

The gate test caught a real regression during 0.1.9 development and is worth
keeping as the first thing you run.

## Corpus reality check

The test material is thinner than the file count suggests:

| Class | Files | Notes |
|---|---|---|
| A7C ARW | 258 | one photographer, one body, ISO 100–10000 |
| S24+ Pro mode | 7 | `LinearRaw`, lossless JPEG w/ restart intervals |
| ProShot | 2 | true CFA |
| S24+ Expert RAW | **1** | JPEG-XL, `BaselineExposure` +3 |

`camera-promode1.dng` is an outlier worth knowing about: p50 at +2.35 EV with
only 3.92 EV of range, i.e. crammed against the top and clipped. It behaves
unlike the other six Pro-mode files, which sit at −1.7 to −5.1 EV like ordinary
scene-linear raw.

Several tuning constants are set from very few data points and are flagged as
such in `docs/KNOWN_LIMITATIONS.md` and `docs/RESEARCH_NOTES.md`:
`whitebalance::MAX_ILLUMINANT_CAST` (two data points), the preview oracle's
guard rails (ten DNGs), and `analyze`'s preset table (never systematically
swept).

## Where the detail lives

- `docs/RESEARCH_NOTES.md` — which parts of the papers in `research/` apply,
  and which do not. Read before implementing anything from them.
- `docs/KNOWN_LIMITATIONS.md` — honest limits, per subsystem.
- `docs/BUILD_STATUS.md` — what has been built and verified, and on what.
- `CHANGELOG.md` — every change with the measurement that justified it.
