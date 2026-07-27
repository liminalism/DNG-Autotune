# Third-party notices

This source package does not bundle third-party model files or compiled
libraries. Cargo retrieves its dependencies when the project is built.

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

## Other Rust dependencies

The direct dependencies are listed in `Cargo.toml`. Their transitive dependency
set is determined by Cargo. Before public binary distribution, generate and
review a complete license inventory with a tool such as `cargo-about` or
`cargo-deny`, and retain all required notices.
