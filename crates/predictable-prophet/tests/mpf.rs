//! `.MPF` reader behaviour (`04-verify.md` §4.1).

use predictable_diagnostics::Severity;
use predictable_prophet::{read_mpf, Cell, DateOrder, MpfOptions};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

fn codes(diags: &[predictable_diagnostics::Diagnostic]) -> Vec<&str> {
    diags.iter().map(|d| d.code.as_str()).collect()
}

#[test]
fn reads_a_well_formed_crlf_file() {
    let bytes = fixture("term10.mpf");
    let read = read_mpf(&bytes, "data/term10.mpf", &MpfOptions::default());
    let mpf = &read.value;

    assert!(
        !read.has_errors(),
        "unexpected errors: {:?}",
        codes(&read.diagnostics)
    );
    assert_eq!(mpf.columns.len(), 10);
    assert_eq!(mpf.rows.len(), 4);

    // Names are snake-cased; the original survives in `source_name`.
    let sa = mpf.column("sum_assured").unwrap();
    assert_eq!(sa.source_name, "SUM_ASSURED");
    assert_eq!(sa.type_letter, 'N');
    assert_eq!(sa.unit, predictable_ir::Unit::None);

    // A quoted field with a thousands separator is still a number.
    assert_eq!(mpf.cell(0, "sum_assured"), Some(&Cell::Float(100_000.0)));
    // `I`, `S`, `B` and `D` all land in their declared shapes.
    assert_eq!(mpf.cell(0, "age_at_entry"), Some(&Cell::Float(47.0)));
    assert_eq!(mpf.cell(0, "pol_term"), Some(&Cell::Int(25)));
    assert_eq!(mpf.cell(2, "smoker"), Some(&Cell::Bool(true)));
    assert_eq!(mpf.cell(0, "smoker"), Some(&Cell::Bool(false)));
    assert_eq!(
        mpf.cell(0, "entry_date"),
        Some(&Cell::Date("2024-06-01".into()))
    );

    // Both `!` comments are retained: they are often the only documentation.
    assert_eq!(mpf.header.comments.len(), 3); // two `!` lines plus `##END##`
    assert_eq!(mpf.header.value("NUMLINES"), Some("4"));
}

#[test]
fn units_are_never_inferred() {
    let bytes = fixture("term10.mpf");
    let read = read_mpf(&bytes, "data/term10.mpf", &MpfOptions::default());
    // Exactly one W0102 per `N` column: AGE_AT_ENTRY, SUM_ASSURED, ANN_PREM.
    let lints: Vec<&str> = read
        .diagnostics
        .iter()
        .filter(|d| d.code == "W0102")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(lints.len(), 3, "{lints:?}");
    assert!(lints.iter().all(|m| m.contains("unit = \"none\"")));
    assert!(read.value.to_fragment_toml().contains("unit = \"none\""));
}

#[test]
fn distinct_string_values_propose_the_gender_enum() {
    let bytes = fixture("term10.mpf");
    let read = read_mpf(&bytes, "data/term10.mpf", &MpfOptions::default());
    let mpf = &read.value;
    assert_eq!(
        mpf.distinct.get("sex").map(Vec::as_slice),
        Some(["F".to_string(), "M".to_string()].as_slice())
    );
    assert_eq!(mpf.gender_enum_column(), Some("sex"));

    let toml = mpf.to_fragment_toml();
    assert!(toml.contains("[[enum]]\nname = \"Gender\""), "{toml}");
    assert!(toml.contains("dtype = \"enum(Gender)\""), "{toml}");
    // PROD_CD is a `T` column with values that are not `{M, F}`: proposed, not adopted.
    assert!(toml.contains("# proposed enum for `prod_cd`"), "{toml}");
    assert!(toml.contains("# name = \"ProdCd\""), "{toml}");
}

#[test]
fn csv_round_trips_the_rows() {
    let bytes = fixture("term10.mpf");
    let read = read_mpf(&bytes, "data/term10.mpf", &MpfOptions::default());
    let csv = read.value.to_csv();
    let mut lines = csv.lines();
    assert_eq!(
        lines.next().unwrap(),
        "spcode,pol_num,age_at_entry,sex,smoker,sum_assured,ann_prem,pol_term,prod_cd,entry_date"
    );
    assert_eq!(
        lines.next().unwrap(),
        "1,POL00042,47,M,false,100000,540,25,TERM01,2024-06-01"
    );
}

#[test]
fn windows_1252_is_decoded_not_refused() {
    let bytes = fixture("latin1.mpf");
    let read = read_mpf(&bytes, "data/latin1.mpf", &MpfOptions::default());
    assert_eq!(
        read.value.encoding,
        Some(predictable_prophet::Encoding::Windows1252)
    );
    assert!(read.value.header.comments[0].contains('’'));
    assert!(read.value.header.comments[0].contains('£'));

    let diag = read.diagnostics.iter().find(|d| d.code == "P0101").unwrap();
    assert_eq!(
        diag.severity,
        Severity::Warning,
        "encoding never fails a read"
    );
    // The span points at the first offending byte in the *original* file.
    let span = diag.primary_span().unwrap();
    assert_eq!(bytes[span.start], 0x92);
    // And the rows still parsed.
    assert_eq!(
        read.value.cell(0, "sum_assured"),
        Some(&Cell::Float(100_000.0))
    );
}

#[test]
fn an_ambiguous_date_order_is_refused_not_guessed() {
    let bytes = fixture("ambiguous_dates.mpf");
    let read = read_mpf(&bytes, "data/ambiguous_dates.mpf", &MpfOptions::default());

    let diag = read.diagnostics.iter().find(|d| d.code == "P0109").unwrap();
    assert_eq!(diag.severity, Severity::Error);
    assert!(
        diag.message.contains("cannot be resolved"),
        "{}",
        diag.message
    );
    assert_eq!(read.value.date_order, DateOrder::Unknown);
    assert!(read.has_errors());

    // Told which order it is, the same bytes read cleanly.
    let read = read_mpf(
        &bytes,
        "data/ambiguous_dates.mpf",
        &MpfOptions {
            date_order: Some(DateOrder::DayFirst),
        },
    );
    assert!(!read.diagnostics.iter().any(|d| d.code == "P0109"));
    assert_eq!(
        read.value.cell(0, "entry_date"),
        Some(&Cell::Date("2024-02-01".into()))
    );

    let read = read_mpf(
        &bytes,
        "data/ambiguous_dates.mpf",
        &MpfOptions {
            date_order: Some(DateOrder::MonthFirst),
        },
    );
    assert_eq!(
        read.value.cell(0, "entry_date"),
        Some(&Cell::Date("2024-01-02".into()))
    );
}

#[test]
fn an_unambiguous_date_order_is_read_from_the_data() {
    // `13/02/2024` can only be DD/MM, so the file resolves itself.
    let src = b"VARIABLE_TYPES,S,D\nPOL_NUM,D1\n*,P1,13/02/2024\n*,P2,01/03/2024\n";
    let read = read_mpf(src, "x.mpf", &MpfOptions::default());
    assert!(!read.diagnostics.iter().any(|d| d.code == "P0109"));
    assert_eq!(read.value.date_order, DateOrder::DayFirst);
    assert_eq!(
        read.value.cell(1, "d1"),
        Some(&Cell::Date("2024-03-01".into()))
    );
}

#[test]
fn header_disagreement_row_lengths_and_percentages() {
    let bytes = fixture("disagree.mpf");
    let read = read_mpf(&bytes, "data/disagree.mpf", &MpfOptions::default());
    let raised = codes(&read.diagnostics);

    // Both column-name sources are present and differ: refused, both listed.
    let p0103 = read.diagnostics.iter().find(|d| d.code == "P0103").unwrap();
    assert_eq!(p0103.spans.len(), 2);
    assert!(p0103.spans[0].label.as_ref().unwrap().contains("LAPSE"));
    assert!(p0103.spans[1]
        .label
        .as_ref()
        .unwrap()
        .contains("LAPSE_RATE"));

    // A short row and a long row are both reported and both skipped.
    assert!(raised.contains(&"P0105"), "{raised:?}");
    assert!(raised.contains(&"P0106"), "{raised:?}");
    assert_eq!(
        read.value.rows.len(),
        1,
        "only the well-formed row survives"
    );

    // `NUMLINES` never wins.
    let p0107 = read.diagnostics.iter().find(|d| d.code == "P0107").unwrap();
    assert!(p0107.message.contains("says 9"), "{}", p0107.message);

    // The percentage column is converted once and reported once.
    let p0108: Vec<&str> = read
        .diagnostics
        .iter()
        .filter(|d| d.code == "P0108")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(p0108.len(), 1, "{p0108:?}");
    assert_eq!(read.value.cell(0, "lapse"), Some(&Cell::Float(0.05)));

    // The span of the short-row diagnostic covers exactly that line.
    let p0105 = read.diagnostics.iter().find(|d| d.code == "P0105").unwrap();
    let span = p0105.primary_span().unwrap();
    assert_eq!(&bytes[span.start..span.end], b"*,POL00002,90000");
}

#[test]
fn a_file_with_no_name_line_is_an_error_not_a_panic() {
    let read = read_mpf(b"! only a comment\n", "x.mpf", &MpfOptions::default());
    assert_eq!(codes(&read.diagnostics), ["P0110"]);
    assert!(read.value.columns.is_empty());
}

#[test]
fn missing_variable_types_is_reported_and_columns_fall_back_to_str() {
    let read = read_mpf(
        b"POL_NUM,SUM_ASSURED\n*,P1,100\n",
        "x.mpf",
        &MpfOptions::default(),
    );
    let p0104 = read.diagnostics.iter().find(|d| d.code == "P0104").unwrap();
    assert!(p0104.message.contains("missing"), "{}", p0104.message);
    assert_eq!(
        read.value.cell(0, "sum_assured"),
        Some(&Cell::Str("100".into()))
    );
}

#[test]
fn a_value_that_contradicts_its_declared_type_is_reported() {
    let read = read_mpf(
        b"VARIABLE_TYPES,S,N\nPOL_NUM,SUM_ASSURED\n*,P1,not-a-number\n",
        "x.mpf",
        &MpfOptions::default(),
    );
    let p0111 = read.diagnostics.iter().find(|d| d.code == "P0111").unwrap();
    assert!(p0111.message.contains("not-a-number"));
    assert_eq!(read.value.cell(0, "sum_assured"), Some(&Cell::Null));
}

#[test]
fn empty_fields_are_null_not_zero() {
    let read = read_mpf(
        b"VARIABLE_TYPES,S,N\nPOL_NUM,SUM_ASSURED\n*,P1,\n",
        "x.mpf",
        &MpfOptions::default(),
    );
    assert_eq!(read.value.cell(0, "sum_assured"), Some(&Cell::Null));
    assert!(!read.diagnostics.iter().any(|d| d.code == "P0111"));
}

#[test]
fn unrecognised_header_keys_are_retained_verbatim() {
    let read = read_mpf(
        b"WEIRD_KEY,17\nVARIABLE_TYPES,S\nPOL_NUM\n*,P1\n",
        "x.mpf",
        &MpfOptions::default(),
    );
    let p0102 = read.diagnostics.iter().find(|d| d.code == "P0102").unwrap();
    assert!(p0102.message.contains("WEIRD_KEY"));
    assert_eq!(
        read.value.header.extra.get("WEIRD_KEY").map(String::as_str),
        Some("17")
    );
}

#[test]
fn output_format_can_carry_the_names_itself() {
    let read = read_mpf(
        b"OUTPUT_FORMAT,POL_NUM,SUM_ASSURED\nVARIABLE_TYPES,S,N\n*,P1,100\n",
        "x.mpf",
        &MpfOptions::default(),
    );
    assert!(!read.has_errors(), "{:?}", codes(&read.diagnostics));
    assert_eq!(read.value.columns.len(), 2);
    assert_eq!(read.value.cell(0, "sum_assured"), Some(&Cell::Float(100.0)));
}
