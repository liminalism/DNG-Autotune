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
//! `/usr/bin/time`, on this corpus, re-swept at 0.1.19 over 33 frames spanning
//! 10.0 to 49.9 megapixels and all four sources, with every per-pixel operator
//! forced on (`--chroma-denoise 2 --local-tone 1 --luma-denoise 2 --sharpen 2
//! --local-white-balance 1 --night-tone 1 --reference --illuminant
//! --spatial-highlight-floor 0 --highlight-method harmonic`):
//!
//! | frame | pixels | peak RSS | bytes/pixel |
//! |---|---|---|---|
//! | `_DSC1105.ARW` (noisy) | 10.5 MP | 672 MiB | **60.7** |
//! | `20260223_192940.dng` (ISO 1600) | 10.0 MP | 638 MiB | 60.3 |
//! | `_DSC1250.ARW` (ISO 12800) | 24.3 MP | 1464 MiB | 60.3 |
//! | `_DSC1139.ARW` (ISO 10000) | 24.3 MP | 1463 MiB | 60.3 |
//! | `proshot.dng` (clean) | 12.5 MP | 570 MiB | 42.5 |
//! | `20260728_114800.dng` (clean) | 49.9 MP | 2075 MiB | 42.2 |
//! | `_DSC1027.ARW` (clean) | 10.5 MP | 460 MiB | 39.5 |
//!
//! The spread is not noise and not resolution: it is `chroma::apply`, which
//! allocates a full-resolution `chroma` (12 B/px), `luma` (4 B/px) and
//! `scratch` (12 B/px) plus the guided stage's `guide` and per-channel `plane`
//! (4 B/px each) — and which runs only on frames noisy enough to need it. The
//! sweep is sharply bimodal for that reason: every frame the chroma stage
//! engages on lands in 60.3–60.7 B/px whatever its size or source, and every
//! frame it does not lands in 39.5–42.5. Two frames of identical dimensions
//! differ by half again depending on their ISO, so the budget has to assume the
//! noisy path always. `--local-tone` is memory-heavy too but peaks *below* the
//! chroma stage, so covering chroma covers it.
//!
//! The 0.1.18 fit of this table was 52.4 B/px and the constant was set to 56.
//! By 0.1.19 the real figure had drifted to 60.7 without anything catching it,
//! so the constant was quietly 8% under the cost it exists to bound. That is
//! the failure mode of fitting tight, and it is why 68 is now chosen over the
//! 65 the same rounding rule would give.
//!
//! **What the semantic models cost.** `--scene-classify`, `--semantic` and
//! `--semantic-faces` load an ONNX graph and run it on a fixed 288x192 proxy,
//! so their cost is a flat per-worker figure and not a per-pixel one: measured
//! +234 MiB on a 10.5 MP frame, +315 MiB on 24.3 MP and +273 MiB on 49.9 MP.
//! A process-wide session cache was tried and rejected — it turns a transient
//! into a resident and moved peak RSS the wrong way (see `CHANGELOG.md`).
//!
//! **How much memory there is.** `MemAvailable` on Linux, `ullAvailPhys` on
//! Windows — the kernel's own estimate of what can be handed out without
//! swapping, which is the right question and not the same as free memory.
//!
//! # Why the most expensive file in the batch sets the pace
//!
//! Workers pull from a shared queue, so any worker may hold any file, and a
//! batch mixing 10 MP phone frames with 50 MP Expert RAW ones can put the five
//! biggest in flight together. Sizing on the batch maximum is the only bound
//! that holds; sizing on the mean would be wrong exactly when it mattered.
//!
//! Expert RAW is LinearRaw — already demosaiced in-camera — so `reconstruct_cfa`
//! never runs and a spatial reserve would be paid for a solver that cannot run.
//! The planner reads photometric interpretation from the same TIFF directory it
//! already probes for dimensions, and a LinearRaw file is budgeted as `Current`
//! even when the requested method is Harmonic. A mixed batch then takes the max
//! of those per-file costs.
//!
//! When the harmonic reserve was 64 B/px — more than the entire per-pixel budget
//! — that discount was large enough to invert the ordering: a 24 MP Bayer frame
//! was budgeted above a 50 MP LinearRaw one, though the measured peaks are
//! 1464 MiB and 2075 MiB the other way round. Since the reserve was re-fitted to
//! what the solver actually adds, pixels dominate again and the biggest file
//! binds, which is the ordinary case a shared queue wants. The discount is still
//! real, just no longer big enough to reorder files.
//!
//! The planner still cannot see the archive-speed spatial floor (that needs a
//! decode). LinearRaw is knowable from the header; the floor is not.
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
/// The measured maximum over the corpus is 60.7 B/px (`_DSC1105.ARW`, where the
/// guided chroma stage runs at full radius); the table in the module
/// documentation has the rest, and the ceiling it describes is structural — the
/// chroma stage's plane count — rather than a property of one frame. Rounded up
/// to 68 rather than fitted, because the cost of overestimating is one fewer
/// worker and the cost of underestimating is a batch killed hours in, and
/// because the previous value was fitted tightly enough that ordinary feature
/// work walked past it unnoticed.
pub const PEAK_BYTES_PER_PIXEL: u64 = 68;

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

/// Flat per-worker reserve for the semantic/classifier ONNX graphs.
///
/// These run on a fixed-size proxy, so unlike everything else here the cost
/// does not scale with the image: measured +234 MiB, +315 MiB and +273 MiB on
/// 10.5, 24.3 and 49.9 megapixel frames respectively, which is one figure with
/// allocator noise around it rather than three. Charged only when an option
/// that loads a model is set; before this it was not charged at all, and a
/// `--scene-classify` batch was budgeted about 28% under what it used.
pub const SEMANTIC_MODEL_BYTES: u64 = 384 << 20;

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
    peak_bytes_for_input(pixels, lens_correction, highlight_method, true, false)
}

/// Budgeted peak for one file, given whether a spatial estimator can run on it.
///
/// `spatial_possible` is false for LinearRaw (Expert RAW): the mosaic solver
/// is never called, so the harmonic/pyramid reserve would be fiction. Unknown
/// or CFA files pass `true` and pay the requested method's extra.
///
/// `semantic_models` is true when the run sets an option that loads an ONNX
/// graph. That cost is flat rather than per-pixel — the models see a fixed
/// proxy — so it is added once, not multiplied by the frame.
pub fn peak_bytes_for_input(
    pixels: u64,
    lens_correction: crate::lens::LensCorrectionMode,
    highlight_method: crate::raw_highlight::HighlightMethod,
    spatial_possible: bool,
    semantic_models: bool,
) -> u64 {
    let method = if spatial_possible {
        highlight_method
    } else {
        crate::raw_highlight::HighlightMethod::Current
    };
    let bytes_per_pixel = PEAK_BYTES_PER_PIXEL
        + if lens_correction == crate::lens::LensCorrectionMode::ProfileExact {
            PROFILE_EXTRA_BYTES_PER_PIXEL
        } else {
            0
        }
        + method.extra_bytes_per_pixel();
    PER_IMAGE_OVERHEAD_BYTES
        .saturating_add(pixels.saturating_mul(bytes_per_pixel))
        .saturating_add(if semantic_models {
            SEMANTIC_MODEL_BYTES
        } else {
            0
        })
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
    raw_probe(path).map(|(width, height, _photometric)| (width, height))
}

/// Pixel count companion to [`raw_dimensions`].
pub fn raw_pixels(path: &Path) -> Option<u64> {
    let (width, height) = raw_dimensions(path)?;
    u64::from(width)
        .checked_mul(u64::from(height))
        .filter(|pixels| *pixels > 0)
}

/// Dimensions and photometric of the raw IFD, without touching pixel data.
///
/// Photometric is `None` when the tag is missing. The caller treats LinearRaw
/// as "spatial highlight cannot run" and everything else as "it might".
fn raw_probe(path: &Path) -> Option<(u32, u32, Option<u32>)> {
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
        .filter_map(dimensions_and_photo)
        .max_by_key(|&(width, height, _)| u64::from(width) * u64::from(height));
    if raw.is_some() {
        return raw;
    }

    tiff.find_ifds_with_filter(|_| true)
        .into_iter()
        .filter_map(dimensions_and_photo)
        .max_by_key(|&(width, height, _)| u64::from(width) * u64::from(height))
}

fn dimensions_of(ifd: &IFD) -> Option<(u32, u32)> {
    let width = entry_u32(ifd, TAG_IMAGE_WIDTH)?;
    let height = entry_u32(ifd, TAG_IMAGE_LENGTH)?;
    (width > 0 && height > 0).then_some((width, height))
}

fn dimensions_and_photo(ifd: &IFD) -> Option<(u32, u32, Option<u32>)> {
    let (width, height) = dimensions_of(ifd)?;
    Some((width, height, entry_u32(ifd, TAG_PHOTOMETRIC)))
}

fn entry_u32(ifd: &IFD, tag: u16) -> Option<u32> {
    ifd.get_entry(tag).map(|entry| entry.value.force_u32(0))
}

/// LinearRaw is already demosaiced; a missing or CFA photometric might still
/// run `reconstruct_cfa`, so the safe default is "spatial possible".
fn spatial_highlight_possible(photometric: Option<u32>) -> bool {
    !matches!(photometric, Some(PHOTOMETRIC_LINEAR_RAW))
}

struct InputProbe {
    pixels: u64,
    from_directory: bool,
    spatial_possible: bool,
}

/// Pixels in one input: probed from the TIFF directory, or estimated from the
/// size on disk when that fails. Estimated inputs are assumed spatial-capable
/// so the budget cannot shrink on a guess.
fn probe_input(path: &Path) -> InputProbe {
    match raw_probe(path) {
        Some((width, height, photometric)) => {
            let pixels = u64::from(width)
                .checked_mul(u64::from(height))
                .filter(|count| *count > 0)
                .unwrap_or(0);
            InputProbe {
                pixels,
                from_directory: true,
                spatial_possible: spatial_highlight_possible(photometric),
            }
        }
        None => {
            let bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
            InputProbe {
                pixels: bytes.saturating_mul(ASSUMED_PIXELS_PER_BYTE),
                from_directory: false,
                spatial_possible: true,
            }
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
        false,
    )
}

pub fn plan_with_highlight_method(
    requested: JobCount,
    jobs: &[InputJob],
    lens_correction: crate::lens::LensCorrectionMode,
    highlight_method: crate::raw_highlight::HighlightMethod,
    semantic_models: bool,
) -> Result<Plan, String> {
    let processors = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);

    let mut largest_pixels = 0;
    let mut per_image_bytes = 0;
    let mut probed = 0;
    let mut estimated = 0;
    for job in jobs {
        let probe = probe_input(&job.input);
        largest_pixels = largest_pixels.max(probe.pixels);
        per_image_bytes = per_image_bytes.max(peak_bytes_for_input(
            probe.pixels,
            lens_correction,
            highlight_method,
            probe.spatial_possible,
            semantic_models,
        ));
        if probe.from_directory {
            probed += 1;
        } else {
            estimated += 1;
        }
    }
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
        // Against the constants, not against literals: the previous form spelled
        // 24 and 64 out, so re-fitting the reserves changed the code and the
        // test that was supposed to check it in the same edit.
        assert_eq!(
            pyramid - current,
            pixels * crate::raw_highlight::HighlightMethod::RawPyramid.extra_bytes_per_pixel()
        );
        assert_eq!(
            harmonic_profile - current,
            pixels
                * (PROFILE_EXTRA_BYTES_PER_PIXEL
                    + crate::raw_highlight::HighlightMethod::Harmonic.extra_bytes_per_pixel())
        );
    }

    #[test]
    fn the_semantic_model_reserve_is_flat_and_only_charged_when_asked_for() {
        let lens = crate::lens::LensCorrectionMode::Embedded;
        let harmonic = crate::raw_highlight::HighlightMethod::Harmonic;
        let small = 10_507_264;
        let large = 49_939_200;
        for pixels in [small, large] {
            let without = peak_bytes_for_input(pixels, lens, harmonic, true, false);
            let with = peak_bytes_for_input(pixels, lens, harmonic, true, true);
            // Flat, because the models see a fixed-size proxy rather than the
            // frame: a 50 megapixel file pays exactly what a 10 megapixel one
            // pays.
            assert_eq!(with - without, SEMANTIC_MODEL_BYTES);
        }
        // And the options predicate is what turns it on, so a default run is
        // not charged for a model it never loads.
        let mut options = crate::types::RunOptions::automatic(std::path::PathBuf::from("out"));
        assert!(!options.loads_semantic_models());
        options.scene_classify = true;
        assert!(options.loads_semantic_models());
    }

    #[test]
    fn linear_raw_does_not_reserve_harmonic_bytes() {
        let pixels = 49_939_200; // 8160×6120 Expert RAW
        let lens = crate::lens::LensCorrectionMode::Embedded;
        let harmonic = crate::raw_highlight::HighlightMethod::Harmonic;
        let cfa = peak_bytes_for_input(pixels, lens, harmonic, true, false);
        let linear = peak_bytes_for_input(pixels, lens, harmonic, false, false);
        assert_eq!(
            linear,
            peak_bytes_for_highlight(pixels, lens, crate::raw_highlight::HighlightMethod::Current)
        );
        assert_eq!(
            cfa - linear,
            pixels * crate::raw_highlight::HighlightMethod::Harmonic.extra_bytes_per_pixel()
        );
    }

    #[test]
    fn linear_raw_is_charged_for_its_pixels_but_not_for_a_solver_it_cannot_run() {
        let lens = crate::lens::LensCorrectionMode::Embedded;
        let harmonic = crate::raw_highlight::HighlightMethod::Harmonic;
        // 50 MP LinearRaw cannot run the solver; 24 MP Bayer can.
        let linear_50mp = peak_bytes_for_input(49_939_200, lens, harmonic, false, false);
        let bayer_24mp = peak_bytes_for_input(24_337_152, lens, harmonic, true, false);
        let naive = peak_bytes_for_highlight(49_939_200, lens, harmonic);
        // Until the 2026-09-05 re-fit the harmonic reserve was 64 B/px, more
        // than the whole per-pixel budget, and it made a 24 megapixel Bayer
        // frame look more expensive than a 50 megapixel LinearRaw one. It is
        // not, and the measured peaks say so: 1464 MiB against 2075 MiB. With
        // the reserve fitted to what the solver actually adds, the biggest file
        // binds again, which is what a shared queue needs.
        assert!(
            linear_50mp > bayer_24mp,
            "50 MP LinearRaw ({linear_50mp}) should bind over 24 MP Bayer ({bayer_24mp})"
        );
        // The LinearRaw discount is still real: it is charged no spatial
        // reserve, because `reconstruct_cfa` is never called on it.
        assert!(
            naive > linear_50mp,
            "sizing 50 MP as harmonic ({naive}) over-reserves against the real ceiling ({linear_50mp})"
        );
    }

    #[test]
    fn spatial_highlight_possible_is_false_only_for_linear_raw() {
        assert!(!spatial_highlight_possible(Some(PHOTOMETRIC_LINEAR_RAW)));
        assert!(spatial_highlight_possible(Some(PHOTOMETRIC_CFA)));
        assert!(spatial_highlight_possible(None));
        assert!(spatial_highlight_possible(Some(2)));
    }
}
