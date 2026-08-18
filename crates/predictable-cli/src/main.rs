//! The `predictable` binary: argument vector in, exit code out.
//!
//! Everything of substance is in the library half ([`predictable_cli`]), which is
//! a pure function of the arguments and the filesystem. This file exists only to
//! give it a process: collect `argv`, print what came back, and exit with the
//! code. That split is what lets every subcommand be tested end to end without
//! spawning anything.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (code, out, err) = predictable_cli::dispatch(&argv);
    let _ = std::io::stdout().write_all(out.as_bytes());
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().write_all(err.as_bytes());
    ExitCode::from(code as u8)
}
