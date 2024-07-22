pub enum Op {
    Build(Vec<String>),
    Die(i32),
    Download(Vec<String>),
    New(String),
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
            "d" | "download" => {
                if args.len() > 2 {
                    return Op::Download(args[2..].to_vec());
                } else {
                    return Op::Die(1);
                }
            }
            "h" | "help" => return Op::Die(0),
            "n" | "new" => {
                if args.len() > 2 {
                    return Op::New(args[2].clone());
                } else {
                    return Op::Die(1);
                }
            }
            _ => {
                return Op::Die(1);
            },
        }
    } else {
        return Op::Die(0);
    }
}
