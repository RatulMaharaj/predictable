//! `.rpt` reader behaviour (`04-verify.md` §4.3).

use std::collections::BTreeMap;

use predictable_diagnostics::Severity;
use predictable_ir::Basis;
use predictable_prophet::{read_rpt, RptLevel, RptOptions};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn a_one_based_period_column_is_refused_until_the_base_is_stated() {
    let bytes = fixture("term_run.rpt");
    let read = read_rpt(&bytes, "prophet/term_run.rpt", &RptOptions::default());

    let p0302 = read.diagnostics.iter().find(|d| d.code == "P0302").unwrap();
    assert_eq!(p0302.severity, Severity::Error);
    assert!(p0302.message.contains("runs 1..3"), "{}", p0302.message);
    assert!(p0302.suggestions[0].message.contains("--period-base 1"));
    // Nothing is rebased on a guess, so there are no rows at all.
    assert_eq!(read.value.period_base, None);
    assert!(read.value.rows.is_empty());
    assert_eq!(read.value.period_min, Some(1));
    assert_eq!(read.value.period_max, Some(3));
}

#[test]
fn with_the_base_stated_the_file_reads_as_a_result_set() {
    let bytes = fixture("term_run.rpt");
    let read = read_rpt(
        &bytes,
        "prophet/term_run.rpt",
        &RptOptions {
            period_base: Some(1),
            timeline_basis: Some(Basis::Monthly),
            ..RptOptions::default()
        },
    );
    let r = &read.value;
    assert!(!read.has_errors(), "{:?}", read.codes());

    assert_eq!(r.run.as_deref(), Some("TERM_BASE_2026Q2"));
    assert_eq!(r.product.as_deref(), Some("TERM_UK"));
    assert_eq!(r.time_units.as_deref(), Some("MONTHS"));
    assert_eq!(r.num_periods, Some(3));
    assert_eq!(r.level, RptLevel::PerModelPoint);
    assert_eq!(r.mp_key_column.as_deref(), Some("POL_NUM"));
    assert_eq!(r.period_column.as_deref(), Some("PERIOD"));
    assert_eq!(r.mp_keys(), vec!["POL00042", "POL00043"]);

    // SPCODE is numeric, so it imports as a component alongside the three measures.
    let ids: Vec<&str> = r.components.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "prophet.spcode",
            "prophet.prem_inc",
            "prophet.dth_claim",
            "prophet.bel_tot"
        ]
    );

    assert_eq!(r.rows.len(), 6);
    assert_eq!(r.rows[0].t, 0, "period 1 rebased onto t = 0");
    assert_eq!(r.rows[0].mp_key, "POL00042");
    assert_eq!(r.rows[0].mp_row, 0);
    assert_eq!(r.rows[3].mp_row, 1);
    assert_eq!(r.rows[2].t, 2);
    assert_eq!(r.value(0, "prophet.prem_inc"), Some(540.0));
    assert_eq!(r.value(2, "prophet.bel_tot"), Some(12_544.51));
    assert_eq!(r.cell_count(), 6 * 4);
}

#[test]
fn the_mapping_file_names_the_components() {
    let bytes = fixture("term_run.rpt");
    let read = read_rpt(
        &bytes,
        "prophet/term_run.rpt",
        &RptOptions {
            period_base: Some(1),
            component_names: BTreeMap::from([
                (
                    "PREM_INC".to_string(),
                    "term_assurance.premium_income".to_string(),
                ),
                ("BEL_TOT".to_string(), "term_assurance.bel".to_string()),
            ]),
            ..RptOptions::default()
        },
    );
    let mapped: Vec<(&str, bool)> = read
        .value
        .components
        .iter()
        .map(|c| (c.id.as_str(), c.mapped))
        .collect();
    assert_eq!(
        mapped,
        [
            ("prophet.spcode", false),
            ("term_assurance.premium_income", true),
            ("prophet.dth_claim", false),
            ("term_assurance.bel", true),
        ]
    );
    assert_eq!(read.value.value(0, "term_assurance.bel"), Some(12_744.51));
}

#[test]
fn time_units_must_match_the_timeline_basis() {
    let read = read_rpt(
        &fixture("term_run.rpt"),
        "prophet/term_run.rpt",
        &RptOptions {
            period_base: Some(1),
            timeline_basis: Some(Basis::Annual),
            ..RptOptions::default()
        },
    );
    let p0303 = read.diagnostics.iter().find(|d| d.code == "P0303").unwrap();
    assert_eq!(p0303.severity, Severity::Error);
    assert!(p0303.message.contains("MONTHS"), "{}", p0303.message);
    assert!(p0303.message.contains("annual"), "{}", p0303.message);
}

#[test]
fn a_grouped_file_diffs_at_group_level() {
    let read = read_rpt(
        &fixture("grouped.rpt"),
        "prophet/grouped.rpt",
        &RptOptions::default(),
    );
    let r = &read.value;

    let p0304 = read.diagnostics.iter().find(|d| d.code == "P0304").unwrap();
    assert_eq!(p0304.severity, Severity::Info);
    assert_eq!(r.level, RptLevel::Grouped);
    // A 0-based period column needs no decision, so the rows are there.
    assert_eq!(r.period_base, Some(0));
    assert_eq!(r.rows.len(), 3);
    assert!(r.rows.iter().all(|row| row.mp_key == "TERM_UK"));
    assert_eq!(r.value(0, "prophet.bel_tot"), Some(1000.0));
    assert!(!read.has_errors(), "{:?}", read.codes());
}

#[test]
fn a_file_with_no_period_column_cannot_be_diffed() {
    let src = b"RUN, X\nPOL_NUM,BEL\nPOL1,100.0\n";
    let read = read_rpt(src, "x.rpt", &RptOptions::default());
    let p0301 = read.diagnostics.iter().find(|d| d.code == "P0301").unwrap();
    assert_eq!(p0301.severity, Severity::Error);
    assert!(p0301.message.contains("PERIOD"), "{}", p0301.message);
    assert!(read.value.rows.is_empty());
}

#[test]
fn an_ambiguous_model_point_key_is_refused() {
    let src = b"PERIOD,POL_NUM,COHORT,BEL\n0,POL1,A,100.0\n1,POL1,A,90.0\n";
    let read = read_rpt(src, "x.rpt", &RptOptions::default());
    let p0305 = read.diagnostics.iter().find(|d| d.code == "P0305").unwrap();
    assert!(
        p0305.message.contains("POL_NUM, COHORT"),
        "{}",
        p0305.message
    );
    assert!(p0305.suggestions[0].message.contains("--mp-key POL_NUM"));

    // Told which column it is, the same bytes read cleanly.
    let read = read_rpt(
        src,
        "x.rpt",
        &RptOptions {
            mp_key_column: Some("COHORT".to_string()),
            ..RptOptions::default()
        },
    );
    assert!(!read.has_errors(), "{:?}", read.codes());
    assert_eq!(read.value.mp_key_column.as_deref(), Some("COHORT"));
    assert_eq!(read.value.mp_keys(), vec!["A"]);
}

#[test]
fn a_ragged_data_row_is_skipped_and_reported() {
    let src = b"PERIOD,BEL\n0,100.0\n1\n2,80.0\n";
    let read = read_rpt(src, "x.rpt", &RptOptions::default());
    assert!(read.codes().contains(&"P0105"));
    assert_eq!(read.value.rows.len(), 2);
}

#[test]
fn an_empty_file_is_an_error_not_a_panic() {
    let read = read_rpt(b"", "x.rpt", &RptOptions::default());
    assert_eq!(read.codes(), ["P0110"]);
    assert!(read.value.components.is_empty());
}
