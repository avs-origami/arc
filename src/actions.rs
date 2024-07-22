use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::process;

use http_req::request;
use lazy_static::lazy_static;
use toml::{Table, Value};

use crate::{log, Res};

const PKG_TEMPLATE: &[u8] = b"[meta]
version = \"\"
maintainer = \"\"
sources = []
checksums = []

[deps]

[mkdeps]
";

lazy_static! {
    static ref HOME: String = env::var("HOME").expect("$HOME is not set!");
    static ref CACHE: String = format!("{}/.cache/arc", *HOME);
}

pub fn print_help(code: i32) {
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
    log::info_ident("r | remove    Remove packages");
    log::info_ident("s | sync      Sync remote repositories");
    log::info_ident("u | upgrade   Upgrade all packages");
    log::info_ident("v | version   Print version");
    eprintln!("\nCreated by AVS Origami\n");
    process::exit(code);
}

pub fn new(name: String) -> Res<()> {
    fs::create_dir(&name)?;
    
    let mut package = File::create(format!("{name}/package.toml"))?;
    package.write_all(PKG_TEMPLATE)?;

    let mut build = OpenOptions::new()
        .write(true)
        .create(true)
        .mode(0o755)
        .open(format!("{name}/build"))?;

    build.write_all(b"#!/bin/sh -e\n")?;

    Ok(())
}

pub fn parse_package(packs: &Vec<String>) -> Res<Vec<Table>> {
    let package_files: Vec<String> = packs.iter()
        .map(|x| fs::read_to_string(format!("{}/package.toml", x)))
        .collect::<Result<_, _>>()?;

    let packs: Vec<Table> = package_files.iter()
        .map(|x| x.parse())
        .collect::<Result<_, _>>()?;

    Ok(packs)
}

pub fn download(packs: &Vec<String>) -> Res<Vec<Vec<String>>> {
    let pack_toml = parse_package(packs)?;
    let files: Vec<Vec<String>> = pack_toml.iter().zip(packs)
        .map(|(x, y)| download_all(&x["meta"]["sources"], y))
        .collect::<Result<_, _>>()?;

    Ok(files)
}

pub fn generate_checksums() -> Res<()> {
    let filenames = download(&vec![".".into()])?;
    let mut hashes = vec![];
    for file in &filenames[0] {
        let data: Vec<u8> = fs::read(file)?;
        let hash = blake3::hash(&data);
        hashes.push(hash.to_string());
    }

    eprintln!("Add the following to package.toml under [meta]:");
    println!("checksums = {hashes:#?}");

    Ok(())
}

pub fn build(packs: &Vec<String>) -> Res<()> {
    let pack_toml = parse_package(&packs)?;
    let filenames = download(&packs)?;
    
    for ((pack, toml), name) in filenames.iter().zip(pack_toml).zip(packs) {
        if let Value::Array(x) = &toml["meta"]["checksums"] {
            verify_checksums(pack, &x, name)?;
        }
    }

    Ok(())
}

pub fn download_all(urls: &Value, name: &String) -> Res<Vec<String>> {
    let mut fnames = vec![];
    if let Value::Array(x) = urls {
        let dir = format!("{}/dl", *CACHE);
        fs::create_dir_all(&dir)?;

        for url in x {
            let mut url = url.to_string();
            url = url.replace("\"", "");
            if ! (url.starts_with("https://") || url.starts_with("http://")) {
                continue;
            }

            let filename = url.split('/').last().unwrap().to_owned();
            let filename = format!("{dir}/{filename}");
            fnames.push(filename.clone());

            if fs::metadata(filename.clone()).is_ok() {
                continue;
            }
            
            loop { 
                let mut body = Vec::new();
                let res = request::get(&url, &mut body)?;

                if res.status_code().is_success() {
                    let mut out = File::create(filename)?;
                    out.write_all(&body)?;
                    break;
                } else if res.status_code().is_redirect() {
                    url = res.headers().get("Location").unwrap().to_owned();                    
                } else {
                    return Err(format!(
                        "Failed to download source {url} ({} {})",
                        res.status_code(),
                        res.reason()
                    ).into());
                }
            }
        } 
    } else {
        return Err(format!("Problem parsing {name}/package.toml: sources is not an array").into());
    }

    Ok(fnames)
}

pub fn verify_checksums(fnames: &Vec<String>, checksums: &Vec<Value>, pack: &String) -> Res<()> {
    if checksums.len() == 0 && fnames.len() != 0 {
        return Err(format!("No checksums found for package {pack}").into());
    }

    for (file, sum) in fnames.iter().zip(checksums) {
        let data: Vec<u8> = fs::read(file)?;
        let hash = blake3::hash(&data);
        if hash.to_string() != sum.to_string().replace("\"", "") {
            return Err(format!("Checksum mismatch for file {file}").into());
        }
    }

    Ok(())
}
