use std::env;
use std::fs;

use anyhow::Context;

use arc::log;
use arc::args::{self, Op};

fn main() {
    // Create the cache directory, if it doesn't exist. This is where source
    // files, builds, and logs are stored.
    match fs::create_dir_all((*arc::CACHE).clone())
        .context("Failed to create cache dir $HOME/.cache/arc")
    {
        Ok(_) => (),
        Err(e) => {
            log::die(&format!("{:#}", &e));
        }
    }

    // Create the package installation cache, if it doesn't exist. This
    // directory is where all package files are tracked by the package manager.
    match fs::create_dir_all("/var/cache/arc/installed")
        .context("Failed to create install cache /var/cache/arc/installed")
    {
        Ok(_) => (),
        Err(e) => {
            log::die(&format!("{:#}", &e));
        }
    }

    // Collect and parse CLI arguments.
    let mut cli_args: Vec<String> = env::args().collect();
    let parsed = args::parse(&mut cli_args);

    if parsed.sync {
        match arc::sync() {
            Ok(_) => (),
            Err(e) => {
                log::die(&format!("{:#}", &e));
            }
        }
    }

    // Match the given command, and execute the appropriate action, storing
    // the result. All commands return a Result<()> which allows for nice
    // error handling.
    let status = match parsed.kind {
        Op::Build(ref x) => arc::build(x, &parsed),
        Op::Checksum => arc::generate_checksums(),
        Op::Die(x, msg) => arc::print_help(x, msg),
        Op::Download(ref x) => arc::download(x),
        Op::Find(x) => arc::search(x),
        Op::Install(ref x) => arc::install(x, &parsed),
        Op::List => arc::list(),
        Op::New(x) => arc::new(x),
        Op::Purge => arc::purge_cache(),
        Op::Remove(ref x) => arc::remove(x, &parsed),
        Op::Upgrade => arc::upgrade(&parsed),
        Op::Version => arc::version(),
    };

    // Report any errors with nice formatting.
    match status {
        Ok(_) => (),
        Err(e) => {
            log::die(&format!("{:#}", &e));
        }
    }
}
