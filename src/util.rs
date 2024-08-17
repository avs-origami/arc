//! This module contains some miscellaneous utility functions.

use std::fs::File;
use std::io::{Read, Write};

pub fn tee(
    mut stream: impl Read,
    file: &mut File,
    mut output: impl Write,
) -> std::io::Result<()> {
    let mut buf = [0u8; 1024];
    loop {
        let num_read = stream.read(&mut buf)?;
        if num_read == 0 {
            break;
        }

        let buf = &buf[..num_read];
        file.write_all(buf)?;
        output.write_all(buf)?;
    }

    Ok(())
}
