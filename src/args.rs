pub enum Op {
    Build(Vec<String>),
    Checksum,
    Die(i32),
    Download(Vec<String>),
    New(String),
    Purge,
    Version,
}

pub fn parse(args: &Vec<String>) -> Op {
    if args.len() > 1 {
        match &args[1][..] {
            "b" | "build" => {
                if args.len() > 2 {
                    return Op::Build(args[2..].to_vec());
                } else {
                    return Op::Die(1);
                }
            },
            "c" | "checksum" => {
                if args.len() > 2 {
                    return Op::Die(1);
                } else {
                    return Op::Checksum;
                }
            },
            "d" | "download" => {
                if args.len() > 2 {
                    return Op::Download(args[2..].to_vec());
                } else {
                    return Op::Die(1);
                }
            },
            "n" | "new" => {
                if args.len() > 2 {
                    return Op::New(args[2].clone());
                } else {
                    return Op::Die(1);
                }
            },
            "p" | "purge" => return Op::Purge, 
            "v" | "version" => return Op::Version,
            "h" | "help" => return Op::Die(0),
            _ => return Op::Die(1),
        }
    } else {
        return Op::Die(0);
    }
}
