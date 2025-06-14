//! This module contains logic that is used by functions in lib.rs but cannot
//! be directly called by the user.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use glob::glob;
use http_req::request;
use indicatif::{ProgressBar, ProgressStyle};
use nix::unistd::Uid;
use serde::Deserialize;

use crate::{info_fmt, info_ident_fmt, ARC_PATH, CACHE, CFG};
use crate::args;
use crate::bars;
use crate::log;
use crate::util;

#[derive(Clone, Debug, Deserialize)]
pub struct Package {
    pub meta: PackMeta,
    pub deps: HashMap<String, String>,
    pub mkdeps: HashMap<String, String>,
    pub provides: Option<HashMap<String, String>>,
    #[serde(skip)]
    pub name: String,
    #[serde(skip)]
    pub depth: usize,
    #[serde(skip)]
    pub dir: String,
    #[serde(skip)]
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PackMeta {
    pub version: String,
    pub maintainer: String,
    pub sources: Vec<String>,
    pub checksums: Vec<String>,
    pub strip: Option<bool>,
}

/// Check if a specific version of a package is installed.
pub fn is_installed(pack: &String, version: &String) -> Result<bool> {
    let mut path = glob(&format!("/var/cache/arc/installed/{pack}@{version}"))?;
    Ok(path.next().is_some())
}

/// Check if a file is tracked by any installed packages.
pub fn is_tracked(file: &String) -> Result<Option<String>> {
    for f in fs::read_dir("/var/cache/arc/installed/")? {
        let uf = f?;
        let content = fs::read_to_string(&uf.path())?;
        if content.contains(&format!("{file}\n")) {
            return Ok(Some(uf.file_name().to_str().unwrap().to_string()));
        }
    }

    Ok(None)
}

/// Given a vector of package names, parse and return the toml data and the
/// path for each, checking both absolute paths and $ARC_PATH for packages.
pub fn parse_package(packs: &Vec<String>) -> Result<Vec<Package>> {
    let mut res = vec![];
    for pack in packs {
        if fs::metadata(format!("{pack}/package.toml")).is_ok() {
            // An absolute or relative path to the package was provided.
            let content = fs::read_to_string(format!("{pack}/package.toml"))
                .context(format!("Failed to read {pack}/package.toml"))?;

            let mut pack_struct: Package = toml::from_str(&content).context(format!("{pack}/package.toml"))?;
            pack_struct.name = pack.clone();
            pack_struct.dir = pack.clone();
            res.push(pack_struct);
        } else {
            // Just the package name was provided, so we search $ARC_PATH.
            let mut broke = false;
            for dir in &(*ARC_PATH) {
                if fs::metadata(format!("{dir}/{pack}/package.toml")).is_ok() {
                    // The package has been found, read data and end search.
                    let content = fs::read_to_string(format!("{dir}/{pack}/package.toml"))
                        .context(format!("Failed to read {dir}/{pack}/package.toml"))?;

                    let mut pack_struct: Package = toml::from_str(&content).context(format!("{pack}/package.toml"))?;
                    pack_struct.name = pack.clone();
                    pack_struct.dir = format!("{dir}/{pack}");
                    res.push(pack_struct);

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

    Ok(res)
}

/// Output a pretty summary of packages that will be affected by an action.
pub fn summary(packs: &Vec<String>, args: &args::Cmd, header: &str) -> Result<(
    Vec<Package>, Vec<Package>, Vec<String>, Vec<Package>, Vec<String>, usize,
)> {
    // Parse all explicit packages, getting package TOML and the path for each.
    let pack_toml = parse_package(&packs)?;

    // Get the length of the longest package name.
    let pad = packs.iter().fold(&packs[0], |acc, item| {
        if item.len() > acc.len() { &item } else { acc }
    }).len();

    // Resolve all dependencies, getting package.toml and the path for each.
    let (dep_toml, mkdep_toml) = resolve_deps(&pack_toml, 1, &mut HashSet::new())?;
    let dep_names: Vec<String> = dep_toml.iter().map(|x| x.name.clone()).collect();
    let mkdep_names: Vec<String> = mkdep_toml.iter().map(|x| x.name.clone()).collect();

    // Get the length of the longest dependency name.
    let pad_dep = if dep_names.len() > 0 {
        dep_names.iter().fold(&dep_names[0], |acc, item| {
            if item.len() > acc.len() { &item } else { acc }
        }).len()
    } else {
        0
    };

    let pad_mkdep = if mkdep_names.len() > 0 {
        mkdep_names.iter().fold(&mkdep_names[0], |acc, item| {
            if item.len() > acc.len() { &item } else { acc }
        }).len()
    } else {
        0
    };

    let pad_dep = if pad_dep >= pad_mkdep { pad_dep } else { pad_mkdep };

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

    let version_pad_mkdep = if mkdep_names.len() > 1 {
        mkdep_toml.iter().fold(
            &mkdep_toml[0],
            |acc, item| {
                let version_acc = &acc.meta.version;
                let version_item = &item.meta.version;

                if version_item.len() > version_acc.len() { &item } else { acc }
            }
        ).meta.version.len()
    } else if mkdep_names.len() == 1 {
        mkdep_toml[0].meta.version.len()
    } else {
        0
    };

    let version_pad_dep = if version_pad_dep >= version_pad_mkdep {
        version_pad_dep
    } else {
        version_pad_mkdep
    };

    let version_pad = if version_pad >= version_pad_dep { version_pad } else { version_pad_dep };
    let real_pad = pad;

    // Still calculating padding: compare the previous name and version lengths
    // to the lengths of the name and version headings, and pick the longest
    // one. This lets us display package names and versions in a neat table.
    let name_header = format!("Package ({})", packs.len() + dep_names.len() + mkdep_names.len());
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
        if is_installed(&toml.name, &toml.meta.version)? && header != "Removing" {
            log::warn(&format!("Package {} is up to date - reinstalling", &toml.name));
        }
    }

    // Output the table of package names and versions, with a confirmation prompt.
    info_fmt!("{} packages:\n", header);
    println!("   {: <pad$} {: <version_pad$}", name_header, version_header);
    eprintln!();

    for toml in &pack_toml {
        if dep_names.contains(&toml.name) { continue; }
        println!("   {: <pad$} {: <version_pad$} (explicit)", toml.name, toml.meta.version);
    }

    if header == "Removing" {
        println!("\n   These packages were dependencies and may no longer be needed:");
    }

    for toml in &dep_toml {
        println!("   {: <pad$} {: <version_pad$} (layer {})", toml.name, toml.meta.version, toml.depth);
    }

    for toml in &mkdep_toml {
        println!("   {: <pad$} {: <version_pad$} (make layer {})", toml.name, toml.meta.version, toml.depth);
    }

    eprintln!();

    if !args.yes { log::prompt(); }

    Ok((pack_toml, dep_toml, dep_names, mkdep_toml, mkdep_names, real_pad))
}

/// Download sources for each package in a vector, optionally using pre-parsed
/// TOML data.
pub fn download_all(
    packs: &Vec<String>,
    pack_toml: Option<Vec<Package>>,
    force: bool,
    pad: Option<usize>
) -> Result<Vec<Package>> {
    let longest = match pad {
        Some(x) => x,
        None => packs.iter().fold(&packs[0], |acc, item| {
            if item.len() > acc.len() { &item } else { acc }
        }).len(),
    };

    if let Some(mut n) = pack_toml {
        // Packages have been parsed somewhere else and provided here. Just
        // read sources for each package and download.
        for pack in n.iter_mut() {
            let sources = download_one(&pack.meta.sources, &pack.name, &pack.dir, force, longest)?;
            pack.sources = sources;
        }

        return Ok(n);
    } else {
        // Packages have not already been parsed, so parse packages then
        // download sources for each package.
        let mut pack_toml = parse_package(packs)?;
        for pack in pack_toml.iter_mut() {
            let sources = download_one(&pack.meta.sources, &pack.name, &pack.dir, force, longest)?;
            pack.sources = sources;
        }

        return Ok(pack_toml);
    }
}

/// Given the parsed TOML data for some packages, recursively identify all
/// dependencies. Returns a list of dependencies, with duplicates and installed
/// packages removed, sorted by install layer (highest layer to lowest layer).
pub fn resolve_deps(
    pack_toml: &Vec<Package>,
    depth: usize,
    visiting: &mut HashSet<String>,
) -> Result<(Vec<Package>, Vec<Package>)> {
    let mut raw_deps = vec![];
    let mut raw_mkdeps = vec![];
    for toml in pack_toml {
        let root = &toml.name;
        for (name, ver_req) in &toml.deps {
            // Check for circular dependencies: if the current dependency branch
            // contains this package, bail.
            if visiting.contains(name) {
                bail!(
                    "Circular dependency detected: package {root} depends on itself.\
                     \n   Current branch: {visiting:?}"
                );
            }

            // If a satisfactory version of this dependency is installed,
            // skip to the next one.
            if is_installed(name, ver_req)? { continue; }

            // Parse this dependency and fill out the 'name' and 'depth' fields.
            let mut dep_toml = parse_package(&vec![name.to_string()])?;
            dep_toml[0].name = name.clone();
            dep_toml[0].depth = depth;

            // Get dependencies and make dependencies of this dependency, and
            // add everything to the list of dependencies.
            visiting.insert(name.clone());
            let mut deps = resolve_deps(&dep_toml, depth + 1, visiting)?;
            visiting.remove(name);
            raw_deps.push(dep_toml.remove(0));
            raw_deps.append(&mut deps.0);
            raw_mkdeps.append(&mut deps.1);
        }

        for (name, ver_req) in &toml.mkdeps {
            // Check for circular dependencies: if the current dependency branch
            // contains this package, bail.
            if visiting.contains(name) {
                bail!(
                    "Circular dependency detected: package {root} depends on itself.\
                     \n   Current branch: {visiting:?}"
                );
            }

            // If a satisfactory version of this make dependency is installed,
            // skip to the next one.
            if is_installed(name, ver_req)? { continue; }

            // Parse this make dependency and fill out the 'name' and 'depth'
            // fields.
            let mut mkdep_toml = parse_package(&vec![name.to_string()])?;
            mkdep_toml[0].name = name.clone();
            mkdep_toml[0].depth = depth;

            // Get dependencies and make dependencies of this dependency, and
            // add everything to the list of dependencies.
            visiting.insert(name.clone());
            let mut deps = resolve_deps(&mkdep_toml, depth + 1, visiting)?;
            visiting.remove(name);
            raw_mkdeps.push(mkdep_toml.remove(0));
            raw_mkdeps.append(&mut deps.0);
            raw_mkdeps.append(&mut deps.1);
        }
    }

    // Remove duplicate dependencies.
    let proc_deps = consolidate_deps(&raw_deps);
    let mut out_mkdeps = consolidate_deps(&raw_mkdeps);
    
    // Remove packages from deps if they are in mkdeps.
    let mut out_deps = vec![];
    for dep in &proc_deps {
        let mut duplicate = false;
        for mkdep in &out_mkdeps {
            if mkdep.name == dep.name {
                duplicate = true;
                break;
            }
        }

        if !duplicate {
            out_deps.push(dep.clone());
        }
    }

    // Sort the remaining dependencies by install layer.
    out_deps.sort_by(|a, b| a.depth.cmp(&b.depth).reverse());
    out_mkdeps.sort_by(|a, b| a.depth.cmp(&b.depth).reverse());
    Ok((out_deps, out_mkdeps))
}

/// Remove duplicate packages from a vector, keeping the copy with the highest
/// depth.
pub fn consolidate_deps(input: &Vec<Package>) -> Vec<Package> {
    let mut out_deps: Vec<Package> = vec![];
    'o: for i in input {
        // If this is a duplicate, don't add it to the return vector.
        for j in &out_deps {
            if j.name == i.name {
                continue 'o;
            }
        }

        // Find the highest depth of all copies of this dependency, and add
        // the corresponding copy to the result vector.
        let mut pack = i.clone();
        for j in input {
            if j.name == pack.name && j.depth > pack.depth {
                pack = j.clone();
            }
        }

        out_deps.push(pack);
    }

    return out_deps;
}

/// Verify checksums for some packages given their parsed TOML data.
pub fn checksums_all(
    pack_toml: &Vec<Package>,
    pad: usize
) -> Result<()> {
    for toml in pack_toml {
        // Read the checksums from package.toml and verify against sources.
        verify_checksums(&toml.sources, &toml.meta.checksums, &toml.name, pad)?;
    }

    Ok(())
}

/// Build packages given their parsed TOML data. The following steps are
/// performed for each package:
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
    pack_toml: &Vec<Package>,
    args: &crate::args::Cmd,
) -> Result<()> {
    for (i, toml) in pack_toml.iter().enumerate() {
        let name = &toml.name;
        let version = &toml.meta.version;
        let dir = &toml.dir;
        info_fmt!("\x1b[36m{}\x1b[0m Building package ({}/{})", name, i + 1, pack_toml.len());

        // Create cache directories for src and destdir.
        let build_dir = format!("{}/build/{name}", *CACHE);
        let src_dir = format!("{build_dir}/src");
        let dest_dir = format!("{build_dir}/dest");
        fs::create_dir_all(&src_dir).context(format!("Couldn't create directory {src_dir}"))?;
        fs::create_dir_all(&dest_dir).context(format!("Couldn't create directory {dest_dir}"))?;

        info_fmt!("\x1b[36m{}\x1b[0m Extracting sources", name);

        for file in &toml.sources {
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
        if args.verbose { eprintln!(); }

        // Resolve the absolute path to the build script.
        let build_script = fs::canonicalize(format!("{dir}/build"))
            .context(format!("Couldn't canonicalize path {dir}/build"))?;

        // Create log.txt to store the build log.
        let log_file = File::create(format!("{dest_dir}/../log.txt"))?;
        let mut build_cmd = Command::new(build_script);
        build_cmd.arg(&dest_dir).arg(&version).current_dir(src_dir);

        let build_status = if !(args.verbose || CFG.verbose_builds) {
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

        if args.verbose || CFG.verbose_builds { eprintln!(); }

        if build_status.success() {
            info_fmt!("\x1b[36m{}\x1b[0m Successfully built package", name);
        } else {
            bail!("Couldn't build package {name}");
        }
        
        // Strip unneeded symbols from binaries to reduce the package size.
        if toml.meta.strip.unwrap_or(CFG.strip) {
            info_fmt!("\x1b[36m{}\x1b[0m Stripping binaries", name);
            for file in glob(&format!("{dest_dir}/**/*"))? {
                let path = format!("{}", file?.display());
                let _ = Command::new("strip")
                    .args(["--strip-unneeded", &path])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        } else {
            info_fmt!("\x1b[36m{}\x1b[0m Not stripping (explicitly disabled)", name);
        }
 
        // Create the package manifest at
        // destdir/var/cache/arc/installed/<name>@<version>.
        info_fmt!("\x1b[36m{}\x1b[0m Generating manifest", name);
        let manifest_dir = format!("{dest_dir}/var/cache/arc/installed");
        let manifest = format!("{manifest_dir}/{name}@{version}");

        fs::create_dir_all(&manifest_dir)
            .context(format!("Couldn't create directory {manifest_dir}"))?;

        let mut manifest_file = File::create(&manifest)
            .context(format!("Couldn't create file {manifest}"))?;

        // Create dummy manifests for any packages provided by this one.
        if let Some(x) = &toml.provides {
            for (nam, ver) in x {
                let man = format!("{manifest_dir}/{nam}@{ver}");
                let mut dum_man = File::create(&man)
                    .context(format!("Couldn't create file {man}"))?;

                dum_man.write_all(format!("-> {name}@{version}\n").as_bytes())
                    .context(format!("Couldn't write to file {man}"))?;
            }
        }

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

        info_fmt!("\x1b[36m{}\x1b[0m Cleaning up", name);
        fs::remove_dir_all(&build_dir).context(format!("Couldn't remove build directory {build_dir}"))?;

        eprintln!();
    }

    Ok(())
}

/// Install some packages given their parsed TOML data. This does the the
/// following:
/// 1. If not running as root, use sudo, doas, or su to become the root user.
/// 2. Extract the manifest 
/// 3. Extract the binary tarball to /.
pub fn install_all(pack_toml: &Vec<Package>) -> Result<()> {
    for toml in pack_toml {
        let name = &toml.name;
        let version = &toml.meta.version;
        let bin_file = format!("{}/bin/{name}@{version}.tar.gz", *CACHE);
        let manifest = format!("./var/cache/arc/installed/{name}@{version}");
        let tmp_dir = format!("{}/tmp/{name}", *CACHE);

        fs::create_dir_all(&tmp_dir).context(format!("Couldn't create temp dir {tmp_dir}"))?;

        Command::new("tar")
            .args(["xf", &bin_file, "-C", &tmp_dir])
            .status()
            .context(format!("Couldn't extract binary tarball to temp dir"))?;

        log::info("Checking for conflicts");
        let manifest_content = fs::read_to_string(format!("{tmp_dir}/{manifest}")).context(format!("Couldn't read manifest at {tmp_dir}/{manifest}"))?;
        for line in manifest_content.lines() {
            if let Some(n) = is_tracked(&line.into())? {
                let other_name = n.split('@').collect::<Vec<&str>>()[0];
                if let Ok(fsmeta) = fs::metadata(line) {
                    if fsmeta.is_file() && other_name != name {
                        if log::prompt_yn(&format!("WARNING: File {line} is already tracked by package {other_name}; overwrite it?"), 33)? {
                            // If the user chooses to use the file from this package, remove the entry
                            // for that file from the other package's manifest.
                            let mut other_manifest = fs::read_to_string(&format!("/var/cache/arc/installed/{n}"))
                                .context(format!("Couldn't read /var/cache/arc/installed/{n}"))?;

                            other_manifest = other_manifest.replace(&(line.to_owned() + "\n"), "");
                    
                            let mut other = File::create(format!("{tmp_dir}/var/cache/arc/installed/{n}"))
                                .context(format!("Couldn't create file {tmp_dir}/var/cache/arc/installed/{n}"))?;

                            other.write_all(other_manifest.as_bytes()).context("Couldn't write new manifest")?;
                        } else {
                            // If the user doesn't want to replace the file, remove the file from the
                            // temp dir and the packge's manifest.
                            fs::remove_file(format!("{tmp_dir}/{line}")).context(format!("Couldn't remove file {tmp_dir}/{line}"))?;
                        
                            let new_content = fs::read_to_string(format!("{tmp_dir}/{manifest}")).context(format!("Couldn't read manifest at {tmp_dir}/{manifest}"))?;
                            let new_manifest = new_content.replace(&(line.to_owned() + "\n"), "");
                
                            let mut this_manifest = File::create(format!("{tmp_dir}/{manifest}"))
                                .context(format!("Couldn't create file {tmp_dir}/var/cache/arc/installed/{n}"))?;

                            this_manifest.write_all(new_manifest.as_bytes()).context("Couldn't write new manifest")?;
                        }
                    }
                }
            }
        }
    }


    let su_command = if let Some(x) = &CFG.su_cmd {
        x.as_str()
    } else if fs::metadata("/bin/sudo").is_ok() {
        "sudo"
    } else if fs::metadata("/bin/doas").is_ok() {
        "doas"
    } else if fs::metadata("/bin/ssu").is_ok() {
        "ssu"
    } else {
        ""
    };

    if ! Uid::effective().is_root() {
        info_fmt!("Using {} to become root", su_command);
    }

    for (i, toml) in pack_toml.iter().enumerate() {
        let name = &toml.name;
        let version = &toml.meta.version;
        let tmp_dir = format!("{}/tmp/{name}", *CACHE);

        let install_dirs = format!("find {tmp_dir}/. -type d -exec sh -c 'mkdir -p \"/${{0#{tmp_dir}}}\"' {{}} \\;");
        let install_files = format!("find {tmp_dir}/. ! -type d -exec sh -c 'cp -d \"$0\" \"/${{0#{tmp_dir}}}\"' {{}} \\;");

        if Uid::effective().is_root() {
            Command::new("sh")
                .args(["-c", &install_dirs])
                .status()
                .context(format!("Couldn't install {name} to /"))?;

            Command::new("sh")
                .args(["-c", &install_files])
                .status()
                .context(format!("Couldn't install {name} to /"))?;

            // Remove the temp dir.
            fs::remove_dir_all(&tmp_dir).context(format!("Couldn't remove temp dir {tmp_dir}"))?;
        } else {
            match su_command {
                "sudo" => {
                    Command::new("sudo")
                        .args(["chown", "-R", "root:root", &tmp_dir])
                        .status()
                        .context(format!("Couldn't change ownership of package files"))?;

                    Command::new("sudo")
                        .args(["sh", "-c", &install_dirs])
                        .status()
                        .context(format!("Couldn't install {name} to /"))?;

                    Command::new("sudo")
                        .args(["sh", "-c", &install_files])
                        .status()
                    .context(format!("Couldn't install {name} to /"))?;
                },
                "doas" => {
                    Command::new("doas")
                        .args(["chown", "-R", "root:root", &tmp_dir])
                        .status()
                        .context(format!("Couldn't change ownership of package files"))?;

                    Command::new("doas")
                        .args(["sh", "-c", &install_dirs])
                        .status()
                        .context(format!("Couldn't install {name} to /"))?;

                    Command::new("doas")
                        .args(["sh", "-c", &install_files])
                        .status()
                    .context(format!("Couldn't install {name} to /"))?;
                },
                "ssu" => {
                    Command::new("ssu")
                        .args(["--", "chown", "-R", "root:root", &tmp_dir])
                        .status()
                        .context(format!("Couldn't change ownership of package files"))?;

                    Command::new("ssu")
                        .args(["--", "sh", "-c", &install_dirs])
                        .status()
                        .context(format!("Couldn't install {name} to /"))?;

                    Command::new("ssu")
                        .args(["--", "sh", "-c", &install_files])
                        .status()
                    .context(format!("Couldn't install {name} to /"))?;
                },
                _ => bail!("Couldn't find a command to elevate privileges"),
            }

            // Remove the temp dir.
            Command::new(&su_command)
                .args(["rm", "-rf", &tmp_dir])
                .status()
                .context(format!("Couldn't remove temp dir {tmp_dir}"))?;

        }

        info_fmt!("Successfully installed {} @ {} ({}/{})", name, version, i + 1, pack_toml.len());
    }
 
    Ok(())
}

/// Download the sources for a single package.
pub fn download_one(
    urls: &Vec<String>,
    name: &String,
    repo_dir: &String,
    force: bool,
    pad: usize
) -> Result<Vec<String>> {
    let mut fnames = vec![];
    // Create a cache directory for downloaded sources.
    let dir = format!("{}/dl", *CACHE);
    fs::create_dir_all(&dir).context(format!("Couldn't create directory {dir}"))?;

    for (i, url) in urls.iter().enumerate() {
        let og_url = url.clone();
        let mut url = url.clone();

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
            // Create a pretty download progress bar.
            let bar = "[{elapsed_precise}] [{bar:30.magenta/magenta}] ({bytes_per_sec}, ETA {eta})";
            let bar_spin = "[{elapsed_precise}] [{spinner:.magenta}] ({bytes_per_sec}, ETA {eta})";
            let bar_fmt = format!("  \x1b[35m->\x1b[0m \x1b[36m{name: <pad$}\x1b[0m {bar} ({}/{}) ({og_url})", i + 1, urls.len());
            let bar_spin_fmt = format!("  \x1b[35m->\x1b[0m \x1b[36m{name: <pad$}\x1b[0m {bar_spin} ({}/{}) ({og_url})", i + 1, urls.len());

            let bar = ProgressBar::new(1);
            let bar_style = ProgressStyle::with_template(&bar_fmt).unwrap().progress_chars("-> ");
            bar.set_style(ProgressStyle::with_template(&bar_spin_fmt).unwrap().tick_strings(&bars::LSPIN));
            bar.enable_steady_tick(Duration::from_millis(30));
            
            loop {
                let mut body = vec![];
                // Get the size of the file to be downloaded, if available.
                let head = request::head(&url)?;
                let len = head.content_len().unwrap_or(0);

                // Try to download the file.
                let res = request::get_with_update(&url, &mut body, |x| util::inc_bar(&bar, x as u64, len, &bar_style))
                    .context(format!("Couldn't connect to {url}"))?;

                if res.status_code().is_success() {
                    // The file was downloaded successfully, save it and
                    // move on to the next file.
                    bar.finish();
                    eprintln!();
                    let mut out = File::create(&filename).context(format!("Couldn't create file {filename}"))?;
                    out.write_all(&body).context(format!("Couldn't save downloaded file to {filename}"))?;
                    break;
                } else if res.status_code().is_redirect() {
                    // The request returned a redirect, get the actual
                    // file location and update the url.
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

    // Return the paths to each downloaded file.
    Ok(fnames)
}

/// Verify the checksums for a set of files.
pub fn verify_checksums(
    fnames: &Vec<String>,
    checksums: &Vec<String>,
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
            &sum[..10],
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
