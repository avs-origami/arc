use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::process::{self, Command};

use anyhow::{bail, Context, Result};
use http_req::request;
use lazy_static::lazy_static;
use toml::{Table, Value};

use crate::log;

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
    eprintln!(r"    .---.");
    eprintln!(r"   /\  \ \   ___ ____");
    eprintln!(r"  /  \ -\ \_/__ / __/");
    eprintln!(r" / / /\  \ \  \_\ |__.");
    eprintln!(r"/__./  \.___\    \___/");
    eprintln!();
    eprintln!("Usage: \x1b[33marc\x1b[0m [b|c|d|h|i|l|n|r|s|u|v] [pkg]..");
    log::info_ident("b | build     Build packages");
    log::info_ident("c | checksum  Generate checksums");
    log::info_ident("d | download  Download sources");
    log::info_ident("h | help      Print this help");
    log::info_ident("i | install   Install built packages");
    log::info_ident("l | list      List installed packages");
    log::info_ident("n | new       Create a blank package");
    log::info_ident("p | purge     Purge the package cache ($HOME/.cache/arc)");
    log::info_ident("r | remove    Remove packages");
    log::info_ident("s | sync      Sync remote repositories");
    log::info_ident("u | upgrade   Upgrade all packages");
    log::info_ident("v | version   Print version");
    eprintln!("\nCreated by AVS Origami\n");
    process::exit(code)
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
            for dir in &(*ARC_PATH) {
                if fs::metadata(format!("{dir}/{pack}/package.toml")).is_ok() {
                    let content = fs::read_to_string(format!("{dir}/{pack}/package.toml"))
                        .context(format!("Failed to read {dir}/{pack}/package.toml"))?;

                    package_files.push(content);
                    package_dirs.push(format!("{dir}/{pack}"));
                }
            }
        }
    }

    let packs: Vec<Table> = package_files.iter().zip(packs)
        .map(|(x, y)| x.parse().context(format!("{y}/package.toml")))
        .collect::<Result<_, _>>()?;

    Ok((packs, package_dirs))
}

pub fn download(packs: &Vec<String>) -> Result<Vec<Vec<String>>> {
    let pack_toml = parse_package(packs)?.0;
    let files: Vec<Vec<String>> = pack_toml.iter().zip(packs)
        .map(|(x, y)| download_all(&x["meta"]["sources"], y))
        .collect::<Result<_>>()?;

    Ok(files)
}

pub fn generate_checksums() -> Result<()> {
    let filenames = download(&vec![".".into()])?;
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
    let (pack_toml, pack_dirs) = parse_package(&packs)?;
    let filenames = download(&packs)?;
    
    for ((pack, toml), name) in filenames.iter().zip(pack_toml).zip(packs) {
        if let Value::Array(x) = &toml["meta"]["checksums"] {
            verify_checksums(pack, &x, name)?;
        }
    }

    for ((pack, name), dir) in filenames.iter().zip(packs).zip(pack_dirs) {
        let src_dir = format!("{}/build/{name}/src", *CACHE);
        let dest_dir = format!("{}/build/{name}/dest", *CACHE);
        fs::create_dir_all(&src_dir).context(format!("Couldn't create directory {src_dir}"))?;
        fs::create_dir_all(&dest_dir).context(format!("Couldn't create directory {dest_dir}"))?;

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

        let build_script = fs::canonicalize(format!("{dir}/build"))
            .context(format!("Couldn't canonicalize path {dir}/build"))?;

        Command::new(build_script)
            .arg(dest_dir)
            .current_dir(src_dir)
            .status()
            .context(format!("Couldn't execute {dir}/build"))?;
    }

    Ok(())
}

pub fn download_all(urls: &Value, name: &String) -> Result<Vec<String>> {
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

            if fs::metadata(filename.clone()).is_ok() {
                continue;
            }

            if url.starts_with("https://") || url.starts_with("http://") {            
                loop { 
                    let mut body = Vec::new();
                    let res = request::get(&url, &mut body)
                        .context(format!("Couldn't connect to {url}"))?;

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
