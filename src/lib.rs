//! This module contains the main commands that can be directly called by the
//! user through command line arguments.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::process;

use anyhow::{Context, Result};
use lazy_static::lazy_static;

pub mod args;
pub mod actions;
pub mod log;
pub mod util;

lazy_static! {
    pub static ref HOME: String = env::var("HOME").unwrap_or_else(|_| {
        log::die("$HOME is not set");
    });

    pub static ref ARC_PATH_STR: String = env::var("ARC_PATH").unwrap_or_else(|_| {
        log::die("$ARC_PATH is not set");
    });

    pub static ref ARC_PATH: Vec<&'static str> = ARC_PATH_STR.split(':').collect();

    pub static ref CACHE: String = format!("{}/.cache/arc", *HOME);
}

const VERSION: &str = env!("CARGO_PKG_VERSION");
const PKG_TEMPLATE: &[u8] = b"[meta]
version = \"\"
maintainer = \"\"
sources = []
checksums = []

[deps]

[mkdeps]
";

/// Print out a pretty help message and terminate with a given exit code.
pub fn print_help(code: i32) -> ! {
    eprintln!();
    eprintln!("    \x1b[35m.---.");
    eprintln!("   \x1b[35m/\\  \\ \\   \x1b[33m___ \x1b[36m____");
    eprintln!("  \x1b[35m/  \\ -\\ \\\x1b[33m_/__ \x1b[36m/ __/");
    eprintln!(" \x1b[35m/ / /\\  \\ \\  \x1b[33m\\_\x1b[36m\\ |__.");
    eprintln!("\x1b[35m/__./  \\.___\\    \x1b[36m\\___/");
    eprintln!("\x1b[0m");
    eprintln!("Usage: \x1b[33marc\x1b[0m [b/c/d/h/i/l/n/p/r/s/u/v] [pkg]..");
    log::info_ident("b / build     Build packages");
    log::info_ident("c / checksum  Generate checksums");
    log::info_ident("d / download  Download sources");
    log::info_ident("h / help      Print this help");
    log::info_ident("i / install   Install built packages");
    log::info_ident("l / list      List installed packages");
    log::info_ident("n / new       Create a blank package");
    log::info_ident("p / purge     Purge the package cache ($HOME/.cache/arc)");
    log::info_ident("r / remove    Remove packages");
    log::info_ident("s / sync      Sync remote repositories");
    log::info_ident("u / upgrade   Upgrade all packages");
    log::info_ident("v / version   Print version");
    eprintln!("\nCreated by AVS Origami\n");
    process::exit(code)
}

/// Print out the version and exit.
pub fn version() -> ! {
    log::info(&format!("Arc package manager version {VERSION}"));
    process::exit(0)
}

/// Create an empty package template in the current directory. Creates the
/// following directory structure:
///
/// <name>
/// |--- package.toml
/// '--- build (executable)
///
pub fn new(name: String) -> Result<()> {
    // Create the package directory, which will contain 'package.toml' and 'build'.
    fs::create_dir(&name).context(format!("Failed to create directory {name}"))?;

    // Create 'package.toml' and write the pagkage template to it.
    let mut package = File::create(format!("{name}/package.toml"))
        .context(format!("Failed to create {name}/package.toml"))?;

    package.write_all(PKG_TEMPLATE).context(format!("Failed to write to {name}/package.toml"))?;

    // Create 'build' with permissions 0755 and add the shebang.
    let mut build = OpenOptions::new()
        .write(true)
        .create(true)
        .mode(0o755)
        .open(format!("{name}/build"))
        .context(format!("Failed to open {name}/build"))?;

    build.write_all(b"#!/bin/sh -e\n").context(format!("Failed to write to {name}/build"))?;

    info_fmt!("Created new package {}", name);
    Ok(())
}

/// Completely remove the cache directory to free up space.
pub fn purge_cache() -> Result<()> {
    fs::remove_dir_all((*CACHE).clone())?;
    Ok(())
}

/// Download the source files for some packages, even if they already exist.
pub fn download(packs: &Vec<String>) -> Result<()> {
    log::info("Downloading sources");
    actions::download_all(packs, None, true, None)?;
    Ok(())
}

/// Generate checksums for the package defined by the current directory. Will
/// download the source files even if they already exist.
pub fn generate_checksums() -> Result<()> {
    // Download the source files and get the path to each one.
    let pack = actions::download_all(&vec![".".into()], None, true, None)?;
    let mut hashes = vec![];
    for file in &pack[0].sources {
        // Remove any prefixes from the file name.
        let file = if &file[3..4] == "+" { &file[4..] } else { &file[..] };
        // Calculate the b3sum for the file and add it to the list of hashes.
        let data: Vec<u8> = fs::read(file).context("Failed to read source file")?;
        let hash = blake3::hash(&data);
        hashes.push(hash.to_string());
    }

    // Pretty-print the hashes, conveniently putting them in TOML format.
    eprintln!("Add the following to package.toml under [meta]:");
    println!("checksums = {hashes:#?}");

    Ok(())
}

/// Build some packages. This does the following steps:
/// 1. Resolve dependencies of each package, and determine which layer
///    to install each.
/// 2. Display a summary of all packages to be installed, and prompt to either
///    continue or abort.
/// 3. Download the source files for all packages to be installed, if they do
///    not already exist in the cache directory.
/// 4. Verify checksums for all the downloaded sources.
/// 5. For each layer, starting at the highest layer (biggest number) and
///    working down to the lowest layer (smallest number):
///      - Build all packages in that layer
///      - Install all packages in that layer
/// 6. Build all remaining explicit packages.
/// 7. Prompt to install remaining explicit packages.
pub fn build(packs: &Vec<String>, verbose: bool) -> Result<()> {
    // Parse all explicit packages, getting package TOML and the path for each.
    let pack_toml = actions::parse_package(&packs)?;

    // Get the length of the longest package name.
    let pad = packs.iter().fold(&packs[0], |acc, item| {
        if item.len() > acc.len() { &item } else { acc }
    }).len();

    // Resolve all dependencies, getting package.toml and the path for each.
    let dep_toml = actions::resolve_deps(&pack_toml, 1)?;
    let dep_names: Vec<String> = dep_toml.iter().map(|x| x.name.clone()).collect();

    // Get the length of the longest dependency name.
    let pad_dep = if dep_names.len() > 0 {
        dep_names.iter().fold(&dep_names[0], |acc, item| {
            if item.len() > acc.len() { &item } else { acc }
        }).len()
    } else {
        0
    };

    // Get the length of the longest package / dependency name.
    let pad = if pad >= pad_dep { pad } else { pad_dep };

    // Determine the length of the longest version string.
    let version_pad = pack_toml.iter().fold(
        &pack_toml[0],
        |acc, item| {
            let version_acc = &acc.meta.version;
            let version_item = &item.meta.version;

            if version_item.len() > version_acc.len() { &item } else { acc }
        }
    ).meta.version.len();

    let version_pad_dep = if dep_names.len() > 1 {
        dep_toml.iter().fold(
            &dep_toml[0],
            |acc, item| {
                let version_acc = &acc.meta.version;
                let version_item = &item.meta.version;

                if version_item.len() > version_acc.len() { &item } else { acc }
            }
        ).meta.version.len()
    } else if dep_names.len() == 1 {
        dep_toml[0].meta.version.len()
    } else {
        0
    };

    let version_pad = if version_pad >= version_pad_dep { version_pad } else { version_pad_dep };
    let real_pad = pad;

    // Still calculating padding: compare the previous name and version lengths
    // to the lengths of the name and version headings, and pick the longest
    // one. This lets us display package names and versions in a neat table.
    let name_header = format!("Package ({})", packs.len() + dep_names.len());
    let version_header = "Version";

    let pad = if pad < name_header.len() + 3 {
        name_header.len() + 3
    } else {
        pad + 3
    };

    let version_pad = if version_pad < version_header.len() + 3 {
        version_header.len() + 3
    } else {
        version_pad + 3
    };

    // If any explicit packages are already installed and the latest version,
    // warn that we are reinstalling.
    for toml in &pack_toml {
        if actions::is_installed(&toml.name, &toml.meta.version)? {
            log::warn(&format!("Package {} is up to date - reinstalling", &toml.name));
        }
    }

    // Output the table of package names and versions, with a confirmation prompt.
    log::info("Building packages:\n");
    info_fmt!("{: <pad$} {: <version_pad$}", name_header, version_header);
    eprintln!();

    for toml in &pack_toml {
        if dep_names.contains(&toml.name) { continue; }
        info_fmt!("{: <pad$} {: <version_pad$} (explicit)", toml.name, toml.meta.version);
    }

    for toml in &dep_toml {
        info_fmt!("{: <pad$} {: <version_pad$} (layer {})", toml.name, toml.meta.version, toml.depth);
    }

    eprintln!();

    log::prompt();

    // Download all source files.
    log::info("Downloading sources");
    let pack_toml = actions::download_all(packs, Some(pack_toml), false, Some(real_pad))?;
    let dep_toml = actions::download_all(&dep_names, Some(dep_toml), false, Some(real_pad))?;
    eprintln!();

    // Verify checksums for all the source files.
    log::info("Verifying checksums");
    actions::checksums_all(&pack_toml, real_pad)?;
    actions::checksums_all(&dep_toml, real_pad)?;
    eprintln!();

    // If we have any dependencies, build and install them first.
    if dep_toml.len() > 0 {
        // All the dependency data is sorted by layer. Determine on which
        // indices the data must be split to separate the packages based
        // on layer.
        let mut layer_idxs = vec![];
        let mut depth = dep_toml[0].depth;
        let mut prev_idx = 0;
        for (i, pack) in dep_toml.iter().enumerate() {
            if pack.depth < depth {
                layer_idxs.push((prev_idx, i));
                depth = pack.depth;
                prev_idx = i;
            }
        }

        layer_idxs.push((prev_idx, dep_toml.len()));

        // Build and install the dependencies, one layer at a time.
        for idx in &layer_idxs {
            actions::build_all(
                &dep_toml[idx.0..idx.1].to_vec(),
                verbose,
            )?;

            info_fmt!("Installing layer {} dependencies", dep_toml[idx.0].depth);
            actions::install_all(&dep_toml[idx.0..idx.1].to_vec())?;
            eprintln!();
        }
    }

    // Build all remaining explicit packages.
    actions::build_all(&pack_toml, verbose)?;

    // Prompt the user, asking whether to install the remaining explicit
    // packages that were just build.
    log::info("Installing built packages.");
    log::prompt();
    actions::install_all(&pack_toml)?;

    Ok(())
}

/// Install some packages for which a complete binary tarball is present in the
/// cache directory.
pub fn install(packs: &Vec<String>) -> Result<()> {
    let pack_toml = actions::parse_package(&packs)?;
    actions::install_all(&pack_toml)?;
    Ok(())
}

/// Uninstall some packages by removing the files listed in each package's
/// manifest.
pub fn remove(packs: &Vec<String>) -> Result<()> {
    for pack in packs {
        // Read the package manifest.
        let manifest = fs::read_to_string(format!("/var/cache/arc/installed/{pack}"))
            .context(format!("Couldn't read package manifest at /var/cache/arc/installed/{pack}"))?;

        // Since the manifest was generated using a glob, we iterate through
        // the lines in reverse to remove the deepest files first.
        for file in manifest.lines().rev() {
            let real_path = fs::canonicalize(file)?;
            if fs::metadata(&real_path)?.is_dir() {
                let _ = fs::remove_dir(&real_path);
            } else {
                fs::remove_file(&real_path)
                    .context(format!("Couldn't remove file {}", real_path.display()))?;
            }
        }
    }

    Ok(())
}
