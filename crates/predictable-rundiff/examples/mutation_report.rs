//! Regenerate the published root-cause hit-rate table (`04-verify.md` §9.2).
//!
//! ```text
//! cargo run -p predictable-rundiff --example mutation_report
//! ```
//!
//! Prints the markdown that `docs/v2/hypotheses.md` publishes. The same code path the harness
//! test asserts against produces it, so the published number cannot drift from the tested one.

use std::path::{Path, PathBuf};

use predictable_rundiff::mutation;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repo root")
        .to_path_buf()
}

fn cli_run(run_file: &Path, out: &Path) -> Result<(), String> {
    let argv: Vec<String> = [
        "run",
        &run_file.display().to_string(),
        "--out",
        &out.display().to_string(),
        "--json",
        "--retain-all",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let (code, stdout, stderr) = predictable_cli::dispatch(&argv);
    if code != 0 {
        return Err(format!("run exited {code}: {stdout}{stderr}"));
    }
    Ok(())
}

fn main() {
    let scratch = std::env::temp_dir().join("predictable-mutation-report");
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let report = mutation::run_catalogue(&repo().join("models"), &scratch, &cli_run)
        .expect("the catalogue applies");
    print!("{}", report.markdown());
    println!("\n| mistake | found |\n|---|---|");
    for (kind, hit, of) in report.by_kind() {
        println!("| {} | {hit}/{of} |", kind.word());
    }
}
