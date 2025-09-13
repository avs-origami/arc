//! This module contains the main commands that can be directly called by the
//! user through command line arguments.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::process::{self, Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use glob::glob;
use indicatif::{ProgressBar, ProgressStyle};
use lazy_static::lazy_static;

pub mod args;
pub mod actions;
pub mod config;
pub mod bars;
pub mod log;
pub mod util;

lazy_static! {
    pub static ref HOME: String = env::var("HOME").unwrap_or_else(|_| {
        log::die("$HOME is not set!");
    });

    pub static ref CFG_STR: String = fs::read_to_string("/etc/moss.toml").unwrap_or_else(|x| {
        log::die(&format!("Couldn't read config file at /etc/moss.toml: {:#}", x));
    });

    pub static ref CFG: config::Config = toml::from_str(&*CFG_STR).unwrap_or_else(|x| {
        log::die(&format!("Couldn't parse config file at /etc/moss.toml: {:#}", x));
    });

    pub static ref ARC_PATH: Vec<String> = CFG.path.clone();

    pub static ref CACHE: String = CFG.cache_dir.clone().unwrap_or(format!("{}/.cache/moss", *HOME));
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
pub fn print_help(code: i32, msg: String) -> ! {
    if msg.len() > 0 {
        log::log(&format!("ERROR: {msg}"), 31);
    }

    let cache_display = if *CACHE == format!("{}/.cache/moss", *HOME) {
        "$HOME/.cache/moss"
    } else {
        &*CACHE.clone()
    };

    eprintln!();
    eprintln!("  \x1b[32m/\\/\\   ___  ___ ___ \x1b[0m");
    eprintln!(" \x1b[33m/    \\ \x1b[32m/ \x1b[36m_\x1b[32m \\/ __/ __|\x1b[0m");
    eprintln!("\x1b[35m/ /\\/\\ \\ \x1b[36m(_)\x1b[90m \\__ \\__ \\");
    eprintln!("\x1b[35m\\/    \\/\x1b[90m\\\x1b[33m___\x1b[90m/|\x1b[33m___\x1b[90m/\x1b[33m___\x1b[90m/");
    eprintln!("\x1b[0m");
    eprintln!("Usage: \x1b[33mmoss\x1b[0m [s/v/y][b/c/d/f/h/i/l/n/p/r/s/u/v] [pkg]...");
    log::info_ident("b / build     Build packages");
    log::info_ident("c / checksum  Generate checksums");
    log::info_ident("d / download  Download sources");
    log::info_ident("f / find      Fuzzy search for a package");
    log::info_ident("h / help      Print this help");
    log::info_ident("i / install   Install built packages");
    log::info_ident("l / list      List installed packages");
    log::info_ident("n / new       Create a blank package");
    info_ident_fmt!("p / purge     Purge the package cache ({cache_display})");
    log::info_ident("r / remove    Remove packages");
    log::info_ident("s / sync      Sync remote repositories");
    log::info_ident("u / upgrade   Upgrade all packages");
    log::info_ident("v / version   Print version");
    eprintln!("Flags:");
    log::info_ident("s  Sync remote repositories");
    log::info_ident("v  Enable verbose builds");
    log::info_ident("y  Skip confirmation prompts");
    eprintln!("\nCreated by AVS Origami\n");
    process::exit(code)
}

/// Print out the version and exit.
pub fn version() -> ! {
    log::info(&format!("Moss package manager version {VERSION}"));
    process::exit(0)
}

/// Create an empty package template in the current directory. Creates the
/// following directory structure:
///
/// <name>
/// |--- package.toml
/// `--- build (executable)
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

/// List installed packages, one per line.
pub fn list() -> Result<()> {
    let installed = glob::glob("/var/cache/moss/installed/*")?;
    for pkg in installed {
        info_fmt!("{}", &pkg?.display().to_string().split('/').last().unwrap());
    }

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

/// Sync remote repositories.
pub fn sync() -> Result<()> {
    log::info("Syncing remote repositories");

    let mut pad = 0;
    for dir in &*ARC_PATH {
        let name = dir.split('/').last().unwrap();
        if name.len() > pad {
            pad = name.len();
        }
    }

    for dir in &*ARC_PATH {
        let name = dir.split('/').last().unwrap();
        let bar = "[{elapsed_precise}] [{spinner:.magenta}]";
        let bar_fmt = format!("  \x1b[35m->\x1b[0m \x1b[36m{name: <pad$}\x1b[0m {bar}");

        let sp = ProgressBar::new_spinner();
        sp.enable_steady_tick(Duration::from_millis(75));
        sp.set_style(ProgressStyle::with_template(&bar_fmt).unwrap().tick_strings(&bars::SPIN));

        Command::new("git")
            .arg("pull")
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context(format!("Couldn't pull repo {dir} with git"))?;

        sp.finish();
    }

    println!("\n");
    Ok(())
}

/// Perform a full system upgrade (update packages that have available updates).
pub fn upgrade(args: &args::Cmd) -> Result<()> {
    log::info("Performing full system upgrade.");
    let installed = glob::glob("/var/cache/moss/installed/*")?;
    let mut packs = vec![];

    for pkg in installed {
        let name = pkg?.display().to_string();
        let basename = name.split('/').last().unwrap();
        let name_no_ver = basename.split('@').nth(0).unwrap().to_string();
        let parsed_maybe_err = actions::parse_package(&vec![name_no_ver]);
        let Ok(prs) = parsed_maybe_err else {
            // If an installed package is not in the repos, ignore it, but only
            // ignore errors caused by "couldn't resolve package."
            let err = parsed_maybe_err.unwrap_err();
            if err.to_string().contains("Couldn't resolve package") {
                continue;
            } else {
                return Err(err);
            }
        };

        let parsed = prs[0].clone();

        if ! actions::is_installed(&parsed.name, &parsed.meta.version)? {
            packs.push(parsed.name);
        }
    }

    if packs.len() > 0 {
        build(&packs, args)?;
    } else {
        log::info("All packages up to date. Congratulations!");
    }

    Ok(())
}

pub fn search(name: String) -> Result<()> {
    for dir in &*ARC_PATH {
        for pkg in fs::read_dir(dir)? {
            let pkg = pkg?; 
            let pkg = pkg.file_name();
            let pkg = pkg.to_str().unwrap();
            if pkg.contains(&name) &&! pkg.starts_with(".") {
                let meta = actions::parse_package(&vec![pkg.into()]);
                if let Ok(x) = meta {
                    info_fmt!("{} @ {}", pkg, x[0].meta.version);
                }
            }
        }
    }

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
pub fn build(packs: &Vec<String>, args: &args::Cmd) -> Result<()> {
    // Output package summary.
    let (pack_toml, dep_toml, dep_names, mkdep_toml, mkdep_names, real_pad) = actions::summary(packs, args, "Building")?;

    // Download all source files.
    log::info("Downloading sources");
    let pack_toml = actions::download_all(packs, Some(pack_toml), false, Some(real_pad))?;
    let dep_toml = actions::download_all(&dep_names, Some(dep_toml), false, Some(real_pad))?;
    let mkdep_toml = actions::download_all(&mkdep_names, Some(mkdep_toml), false, Some(real_pad))?;
    eprintln!();

    // Verify checksums for all the source files.
    log::info("Verifying checksums");
    actions::checksums_all(&pack_toml, real_pad)?;
    actions::checksums_all(&dep_toml, real_pad)?;
    eprintln!();

    // If we have any make dependencies, build and install them first.
    if mkdep_toml.len() > 0 {
        // All the dependency data is sorted by layer. Determine on which
        // indices the data must be split to separate the packages based
        // on layer.
        let mut layer_idxs = vec![];
        let mut depth = mkdep_toml[0].depth;
        let mut prev_idx = 0;
        for (i, pack) in mkdep_toml.iter().enumerate() {
            if pack.depth < depth {
                layer_idxs.push((prev_idx, i));
                depth = pack.depth;
                prev_idx = i;
            }
        }

        layer_idxs.push((prev_idx, mkdep_toml.len()));

        // Build and install the dependencies, one layer at a time.
        for idx in &layer_idxs {
            actions::build_all(
                &mkdep_toml[idx.0..idx.1].to_vec(),
                args,
            )?;

            info_fmt!("Installing layer {} make dependencies", mkdep_toml[idx.0].depth);
            actions::install_all(&mkdep_toml[idx.0..idx.1].to_vec())?;
            eprintln!();
        }
    }

    // If we have any dependencies, build and install them next.
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
                args,
            )?;

            info_fmt!("Installing layer {} dependencies", dep_toml[idx.0].depth);
            for inst in &dep_toml[idx.0..idx.1] {
                actions::install_all(&vec![inst.clone()])?;
                eprintln!();
            }
        }
    }

    // Build all remaining explicit packages.
    actions::build_all(&pack_toml, args)?;

    // Prompt the user, asking whether to install the remaining explicit
    // packages that were just build.
    log::info("Installing built packages.");
    if !args.yes { log::prompt(); }
    actions::install_all(&pack_toml)?;

    Ok(())
}

/// Install some packages for which a complete binary tarball is present in the
/// cache directory.
pub fn install(packs: &Vec<String>, args: &args::Cmd) -> Result<()> {
    let (pack_toml, _, _, _, _, _) = actions::summary(packs, args, "Installing")?;
    actions::install_all(&pack_toml)?;
    Ok(())
}

/// Uninstall some packages by removing the files listed in each package's
/// manifest.
pub fn remove(packs: &Vec<String>, args: &args::Cmd) -> Result<()> {
    let _ = actions::summary(packs, args, "Removing")?;

    for pack in packs {
        // Make sure the package is installed.
        if !actions::is_installed(pack, &"*".into())? {
            bail!("Package {pack} is not installed");
        }

        // Read the package manifest.
        let mut manifest_glob = glob(&format!("/var/cache/moss/installed/{pack}@*"))
            .context(format!("Error constructing glob /var/cache/moss/installed/{pack}@*"))?;

        let manifest_path = manifest_glob.next().unwrap().context("Couldn't get manifest path")?;

        let manifest = fs::read_to_string(&manifest_path)
            .context(format!("Couldn't read manifest of package {pack} at {}", manifest_path.display()))?;

        if manifest.starts_with("->") {
            let real_pack = &manifest.lines().next().unwrap()[3..];
            let real_name = real_pack.split('@').next().unwrap();
            bail!("Package '{pack}' is provided by '{real_pack}'; to remove it, remove '{real_name}' instead");
        }

        // Since the manifest was generated using a glob, we iterate through
        // the lines in reverse to remove the deepest files first.
        for file in manifest.lines().rev() {
            if file == "/var/cache/moss/installed" {
                continue;
            }

            let _ = Command::new("rmdir")
                .arg(file)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();

            if let Some(_) = actions::is_tracked(&file.into())? {
                if !fs::symlink_metadata(file)?.file_type().is_symlink() {
                    let _ = Command::new("rm")
                        .arg(file)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            } else {
                let _ = Command::new("rm")
                    .arg(file)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }

        info_fmt!("{pack} Successfully uninstalled package");
    }

    Ok(())
}
