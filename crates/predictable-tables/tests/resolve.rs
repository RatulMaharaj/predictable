//! `TableResolver` behaviour and digest verification — `01-ir.md` §2.9/§2.9.1.

mod common;

use std::path::{Path, PathBuf};

use common::{csv, decl, key, value};
use predictable_ir::{DType, KeyPolicy, LitValue, TableSource};
use predictable_tables::{
    canonical_rows_text, load, load_bytes, project_root, verify_digest, FsResolver, InlineResolver,
    KeyArg, LoadOptions, MapResolver, Outcome, ResolveError, SchemeResolver, TableError,
    TableFormat, TableResolver,
};

const MORTALITY: &str = "age,qx\n40,0.001\n41,0.0011\n42,0.0013\n";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("predictable-tables-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("models/term/tables")).unwrap();
    std::fs::write(dir.join("predictable.toml"), "[project]\n").unwrap();
    std::fs::write(dir.join("models/term/tables/mortality.csv"), MORTALITY).unwrap();
    std::fs::write(dir.join("secrets.csv"), "age,qx\n40,9.9\n").unwrap();
    dir
}

fn table_decl() -> predictable_ir::TableDecl {
    decl(
        "mortality",
        vec![key("age", DType::I64, KeyPolicy::Exact)],
        vec![value("qx", DType::F64)],
    )
}

#[test]
fn fs_resolver_reads_relative_to_the_declaring_file() {
    let root = scratch("fs-relative");
    let base = root.join("models/term");
    let mut d = table_decl();
    d.source = TableSource::File("tables/mortality.csv".into());

    let bytes = FsResolver::new().resolve(&d, &base).unwrap();
    assert_eq!(bytes.bytes, MORTALITY.as_bytes());
    assert_eq!(bytes.format, TableFormat::Csv);
    assert!(bytes.origin.ends_with("models/term/tables/mortality.csv"));

    // ...and not relative to the process working directory.
    let elsewhere = root.join("models");
    assert!(FsResolver::new().resolve(&d, &elsewhere).is_err());
}

#[test]
fn project_root_is_the_nearest_predictable_toml() {
    let root = scratch("root-discovery");
    assert_eq!(project_root(&root.join("models/term")), root);
    // With no `predictable.toml` anywhere, the module's own directory is the root.
    let bare = std::env::temp_dir().join("predictable-tables-bare/models");
    std::fs::create_dir_all(&bare).unwrap();
    assert_eq!(project_root(&bare), bare);
}

#[test]
fn escaping_the_project_root_is_refused() {
    let root = scratch("escape");
    let base = root.join("models/term");
    let mut d = table_decl();
    d.source = TableSource::File("../../../secrets.csv".into());

    let err = FsResolver::new().resolve(&d, &base).unwrap_err();
    assert!(matches!(err, ResolveError::EscapesRoot { .. }), "{err}");
    assert!(err.to_string().contains("escapes the project root"));

    // ...and it renders as the registered code for that rule.
    let diag = predictable_tables::diagnostic(&TableError::Resolve(err)).unwrap();
    assert_eq!(diag.code, "E0801");
}

#[test]
fn a_dotdot_that_stays_inside_the_root_is_fine() {
    let root = scratch("dotdot-inside");
    let base = root.join("models/term");
    let mut d = table_decl();
    d.source = TableSource::File("../term/tables/mortality.csv".into());
    assert!(FsResolver::new().resolve(&d, &base).is_ok());
}

#[test]
fn absolute_sources_are_refused() {
    let root = scratch("absolute");
    let mut d = table_decl();
    d.source = TableSource::File(root.join("secrets.csv").to_string_lossy().to_string());
    let err = FsResolver::new()
        .resolve(&d, &root.join("models/term"))
        .unwrap_err();
    assert!(matches!(err, ResolveError::AbsolutePath { .. }), "{err}");
}

#[test]
fn a_pinned_root_beats_discovery() {
    let root = scratch("pinned");
    let base = root.join("models/term");
    let mut d = table_decl();
    d.source = TableSource::File("../../secrets.csv".into());
    // Discovery finds `predictable.toml` at the top, so this is legal...
    assert!(FsResolver::new().resolve(&d, &base).is_ok());
    // ...but a host that pins the module directory sandboxes it out.
    let err = FsResolver::rooted_at(&base).resolve(&d, &base).unwrap_err();
    assert!(matches!(err, ResolveError::EscapesRoot { .. }), "{err}");
}

#[test]
fn map_resolver_serves_the_resource_scheme() {
    let mut map = MapResolver::new();
    map.insert("sa8990", MORTALITY);
    let mut d = table_decl();
    d.source = TableSource::Resource("sa8990".into());

    let bytes = map.resolve(&d, Path::new(".")).unwrap();
    assert_eq!(bytes.bytes, MORTALITY.as_bytes());
    assert_eq!(bytes.origin, "resource:sa8990");
    assert_eq!(map.names().collect::<Vec<_>>(), ["sa8990"]);

    d.source = TableSource::Resource("missing".into());
    let err = map.resolve(&d, Path::new(".")).unwrap_err();
    assert!(matches!(err, ResolveError::UnknownResource { .. }), "{err}");
}

#[test]
fn resolvers_refuse_schemes_they_do_not_serve() {
    let mut d = table_decl();
    d.source = TableSource::Inline;
    let err = FsResolver::new().resolve(&d, Path::new(".")).unwrap_err();
    assert!(matches!(err, ResolveError::WrongScheme { .. }), "{err}");

    d.source = TableSource::File("t.csv".into());
    let err = InlineResolver.resolve(&d, Path::new(".")).unwrap_err();
    assert!(matches!(err, ResolveError::WrongScheme { .. }), "{err}");
}

fn inline_decl() -> predictable_ir::TableDecl {
    let mut d = decl(
        "lapse_rates",
        vec![key("policy_year", DType::I64, KeyPolicy::Step)],
        vec![value("lapse_pa", DType::F64)],
    );
    d.source = TableSource::Inline;
    d.rows = Some(vec![
        vec![LitValue::Int(1), LitValue::Float(0.12)],
        vec![LitValue::Int(2), LitValue::Float(0.08)],
        vec![LitValue::Int(3), LitValue::Float(0.05)],
    ]);
    d
}

#[test]
fn inline_tables_carry_their_own_content() {
    let d = inline_decl();
    let bytes = InlineResolver.resolve(&d, Path::new(".")).unwrap();
    assert_eq!(bytes.format, TableFormat::Inline);
    assert_eq!(bytes.origin, "inline");
    assert_eq!(
        String::from_utf8(bytes.bytes.clone()).unwrap(),
        "[\n  [1, 0.12],\n  [2, 0.08],\n  [3, 0.05],\n]\n"
    );
    assert_eq!(
        canonical_rows_text(d.rows.as_ref().unwrap()),
        String::from_utf8(bytes.bytes).unwrap()
    );

    let table = load(&d, Path::new("."), &InlineResolver, &LoadOptions::default()).unwrap();
    // `step` returns the predecessor: year 2 in band 2, year 7 still in band 3.
    assert_eq!(table.lookup_f64(&[KeyArg::Int(2)], 0), Outcome::Hit(0.08));
    assert_eq!(table.lookup_f64(&[KeyArg::Int(7)], 0), Outcome::Hit(0.05));
    // Below the first band there is no predecessor, and `on_missing` is `error`.
    assert!(matches!(
        table.lookup_f64(&[KeyArg::Int(0)], 0),
        Outcome::Trap(_)
    ));
}

#[test]
fn the_inline_digest_is_a_function_of_the_declaration_alone() {
    let d = inline_decl();
    let bytes = InlineResolver.resolve(&d, Path::new(".")).unwrap();
    let check = verify_digest(&d, &bytes, &LoadOptions::default()).unwrap();
    // Stable across processes and platforms: same rows, same digest.
    let again = InlineResolver.resolve(&d, Path::new(".")).unwrap();
    assert_eq!(
        check.actual,
        verify_digest(&d, &again, &LoadOptions::default())
            .unwrap()
            .actual
    );
    assert!(check.actual.starts_with("sha256:"));
    assert_eq!(check.declared, None);
    assert!(!check.drifted);
}

#[test]
fn scheme_resolver_dispatches_on_the_source_scheme() {
    let root = scratch("scheme");
    let mut map = MapResolver::new();
    map.insert("sa8990", MORTALITY);
    let dispatcher = SchemeResolver::with_resources(map);

    let mut file = table_decl();
    file.source = TableSource::File("tables/mortality.csv".into());
    let mut resource = table_decl();
    resource.source = TableSource::Resource("sa8990".into());
    let inline = inline_decl();

    let base = root.join("models/term");
    assert_eq!(
        dispatcher.resolve(&file, &base).unwrap().bytes,
        MORTALITY.as_bytes()
    );
    assert_eq!(
        dispatcher.resolve(&resource, &base).unwrap().origin,
        "resource:sa8990"
    );
    assert_eq!(dispatcher.resolve(&inline, &base).unwrap().origin, "inline");
}

#[test]
fn the_digest_is_the_tables_identity() {
    let mut d = table_decl();
    let bytes = csv(MORTALITY);
    let actual = verify_digest(&d, &bytes, &LoadOptions::default())
        .unwrap()
        .actual;

    // Pinned and matching: fine, and not drifted.
    d.digest = Some(actual.clone());
    let table = load_bytes(&d, csv(MORTALITY), &LoadOptions::default()).unwrap();
    assert_eq!(table.digest, actual);
    assert!(!table.drifted);

    // A `resource:` copy of the same bytes agrees with the `file:` original —
    // this is what makes the resolver swap safe.
    let mut map = MapResolver::new();
    map.insert("sa8990", MORTALITY);
    let mut as_resource = d.clone();
    as_resource.source = TableSource::Resource("sa8990".into());
    let via_map = load(&as_resource, Path::new("."), &map, &LoadOptions::default()).unwrap();
    assert_eq!(via_map.digest, table.digest);
}

#[test]
fn drifted_content_is_refused_unless_allowed() {
    let mut d = table_decl();
    d.digest = Some(
        verify_digest(&d, &csv(MORTALITY), &LoadOptions::default())
            .unwrap()
            .actual,
    );

    let edited = "age,qx\n40,0.002\n41,0.0011\n42,0.0013\n";
    let err = load_bytes(&d, csv(edited), &LoadOptions::default()).unwrap_err();
    let TableError::DigestMismatch {
        declared, actual, ..
    } = &err
    else {
        panic!("expected a digest mismatch, got {err}");
    };
    assert_ne!(declared, actual);
    assert!(err.to_string().contains("--allow-table-drift"));

    let allowed = LoadOptions {
        allow_table_drift: true,
    };
    let table = load_bytes(&d, csv(edited), &allowed).unwrap();
    assert!(table.drifted);
    assert_eq!(table.lookup_f64(&[KeyArg::Int(40)], 0), Outcome::Hit(0.002));
}

#[test]
fn parquet_and_arrow_are_not_this_crates_business() {
    let d = table_decl();
    let bytes = predictable_tables::TableBytes {
        bytes: b"PAR1".to_vec(),
        origin: "tables/mortality.parquet".into(),
        format: TableFormat::Parquet,
    };
    let err = load_bytes(&d, bytes, &LoadOptions::default()).unwrap_err();
    assert!(matches!(err, TableError::UnsupportedFormat { .. }), "{err}");
    assert!(err.to_string().contains("Parquet"));
}
