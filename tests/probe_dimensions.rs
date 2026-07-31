//! The `--jobs auto` budget rests on knowing how big each input is before it is
//! opened, and `memory::raw_pixels` gets that from the TIFF directory rather
//! than from the decoder. This checks the shortcut against the decoder itself,
//! over every RAW on hand.
//!
//! Two things have to hold, and they are not the same thing:
//!
//! - the probe must never come in **under** the decoded frame, because a budget
//!   built on an underestimate is how a batch gets OOM-killed;
//! - it must not come in wildly over, or the budget starves the batch of
//!   workers for no reason.
//!
//! A small overshoot is expected and correct: the directory declares the stored
//! frame, and the developer crops the active area out of it.
//!
//! `raw/` is not distributable, so this skips when it is absent.

use raw_autotune::memory;
use std::path::{Path, PathBuf};

/// The stored frame may exceed the decoded one by this much before the probe is
/// treated as too loose. Sony ARW stores 6048x4024 and develops 6024x4012, a
/// 0.8% overshoot; this leaves room for a source with a wider border.
const MAX_OVERSHOOT: f64 = 1.10;

fn corpus() -> Vec<PathBuf> {
    fn walk(directory: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        // Sorted, so a failure names the same file on every machine.
        let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, found);
            } else if matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("ARW" | "arw" | "DNG" | "dng")
            ) {
                found.push(path);
            }
        }
    }

    let mut found = Vec::new();
    walk(Path::new("raw"), &mut found);
    found
}

#[test]
fn the_probe_matches_what_the_decoder_finds() {
    let corpus = corpus();
    if corpus.is_empty() {
        eprintln!("skipping the_probe_matches_what_the_decoder_finds: no RAW corpus under raw/");
        return;
    }

    let mut checked = 0;
    let mut worst_overshoot = 1.0_f64;
    for path in &corpus {
        let probed = memory::raw_pixels(path)
            .unwrap_or_else(|| panic!("{}: the directory should be readable", path.display()));

        // `decode_dummy` walks the container and allocates the frame without
        // decompressing a pixel, which is the cheapest exact answer available.
        let source = rawler::rawsource::RawSource::new(path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let Ok(decoded) = rawler::decode_dummy(&source) else {
            continue;
        };
        let decoded_pixels = (decoded.width * decoded.height) as u64;

        assert!(
            probed >= decoded_pixels,
            "{}: probe found {probed} pixels, decoder found {decoded_pixels} — \
             an underestimate is what the budget cannot survive",
            path.display()
        );
        assert!(
            probed as f64 <= decoded_pixels as f64 * MAX_OVERSHOOT,
            "{}: probe found {probed} pixels against the decoder's {decoded_pixels}, \
             which is too loose to budget from",
            path.display()
        );
        worst_overshoot = worst_overshoot.max(probed as f64 / decoded_pixels as f64);
        checked += 1;
    }

    assert!(checked > 0, "the corpus was found but nothing decoded");
    eprintln!(
        "probe agreed with the decoder on {checked} of {} files, worst overshoot {:.4}x",
        corpus.len(),
        worst_overshoot
    );
}
