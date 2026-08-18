//! `.fac` reader behaviour (`04-verify.md` §4.2).

use predictable_diagnostics::Severity;
use predictable_prophet::{read_fac, ProposedPolicy};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn the_last_dimension_varies_fastest() {
    let bytes = fixture("sa8990.fac");
    let read = read_fac(&bytes, "tables/sa8990.fac");
    let table = &read.value;

    assert!(!read.has_errors(), "{:?}", read.codes());
    assert_eq!(table.name, "sa8990");
    assert_eq!(table.dims.len(), 3);
    assert_eq!(table.cell_count(), 3 * 2 * 2);
    assert_eq!(table.data.len(), 12);

    // Cells 0..3 walk SMOKER then SEX, with AGE held at 40.
    assert_eq!(table.key_at(0), vec![40, 1, 0]);
    assert_eq!(table.key_at(1), vec![40, 1, 1]);
    assert_eq!(table.key_at(2), vec![40, 2, 0]);
    assert_eq!(table.key_at(3), vec![40, 2, 1]);
    assert_eq!(table.key_at(4), vec![41, 1, 0]);
    assert_eq!(table.key_at(11), vec![42, 2, 1]);

    assert_eq!(table.values_at(0), &[0.001]);
    assert_eq!(table.values_at(1), &[0.0025]);
    assert_eq!(table.values_at(11), &[0.00525]);
}

#[test]
fn the_ordering_is_restated_with_a_corner_of_the_table() {
    let read = read_fac(&fixture("sa8990.fac"), "tables/sa8990.fac");
    let p0202 = read.diagnostics.iter().find(|d| d.code == "P0202").unwrap();
    assert_eq!(p0202.severity, Severity::Info);
    assert!(
        p0202.message.contains("`SMOKER` varying fastest"),
        "{}",
        p0202.message
    );
    // Four resolved keys, so a human can eyeball the corner against the source.
    assert!(
        p0202.message.contains("(age=40, sex=1, smoker=0) -> 0.001"),
        "{}",
        p0202.message
    );
    assert_eq!(p0202.message.matches("->").count(), 4);
}

#[test]
fn every_proposed_policy_is_a_reviewed_decision() {
    let read = read_fac(&fixture("sa8990.fac"), "tables/sa8990.fac");
    let proposals: Vec<&str> = read
        .diagnostics
        .iter()
        .filter(|d| d.code == "P0204")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(proposals.len(), 3, "{proposals:?}");
    assert!(proposals[0].contains("key `age` proposed with `policy = \"clamp\"`"));
    assert!(proposals[1].contains("`policy = \"exact\"`"));

    let table = &read.value;
    assert_eq!(table.dims[0].policy, ProposedPolicy::Clamp);
    assert_eq!(table.dims[1].policy, ProposedPolicy::Exact);
}

#[test]
fn emits_a_long_format_csv_and_a_table_fragment() {
    let read = read_fac(&fixture("sa8990.fac"), "tables/sa8990.fac");
    let csv = read.value.to_csv();
    let mut lines = csv.lines();
    assert_eq!(lines.next().unwrap(), "age,sex,smoker,value");
    assert_eq!(lines.next().unwrap(), "40,1,0,0.001");
    assert_eq!(lines.next().unwrap(), "40,1,1,0.0025");
    assert_eq!(csv.lines().count(), 13);

    let fragment = read.value.to_fragment_toml();
    assert!(fragment.contains("name = \"sa8990\""));
    assert!(fragment.contains("{ name = \"age\", dtype = \"i64\", policy = \"clamp\" }"));
    assert!(fragment.contains("on_missing = \"error\""));
    assert!(fragment.contains(&format!("digest = \"sha256:{}\"", read.value.digest())));
    // The digest is over the emitted CSV, so it is stable across reads.
    let again = read_fac(&fixture("sa8990.fac"), "tables/sa8990.fac");
    assert_eq!(again.value.digest(), read.value.digest());
}

#[test]
fn a_value_count_mismatch_is_refused_never_padded() {
    let read = read_fac(&fixture("short.fac"), "tables/short.fac");
    let p0203 = read.diagnostics.iter().find(|d| d.code == "P0203").unwrap();
    assert_eq!(p0203.severity, Severity::Error);
    assert!(
        p0203.message.contains("expected 6 value(s)")
            && p0203.message.contains("found 4: 2 missing"),
        "{}",
        p0203.message
    );
    // Nothing was invented to fill the gap.
    assert_eq!(read.value.data.len(), 4);
}

#[test]
fn a_malformed_dimension_block_is_reported() {
    let src = b"DIMENSIONS, 3\nDIM1, AGE, 40, 42\nDIM2, SEX, 1\nDATA\n1.0\n";
    let read = read_fac(src, "x.fac");
    let messages: Vec<&str> = read
        .diagnostics
        .iter()
        .filter(|d| d.code == "P0201")
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("says 3 but 2")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("needs `NAME, LO, HI`")),
        "{messages:?}"
    );
    assert!(read.has_errors());
}

#[test]
fn multi_value_tables_split_into_named_columns() {
    let src =
        b"DIMENSIONS, 1\nDIM1, AGE, 40, 41\nVALUES, qx, ix\nDATA\n0.001, 0.02\n0.0012, 0.021\n";
    let read = read_fac(src, "x.fac");
    assert!(!read.has_errors(), "{:?}", read.codes());
    assert_eq!(read.value.values, vec!["qx".to_string(), "ix".to_string()]);
    assert_eq!(read.value.values_at(0), &[0.001, 0.02]);
    assert_eq!(read.value.values_at(1), &[0.0012, 0.021]);
    assert_eq!(read.value.to_csv().lines().next().unwrap(), "age,qx,ix");
}

#[test]
fn whitespace_separated_value_blocks_are_accepted() {
    let src = b"DIMENSIONS, 1\nDIM1, AGE, 1, 4\nDATA\n0.1 0.2\n0.3   0.4\n";
    let read = read_fac(src, "x.fac");
    assert!(!read.has_errors(), "{:?}", read.codes());
    assert_eq!(read.value.data, vec![0.1, 0.2, 0.3, 0.4]);
}

#[test]
fn an_empty_file_is_an_error_not_a_panic() {
    let read = read_fac(b"", "x.fac");
    assert!(read.has_errors());
    assert!(read.codes().contains(&"P0201"));
    assert!(read.value.dims.is_empty());
}
