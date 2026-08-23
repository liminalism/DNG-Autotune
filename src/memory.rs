//! How many RAW files this program may develop at once.
//!
//! `--jobs` used to default to 1 with the documentation telling the user to
//! keep it at 3 or below for 50-megapixel files, because `--jobs 8` over the
//! full corpus was reliably killed by the OOM killer on a 31 GiB machine. That
//! is a per-source flag in everything but name, and `docs/PLAN.md` criterion 5
//! ("batch runs are unattended") does not survive a batch that dies two hours
//! in. The program knows the size of every file it is about to open and the
//! operating system will say how much memory is free, so it can do the
//! arithmetic itself.
//!
//! # The two numbers
//!
//! **What one image costs.** Peak resident set was measured per file with
//! `/usr/bin/time -v`, on this corpus, at 0.1.18, with every optional operator
//! forced on (`--chroma-denoise 2 --local-tone 1`):
//!
//! | frame | pixels | peak RSS | bytes/pixel |
//! |---|---|---|---|
//! | `_DSC0883.ARW` | 10.5 MP | 417 MiB | 41.6 |
//! | `20260729_114901.dng` | 12.5 MP | 479 MiB | 40.2 |
//! | `_DSC1236.ARW` | 24.3 MP | 913 MiB | 39.3 |
//! | `_DSC1250.ARW` (ISO 12800) | 24.3 MP | 1216 MiB | **52.4** |
//! | `20260728_114800.dng` | 49.9 MP | 1882 MiB | 39.5 |
//!
//! The spread is not noise and not resolution: it is `chroma::apply`, which
//! allocates a full-resolution `chroma` (12 B/px), `luma` (4 B/px) and
//! `scratch` (12 B/px) plus the guided stage's `guide` and per-channel `plane`
//! (4 B/px each) — and which runs only on frames noisy enough to need it. Two
//! frames of identical dimensions differ twofold in peak memory depending on
//! their ISO, so the budget has to assume the noisy path always. `--local-tone`
//! is memory-heavy too but peaks *below* the chroma stage (38.4 B/px against
//! 52.4 on the same frame), so covering chroma covers it.
//!
//! **How much memory there is.** `MemAvailable` on Linux, `ullAvailPhys` on
//! Windows — the kernel's own estimate of what can be handed out without
//! swapping, which is the right question and not the same as free memory.
//!
//! # Why the largest file in the batch sets the pace
//!
//! Workers pull from a shared queue, so any worker may hold any file, and a
//! batch mixing 10 MP phone frames with 50 MP Expert RAW ones can put the five
//! biggest in flight together. Sizing on the batch maximum is the only bound
//! that holds; sizing on the mean would be wrong exactly when it mattered.
//!
//! Nothing here changes what is rendered. A file develops identically alone or
//! in a batch (`docs/PLAN.md`, "determinism is a product property"), so the
//! worker count is free to depend on the machine.

use crate::types::InputJob;
use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, IFD};
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::str::FromStr;

/// Peak working set per developed pixel, in bytes.
///
/// The measured maximum over the corpus is 52.4 B/px (`_DSC1250.ARW`, ISO
/// 12800, where the guided chroma stage runs at full radius); the table in the
/// module documentation has the rest. Rounded up to 56 rather than fitted,
/// because the cost of overestimating is one fewer worker and the cost of
/// underestimating is a batch killed hours in.
pub const PEAK_BYTES_PER_PIXEL: u64 = 56;

/// Fixed cost of having a file in flight at all, independent of its size:
/// the decoder's own tables, the memory-mapped source, and the allocator's
/// per-thread arenas. Fitting the table above puts it near 26 MiB.
pub const PER_IMAGE_OVERHEAD_BYTES: u64 = 64 << 20;

/// Share of available memory the batch may plan to occupy, in percent.
///
/// The remainder is not a rounding allowance. `MemAvailable` is an estimate to
/// begin with, it moves while the batch runs, the batch's own output and page
/// cache come out of it, and something else on the machine may want memory
/// before this finishes. Concurrent workers are assumed to peak together, which
/// is pessimistic but is the only assumption that holds when the schedule is a
/// shared queue.
///
/// The deliberately conservative half-memory ceiling leaves room for the
/// desktop, filesystem cache, allocator fragmentation, and changes in
/// availability between planning and the workers reaching their peaks.
pub const AVAILABLE_PERCENT: u64 = 50;

/// Additional transient source/output planes reserved for exact-profile
/// geometry. This is deliberately additive to the measured worst ordinary
/// pipeline rather than assuming the two peaks can never overlap as the code
/// evolves.
pub const PROFILE_EXTRA_BYTES_PER_PIXEL: u64 = 16;

/// Assumed pixels per byte of file when the dimensions cannot be read.
///
/// Every RAW this program targets is TIFF-based and gets probed properly, so
/// this is the path for a format whose directory could not be parsed. The
/// corpus sits at 2.0 bytes per pixel on disk (uncompressed 16-bit ARW and
/// DNG); lossless-compressed raws sit nearer 1.2. Assuming 1.0 therefore
/// overestimates the pixel count roughly twofold, which is the direction to be
/// wrong in.
const ASSUMED_PIXELS_PER_BYTE: u64 = 1;

// TIFF tags. Duplicated from `preview.rs` rather than shared: that module's
// constants describe what a preview looks like, these describe what raw image
// data looks like, and the two lists happen to overlap.
const TAG_IMAGE_WIDTH: u16 = 256;
const TAG_IMAGE_LENGTH: u16 = 257;
const TAG_PHOTOMETRIC: u16 = 262;
const TAG_SUB_IFDS: u16 = 330;

const PHOTOMETRIC_CFA: u32 = 32803;
const PHOTOMETRIC_LINEAR_RAW: u32 = 34892;

/// The `--jobs` argument: a number, or the automatic decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCount {
    /// Choose from available memory and the size of the largest input.
    Auto,
    /// Request at most this many; the memory safety ceiling may lower it.
    Fixed(usize),
}

impl FromStr for JobCount {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        match text.parse::<usize>() {
            Ok(0) => Err("--jobs must be at least 1, or `auto`".to_string()),
            Ok(count) => Ok(Self::Fixed(count)),
            Err(_) => Err(format!(
                "expected a positive number or `auto`, got `{text}`"
            )),
        }
    }
}

impl fmt::Display for JobCount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::Fixed(count) => write!(formatter, "{count}"),
        }
    }
}

/// The decision, and enough of its inputs to explain it on the console.
#[derive(Debug, Clone)]
pub struct Plan {
    /// How many files to hold in flight.
    pub workers: usize,
    /// Pixels in the largest input, as probed or estimated.
    pub largest_pixels: u64,
    /// Budgeted peak working set for one file of that size.
    pub per_image_bytes: u64,
    /// What the operating system reported, when it could be asked.
    pub available_bytes: Option<u64>,
    /// True when the processor count, not memory, was the binding constraint.
    pub cpu_limited: bool,
    /// Inputs whose dimensions came from the file rather than from its size.
    pub probed: usize,
    /// Inputs whose size had to be guessed from their bytes on disk.
    pub estimated: usize,
}

impl Plan {
    /// One line for the run header, saying what was decided and on what.
    ///
    /// An estimated input is called out rather than folded in silently: it is
    /// the one path here where the number the budget rests on is a guess, and a
    /// batch that later dies of memory should not have hidden that.
    pub fn describe(&self, requested: JobCount) -> String {
        match requested {
            JobCount::Fixed(count) if self.workers < count => format!(
                " (memory safety capped requested {count} to {}; {} available, {} per image)",
                self.workers,
                self.available_bytes
                    .map(gibibytes)
                    .unwrap_or_else(|| "unknown".into()),
                gibibytes(self.per_image_bytes),
            ),
            JobCount::Fixed(_) => String::new(),
            JobCount::Auto => match self.available_bytes {
                Some(available) => format!(
                    " (auto: {} available, {} per image at {:.1} MP{}{})",
                    gibibytes(available),
                    gibibytes(self.per_image_bytes),
                    self.largest_pixels as f64 / 1e6,
                    if self.cpu_limited {
                        ", CPU-limited"
                    } else {
                        ""
                    },
                    match self.estimated {
                        0 => String::new(),
                        count => format!(", {count} file(s) sized by guess"),
                    },
                ),
                None => " (auto: this platform does not report available memory)".to_string(),
            },
        }
    }
}

fn gibibytes(bytes: u64) -> String {
    format!("{:.2} GiB", bytes as f64 / (1u64 << 30) as f64)
}

/// Budgeted peak working set for one image of `pixels` pixels.
pub fn peak_bytes(pixels: u64) -> u64 {
    PER_IMAGE_OVERHEAD_BYTES.saturating_add(pixels.saturating_mul(PEAK_BYTES_PER_PIXEL))
}

pub fn peak_bytes_for(pixels: u64, lens_correction: crate::lens::LensCorrectionMode) -> u64 {
    peak_bytes_for_highlight(
        pixels,
        lens_correction,
        crate::raw_highlight::HighlightMethod::Current,
    )
}

pub fn peak_bytes_for_highlight(
    pixels: u64,
    lens_correction: crate::lens::LensCorrectionMode,
    highlight_method: crate::raw_highlight::HighlightMethod,
) -> u64 {
    let bytes_per_pixel = PEAK_BYTES_PER_PIXEL
        + if lens_correction == crate::lens::LensCorrectionMode::ProfileExact {
            PROFILE_EXTRA_BYTES_PER_PIXEL
        } else {
            0
        }
        + highlight_method.extra_bytes_per_pixel();
    PER_IMAGE_OVERHEAD_BYTES.saturating_add(pixels.saturating_mul(bytes_per_pixel))
}

/// Memory the operating system says can be handed out without swapping.
///
/// `None` on a platform with no answer, which is treated as "do not guess".
#[cfg(target_os = "linux")]
pub fn available_bytes() -> Option<u64> {
    // MemAvailable, not MemFree: the kernel's own estimate of what a new
    // workload can take, which counts reclaimable page cache. MemFree on a
    // machine that has been running a while is near zero and would put every
    // batch on one worker.
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kibibytes: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return kibibytes.checked_mul(1024);
        }
    }
    None
}

/// Memory the operating system says can be handed out without swapping.
#[cfg(windows)]
pub fn available_bytes() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_physical: u64,
        available_physical: u64,
        total_page_file: u64,
        available_page_file: u64,
        total_virtual: u64,
        available_virtual: u64,
        available_extended_virtual: u64,
    }

    // Declared here rather than taken from a crate: this is the only Windows
    // API this program calls, and `windows-sys` would be a dependency tree for
    // one struct and one function.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    let mut status = MemoryStatusEx {
        length: size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_physical: 0,
        available_physical: 0,
        total_page_file: 0,
        available_page_file: 0,
        total_virtual: 0,
        available_virtual: 0,
        available_extended_virtual: 0,
    };
    // SAFETY: `status` is a correctly sized and initialised MEMORYSTATUSEX with
    // its `dwLength` set, which is the entire contract of the call.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    // Physical memory is the analogue of Linux's MemAvailable. The commit limit
    // is the other bound, and a batch that fits in RAM but not in the page file
    // would fail to commit, so take the smaller.
    ok.then(|| status.available_physical.min(status.available_page_file))
}

/// Memory the operating system says can be handed out without swapping.
#[cfg(not(any(target_os = "linux", windows)))]
pub fn available_bytes() -> Option<u64> {
    None
}

/// Dimensions of the raw image in a TIFF-based RAW file, read from the
/// directory without touching the pixel data.
///
/// The raw image is the largest IFD declaring a CFA or LinearRaw photometric
/// interpretation. Both are needed: Sony ARW and Samsung Pro-mode DNG store a
/// Bayer mosaic (CFA), while Expert RAW stores demosaiced LinearRaw. Falling
/// back to the largest IFD of any kind covers files whose raw IFD carries no
/// photometric tag, at the cost of reading a full-size embedded preview's
/// dimensions instead — which is the safe direction, since the preview is never
/// larger than the frame.
///
/// `None` for anything that is not a readable TIFF, which the caller estimates
/// from the file size instead.
pub fn raw_dimensions(path: &Path) -> Option<(u32, u32)> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let tiff = GenericTiffReader::new(&mut reader, 0, 0, None, &[TAG_SUB_IFDS]).ok()?;

    // `find_ifds_with_filter` walks a `HashMap` of sub-IFDs, so its order
    // varies per process. Taking a maximum is order-independent, which is why
    // this needs no explicit sort the way `preview.rs` does.
    let raw = tiff
        .find_ifds_with_filter(|ifd| {
            matches!(
                entry_u32(ifd, TAG_PHOTOMETRIC),
                Some(PHOTOMETRIC_CFA | PHOTOMETRIC_LINEAR_RAW)
            )
        })
        .into_iter()
        .filter_map(dimensions_of)
        .max_by_key(|&(width, height)| u64::from(width) * u64::from(height));
    if raw.is_some() {
        return raw;
    }

    tiff.find_ifds_with_filter(|_| true)
        .into_iter()
        .filter_map(dimensions_of)
        .max_by_key(|&(width, height)| u64::from(width) * u64::from(height))
}

/// Pixel count companion to [`raw_dimensions`].
pub fn raw_pixels(path: &Path) -> Option<u64> {
    let (width, height) = raw_dimensions(path)?;
    u64::from(width)
        .checked_mul(u64::from(height))
        .filter(|pixels| *pixels > 0)
}

fn dimensions_of(ifd: &IFD) -> Option<(u32, u32)> {
    let width = entry_u32(ifd, TAG_IMAGE_WIDTH)?;
    let height = entry_u32(ifd, TAG_IMAGE_LENGTH)?;
    (width > 0 && height > 0).then_some((width, height))
}

fn entry_u32(ifd: &IFD, tag: u16) -> Option<u32> {
    ifd.get_entry(tag).map(|entry| entry.value.force_u32(0))
}

/// Pixels in one input: probed from the TIFF directory, or estimated from the
/// size on disk when that fails.
fn pixels_for(path: &Path) -> (u64, bool) {
    match raw_pixels(path) {
        Some(pixels) => (pixels, true),
        None => {
            let bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
            (bytes.saturating_mul(ASSUMED_PIXELS_PER_BYTE), false)
        }
    }
}

/// Workers that fit, given the budget. Pure, so the arithmetic is testable
/// without a machine of a particular size.
fn workers_for(available: u64, per_image: u64, processors: usize) -> usize {
    // Scale before dividing: `available / 100 * 70` throws away up to 99 bytes
    // of the numerator, which is nothing, but it also lands a whole worker
    // short at exactly the round figures a test would pick.
    let budget = available.saturating_mul(AVAILABLE_PERCENT) / 100;
    let fits = (budget / per_image.max(1)) as usize;
    fits.min(processors.max(1))
}

/// Decide how many files to hold in flight.
///
/// Every input is probed and the largest sets the safety ceiling. `Auto` also
/// respects the processor count; a fixed request is treated as an upper bound,
/// never as permission to exceed the memory budget.
pub fn plan(
    requested: JobCount,
    jobs: &[InputJob],
    lens_correction: crate::lens::LensCorrectionMode,
) -> Result<Plan, String> {
    plan_with_highlight_method(
        requested,
        jobs,
        lens_correction,
        crate::raw_highlight::HighlightMethod::Current,
    )
}

pub fn plan_with_highlight_method(
    requested: JobCount,
    jobs: &[InputJob],
    lens_correction: crate::lens::LensCorrectionMode,
    highlight_method: crate::raw_highlight::HighlightMethod,
) -> Result<Plan, String> {
    let processors = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);

    let mut largest_pixels = 0;
    let mut probed = 0;
    let mut estimated = 0;
    for job in jobs {
        let (pixels, from_directory) = pixels_for(&job.input);
        largest_pixels = largest_pixels.max(pixels);
        if from_directory {
            probed += 1;
        } else {
            estimated += 1;
        }
    }

    let per_image_bytes =
        peak_bytes_for_highlight(largest_pixels, lens_correction, highlight_method);
    let available_bytes = available_bytes();

    let workers = match (requested, available_bytes) {
        (JobCount::Fixed(count), Some(available)) => {
            count.min(workers_for(available, per_image_bytes, processors))
        }
        (JobCount::Fixed(count), None) => count,
        (JobCount::Auto, Some(available)) => workers_for(available, per_image_bytes, processors),
        // No answer from the platform is not a licence to guess: one worker is
        // what this program did for its whole life before the budget existed.
        (JobCount::Auto, None) => 1,
    };

    if workers == 0 {
        let available = available_bytes.unwrap_or(0);
        return Err(format!(
            "memory safety check refused to start: the largest input needs about {} but only {} of the reported {} available memory is reserved for raw-autotune; close other applications or add memory/swap, then retry",
            gibibytes(per_image_bytes),
            gibibytes(available.saturating_mul(AVAILABLE_PERCENT) / 100),
            gibibytes(available),
        ));
    }

    Ok(Plan {
        workers,
        largest_pixels,
        per_image_bytes,
        available_bytes,
        cpu_limited: workers == processors && workers > 1,
        probed,
        estimated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn job(path: &str) -> InputJob {
        InputJob {
            input: PathBuf::from(path),
            relative: PathBuf::from(path),
        }
    }

    #[test]
    fn a_fixed_count_is_never_raised_and_is_still_memory_checked() {
        let plan = plan(
            JobCount::Fixed(8),
            &[job("nowhere.ARW")],
            crate::lens::LensCorrectionMode::Embedded,
        )
        .unwrap();
        assert!((1..=8).contains(&plan.workers));
        assert_eq!(plan.estimated, 1);
    }

    #[test]
    fn an_unreadable_input_falls_back_to_its_size_on_disk() {
        // Nonexistent, so both the probe and the metadata call fail; the batch
        // still has to run.
        let plan = plan(
            JobCount::Auto,
            &[job("no-such-file.ARW")],
            crate::lens::LensCorrectionMode::Embedded,
        )
        .unwrap();
        assert_eq!(plan.probed, 0);
        assert_eq!(plan.estimated, 1, "the guess has to be reported as one");
        assert!(plan.workers >= 1);
    }

    #[test]
    fn the_budget_is_the_stated_share_of_available_memory() {
        // 10 GiB available, 1 GiB per image, plenty of processors: the
        // conservative half-memory budget admits five whole images.
        assert_eq!(workers_for(10 << 30, 1 << 30, 64), 5);
    }

    #[test]
    fn the_processor_count_is_a_ceiling() {
        assert_eq!(workers_for(1024 << 30, 1 << 30, 4), 4);
    }

    #[test]
    fn a_machine_too_small_for_one_image_is_rejected() {
        assert_eq!(workers_for(1 << 28, 4 << 30, 8), 0);
        assert_eq!(workers_for(0, 4 << 30, 8), 0);
    }

    #[test]
    fn the_plan_never_budgets_more_than_it_was_given() {
        // The invariant the whole module exists for: whatever it chooses, the
        // workers it chose fit inside the share of memory it was allowed.
        for available in [1u64 << 30, 8 << 30, 31 << 30, 64 << 30] {
            for pixels in [10_000_000, 24_337_000, 49_939_200, 200_000_000] {
                let per_image = peak_bytes(pixels);
                let workers = workers_for(available, per_image, 20);
                assert!(
                    workers as u64 * per_image <= available.saturating_mul(AVAILABLE_PERCENT) / 100,
                    "{workers} x {per_image} exceeds {available}"
                );
            }
        }
    }

    #[test]
    fn the_documented_oom_configuration_is_not_chosen_on_the_machine_that_saw_it() {
        // README's failure: eight 50 MP frames in flight on a 31 GiB machine.
        // With the memory that machine actually reports available, rather than
        // its nameplate total, the budget has to land below eight.
        let per_image = peak_bytes(8160 * 6120);
        assert!(workers_for(24 << 30, per_image, 20) < 8);
    }

    #[test]
    fn a_50_megapixel_frame_is_budgeted_above_its_measured_peak() {
        // 20260728_114800.dng measured 1882 MiB with every operator forced on.
        // The budget must not sit under a number that has actually been seen.
        assert!(peak_bytes(8160 * 6120) > 1882 * (1 << 20));
    }

    #[test]
    fn job_counts_parse_the_way_the_flag_documents() {
        assert_eq!("auto".parse::<JobCount>(), Ok(JobCount::Auto));
        assert_eq!("AUTO".parse::<JobCount>(), Ok(JobCount::Auto));
        assert_eq!("4".parse::<JobCount>(), Ok(JobCount::Fixed(4)));
        assert!("0".parse::<JobCount>().is_err());
        assert!("-1".parse::<JobCount>().is_err());
        assert!("some".parse::<JobCount>().is_err());
    }

    #[test]
    fn a_job_count_round_trips_through_its_display_form() {
        for count in [JobCount::Auto, JobCount::Fixed(1), JobCount::Fixed(12)] {
            assert_eq!(count.to_string().parse::<JobCount>(), Ok(count));
        }
    }

    #[test]
    fn available_memory_is_either_absent_or_plausible() {
        // On the platforms this runs on it must answer, and the answer must be
        // a real quantity rather than a zero from a mis-parsed field.
        if let Some(bytes) = available_bytes() {
            assert!(bytes > (16 << 20), "implausibly small: {bytes}");
        } else {
            assert!(
                !cfg!(any(target_os = "linux", windows)),
                "the platform should have reported available memory"
            );
        }
    }

    #[test]
    fn spatial_highlight_reserves_are_additive_with_exact_lens_profiles() {
        let pixels = 24_000_000;
        let current = peak_bytes_for_highlight(
            pixels,
            crate::lens::LensCorrectionMode::Embedded,
            crate::raw_highlight::HighlightMethod::Current,
        );
        let pyramid = peak_bytes_for_highlight(
            pixels,
            crate::lens::LensCorrectionMode::Embedded,
            crate::raw_highlight::HighlightMethod::RawPyramid,
        );
        let harmonic_profile = peak_bytes_for_highlight(
            pixels,
            crate::lens::LensCorrectionMode::ProfileExact,
            crate::raw_highlight::HighlightMethod::Harmonic,
        );
        assert_eq!(pyramid - current, pixels * 24);
        assert_eq!(
            harmonic_profile - current,
            pixels * (PROFILE_EXTRA_BYTES_PER_PIXEL + 64)
        );
    }
}
