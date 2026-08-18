//! Survey the corpus for chunk- and executor-dependence, and say which components move.
//!
//! The test suite asserts; this prints. Run it when a determinism test fails and you want the
//! shape of the failure rather than its first line:
//!
//! ```text
//! cargo run -p predictable-determinism --example divergence_survey
//! ```

use std::collections::BTreeSet;

use predictable_determinism::corpus::Executors;
use predictable_determinism::{cases, load, project, render_golden};

fn main() {
    for case in cases() {
        let loaded = match load(&case) {
            Ok(l) => l,
            Err(why) => {
                println!("{:<22} skipped: {why}", case.name);
                continue;
            }
        };
        let base = match project(&loaded, 1, &predictable_runner::SerialExecutor) {
            Ok(b) => b,
            Err(why) => {
                println!("{:<22} skipped: {why}", case.name);
                continue;
            }
        };
        let want = render_golden(&loaded, &base);
        let mut moved: BTreeSet<String> = BTreeSet::new();
        for size in [1usize, 2, 3, 5, 1024] {
            for (label, exec) in Executors::all() {
                let got = render_golden(&loaded, &project(&loaded, size, exec.as_ref()).unwrap());
                for (l, r) in want.lines().zip(got.lines()) {
                    if l != r {
                        // Field 1 of a result line is the component; anything else prints whole.
                        let component = l.split_whitespace().nth(1).unwrap_or(l);
                        moved.insert(format!("{component} (C={size}, {label})"));
                    }
                }
            }
        }
        if moved.is_empty() {
            println!("{:<22} stable", case.name);
        } else {
            println!("{:<22} MOVED:", case.name);
            for m in moved {
                println!("    {m}");
            }
        }
    }
}
