# Third-party notices

This source package does not bundle third-party model files or compiled
libraries. Cargo retrieves its dependencies when the project is built.

## This project's own licence

**AGPL-3.0-or-later** since 0.1.18 (MIT before that); see `LICENSE`. The change
was made so that GPL-family RAW tooling stops being off limits — see
`CHANGELOG.md` 0.1.18. Since the program itself is now strong-copyleft, the
practical distribution question is no longer "may we use this dependency" but
"have we met the obligations of everything we ship", which is what the rest of
this file is about.

## Rawler

- Project: `dnglab/dnglab`, crate `rawler`
- Version selected here: 0.7.2
- License declared by the crate: LGPL-2.1

Rawler provides RAW container decoding, metadata, normalization, demosaic, white
balance, crop, and initial color calibration used by this prototype.

Rust commonly links crate code into the resulting executable. Anyone
redistributing compiled binaries should review the LGPL-2.1 obligations and
include the applicable notices/source or relinking provisions required for the
chosen distribution method. This file is not legal advice.

Linking an LGPL library from a copyleft program is the case the LGPL was written
for, so this crate being AGPL-3.0-or-later raises no new question here. Note that
nothing in this project copies Rawler source into its own files: `src/ljpeg.rs`
and `src/color.rs` reimplement behaviour Rawler gets wrong, written from
specifications and from the observed defect, not transcribed. That was originally
a licence necessity and is now merely good hygiene — it keeps the provenance of
every file in `src/` unambiguous.

## Other Rust dependencies

The direct dependencies are listed in `Cargo.toml`. Their transitive dependency
set is determined by Cargo. Before public binary distribution, generate and
review a complete license inventory with a tool such as `cargo-about` or
`cargo-deny`, and retain all required notices.
