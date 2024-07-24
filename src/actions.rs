use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::process::{self, Command};

use anyhow::{bail, Context, Result};
use http_req::request;
use indicatif::{ProgressBar, ProgressStyle};
use lazy_static::lazy_static;
use toml::{Table, Value};

use crate::log;
use crate::info_fmt;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const PKG_TEMPLATE: &[u8] = b"[meta]
version = \"\"
maintainer = \"\"
sources = []
checksums = []

[deps]

[mkdeps]
";

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

pub fn purge() -> Result<()> {
    fs::remove_dir_all((*CACHE).clone())?;
    Ok(())
}

pub fn parse_package(packs: &Vec<String>) -> Result<(Vec<Table>, Vec<String>)> {
    let mut package_files = vec![];
    let mut package_dirs = vec![];

    for pack in packs {
        if fs::metadata(format!("{pack}/package.toml")).is_ok() {
            let content = fs::read_to_string(format!("{pack}/package.toml"))
                .context(format!("Failed to read {pack}/package.toml"))?;

            package_files.push(content);
            package_dirs.push(format!("{pack}"))
        } else {
            let mut broke = false;
            for dir in &(*ARC_PATH) {
                if fs::metadata(format!("{dir}/{pack}/package.toml")).is_ok() {
                    let content = fs::read_to_string(format!("{dir}/{pack}/package.toml"))
                        .context(format!("Failed to read {dir}/{pack}/package.toml"))?;

                    package_files.push(content);
                    package_dirs.push(format!("{dir}/{pack}"));

                    broke = true;
                    break;
                }
            }

            if ! broke {
                bail!("Couldn't resolve package {pack}");
            }
        }
    }

    let packs: Vec<Table> = package_files.iter().zip(packs)
        .map(|(x, y)| x.parse().context(format!("{y}/package.toml")))
        .collect::<Result<_, _>>()?;

    Ok((packs, package_dirs))
}

pub fn download(
    packs: &Vec<String>,
    pack_toml: Option<&Vec<Table>>,
    force: bool
) -> Result<Vec<Vec<String>>> {
    let files: Vec<Vec<String>> = if let Some(n) = pack_toml {
        n.iter().zip(packs)
            .map(|(x, y)| download_all(&x["meta"]["sources"], y, force))
            .collect::<Result<_>>()?
    } else {
        let pack_toml = parse_package(packs)?.0;
        pack_toml.iter().zip(packs)
            .map(|(x, y)| download_all(&x["meta"]["sources"], y, force))
            .collect::<Result<_>>()?
    };

    Ok(files)
}

pub fn generate_checksums() -> Result<()> {
    let filenames = download(&vec![".".into()], None, true)?;
    let mut hashes = vec![];
    for file in &filenames[0] {
        let data: Vec<u8> = fs::read(file).context("Failed ")?;
        let hash = blake3::hash(&data);
        hashes.push(hash.to_string());
    }

    eprintln!("Add the following to package.toml under [meta]:");
    println!("checksums = {hashes:#?}");

    Ok(())
}

pub fn build(packs: &Vec<String>) -> Result<()> {
    let mut info_packs = format!("Building packages: ");
    for pack in packs {
        info_packs.push_str(pack);
        info_packs.push(' ');
    }

    log::info(&info_packs);

    let (pack_toml, pack_dirs) = parse_package(&packs)?;
    let filenames = download(&packs, Some(&pack_toml), false)?;
    
    for ((pack, toml), name) in filenames.iter().zip(pack_toml).zip(packs) {
        if let Value::Array(x) = &toml["meta"]["checksums"] {
            verify_checksums(pack, &x, name)?;
        } else {
            bail!("Problem parsing {name}/package.toml: checksums is not an array");
        }
    }

    for ((pack, name), dir) in filenames.iter().zip(packs).zip(pack_dirs) {
        info_fmt!("\x1b[36m{}\x1b[0m Building package", name);

        let src_dir = format!("{}/build/{name}/src", *CACHE);
        let dest_dir = format!("{}/build/{name}/dest", *CACHE);
        fs::create_dir_all(&src_dir).context(format!("Couldn't create directory {src_dir}"))?;
        fs::create_dir_all(&dest_dir).context(format!("Couldn't create directory {dest_dir}"))?;

        info_fmt!("\x1b[36m{}\x1b[0m Extracting sources", name);

        for file in pack {
            if file.contains(".tar") {
                Command::new("tar")
                    .args(["xf", file, "-C", &src_dir, "--strip-components=1"])
                    .status()
                    .context(format!("Failed to untar {file}"))?;
            } else {
                let basename = file.split('/').last().unwrap();
                fs::copy(file, format!("{src_dir}/{basename}"))
                    .context(format!("Couldn't copy {file} to build dir"))?;
            }
        }

        info_fmt!("\x1b[36m{}\x1b[0m Running build script", name);

        let build_script = fs::canonicalize(format!("{dir}/build"))
            .context(format!("Couldn't canonicalize path {dir}/build"))?;

        let build_status = Command::new(build_script)
            .arg(dest_dir)
            .current_dir(src_dir)
            // .stdout(process::Stdio::null())
            // .stderr(process::Stdio::null())
            .status()
            .context(format!("Couldn't execute {dir}/build"))?;

        if build_status.success() {
            info_fmt!("\x1b[36m{}\x1b[0m Successfully built package", name);
        } else {
            bail!("Couldn't build package {name}");
        }
    }

    Ok(())
}

pub fn download_all(urls: &Value, name: &String, force: bool) -> Result<Vec<String>> {
    let mut fnames = vec![];
    if let Value::Array(x) = urls {
        let dir = format!("{}/dl", *CACHE);
        fs::create_dir_all(&dir).context(format!("Couldn't create directory {dir}"))?;

        for url in x {
            let mut url = url.to_string();
            url = url.replace("\"", "");
            
            let filename = url.split('/').last().unwrap().to_owned();
            let filename = format!("{dir}/{filename}");
            fnames.push(filename.clone());

            if fs::metadata(filename.clone()).is_ok() &&! force {
                info_fmt!("\x1b[36m{}\x1b[0m {} already downloaded, skipping", name, url);
                continue;
            } 

            if url.starts_with("https://") || url.starts_with("http://") {
                loop {
                    let mut body = vec![];
                    let head = request::head(&url)?;
                    let len = head.content_len().unwrap_or(0);
                    let len_fmt = if len > 0 {
                        format!(" ({})", indicatif::BinaryBytes(len as u64))
                    } else {
                        format!("")
                    };

                    info_fmt!("\x1b[36m{}\x1b[0m Downloading {}{}", name, url, len_fmt);
                    
                    let bar = ProgressBar::new(len as u64);
                    bar.set_style(ProgressStyle::with_template(
                        "\x1b[35m->\x1b[0m [{elapsed_precise}] [{bar:30.magenta/magenta}] ({bytes_per_sec}, ETA {eta})"
                    ).unwrap().progress_chars("-> "));

                    let res = request::get_with_update(&url, &mut body, |x| bar.inc(x as u64))
                        .context(format!("Couldn't connect to {url}"))?;

                    bar.finish();

                    if res.status_code().is_success() {
                        let mut out = File::create(&filename)?;
                        out.write_all(&body)
                            .context(format!("Couldn't save downloaded file to {filename}"))?;

                        break;
                    } else if res.status_code().is_redirect() {
                        url = res.headers().get("Location").unwrap().to_owned();                    
                    } else {
                        bail!(
                            "Failed to download source {url} ({} {})",
                            res.status_code(),
                            res.reason()
                        );
                    }
                    
                }
            } else if url.starts_with("git+") {
                bail!("Git sources are not yet supported ({url})");
            } else {
                fs::copy(format!("{name}/{url}"), filename)
                    .context(format!("Could not copy local file {name}/{url} to build directory"))?;
            }
        } 
    } else {
        bail!("Problem parsing {name}/package.toml: sources is not an array");
    }

    Ok(fnames)
}

pub fn verify_checksums(fnames: &Vec<String>, checksums: &Vec<Value>, pack: &String) -> Result<()> {
    info_fmt!("\x1b[36m{}\x1b[0m Verifying checksums", pack);
    if fnames.len() > checksums.len() {
        bail!("Missing one or more checksums for package {pack}");
    }

    for (file, sum) in fnames.iter().zip(checksums) {
        let data: Vec<u8> = fs::read(file).context(format!("Couldn't read file {file}"))?;
        let hash = blake3::hash(&data);
        if hash.to_string() != sum.to_string().replace("\"", "") {
            bail!("Checksum mismatch for file {file}");
        }
    }

    Ok(())
}
