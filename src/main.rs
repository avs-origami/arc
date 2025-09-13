use std::env;
use std::fs;

use anyhow::Context;

use moss::log;
use moss::args::{self, Op};

fn main() {
    // Create the cache directory, if it doesn't exist. This is where source
    // files, builds, and logs are stored.
    match fs::create_dir_all((*moss::CACHE).clone())
        .context("Failed to create cache dir $HOME/.cache/moss")
    {
        Ok(_) => (),
        Err(e) => {
            log::die(&format!("{:#}", &e));
        }
    }

    // Create the package installation cache, if it doesn't exist. This
    // directory is where all package files are tracked by the package manager.
    match fs::create_dir_all("/var/cache/moss/installed")
        .context("Failed to create install cache /var/cache/moss/installed")
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
        match moss::sync() {
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
        Op::Build(ref x) => moss::build(x, &parsed),
        Op::Checksum => moss::generate_checksums(),
        Op::Die(x, msg) => moss::print_help(x, msg),
        Op::Download(ref x) => moss::download(x),
        Op::Find(x) => moss::search(x),
        Op::Install(ref x) => moss::install(x, &parsed),
        Op::List => moss::list(),
        Op::New(x) => moss::new(x),
        Op::Purge => moss::purge_cache(),
        Op::Remove(ref x) => moss::remove(x, &parsed),
        Op::Upgrade => moss::upgrade(&parsed),
        Op::Version => moss::version(),
    };

    // Report any errors with nice formatting.
    match status {
        Ok(_) => (),
        Err(e) => {
            log::die(&format!("{:#}", &e));
        }
    }
}
