use crate::types::{InputJob, OutputPaths, RunOptions};
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn is_supported_raw(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let extension = extension.to_ascii_uppercase();
    rawler::decoders::supported_extensions()
        .iter()
        .any(|supported| supported.eq_ignore_ascii_case(extension.as_str()))
}

fn safe_root_label(path: &Path) -> PathBuf {
    path.file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("input"))
}

fn fnv1a_path_hash(path: &Path) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn disambiguated_relative(relative: &Path, input: &Path) -> PathBuf {
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let stem = relative
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("image");
    let suffix = format!("{:08x}", fnv1a_path_hash(input) as u32);
    let filename = match relative.extension().and_then(|value| value.to_str()) {
        Some(extension) => format!("{stem}__{suffix}.{extension}"),
        None => format!("{stem}__{suffix}"),
    };
    parent.join(filename)
}

pub fn discover_inputs(inputs: &[PathBuf], recursive: bool) -> Result<Vec<InputJob>> {
    let mut jobs = Vec::new();

    for input in inputs {
        if input.is_file() {
            if !is_supported_raw(input) {
                bail!(
                    "unsupported or unrecognized RAW extension: {}",
                    input.display()
                );
            }

            let relative = input
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("image.raw"));

            jobs.push(InputJob {
                input: input.clone(),
                relative,
            });
            continue;
        }

        if !input.is_dir() {
            bail!(
                "input is neither a file nor a directory: {}",
                input.display()
            );
        }

        let root_label = safe_root_label(input);
        let mut walker = WalkDir::new(input).follow_links(false);
        if !recursive {
            walker = walker.max_depth(1);
        }

        for entry in walker {
            let entry = entry.with_context(|| {
                format!("failed while walking input directory {}", input.display())
            })?;
            if !entry.file_type().is_file() || !is_supported_raw(entry.path()) {
                continue;
            }

            let inside_root = entry.path().strip_prefix(input).unwrap_or(entry.path());
            jobs.push(InputJob {
                input: entry.path().to_path_buf(),
                relative: root_label.join(inside_root),
            });
        }
    }

    jobs.sort_by(|a, b| a.input.cmp(&b.input));

    let mut seen = HashSet::new();
    jobs.retain(|job| seen.insert(job.input.clone()));

    let mut relative_paths = HashSet::new();
    for job in &mut jobs {
        if !relative_paths.insert(job.relative.clone()) {
            job.relative = disambiguated_relative(&job.relative, &job.input);
            relative_paths.insert(job.relative.clone());
        }
    }

    if jobs.is_empty() {
        bail!("no supported RAW files were found");
    }

    Ok(jobs)
}

pub fn output_paths(job: &InputJob, options: &RunOptions) -> Result<OutputPaths> {
    let parent = job.relative.parent().unwrap_or_else(|| Path::new(""));
    let stem = job
        .relative
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("input filename is not valid UTF-8")?;

    let destination_dir = options.output_dir.join(parent);
    let image = destination_dir
        .join(format!("{}_{}", stem, options.preset.as_str()))
        .with_extension(options.format.extension());
    let sidecar = destination_dir
        .join(format!("{}_{}", stem, options.preset.as_str()))
        .with_extension("json");
    let baseline = destination_dir
        .join(format!("{}_baseline", stem))
        .with_extension(options.format.extension());

    Ok(OutputPaths {
        image,
        sidecar,
        baseline,
    })
}
