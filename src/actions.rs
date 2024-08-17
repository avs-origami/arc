use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

use anyhow::{bail, Context, Result};
use glob::glob;
use http_req::request;
use indicatif::{ProgressBar, ProgressStyle};
use nix::unistd::Uid;
use toml::{Table, Value};
use toml::map::Map;

use crate::{info_fmt, info_ident_fmt, ARC_PATH, CACHE};
use crate::log;
use crate::util;

#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub ver: String,
    pub depth: usize,
}

pub fn is_installed(pack: &String, version: &str) -> Result<bool> {
    let mut path = glob(&format!("/var/cache/arc/installed/{pack}@{version}"))?;
    Ok(path.next().is_some())
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

pub fn download_all(
    packs: &Vec<String>,
    pack_toml: Option<&Vec<Table>>,
    pack_dirs: Option<&Vec<String>>,
    force: bool,
    pad: Option<usize>
) -> Result<Vec<Vec<String>>> {
    let longest = match pad {
        Some(x) => x,
        None => packs.iter().fold(&packs[0], |acc, item| {
            if item.len() > acc.len() { &item } else { acc }
        }).len(),
    };

    let files: Vec<Vec<String>> = if let Some(n) = pack_toml {
        n.iter().zip(packs).zip(pack_dirs.unwrap())
            .map(|((x, y), z)| download_one(&x["meta"]["sources"], y, z, force, longest))
            .collect::<Result<_>>()?
    } else {
        let (pack_toml, pack_dirs) = parse_package(packs)?;
        pack_toml.iter().zip(packs).zip(&pack_dirs)
            .map(|((x, y), z)| download_one(&x["meta"]["sources"], y, z, force, longest))
            .collect::<Result<_>>()?
    };

    Ok(files)
}

pub fn resolve_deps(
    packs: &Vec<String>,
    pack_toml: &Vec<Map<String, Value>>,
    depth: usize,
) -> Result<Vec<Package>> {
    let mut raw_output = vec![];
    for (pack, toml) in packs.iter().zip(pack_toml) {
        for (name, ver_req) in toml["deps"].as_table().unwrap() {
            let res = Package { name: name.to_string(), ver: ver_req.to_string(), depth };
            raw_output.push(res);

            let (dep_toml, _) = parse_package(&vec![name.to_string()])?;
            let mut deps = resolve_deps(&vec![name.to_string()], &dep_toml, depth + 1)?;
            raw_output.append(&mut deps);
        }
    }

    let mut output: Vec<Package> = vec![];
    'o: for i in &raw_output {
        if is_installed(&i.name, &i.ver)? {
            continue 'o;
        }

        for j in &output {
            if j.name == i.name {
                continue 'o;
            }
        }

        let mut pack = i.clone();
        for j in &raw_output {
            if j.name == pack.name && j.depth > pack.depth {
                pack = j.clone();
            }
        }

        output.push(pack);
    }

    output.sort_by(|a, b| a.depth.cmp(&b.depth).reverse());
    Ok(output)
}

pub fn checksums_all(
    packs: &Vec<String>,
    pack_toml: &Vec<Map<String, Value>>,
    pack_dirs: &Vec<String>,
    filenames: &Vec<Vec<String>>,
    pad: usize
) -> Result<()> {
    for ((pack, toml), name) in filenames.iter().zip(pack_toml).zip(packs) {
        if let Value::Array(x) = &toml["meta"]["checksums"] {
            verify_checksums(pack, &x, name, pad)?;
        } else {
            bail!("Problem parsing {name}/package.toml: checksums is not an array");
        }
    }

    Ok(())
}

pub fn build_all(
    packs: &Vec<String>,
    pack_toml: &Vec<Map<String, Value>>,
    pack_dirs: &Vec<String>,
    filenames: &Vec<Vec<String>>,
    verbose: bool,
) -> Result<()> {
    for (i, (((pack, name), dir), toml)) in filenames.iter()
        .zip(packs).zip(pack_dirs).zip(pack_toml)
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

        info_fmt!("\x1b[36m{}\x1b[0m Running build script", name);
        if verbose { eprintln!(); }

        let build_script = fs::canonicalize(format!("{dir}/build"))
            .context(format!("Couldn't canonicalize path {dir}/build"))?;

        let log_file = File::create(format!("{dest_dir}/../log.txt"))?;
        let mut build_cmd = Command::new(build_script);
        build_cmd.arg(&dest_dir).arg(&version).current_dir(src_dir);

        let build_status = if !verbose {
            build_cmd.stdout(log_file.try_clone()?).stderr(log_file.try_clone()?);
            build_cmd.status().context(format!("Couldn't execute {dir}/build"))?
        } else {
            build_cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = build_cmd.spawn().context(format!("Couldn't execute {dir}/build"))?;
            let child_out = child.stdout.take().context(format!("Couldn't take stdout of child {dir}/build"))?;
            let child_err = child.stderr.take().context(format!("Couldn't take stderr of child {dir}/build"))?;

            let mut log_out = log_file.try_clone()?;
            let thread_out = thread::spawn(move || {
                util::tee(child_out, &mut log_out, io::stdout()).expect(&format!("Couldn't tee output of build"));
            });

            let mut log_err = log_file.try_clone()?;
            let thread_err = thread::spawn(move || {
                util::tee(child_err, &mut log_err, io::stdout()).expect(&format!("Couldn't tee output of build"));
            });

            thread_out.join().unwrap();
            thread_err.join().unwrap();
            child.wait().context(format!("Couldn't wait on child process {dir}/build"))?
        };

        if verbose { eprintln!(); }

        if build_status.success() {
            info_fmt!("\x1b[36m{}\x1b[0m Successfully built package", name);
        } else {
            bail!("Couldn't build package {name}");
        }

        info_fmt!("\x1b[36m{}\x1b[0m Generating manifest", name);

        let manifest_dir = format!("{dest_dir}/var/cache/arc/installed");
        let manifest = format!("{manifest_dir}/{name}@{version}");

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

        eprintln!();
    }

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

pub fn download_one(
    urls: &Value,
    name: &String,
    repo_dir: &String,
    force: bool,
    pad: usize
) -> Result<Vec<String>> {
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
                info_ident_fmt!("\x1b[36m{: <pad$}\x1b[0m {} already downloaded, skipping", name, url);
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
                        eprintln!();
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
                fs::copy(format!("{repo_dir}/{url}"), filename)
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
            "\x1b[36m{: <pad$}\x1b[0m {} / {} ({})",
            pack,
            &sum.as_str().unwrap()[..10],
            &hash.to_string()[..10],
            Path::new(file).file_name().unwrap().to_str().unwrap(),
        );

        if hash.to_string() != sum.to_string().replace("\"", "") {
            bail!("Checksum mismatch for file {file}");
        }
    }

    Ok(())
}
