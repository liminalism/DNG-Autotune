//! Minimal front-end for the unattended automatic profile.
//!
//! The flag CLI remains the escape hatch for diagnostics. A bare executable
//! asks only where the RAWs are and where the archive should be written.

use anyhow::Result;
use raw_autotune::types::{OutputFormat, RunOptions};
use std::path::PathBuf;

use crate::interactive_menu::{CANCELLED, prompt_line};

/// Gather a run interactively.
pub fn run() -> Result<Option<(Vec<PathBuf>, RunOptions)>> {
    banner();
    match gather() {
        Ok(pair) => Ok(Some(pair)),
        Err(error) if error.to_string() == CANCELLED => {
            println!("\nCancelled. Nothing was written.");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn banner() {
    println!("+------------------------------------------+");
    println!("|       raw-autotune - automatic RAW       |");
    println!("+------------------------------------------+");
    println!(
        "Choose the source and destination. Colour, demosaic, noise, tone, metadata and \
         concurrency are selected automatically.\n"
    );
}

fn gather() -> Result<(Vec<PathBuf>, RunOptions)> {
    let inputs = collect_inputs()?;
    if inputs.is_empty() {
        println!("No inputs selected.");
        return Err(anyhow::anyhow!(CANCELLED));
    }

    let output_dir = collect_output_dir()?;
    let options = RunOptions::automatic(output_dir);
    print_summary(&inputs, &options);
    Ok((inputs, options))
}

fn collect_inputs() -> Result<Vec<PathBuf>> {
    println!("Input RAW files or folders");
    println!("Enter a path per line (drag-and-drop works). Blank line when done.\n");
    let mut inputs = Vec::new();
    loop {
        let prompt = if inputs.is_empty() {
            "RAW file or folder: ".to_string()
        } else {
            format!("Another path ({} added, blank to finish): ", inputs.len())
        };
        let entered = prompt_line(&prompt)?;
        if entered.is_empty() {
            break;
        }
        let path = PathBuf::from(&entered);
        if !path.exists() {
            println!("  ! not found: {}", path.display());
            continue;
        }
        println!("  + {}", path.display());
        inputs.push(path);
    }
    Ok(inputs)
}

fn collect_output_dir() -> Result<PathBuf> {
    println!("\nOutput directory");
    let entered = prompt_line("Output directory [raw-autotune-output]: ")?;
    Ok(if entered.is_empty() {
        PathBuf::from("raw-autotune-output")
    } else {
        PathBuf::from(entered)
    })
}

fn print_summary(inputs: &[PathBuf], options: &RunOptions) {
    println!("\nAutomatic profile");
    println!("  Inputs:       {} path(s)", inputs.len());
    println!("  Output:       {}", options.output_dir.display());
    println!(
        "  Images:       {} q{} 4:4:4, optimized",
        match options.format {
            OutputFormat::Jpeg => "JPEG",
            OutputFormat::Png => "PNG",
            OutputFormat::Tiff => "TIFF",
        },
        options.jpeg.quality
    );
    println!("  Demosaic:     adaptive PPG/RCD/AMaZE");
    println!("  DNG colour:   automatic");
    println!(
        "  Corrections:  hot/dead {:.2}, highlights {:.2}, chroma and sharpen automatic",
        options.hot_pixels, options.highlight_reconstruction
    );
    println!(
        "  Report:       {}",
        options
            .summary_path
            .as_ref()
            .map_or_else(|| "off".to_string(), |path| path.display().to_string())
    );
    println!("  Profile:      {}", RunOptions::AUTO_PROFILE_VERSION);
    println!("\nProcessing starts now. Existing outputs are skipped.\n");
}
