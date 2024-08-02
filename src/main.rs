use std::env;
use std::fs;

use anyhow::Context;

mod args;
mod actions;
mod log;

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

    let cli_args: Vec<String> = env::args().collect();
    let status = match args::parse(&cli_args) {
        Op::Build(x) => actions::build(&x),
        Op::Checksum => actions::generate_checksums(),
        Op::Die(x) => actions::print_help(x),
        Op::Download(x) => {
            match actions::download(&x, None, true) {
                Ok(_) => Ok(()),
                Err(e) => Err(e),
            }
        },
        Op::Install(x) => actions::install(&x),
        Op::New(x) => actions::new(x),
        Op::Purge => actions::purge(),
        Op::Version => actions::version(),
    };

    match status {
        Ok(_) => (),
        Err(e) => {
            log::die(&format!("{:#}", &e));
        }
    }
}
