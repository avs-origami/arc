use std::env;
use std::error::Error;

mod args;
mod actions;
mod log;

use args::Op;

type Res<T> = Result<T, Box<dyn Error>>;

fn main() -> Res<()> {
    let cli_args: Vec<String> = env::args().collect();
    match args::parse(&cli_args) {
        Op::Build(x) => actions::build(x)?,
        Op::Die(x) => actions::print_help(x),
        Op::Download(x) => actions::download(x)?,
        Op::New(x) => actions::new(x)?,
    }

    Ok(())
}
