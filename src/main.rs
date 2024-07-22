use std::env;
use std::error::Error;

mod args;
mod actions;
mod log;

use args::Op;

type Res<T> = Result<T, Box<dyn Error>>;

fn main() {
    let cli_args: Vec<String> = env::args().collect();
    let status = match args::parse(&cli_args) {
        Op::Build(x) => actions::build(&x),
        Op::Checksum => actions::generate_checksums(),
        Op::Die(x) => Ok(actions::print_help(x)),
        Op::Download(x) => {
            let res = actions::download(&x);
            match res {
                Ok(_) => Ok(()),
                Err(e) => Err(e),
            }
        },
        Op::New(x) => actions::new(x),
    };

    match status {
        Ok(_) => (),
        Err(e) => {
            log::die(&e.to_string());
        }
    }
}
