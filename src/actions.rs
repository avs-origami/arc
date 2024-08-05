use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process::{self, Command};

use anyhow::{bail, Context, Result};
use glob::glob;
use http_req::request;
use indicatif::{ProgressBar, ProgressStyle};
use lazy_static::lazy_static;
use nix::unistd::Uid;
use toml::{Table, Value};
use toml::map::Map;

use crate::log;
use crate::{info_fmt, info_ident_fmt};

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

#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub depth: usize,
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
    log::info("Downloading sources");

    let longest = packs.iter().fold(&packs[0], |acc, item| {
        if item.len() > acc.len() { &item } else { acc }
    });

    let files: Vec<Vec<String>> = if let Some(n) = pack_toml {
        n.iter().zip(packs)
            .map(|(x, y)| download_all(&x["meta"]["sources"], y, force, longest.len()))
            .collect::<Result<_>>()?
    } else {
        let pack_toml = parse_package(packs)?.0;
        pack_toml.iter().zip(packs)
            .map(|(x, y)| download_all(&x["meta"]["sources"], y, force, longest.len()))
            .collect::<Result<_>>()?
    };

    Ok(files)
}

pub fn generate_checksums() -> Result<()> {
    let filenames = download(&vec![".".into()], None, true)?;
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

pub fn build(packs: &Vec<String>) -> Result<()> {
    let (pack_toml, pack_dirs) = parse_package(&packs)?;

    let pad = packs.iter().fold(&packs[0], |acc, item| {
        if item.len() > acc.len() { &item } else { acc }
    }).len();

    let deps = resolve_deps(packs, &pack_toml, 0)?;
    let dep_names: Vec<String> = deps.iter().map(|x| x.name.clone()).collect();
    let (dep_toml, dep_dirs) = parse_package(&dep_names)?;

    log::info("Building packages (explicit):");
    for (pack, toml) in packs.iter().zip(&pack_toml) {
        let version = toml["meta"]["version"].as_str().unwrap();
        info_ident_fmt!("{: <pad$} @ {}", pack, version);
    }

    log::info("Building packages (implicit):");
    for (pack, toml) in deps.iter().zip(&dep_toml) {
        let version = toml["meta"]["version"].as_str().unwrap();
        eprint!("{}", " ".repeat(pack.depth * 2));
        info_ident_fmt!("{: <pad$} @ {}", pack.name, version);
    }

    eprintln!();

    log::info("Press Enter to continue or Ctrl+C to abort");
    let _ = io::stdin().read(&mut [0u8]);

    build_all(packs, pack_toml, pack_dirs, pad)?;
    Ok(())

}

pub fn install(packs: &Vec<String>) -> Result<()> {
    let (pack_toml, _) = parse_package(&packs)?;
    install_all(packs, &pack_toml)?;
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

pub fn resolve_deps(
    packs: &Vec<String>,
    pack_toml: &Vec<Map<String, Value>>,
    depth: usize,
) -> Result<Vec<Package>> {
    let mut output = vec![];
    for pack in pack_toml {
        for (name, _ver_req) in pack["deps"].as_table().unwrap() {
            let res = Package { name: name.to_string(), depth };
            output.push(res);

            let (dep_toml, _) = parse_package(&vec![name.to_string()])?;
            let mut deps = resolve_deps(&vec![name.to_string()], &dep_toml, depth + 1)?;
            output.append(&mut deps);
        }
    }

    Ok(output)
}

pub fn build_all(
    packs: &Vec<String>,
    pack_toml: Vec<Map<String, Value>>,
    pack_dirs: Vec<String>,
    pad: usize
) -> Result<()> {
    let filenames = download(&packs, Some(&pack_toml), false)?;
    eprintln!("\n");
   
    log::info("Verifying checksums");
    for ((pack, toml), name) in filenames.iter().zip(&pack_toml).zip(packs) {
        if let Value::Array(x) = &toml["meta"]["checksums"] {
            verify_checksums(pack, &x, name, pad)?;
        } else {
            bail!("Problem parsing {name}/package.toml: checksums is not an array");
        }
    }

    eprintln!();

    for (i, (((pack, name), dir), toml)) in filenames.iter()
        .zip(packs).zip(pack_dirs).zip(&pack_toml)
        .enumerate()
    {
        let version = toml["meta"]["version"].as_str().unwrap();
        info_fmt!("\x1b[36m{}\x1b[0m Building package ({}/{})", name, i + 1, filenames.len());

        let src_dir = format!("{}/build/{name}/src", *CACHE);
        let dest_dir = format!("{}/build/{name}/dest", *CACHE);
        fs::create_dir_all(&src_dir).context(format!("Couldn't create directory {src_dir}"))?;
        fs::create_dir_all(&dest_dir).context(format!("Couldn't create directory {dest_dir}"))?;

        info_fmt!("\x1b[36m{}\x1b[0m Extracting sources", name);

        for file in pack {
            if file.starts_with("tar+") {
                let file = &file[4..];
                let basename = file.split('/').last().unwrap();
                fs::copy(file, format!("{src_dir}/{basename}"))
                    .context(format!("Couldn't copy {file} to build dir"))?;
            } else if file.contains(".tar") {
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

        info_fmt!("\x1b[36m{}\x1b[0m Running build script\n", name);

        let build_script = fs::canonicalize(format!("{dir}/build"))
            .context(format!("Couldn't canonicalize path {dir}/build"))?;

        let build_status = Command::new(build_script)
            .arg(&dest_dir)
            .current_dir(src_dir)
            .status()
            .context(format!("Couldn't execute {dir}/build"))?;

        eprintln!();

        if build_status.success() {
            info_fmt!("\x1b[36m{}\x1b[0m Successfully built package", name);
        } else {
            bail!("Couldn't build package {name}");
        }

        info_fmt!("\x1b[36m{}\x1b[0m Generating manifest", name);
        
        let manifest_dir = format!("{dest_dir}/var/cache/arc/installed");
        let manifest = format!("{manifest_dir}/{name}");

        fs::create_dir_all(&manifest_dir)
            .context(format!("Couldn't create directory {manifest_dir}"))?;

        let mut manifest_file = File::create(&manifest)
            .context(format!("Couldn't create file {manifest}"))?;

        let mut manifest_content = String::new();
        for file in glob(&format!("{dest_dir}/**/*"))? {
            let line = format!("{}\n", file?.display());
            manifest_content.push_str(&line.replace(&dest_dir, ""));
        }

        manifest_file.write_all(manifest_content.as_bytes())
            .context(format!("Couldn't write to file {manifest}"))?;

        info_fmt!("\x1b[36m{}\x1b[0m Creating tarball", name);

        let bin_dir = format!("{}/bin", *CACHE);
        fs::create_dir_all(&bin_dir).context(format!("Couldn't create directory {bin_dir}"))?;

        Command::new("tar")
            .args(["czf", &format!("{}/{}@{}.tar.gz", bin_dir, name, version), "."])
            .current_dir(&dest_dir)
            .status()
            .context("Couldn't create tarball of built package")?;

        info_fmt!("\x1b[36m{}\x1b[0m Successfully created tarball", name);
        eprintln!();
    }

    log::info("Installing built packages.");
    log::info("Press Enter to continue or Ctrl+C to abort");
    let _ = io::stdin().read(&mut [0u8]);
    install_all(packs, &pack_toml)?;
 
    Ok(())
}

pub fn install_all(packs: &Vec<String>, pack_toml: &Vec<Map<String, Value>>) -> Result<()> {
    let su_command = if fs::metadata("/bin/sudo").is_ok() {
        "sudo"
    } else if fs::metadata("bin/doas").is_ok() {
        "doas"
    } else if fs::metadata("/bin/su").is_ok() {
        "su"
    } else {
        ""
    };

    if ! Uid::effective().is_root() {
        log::info("Using sudo to become root.");
    }

    for (i, (name, toml)) in packs.iter().zip(pack_toml).enumerate() {
        let version = toml["meta"]["version"].as_str().unwrap();
        let bin_file = format!("{}/bin/{}@{}.tar.gz", *CACHE, name, version);

        if Uid::effective().is_root() {
            Command::new("tar")
                .args(["xf", &bin_file, "-C", "/"])
                .status()
                .context(format!("Couldn't extract {bin_file} to /"))?;
        } else {
            match su_command {
                "sudo" => {
                    Command::new("sudo")
                        .args(["tar", "xf", &bin_file, "-C", "/"])
                        .status()
                        .context(format!("Couldn't extract {bin_file} to /"))?;
                },
                "doas" => {
                    Command::new("doas")
                        .args(["tar", "xf", &bin_file, "-C", "/"])
                        .status()
                        .context(format!("Couldn't extract {bin_file} to /"))?;
                },
                "su" => {
                    Command::new("su")
                        .args(["-c", "tar", "xf", &bin_file, "-C", "/"])
                        .status()
                        .context(format!("Couldn't extract {bin_file} to /"))?;
                },
                _ => bail!("Couldn't find a command to elevate privileges"),
            }
        }

        info_fmt!("Successfully installed {} @ {} ({}/{})", name, version, i + 1, packs.len());
    }

    Ok(())
}

pub fn download_all(urls: &Value, name: &String, force: bool, pad: usize) -> Result<Vec<String>> {
    let mut fnames = vec![];
    if let Value::Array(x) = urls {
        let dir = format!("{}/dl", *CACHE);
        fs::create_dir_all(&dir).context(format!("Couldn't create directory {dir}"))?;

        for (i, url) in x.iter().enumerate() {
            let og_url = url.to_string().replace("\"", "");
            let mut url = url.to_string().replace("\"", "");
            
            let filename = url.split('/').last().unwrap().to_owned();
            let filename = format!("{dir}/{filename}");

            if url.starts_with("tar+") {
                url = url[4..].to_string();
                fnames.push("tar+".to_owned() + &filename);
            } else {
                fnames.push(filename.clone());
            }
 
            if fs::metadata(filename.clone()).is_ok() &&! force {
                info_fmt!("\x1b[36m{}\x1b[0m {} already downloaded, skipping", name, url);
                continue;
            }

            if url.starts_with("https://") || url.starts_with("http://") {
                loop {
                    let mut body = vec![];
                    let head = request::head(&url)?;
                    let len = head.content_len().unwrap_or(0);
                    let bar = "[{elapsed_precise}] [{bar:30.magenta/magenta}] ({bytes_per_sec}, ETA {eta})";
                    let bar_fmt = format!("  \x1b[35m->\x1b[0m \x1b[36m{name: <pad$}\x1b[0m {bar} ({}/{}) ({og_url})", i + 1, x.len());

                    let bar = ProgressBar::new(len as u64);
                    bar.set_style(ProgressStyle::with_template(&bar_fmt).unwrap().progress_chars("-> "));

                    let res = request::get_with_update(&url, &mut body, |x| bar.inc(x as u64))
                        .context(format!("Couldn't connect to {url}"))?;

                    if res.status_code().is_success() {
                        bar.finish();
                        let mut out = File::create(&filename).context(format!("Couldn't create file {filename}"))?;
                        out.write_all(&body)
                            .context(format!("Couldn't save downloaded file to {filename}"))?;

                        break;
                    } else if res.status_code().is_redirect() {
                        bar.finish_and_clear();
                        url = res.headers().get("Location").unwrap().to_owned();                    
                    } else {
                        bar.finish_and_clear();
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

pub fn verify_checksums(
    fnames: &Vec<String>,
    checksums: &Vec<Value>,
    pack: &String,
    pad: usize
) -> Result<()> {
    if fnames.len() > checksums.len() {
        bail!("Missing one or more checksums for package {pack}");
    }

    for (file, sum) in fnames.iter().zip(checksums) {
        let file = if file.starts_with("tar+") { &file[4..] } else { &file[..] };
        let data: Vec<u8> = fs::read(file).context(format!("Couldn't read file {file}"))?;
        let hash = blake3::hash(&data);

        info_ident_fmt!(
            "\x1b[36m{: <pad$}\x1b[0m {} / {}",
            pack,
            &sum.as_str().unwrap()[..10],
            &hash.to_string()[..10]
        );

        if hash.to_string() != sum.to_string().replace("\"", "") {
            bail!("Checksum mismatch for file {file}");
        }
    }

    Ok(())
}
