//! Small terminal input helpers for the minimal automatic front-end.

use anyhow::Result;
use std::io::{self, Write};

/// Raised when an interactive run is cancelled before work starts.
pub const CANCELLED: &str = "cancelled";

/// Read one typed line, trimming whitespace and the quotes terminals add to a
/// drag-and-dropped path. EOF is treated as a blank line.
pub fn prompt_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
        return Ok(String::new());
    }
    Ok(input
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string())
}
