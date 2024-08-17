//! This module contains logic that is used by functions in lib.rs but cannot
//! be directly called by the user.

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

/// Check if a specific version of a package is installed.
pub fn is_installed(pack: &String, version: &str) -> Result<bool> {
    let mut path = glob(&format!("/var/cache/arc/installed/{pack}@{version}"))?;
    Ok(path.next().is_some())
}

/// Given a vector of package names, parse and return the toml data and the
/// path for each, checking both absolute paths and $ARC_PATH for packages.
pub fn parse_package(packs: &Vec<String>) -> Result<(Vec<Table>, Vec<String>)> {
    let mut package_files = vec![];
    let mut package_dirs = vec![];

    for pack in packs {
        if fs::metadata(format!("{pack}/package.toml")).is_ok() {
            // An absolute or relative path to the package was provided.
            let content = fs::read_to_string(format!("{pack}/package.toml"))
                .context(format!("Failed to read {pack}/package.toml"))?;

            package_files.push(content);
            package_dirs.push(format!("{pack}"))
        } else {
            // Just the package name was provided, so we search $ARC_PATH.
            let mut broke = false;
            for dir in &(*ARC_PATH) {
                if fs::metadata(format!("{dir}/{pack}/package.toml")).is_ok() {
                    // The package has been found, read data and end search.
                    let content = fs::read_to_string(format!("{dir}/{pack}/package.toml"))
                        .context(format!("Failed to read {dir}/{pack}/package.toml"))?;

                    package_files.push(content);
                    package_dirs.push(format!("{dir}/{pack}"));

                    broke = true;
                    break;
                }
            }

            // If search did not finish, quit with error.
            if ! broke {
                bail!("Couldn't resolve package {pack}");
            }
        }
    }

    // Read the content of each toml file and parse it.
    let packs: Vec<Table> = package_files.iter().zip(packs)
        .map(|(x, y)| x.parse().context(format!("{y}/package.toml")))
        .collect::<Result<_, _>>()?;

    Ok((packs, package_dirs))
}

/// Download sources for each package in a vector.
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
        // Packages have been parsed somewhere else and provided here. Just
        // read sources for each package and download.
        n.iter().zip(packs).zip(pack_dirs.unwrap())
            .map(|((x, y), z)| download_one(&x["meta"]["sources"], y, z, force, longest))
            .collect::<Result<_>>()?
    } else {
        // Packages have not already been parsed, so parse packages then
        // download sources for each package.
        let (pack_toml, pack_dirs) = parse_package(packs)?;
        pack_toml.iter().zip(packs).zip(&pack_dirs)
            .map(|((x, y), z)| download_one(&x["meta"]["sources"], y, z, force, longest))
            .collect::<Result<_>>()?
    };

    // Return the paths to the downloaded files.
    Ok(files)
}

/// Given some packages and their parsed toml, recursively identify all
/// dependencies. Returns a list of dependencies, with duplicates and installed
/// packages removed, sorted by install layer (highest layer to lowest layer).
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

    // Remove packages that are duplicates or are already installed, and leave
    // the copy of each package with the highest install layer.
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

    // Sort the remaining dependencies by install layer.
    output.sort_by(|a, b| a.depth.cmp(&b.depth).reverse());
    Ok(output)
}

/// Verify checksums for some packages given their names, parsed toml, and
/// paths to source files.
pub fn checksums_all(
    packs: &Vec<String>,
    pack_toml: &Vec<Map<String, Value>>,
    pack_dirs: &Vec<String>,
    filenames: &Vec<Vec<String>>,
    pad: usize
) -> Result<()> {
    for ((pack, toml), name) in filenames.iter().zip(pack_toml).zip(packs) {
        // Read the checksums from package.toml and verify against sources.
        if let Value::Array(x) = &toml["meta"]["checksums"] {
            verify_checksums(pack, &x, name, pad)?;
        } else {
            bail!("Problem parsing {name}/package.toml: checksums is not an array");
        }
    }

    Ok(())
}

/// Build packages given their names, parsed toml, paths to the packages, and
/// paths to source files. The following steps are performed for each package:
/// 1. Create cache directories for the package source and the destdir.
/// 2. Extract archives (.tar.*) to the src directory, and copy all other files.
/// 3. Execute the build script inside the src directory, passing the destdir
///    and the package version as $1 and $2, respectively, and:
///      - If the 'v' flag was provided, tee the output to stdout and log.txt.
///      - Otherwise, pipe the output to log.txt.
/// 4. Generate a package manifest using a glob of the destdir, and write it to
///    destdir/var/cache/arc/installed/<name>@<version>.
/// 5. Generate a tarball of the destdir and save it in the cache directory.
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
        // Grab the package version from its parsed toml.
        let version = toml["meta"]["version"].as_str().unwrap();
        info_fmt!("\x1b[36m{}\x1b[0m Building package ({}/{})", name, i + 1, filenames.len());

        // Create cache directories for src and destdir.
        let src_dir = format!("{}/build/{name}/src", *CACHE);
        let dest_dir = format!("{}/build/{name}/dest", *CACHE);
        fs::create_dir_all(&src_dir).context(format!("Couldn't create directory {src_dir}"))?;
        fs::create_dir_all(&dest_dir).context(format!("Couldn't create directory {dest_dir}"))?;

        info_fmt!("\x1b[36m{}\x1b[0m Extracting sources", name);

        for file in pack {
            if file.starts_with("tar+") {
                // Don't extract this tarball, just copy it as-is.
                let file = &file[4..];
                let basename = file.split('/').last().unwrap();
                fs::copy(file, format!("{src_dir}/{basename}"))
                    .context(format!("Couldn't copy {file} to build dir"))?;
            } else if file.contains(".tar") {
                // This is a tarball, extract it to srcdir.
                Command::new("tar")
                    .args(["xf", file, "-C", &src_dir, "--strip-components=1"])
                    .status()
                    .context(format!("Failed to untar {file}"))?;
            } else {
                // This is not a tarball, just copy it as-is.
                let basename = file.split('/').last().unwrap();
                fs::copy(file, format!("{src_dir}/{basename}"))
                    .context(format!("Couldn't copy {file} to build dir"))?;
            }
        }

        info_fmt!("\x1b[36m{}\x1b[0m Running build script", name);
        if verbose { eprintln!(); }

        // Resolve the absolute path to the build script.
        let build_script = fs::canonicalize(format!("{dir}/build"))
            .context(format!("Couldn't canonicalize path {dir}/build"))?;

        // Create log.txt to store the build log.
        let log_file = File::create(format!("{dest_dir}/../log.txt"))?;
        let mut build_cmd = Command::new(build_script);
        build_cmd.arg(&dest_dir).arg(&version).current_dir(src_dir);

        let build_status = if !verbose {
            // This is the default behavior if the 'v' flag wasn't given. Just
            // pipe the build output to log.txt.
            build_cmd.stdout(log_file.try_clone()?).stderr(log_file.try_clone()?);
            build_cmd.status().context(format!("Couldn't execute {dir}/build"))?
        } else {
            // If the 'v' flag was provided, tee the build output to stdout and
            // log.txt.
            build_cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = build_cmd.spawn().context(format!("Couldn't execute {dir}/build"))?;
            let child_out = child.stdout.take().context(format!("Couldn't take stdout of child {dir}/build"))?;
            let child_err = child.stderr.take().context(format!("Couldn't take stderr of child {dir}/build"))?;

            // We want to tee both stdout and stderr, so spawn a separate
            // thread to handle each one.
            let mut log_out = log_file.try_clone()?;
            let thread_out = thread::spawn(move || {
                util::tee(child_out, &mut log_out, io::stdout()).expect(&format!("Couldn't tee output of build"));
            });

            let mut log_err = log_file.try_clone()?;
            let thread_err = thread::spawn(move || {
                util::tee(child_err, &mut log_err, io::stdout()).expect(&format!("Couldn't tee output of build"));
            });

            // Wait for the build script to finish.
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

        // Create the package manifest at
        // destdir/var/cache/arc/installed/<name>@<version>.
        let manifest_dir = format!("{dest_dir}/var/cache/arc/installed");
        let manifest = format!("{manifest_dir}/{name}@{version}");

        fs::create_dir_all(&manifest_dir)
            .context(format!("Couldn't create directory {manifest_dir}"))?;

        let mut manifest_file = File::create(&manifest)
            .context(format!("Couldn't create file {manifest}"))?;

        // Use a glob to get the contents of destdir.
        let mut manifest_content = String::new();
        for file in glob(&format!("{dest_dir}/**/*"))? {
            let line = format!("{}\n", file?.display());
            manifest_content.push_str(&line.replace(&dest_dir, ""));
        }

        manifest_file.write_all(manifest_content.as_bytes())
            .context(format!("Couldn't write to file {manifest}"))?;

        info_fmt!("\x1b[36m{}\x1b[0m Creating tarball", name);

        // Create a cache directory to store built package tarballs.
        let bin_dir = format!("{}/bin", *CACHE);
        fs::create_dir_all(&bin_dir).context(format!("Couldn't create directory {bin_dir}"))?;

        // Create the tarball.
        Command::new("tar")
            .args(["czf", &format!("{}/{}@{}.tar.gz", bin_dir, name, version), "."])
            .current_dir(&dest_dir)
            .status()
            .context("Couldn't create tarball of built package")?;

        eprintln!();
    }

    Ok(())
}

/// Install some packages given their names and parsed toml. This does the the
/// following:
/// 1. If not running as root, use sudo, doas, or su to become the root user.
/// 2. Extract the binary tarball to /.
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

// Download the sources for a single package.
pub fn download_one(
    urls: &Value,
    name: &String,
    repo_dir: &String,
    force: bool,
    pad: usize
) -> Result<Vec<String>> {
    let mut fnames = vec![];
    if let Value::Array(x) = urls {
        // Create a cache directory for downloaded sources.
        let dir = format!("{}/dl", *CACHE);
        fs::create_dir_all(&dir).context(format!("Couldn't create directory {dir}"))?;

        for (i, url) in x.iter().enumerate() {
            let og_url = url.to_string().replace("\"", "");
            let mut url = url.to_string().replace("\"", "");

            let filename = url.split('/').last().unwrap().to_owned();
            let filename = format!("{dir}/{filename}");

            // Remove any prefixes from the url.
            if &url[3..4] == "+" {
                url = url[4..].to_string();
                fnames.push("tar+".to_owned() + &filename);
            } else {
                fnames.push(filename.clone());
            }

            // If a file is already downloaded and we are not forcing the
            // download, skip this file.
            if fs::metadata(filename.clone()).is_ok() &&! force {
                info_ident_fmt!("\x1b[36m{: <pad$}\x1b[0m {} already downloaded, skipping", name, url);
                continue;
            }

            if url.starts_with("https://") || url.starts_with("http://") {
                // This is a remote url, so download it from the internet.
                loop {
                    let mut body = vec![];

                    // Get the size of the file to be downloaded, if available.
                    let head = request::head(&url)?;
                    let len = head.content_len().unwrap_or(0);

                    // Create a pretty download progress bar.
                    let bar = "[{elapsed_precise}] [{bar:30.magenta/magenta}] ({bytes_per_sec}, ETA {eta})";
                    let bar_fmt = format!("  \x1b[35m->\x1b[0m \x1b[36m{name: <pad$}\x1b[0m {bar} ({}/{}) ({og_url})", i + 1, x.len());

                    let bar = ProgressBar::new(len as u64);
                    bar.set_style(ProgressStyle::with_template(&bar_fmt).unwrap().progress_chars("-> "));

                    // Try to download the file.
                    let res = request::get_with_update(&url, &mut body, |x| bar.inc(x as u64))
                        .context(format!("Couldn't connect to {url}"))?;

                    if res.status_code().is_success() {
                        // The file was downloaded successfully, save it and
                        // move on to the next file.
                        bar.finish();
                        eprintln!();
                        let mut out = File::create(&filename).context(format!("Couldn't create file {filename}"))?;
                        out.write_all(&body)
                            .context(format!("Couldn't save downloaded file to {filename}"))?;

                        break;
                    } else if res.status_code().is_redirect() {
                        // The request returned a redirect, get the actual
                        // file location and update the url.
                        bar.finish_and_clear();
                        url = res.headers().get("Location").unwrap().to_owned();
                    } else {
                        // The request returned a different failure code, bail.
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
                // This is a local file, copy it to the download cache.
                fs::copy(format!("{repo_dir}/{url}"), filename)
                    .context(format!("Could not copy local file {name}/{url} to download cache"))?;
            }
        }
    } else {
        bail!("Problem parsing {name}/package.toml: sources is not an array");
    }

    // Return the paths to each downloaded file.
    Ok(fnames)
}

// Verify the checksums for a set of files.
pub fn verify_checksums(
    fnames: &Vec<String>,
    checksums: &Vec<Value>,
    pack: &String,
    pad: usize
) -> Result<()> {
    // Make sure we aren't missing any checksums.
    if fnames.len() > checksums.len() {
        bail!("Missing one or more checksums for package {pack}");
    }

    for (file, sum) in fnames.iter().zip(checksums) {
        // Remove any prefixes from the filename.
        let file = if &file[3..4] == "+" { &file[4..] } else { &file[..] };

        // Read the file and generate its b3sum.
        let data: Vec<u8> = fs::read(file).context(format!("Couldn't read file {file}"))?;
        let hash = blake3::hash(&data);

        info_ident_fmt!(
            "\x1b[36m{: <pad$}\x1b[0m {} / {} ({})",
            pack,
            &sum.as_str().unwrap()[..10],
            &hash.to_string()[..10],
            Path::new(file).file_name().unwrap().to_str().unwrap(),
        );

        // Compare the generated hash to the provided one.
        if hash.to_string() != sum.to_string().replace("\"", "") {
            bail!("Checksum mismatch for file {file}");
        }
    }

    Ok(())
}
