//! The WASI half of the determinism gate: request on stdin, response on stdout.
//!
//! `03-engine.md` §9 targets `wasm32-wasip1` "for the CI determinism check
//! under wasmtime". This binary is that check's guest. It exists as a binary
//! rather than as a test because a `#[test]` cannot run under `wasmtime` without
//! a harness shim, and because the same executable is useful natively: run it
//! on the host, run it in the guest, compare the bytes.
//!
//! It reads one JSON request from stdin and writes [`predictable_wasm::call`]'s
//! response to stdout. No arguments, no files, no clock — the guest is given no
//! preopens by the gate, so a filesystem read would fail rather than silently
//! differ.

use std::io::{Read, Write};

fn main() {
    let mut request = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut request) {
        eprintln!("stdin: {e}");
        std::process::exit(2);
    }
    let response = predictable_wasm::call(&request);
    let mut stdout = std::io::stdout();
    if stdout.write_all(response.as_bytes()).is_err() || stdout.flush().is_err() {
        std::process::exit(2);
    }
}
