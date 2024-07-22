use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::process;

use http_req::request;
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

pub fn print_help(code: i32) {
    eprintln!();
    eprintln!(r"    .---.");
    eprintln!(r"   /\  \ \   ___ ____");
    eprintln!(r"  /  \ -\ \_/__ / __/");
    eprintln!(r" / / /\  \ \  \_\ |__.");
    eprintln!(r"/__./  \.___\    \___/");
    eprintln!();
    eprintln!("Usage: \x1b[33marc\x1b[0m [b|c|d|h|i|l|n|r|u|U|v] [pkg]..");
    log::info_ident("b | build     Build packages");
    log::info_ident("c | checksum  Generate checksums");
    log::info_ident("d | download  Download sources");
    log::info_ident("h | help      Print this help");
    log::info_ident("i | install   Install built packages");
    log::info_ident("l | list      List installed packages");
    log::info_ident("n | new       Create a blank package");
    log::info_ident("r | remove    Remove packages");
    log::info_ident("u | update    Update repositories");
    log::info_ident("U | upgrade   Update all packages");
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

pub fn download(packs: Vec<String>) -> Res<()> {
    let package_files: Vec<String> = packs.iter()
        .map(|x| fs::read_to_string(format!("{}/package.toml", x)))
        .collect::<Result<_, _>>()?;

    let packs: Vec<Table> = package_files.iter()
        .map(|x| x.parse())
        .collect::<Result<_, _>>()?;

    packs.iter().try_for_each(|x| download_all(&x["meta"]["sources"]))?;

    Ok(())
}

pub fn build(packs: Vec<String>) -> Res<()> {
    let package_files: Vec<String> = packs.iter()
        .map(|x| fs::read_to_string(format!("{}/package.toml", x)))
        .collect::<Result<_, _>>()?;

    let packs: Vec<Table> = package_files.iter()
        .map(|x| x.parse())
        .collect::<Result<_, _>>()?;

    packs.iter().try_for_each(|x| download_all(&x["meta"]["sources"]))?;

    Ok(())
}

pub fn download_all(urls: &Value) -> Res<()> {
    if let Value::Array(x) = urls {
        for url in x {
            let mut url = url.to_string();
            url = url.replace("\"", "");
            if ! (url.starts_with("https://") || url.starts_with("http://")) {
                continue;
            }

            let filename = url.split('/').last().unwrap().to_owned();

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
                    log::die(&format!(
                        "Failed to download source {url} ({} {})",
                        res.status_code(),
                        res.reason()
                    ));
                }
            }
        }
    }

    Ok(())
}
