//! Render a conformance case's diagnostics as a terminal would show them.
//!
//! `cargo run -p predictable-check --example render_case -- invalid/e12-unresolved-name`
use std::path::PathBuf;

fn main() {
    let case = std::env::args()
        .nth(1)
        .expect("usage: render_case <case dir>");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("conformance")
        .join(&case);
    let mut inputs = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("case directory") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "pir") {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            inputs.push(predictable_check::Input::new(
                name,
                std::fs::read_to_string(&path).unwrap(),
            ));
        }
    }
    inputs.sort_by(|a, b| a.name.cmp(&b.name));
    let result = predictable_check::check(&inputs);
    print!(
        "{}",
        predictable_diagnostics::render_all(
            &result.diagnostics,
            &result.sources,
            &Default::default()
        )
    );
}
