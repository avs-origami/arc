use std::process::exit;

pub fn log(msg: &str, color: usize) {
    eprintln!("\x1b[{color}m->\x1b[0m {msg}");
}

pub fn log_ident(msg: &str, color: usize) {
    eprintln!("  \x1b[{color}m->\x1b[0m {msg}");
}

pub fn info(msg: &str) {
    log(msg, 35);
}

pub fn info_ident(msg: &str) {
    log_ident(msg, 35);
}

pub fn warn(msg: &str) {
    log(&format!("WARNING: {msg}"), 33);
}

pub fn die(msg: &str) {
    log(&format!("ERROR: {msg}"), 31);
    exit(1);
}
