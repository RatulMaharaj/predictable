//! Print the tapes a `.pir` model lowers to.
//!
//! ```text
//! cargo run -p predictable-tape --example dump_tape -- model.pir [more.pir ...]
//! ```
//!
//! This is the readable form of what the kernel will execute, and it is what the
//! docs page quotes.

use predictable_check::Input;
use predictable_plan::{plan_sources, PlanOptions};
use predictable_tape::lower_plan;

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: dump_tape <model.pir> [...]");
        std::process::exit(2);
    }
    let inputs: Vec<Input> = paths
        .iter()
        .map(|p| Input::new(p.clone(), std::fs::read_to_string(p).expect("readable")))
        .collect();

    let plan = match plan_sources(&inputs, &PlanOptions::default()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let prog = match lower_plan(&plan) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("# tape_digest {}", prog.digest);
    println!("# peel {} | ops {}", prog.peel, prog.op_count());
    print!("{}", prog.text());
}
