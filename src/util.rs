//! This module contains some miscellaneous utility functions.

use std::io::{Read, Write};

use indicatif::{ProgressBar, ProgressStyle};

/// Write data from a stream to two different outputs concurrently.
pub fn tee(
    mut stream: impl Read,
    mut out_1: impl Write,
    mut out_2: impl Write,
) -> std::io::Result<()> {
    let mut buf = [0u8; 1024];
    loop {
        let num_read = stream.read(&mut buf)?;
        if num_read == 0 {
            break;
        }

        let buf = &buf[..num_read];
        out_1.write_all(buf)?;
        out_2.write_all(buf)?;
    }

    Ok(())
}

/// Change to bar if not already, and increment a status bar.
pub fn inc_bar(bar: &ProgressBar, amt: u64, style: &ProgressStyle) {
    bar.set_style(style.clone());
    bar.inc(amt)
}
