#!/usr/bin/env sh
set -eu

cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo was not found; install Rust with rustup first" >&2
  exit 1
fi

cargo test
cargo build --release

mkdir -p dist/raw-autotune
cp target/release/raw-autotune dist/raw-autotune/
cp README.md RUN_ME_FIRST.txt CHANGELOG.md LICENSE-MIT THIRD_PARTY.md dist/raw-autotune/
cp docs/KNOWN_LIMITATIONS.md docs/TESTING.md docs/BUILD_STATUS.md docs/DESIGN.md docs/ROADMAP.md dist/raw-autotune/

if command -v zip >/dev/null 2>&1; then
  rm -f dist/raw-autotune-linux.zip
  (cd dist/raw-autotune && zip -q -r ../raw-autotune-linux.zip .)
  echo "ZIP package: dist/raw-autotune-linux.zip"
fi

echo "Executable: target/release/raw-autotune"
echo "Package directory: dist/raw-autotune"
