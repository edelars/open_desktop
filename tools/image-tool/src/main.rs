//! `od-image-tool info|check <image>`: see the library crate for the exit
//! statuses and the read-only guarantee.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let run = od_image_tool::run(&args);
    let _ = std::io::stdout().write_all(run.stdout.as_bytes());
    let _ = std::io::stderr().write_all(run.stderr.as_bytes());
    ExitCode::from(run.exit.code())
}
