//! This module contains logic to parse command line arguments.

#[derive(Debug)]
pub enum Op {
    Build(Vec<String>),
    Checksum,
    Die(i32, String),
    Download(Vec<String>),
    Find(String),
    Install(Vec<String>),
    List,
    New(String),
    Purge,
    Remove(Vec<String>),
    Upgrade,
    Version,
}

impl Default for Op {
    fn default() -> Self {
        Op::Die(0, "".into())
    }
}

#[derive(Debug, Default)]
pub struct Cmd {
    pub kind: Op,
    pub sync: bool,
    pub verbose: bool,
    pub yes: bool,
}

/// Parse command line arguments.
pub fn parse(args: &mut Vec<String>) -> Cmd {
    if args.len() > 1 {
        let mut cmd = Cmd::default();

        cmd.kind = 'o: loop { match args[1].as_str() {
            "b" | "build" => {
                if args.len() > 2 {
                    break Op::Build(args[2..].to_vec());
                } else {
                    break Op::Die(1, "Missing required argument(s) for command 'build'".into());
                }
            },
            "c" | "checksum" => {
                if args.len() > 2 {
                    break Op::Die(1, "Too many arguments for command 'checksum'".into());
                } else {
                    break Op::Checksum;
                }
            },
            "d" | "download" => {
                if args.len() > 2 {
                    break Op::Download(args[2..].to_vec());
                } else {
                    break Op::Die(1, "Missing required argument(s) for command 'download'".into());
                }
            },
            "f" | "find" => {
                if args.len() > 2 {
                    break Op::Find(args[2].clone());
                } else {
                    break Op::Die(1, "Missing required argument for command 'find'".into());
                }
            },
            "i" | "install" => {
                if args.len() > 2 {
                    break Op::Install(args[2..].to_vec());
                } else {
                    break Op::Die(1, "Missing required argument(s) for command 'install'".into());
                }
            },
            "n" | "new" => {
                if args.len() > 2 {
                    break Op::New(args[2].clone());
                } else {
                    break Op::Die(1, "Missing required argument for command 'new'".into());
                }
            },
            "r" | "remove" => {
                if args.len() > 2 {
                    break Op::Remove(args[2..].to_vec());
                } else {
                    break Op::Die(1, "Missing required argument(s) for command 'remove'".into());
                }
            },
            "l" | "list" => break Op::List,
            "p" | "purge" => break Op::Purge,
            "u" | "upgrade" => break Op::Upgrade,
            "v" | "version" => break Op::Version,
            "h" | "help" => break Op::Die(0, "".into()),
            x => {
                for c in x.chars() {
                    match c {
                        's' => cmd.sync = true,
                        'v' => cmd.verbose = true,
                        'y' => cmd.yes = true,
                        _ => continue,
                    }

                    args[1].remove(0);
                    continue 'o;
                }

                break Op::Die(1, format!("Unknown command {x}"));
            },
        }};

        return cmd;
    } else {
        return Cmd::default();
    }
}
