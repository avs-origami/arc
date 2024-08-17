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

pub fn version() -> ! {
    log::info(&format!("Arc package manager version {VERSION}"));
    process::exit(0)
}

pub fn new(name: String) -> Result<()> {
    fs::create_dir(&name).context(format!("Failed to create directory {name}"))?;

    let mut package = File::create(format!("{name}/package.toml"))
        .context(format!("Failed to create {name}/package.toml"))?;

    package.write_all(PKG_TEMPLATE).context(format!("Failed to write to {name}/package.toml"))?;

    let mut build = OpenOptions::new()
        .write(true)
        .create(true)
        .mode(0o755)
        .open(format!("{name}/build"))
        .context(format!("Failed to open {name}/build"))?;

    build.write_all(b"#!/bin/sh -e\n").context(format!("Failed to write to {name}/build"))?;

    Ok(())
}

pub fn purge_cache() -> Result<()> {
    fs::remove_dir_all((*CACHE).clone())?;
    Ok(())
}

pub fn download(packs: &Vec<String>) -> Result<()> {
    log::info("Downloading sources");
    actions::download_all(packs, None, None, true, None)?;
    Ok(())
}

pub fn generate_checksums() -> Result<()> {
    let filenames = actions::download_all(&vec![".".into()], None, None, true, None)?;
    let mut hashes = vec![];
    for file in &filenames[0] {
        let file = if file.starts_with("tar+") { &file[4..] } else { &file[..] };
        let data: Vec<u8> = fs::read(file).context("Failed to read source file")?;
        let hash = blake3::hash(&data);
        hashes.push(hash.to_string());
    }

    eprintln!("Add the following to package.toml under [meta]:");
    println!("checksums = {hashes:#?}");

    Ok(())
}

pub fn build(packs: &Vec<String>, verbose: bool) -> Result<()> {
    let (pack_toml, pack_dirs) = actions::parse_package(&packs)?;

    let pad = packs.iter().fold(&packs[0], |acc, item| {
        if item.len() > acc.len() { &item } else { acc }
    }).len();

    let deps = actions::resolve_deps(packs, &pack_toml, 1)?;
    let dep_names: Vec<String> = deps.iter().map(|x| x.name.clone()).collect();
    let (dep_toml, dep_dirs) = actions::parse_package(&dep_names)?;

    let pad_dep = if dep_names.len() > 0 {
        dep_names.iter().fold(&dep_names[0], |acc, item| {
            if item.len() > acc.len() { &item } else { acc }
        }).len()
    } else {
        0
    };

    let pad = if pad >= pad_dep { pad } else { pad_dep };

    let version_pad = pack_toml.iter().fold(
        &pack_toml[0],
        |acc, item| {
            let version_acc = acc["meta"]["version"].as_str().unwrap();
            let version_item = item["meta"]["version"].as_str().unwrap();

            if version_item.len() > version_acc.len() { &item } else { acc }
        }
    )["meta"]["version"].as_str().unwrap().len();

    let version_pad_dep = if dep_names.len() > 1 {
        dep_toml.iter().fold(
            &dep_toml[0],
            |acc, item| {
                let version_acc = acc["meta"]["version"].as_str().unwrap();
                let version_item = item["meta"]["version"].as_str().unwrap();

                if version_item.len() > version_acc.len() { &item } else { acc }
            }
        )["meta"]["version"].as_str().unwrap().len()
    } else if dep_names.len() == 1 {
        dep_toml[0]["meta"]["version"].as_str().unwrap().len()
    } else {
        0
    };

    let version_pad = if version_pad >= version_pad_dep { version_pad } else { version_pad_dep };
    let real_pad = pad;

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

    for (pack, toml) in packs.iter().zip(&pack_toml) {
        if actions::is_installed(pack, toml["meta"]["version"].as_str().unwrap())? {
            log::warn(&format!("Package {pack} is up to date - reinstalling"));
        }
    }

    log::info("Building packages:\n");
    info_fmt!("{: <pad$} {: <version_pad$}", name_header, version_header);
    eprintln!();

    for (pack, toml) in packs.iter().zip(&pack_toml) {
        if dep_names.contains(pack) { continue; }
        let version = toml["meta"]["version"].as_str().unwrap();
        info_fmt!("{: <pad$} {: <version_pad$} (explicit)", pack, version);
    }

    for (pack, toml) in deps.iter().zip(&dep_toml) {
        let version = toml["meta"]["version"].as_str().unwrap();
        info_fmt!("{: <pad$} {: <version_pad$} (layer {})", pack.name, version, pack.depth);
    }

    eprintln!();

    log::prompt();

    log::info("Downloading sources");
    let filenames = actions::download_all(packs, Some(&pack_toml), Some(&pack_dirs), false, Some(real_pad))?;
    let dep_filenames = actions::download_all(&dep_names, Some(&dep_toml), Some(&dep_dirs), false, Some(real_pad))?;
    eprintln!();

    log::info("Verifying checksums");
    actions::checksums_all(packs, &pack_toml, &pack_dirs, &filenames, real_pad)?;
    actions::checksums_all(&dep_names, &dep_toml, &dep_dirs, &dep_filenames, real_pad)?;
    eprintln!();

    if deps.len() > 0 {
        let mut layer_idxs = vec![];
        let mut depth = deps[0].depth;
        let mut prev_idx = 0;
        for (i, pack) in deps.iter().enumerate() {
            if pack.depth < depth {
                layer_idxs.push((prev_idx, i));
                depth = pack.depth;
                prev_idx = i;
            }
        }

        layer_idxs.push((prev_idx, deps.len()));

        for idx in &layer_idxs {
            actions::build_all(
                &dep_names[idx.0..idx.1].to_vec(),
                &dep_toml[idx.0..idx.1].to_vec(),
                &dep_dirs[idx.0..idx.1].to_vec(),
                &dep_filenames[idx.0..idx.1].to_vec(),
                verbose,
            )?;

            info_fmt!("Installing layer {} dependencies", deps[idx.0].depth);
            actions::install_all(&dep_names[idx.0..idx.1].to_vec(), &dep_toml[idx.0..idx.1].to_vec())?;
        }
    }

    actions::build_all(packs, &pack_toml, &pack_dirs, &filenames, verbose)?;

    log::info("Installing built packages.");
    log::prompt();
    actions::install_all(packs, &pack_toml)?;

    Ok(())
}

pub fn install(packs: &Vec<String>) -> Result<()> {
    let (pack_toml, _) = actions::parse_package(&packs)?;
    actions::install_all(packs, &pack_toml)?;
    Ok(())
}

pub fn remove(packs: &Vec<String>) -> Result<()> {
    for pack in packs {
        let manifest = fs::read_to_string(format!("/var/cache/arc/installed/{pack}"))
            .context(format!("Couldn't read package manifest at /var/cache/arc/installed/{pack}"))?;

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
