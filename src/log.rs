//! This module contains functions to log messages to the terminal with
//! consistent formatting.

use std::io::{self, Read, Write};
use std::process::exit;

use anyhow::Result;

/// Log a message with a colored arrow at the beginning.
pub fn log(msg: &str, color: usize) {
    eprintln!("\x1b[{color}m->\x1b[0m {msg}");
}

/// Same as log, but with a two space indent before the arrow.
pub fn log_ident(msg: &str, color: usize) {
    eprintln!("  \x1b[{color}m->\x1b[0m {msg}");
}

/// Log a message with a magenta arrow.
pub fn info(msg: &str) {
    log(msg, 35);
}

/// Same as info, but with a two space indent before the arrow.
pub fn info_ident(msg: &str) {
    log_ident(msg, 35);
}

/// Log a message with a yellow arrow and WARNING: prefixing the message.
pub fn warn(msg: &str) {
    log(&format!("WARNING: {msg}"), 33);
}

/// Log a message with a red arrow and ERROR: prefixing the message, then exit
/// with a non-zero exit code.
pub fn die(msg: &str) -> ! {
    log(&format!("ERROR: {msg}"), 31);
    exit(1)
}

/// Wait for the user to confirm that it is okay to continue.
pub fn prompt() {
    info("Press Enter to continue or Ctrl+C to abort");
    let _ = io::stdin().read(&mut [0u8]);
}

/// Ask the user a yes-no question
pub fn prompt_yn(q: &str, col: usize) -> Result<bool> {
    print!("\x1b[{col}m->\x1b[0m {q} [Y/n] ");
    io::stdout().flush()?;
    let mut resp = String::new();
    io::stdin().read_line(&mut resp)?;
    let res = !(resp.starts_with("n") || resp.starts_with("N"));
    return Ok(res);
}

#[macro_export]
/// Macro version of info that allows for format! style syntax.
macro_rules! info_fmt {
    ($($t:tt)*) => {{
        eprint!("\x1b[35m->\x1b[0m ");
        eprintln!($($t)*);
    }};
}

#[macro_export]
/// Macro version of info_ident that allows for format! style syntax.
macro_rules! info_ident_fmt {
    ($($t:tt)*) => {{
        eprint!("  \x1b[35m->\x1b[0m ");
        eprintln!($($t)*);
    }};
}
