//! This module contains some miscellaneous utility functions.

use std::io::{Read, Write};

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
