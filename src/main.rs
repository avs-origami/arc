use std::env;
use std::fs;

use anyhow::Context;

use arc::log;
use arc::args::{self, Op};

fn main() {
    match fs::create_dir_all((*arc::CACHE).clone())
        .context("Failed to create cache dir $HOME/.cache/arc")
    {
        Ok(_) => (),
        Err(e) => {
            log::die(&format!("{:#}", &e));
        }
    }

    let mut cli_args: Vec<String> = env::args().collect();
    let parsed = args::parse(&mut cli_args);

    if parsed.sync {
        log::warn("sync is not implemented yet");
    }

    let status = match parsed.kind {
        Op::Build(x) => arc::build(&x, parsed.verbose),
        Op::Checksum => arc::generate_checksums(),
        Op::Die(x) => arc::print_help(x),
        Op::Download(x) => arc::download(&x),
        Op::Install(x) => arc::install(&x),
        Op::New(x) => arc::new(x),
        Op::Purge => arc::purge_cache(),
        Op::Remove(x) => arc::remove(&x),
        Op::Version => arc::version(),
    };

    match status {
        Ok(_) => (),
        Err(e) => {
            log::die(&format!("{:#}", &e));
        }
    }
}
