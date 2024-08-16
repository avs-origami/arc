use std::env;
use std::fs;

use anyhow::Context;

mod args;
mod actions;
mod log;
mod util;

use args::Op;

fn main() {
    match fs::create_dir_all((*actions::CACHE).clone())
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
        Op::Build(x) => actions::build(&x, parsed.verbose),
        Op::Checksum => actions::generate_checksums(),
        Op::Die(x) => actions::print_help(x),
        Op::Download(x) => actions::action_download(&x),
        Op::Install(x) => actions::install(&x),
        Op::New(x) => actions::new(x),
        Op::Purge => actions::purge(),
        Op::Remove(x) => actions::remove(&x),
        Op::Version => actions::version(),
    };

    match status {
        Ok(_) => (),
        Err(e) => {
            log::die(&format!("{:#}", &e));
        }
    }
}
