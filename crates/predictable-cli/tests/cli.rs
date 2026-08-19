//! End-to-end behaviour of `predictable fmt` and `predictable digest`.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_predictable");

const UGLY: &str = "format = \"pir/1\"\nmodule = \"m\"\n\n[[component]]\nexpr  = \"( a )*1.050\"\nname = \"x\"\nkind = \"Derived\"\n";
const CANONICAL: &str = "format = \"pir/1\"\nmodule = \"m\"\n\n[[component]]\nname = \"x\"\nkind = \"Derived\"\nexpr = \"a * 1.05\"\n";

/// A scratch directory unique to the calling test.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("predictable-cli-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(BIN).args(args).output().expect("runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.display().to_string()
}

#[test]
fn fmt_rewrites_a_file_in_place_and_is_then_a_no_op() {
    let dir = scratch("fmt-write");
    let path = write(&dir, "m.pir", UGLY);

    let (code, stdout, _) = run(&["fmt", &path]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), CANONICAL);

    let (code, stdout, _) = run(&["fmt", &path]);
    assert_eq!(code, 0);
    assert!(
        !stdout.contains("formatted"),
        "second run should change nothing: {stdout}"
    );
}

#[test]
fn fmt_check_reports_without_writing_and_exits_1() {
    let dir = scratch("fmt-check");
    let path = write(&dir, "m.pir", UGLY);

    let (code, stdout, _) = run(&["fmt", "--check", &path]);
    assert_eq!(code, 1);
    assert!(stdout.contains("not canonical"), "{stdout}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        UGLY,
        "--check must not write"
    );

    write(&dir, "m.pir", CANONICAL);
    let (code, _, _) = run(&["fmt", "--check", &path]);
    assert_eq!(code, 0);
}

#[test]
fn fmt_walks_a_directory() {
    let dir = scratch("fmt-dir");
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    write(&dir, "a.pir", UGLY);
    write(&dir.join("nested"), "b.pir", UGLY);
    write(&dir, "notes.txt", "left alone");

    let (code, stdout, _) = run(&["fmt", &dir.display().to_string()]);
    assert_eq!(code, 0);
    assert_eq!(stdout.matches("formatted").count(), 2, "{stdout}");
    assert_eq!(
        std::fs::read_to_string(dir.join("nested/b.pir")).unwrap(),
        CANONICAL
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
        "left alone"
    );
}

#[test]
fn fmt_refuses_a_file_that_does_not_parse_and_leaves_it_alone() {
    let dir = scratch("fmt-broken");
    let broken = "format = \"pir/1\"\n[[component\n";
    let path = write(&dir, "m.pir", broken);

    let (code, _, stderr) = run(&["fmt", &path]);
    assert_eq!(code, 2);
    assert!(stderr.contains("cannot format"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
}

#[test]
fn digest_is_stable_across_a_reformat() {
    let dir = scratch("digest");
    let ugly = write(&dir, "ugly.pir", UGLY);
    let tidy = write(&dir, "tidy.pir", CANONICAL);

    let (_, a, _) = run(&["digest", &ugly]);
    let (_, b, _) = run(&["digest", &tidy]);
    // Different file names, so only the per-file digests can be compared.
    let (_, a_each, _) = run(&["digest", "--each", &ugly]);
    let (_, b_each, _) = run(&["digest", "--each", &tidy]);
    assert_ne!(a, b, "model_digest covers the file name");
    assert_eq!(
        a_each.split_whitespace().next(),
        b_each.split_whitespace().next(),
        "canonical text is identical, so the file digest must be"
    );
    assert!(a.contains("model_digest"));
}

#[test]
fn digest_kinds_are_wired_up() {
    let dir = scratch("digest-kinds");
    let run_file = write(
        &dir,
        "run.pir",
        "format = \"pir/1\"\n\n[run]\nproduct = \"p\"\nout = \"runs/x\"\n\n[run.exec]\nthreads = 1\n",
    );
    let noisy = write(
        &dir,
        "run2.pir",
        "format = \"pir/1\"\n\n[run]\nproduct = \"p\"\nout = \"runs/DIFFERENT\"\n\n[run.exec]\nthreads = 64\n",
    );
    let (_, a, _) = run(&["digest", "--kind", "run", &run_file]);
    let (_, b, _) = run(&["digest", "--kind", "run", &noisy]);
    assert_eq!(a.split_whitespace().next(), b.split_whitespace().next());

    let csv = write(&dir, "t.csv", "age,qx\n30,0.001\n");
    let (code, out, _) = run(&["digest", "--kind", "file", &csv]);
    assert_eq!(code, 0);
    assert!(out.starts_with("sha256:"), "{out}");

    let module = write(
        &dir,
        "inline.pir",
        "format = \"pir/1\"\nmodule = \"m\"\n\n[[table]]\nname = \"t\"\nsource = \"inline\"\nrows = [\n  [1, 0.12],\n  [2, 0.08],\n]\n",
    );
    let (code, out, _) = run(&["digest", "--kind", "table:t", &module]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("(table t)"), "{out}");

    let (code, _, stderr) = run(&["digest", "--kind", "table:missing", &module]);
    assert_eq!(code, 2);
    assert!(stderr.contains("no inlined table"), "{stderr}");
}

#[test]
fn unknown_arguments_fail_loudly() {
    assert_eq!(run(&["frobnicate"]).0, 2);
    assert_eq!(run(&["fmt", "--wat", "x.pir"]).0, 2);
    assert_eq!(run(&["fmt"]).0, 2);
    assert_eq!(run(&["--help"]).0, 0);
}
