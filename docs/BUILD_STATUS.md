# Build and validation status

## Verified

Built, tested, linted, and batch-rendered on Linux with Rust 1.94.1
(`x86_64-unknown-linux-gnu`) on 2026-07-28:

```text
cargo test --release                       # 83 passed
cargo clippy --all-targets --release       # no warnings
cargo fmt --check                          # clean
cargo build --release                      # clean
Sony A7C ARW auto-preset sweep             # 774/774 renders
local-tone 268-file maximum-strength pass  # 268/268; 2.05 GiB peak RSS
```

Built and tested on Windows 11 with the pinned toolchain (Rust 1.89.0,
`x86_64-pc-windows-msvc`):

```text
cargo build --release                      # clean
cargo test --release                       # 23 passed
cargo clippy --all-targets --release       # no warnings
cargo fmt                                  # applied
```

The release binary was run over a batch of 7 Samsung Galaxy DNGs (one Bayer CFA
file and six linear DNGs) writing JPEG output with sidecars. All 7 completed
and were visually confirmed to be correct renders of their scenes.

## Fixes required to reach that state

The original source package was assembled without a Rust toolchain, so it had
never been compiled. Three defects had to be corrected:

1. **Compile errors against Rawler 0.7.2.** `RawDevelop::new_with` does not
   exist — `RawDevelop` is a plain struct with a public `steps` field — and
   there is no `ProcessingStep::FujiRotate`. See `src/pipeline.rs`.

2. **Sensor level layouts Rawler cannot consume.** Linear DNGs that record a
   2x2 `BlackLevelRepeatDim` with `cpp` samples per position (12 black levels)
   alongside 3 white levels made Rawler panic. See `src/levels.rs`.

3. **Silently corrupt lossless JPEG decoding.** Rawler 0.7.2 ignores the `DRI`
   marker and never resynchronizes on `RSTn` markers, so any lossless JPEG
   using restart intervals decodes to a smooth diagonal ramp instead of an
   image — without reporting an error. See `src/ljpeg.rs` and
   `src/redecode.rs`.

Defect 3 is the important one: it does not fail loudly, so it would have
produced plausible-looking batch output that was entirely garbage.

## Debug helpers

Four examples exist for inspecting files that misbehave:

```bash
cargo run --release --example probe-levels -- photo.dng
cargo run --release --example probe-tiff   -- photo.dng
cargo run --release --example probe-jpeg   -- photo.dng
cargo run --release --example probe-decode -- photo.dng dump.png
```

`probe-decode` is the quickest way to separate a decoding fault from a
processing fault: it writes Rawler's sample buffer straight to a PNG, before
any raw-autotune processing. It deliberately does *not* apply the repair in
`src/redecode.rs`, so it shows what Rawler alone produced.

## Not verified

- macOS builds.
- Cameras other than the two Samsung models in the test batch.
- Tiled lossless JPEG reassembly in `src/redecode.rs`: the code path exists and
  derives its geometry from `TileWidth`/`TileLength`, but every test file was
  single-strip, so it has not been exercised against a real tiled file.
