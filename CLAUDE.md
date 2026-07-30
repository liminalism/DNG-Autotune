# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A batch RAW developer in Rust: fully automatic RAW→JPEG/TIFF/PNG rendering for
archiving, replacing the camera's internal JPEG engine — not an interactive
editor. Target sources: Sony A7C ARW, Samsung S24+ Expert RAW / Pro mode DNG,
ProShot DNG. `docs/PLAN.md` is the current development plan; `docs/STATUS.md`
records what actually works per source and the standing verification steps.

## Commands

```bash
cargo build --release          # Rawler 0.7.2 requires Rust 1.89 (pinned in rust-toolchain.toml)
cargo test --release           # full suite (~83 tests)
cargo test --release <name>    # single test by substring
cargo clippy --lib --bins --release
```

Debug probes (library examples that use the *corrected* decode path — see
"Determinism and invariants" below for why that matters):

```bash
cargo run --release --example probe-decode -- file.dng dump.png --raw   # what Rawler alone produced
```

Typical runs, batch survey (`--dry-run --summary`) being the main analysis tool:

```bash
raw-autotune raw-folder --output processed --format jpeg --jobs 4
raw-autotune raw-folder --dry-run --summary survey.json
```

Test RAW files live under `raw/` (not distributable, ~258 A7C ARWs + a few
phone DNGs).

## Regression gate — run after any change

From `docs/STATUS.md`, in this order:

1. **Output must not move when new features are off**: run
   `--dry-run --summary after.json` over the corpus and diff field-by-field
   against a before-run (ignore `elapsed_ms`).
2. `cargo test --release` and clippy.
3. Determinism: same input, repeated runs with `--jobs 8`, byte-identical
   sidecars.

New opt-in features must be byte-identical to the old path at strength 0
(e.g. `--local-tone 0` is guaranteed byte-identical to omitting it).

## Architecture

Data flow, one file at a time (`pipeline::process_job` orchestrates; panics
per file are caught so a batch survives bad inputs):

1. **Decode + repair** — `rawler::decode_file`, then `lib.rs::apply_corrections`:
   `redecode.rs` decides whether to substitute the in-house lossless-JPEG
   decoder (`ljpeg.rs`) for Rawler's broken one, and `levels.rs` reconciles
   black/white levels. Every repair is recorded in the sidecar's
   `level_normalization` field.
2. **Develop to scene-linear RGB** — Rawler's `RawDevelop` step list with
   `ProcessingStep::SRgb` deliberately omitted; this crate owns the view
   transform.
3. **Analyze** — `analyze.rs` samples EV percentiles/statistics, classifies
   tonality, and chooses `ToneParams` per preset. `noise.rs` fits a per-frame
   sensor noise model that bounds the black point; `noiseprofile.rs` pools it
   per camera/ISO. `preview.rs` optionally extracts the camera's embedded
   preview as an exposure oracle.
4. **Optional local operators** — `whitebalance.rs` (multi-illuminant, opt-in)
   and `localtone.rs` (Gaussian-surround local exposure, opt-in, memory-heavy).
5. **Render + write** — `tone.rs` applies the hue-preserving, middle-gray-
   anchored view transform; `metrics.rs` measures the output; `output.rs`
   writes the image plus a JSON sidecar (`types.rs` defines the schema;
   bump `REPORT_SCHEMA_VERSION` when changing it).

The crate is a library so the `probe-*` examples exercise the exact decode
path the binary uses.

## Determinism and invariants

- **Determinism is a product property.** No random seeds, no dependence on
  batch composition or thread scheduling in the render path. `--pool-noise`
  is the one documented exception; `--noise-profile` is the reproducible
  route.
- A file must develop identically alone or in a batch.

## Known traps (hard-won; full list in docs/STATUS.md and docs/KNOWN_LIMITATIONS.md)

- **Rawler 0.7.2 silently mis-decodes lossless-JPEG DNGs that use restart
  intervals** (Samsung linear DNGs): output is a smooth diagonal ramp,
  reported as success. `ljpeg.rs`/`redecode.rs` exist for this; if a new
  camera produces gradient-looking garbage, look there first.
- **Never locate embedded previews by scanning for JPEG markers** — Samsung
  appends a gain map in the same strip and the preview embeds a thumbnail
  whose EOI comes first. Use the TIFF directory (see `preview.rs`).
- `tiff.root_ifd().find_ifds_with_tag(...)` only sees the first IFD; use the
  `TiffReader` trait method to reach chained/Sony preview IFDs.
- **Sort IFD candidates explicitly** — `find_ifds_with_filter` walks a
  `HashMap`, so its order is nondeterministic per process.

## Docs and research

- `docs/RESEARCH_NOTES.md` — which parts of the papers in `research/` apply
  and which do not. Read it before implementing anything from a paper.
- `CHANGELOG.md` records every change with the measurement that justified it;
  keep doing that.
- In `research/`, read the markdown conversions in `research/markdowns/`;
  do not read the PDFs in `research/original-pdfs-*` (the directory name says
  so — it wastes tokens).
