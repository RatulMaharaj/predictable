//! `[[aggregation]]` behaviour (`01-ir.md` §8.3, ruling Q4): monoid folds, flat ordered group
//! keys, filters, `over_t`, and the chunk-order determinism the whole design exists for.

mod common;

use common::{model, runner, term_chunks, TERM};
use predictable_ir::run::{Aggregation, AggregationOp, OverT};
use predictable_runner::{minimal_run_file, CancelFlag, LocalExecutor, SerialExecutor};

fn agg(name: &str, group_by: &[&str], measure: &str, op: AggregationOp) -> Aggregation {
    Aggregation {
        name: name.to_string(),
        group_by: group_by.iter().map(|k| (*k).to_string()).collect(),
        measure: measure.to_string(),
        op,
        weight: None,
        filter: None,
        over_t: None,
    }
}

#[test]
fn sums_by_an_ordered_key_tuple_and_never_nests() {
    let m = model(TERM);
    let mut run = minimal_run_file();
    run.aggregations = vec![
        agg(
            "bel_by_product",
            &["product_code"],
            "bel",
            AggregationOp::Sum,
        ),
        agg("bel_total", &[], "bel", AggregationOp::Sum),
    ];
    let r = runner(&m, &run);
    let p = r
        .project(&term_chunks(2), &SerialExecutor, &CancelFlag::new())
        .unwrap();

    // Group keys are the canonical text of the modelpoint field, not a dictionary code.
    let by_product: Vec<_> = p
        .aggregates
        .iter()
        .filter(|r| r.aggregation == "bel_by_product")
        .map(|r| (r.group_key.clone(), r.value))
        .collect();
    assert_eq!(by_product.len(), 2, "one flat row per distinct tuple");
    assert_eq!(by_product[0].0, "product_code=TERM_IE");
    assert_eq!(by_product[1].0, "product_code=TERM_UK");

    // No subtotal rows: a total is a second declared block, not a parent row.
    let total = p.aggregate("bel_total", None).unwrap();
    assert!((by_product[0].1 + by_product[1].1 - total).abs() < 1e-9);
}

#[test]
fn partials_combine_in_chunk_order_whatever_the_executor_did() {
    let m = model(TERM);
    let mut run = minimal_run_file();
    run.aggregations = vec![agg(
        "bel_by_product",
        &["product_code"],
        "bel",
        AggregationOp::Sum,
    )];
    let r = runner(&m, &run);
    let cancel = CancelFlag::new();

    let serial = r
        .project(&term_chunks(1), &SerialExecutor, &cancel)
        .unwrap();
    let parallel = r
        .project(&term_chunks(1), &LocalExecutor::new(4), &cancel)
        .unwrap();
    let whole = r
        .project(&term_chunks(1024), &SerialExecutor, &cancel)
        .unwrap();

    let bits = |rows: &[predictable_io::outbound::AggregateRow]| -> Vec<(String, u64)> {
        rows.iter()
            .map(|r| (r.group_key.clone(), r.value.to_bits()))
            .collect()
    };
    assert_eq!(bits(&serial.aggregates), bits(&parallel.aggregates));
    // Chunking changes the fold's grouping of additions, and the answer survives it because
    // partials combine in index order.
    assert_eq!(bits(&serial.aggregates), bits(&whole.aggregates));
}

#[test]
fn a_filter_excludes_modelpoints_from_every_group() {
    let m = model(TERM);
    let mut run = minimal_run_file();
    let mut filtered = agg(
        "bel_in_force",
        &["product_code"],
        "bel",
        AggregationOp::Count,
    );
    filtered.filter = Some("in_force".to_string());
    run.aggregations = vec![
        filtered,
        agg("bel_all", &["product_code"], "bel", AggregationOp::Count),
    ];
    let r = runner(&m, &run);
    let p = r
        .project(&term_chunks(2), &SerialExecutor, &CancelFlag::new())
        .unwrap();

    // POL4 (TERM_IE) has in_force = false.
    let count = |name: &str, key: &str| {
        p.aggregates
            .iter()
            .find(|r| r.aggregation == name && r.group_key == key)
            .map(|r| r.value)
    };
    assert_eq!(count("bel_all", "product_code=TERM_IE"), Some(2.0));
    assert_eq!(count("bel_in_force", "product_code=TERM_IE"), Some(1.0));
    assert_eq!(count("bel_in_force", "product_code=TERM_UK"), Some(2.0));
}

#[test]
fn a_series_measure_gives_one_row_per_t_or_one_total() {
    let m = model(TERM);
    let mut run = minimal_run_file();
    let mut each = agg(
        "claims_each",
        &["product_code"],
        "claims",
        AggregationOp::Sum,
    );
    each.over_t = Some(OverT::Each);
    let mut total = agg(
        "claims_total",
        &["product_code"],
        "claims",
        AggregationOp::Sum,
    );
    total.over_t = Some(OverT::Total);
    run.aggregations = vec![each, total];
    let r = runner(&m, &run);
    let p = r
        .project(&term_chunks(2), &SerialExecutor, &CancelFlag::new())
        .unwrap();

    let each_rows: Vec<_> = p
        .aggregates
        .iter()
        .filter(|r| r.aggregation == "claims_each")
        .collect();
    // T = 3, so four periods, two groups.
    assert_eq!(each_rows.len(), 8);
    assert!(each_rows.iter().all(|r| r.t >= 0));
    // Ascending `t` within a group.
    let uk: Vec<i32> = each_rows
        .iter()
        .filter(|r| r.group_key == "product_code=TERM_UK")
        .map(|r| r.t)
        .collect();
    assert_eq!(uk, vec![0, 1, 2, 3]);

    let total_rows: Vec<_> = p
        .aggregates
        .iter()
        .filter(|r| r.aggregation == "claims_total")
        .collect();
    assert_eq!(total_rows.len(), 2);
    assert!(total_rows.iter().all(|r| r.t == -1));

    // `total` is the sequential sum over `t` of the `each` rows for the same group.
    let mut want = 0.0;
    for row in each_rows
        .iter()
        .filter(|r| r.group_key == "product_code=TERM_UK")
    {
        want += row.value;
    }
    let got = total_rows
        .iter()
        .find(|r| r.group_key == "product_code=TERM_UK")
        .unwrap()
        .value;
    assert!((got - want).abs() < 1e-9);
}

#[test]
fn weighted_mean_needs_its_weight_and_uses_it() {
    let m = model(TERM);
    let mut run = minimal_run_file();
    let mut weighted = agg(
        "bel_per_policy",
        &["product_code"],
        "bel",
        AggregationOp::WeightedMean,
    );
    weighted.weight = Some("sum_assured".to_string());
    run.aggregations = vec![weighted];
    let r = runner(&m, &run);
    let p = r
        .project(&term_chunks(2), &SerialExecutor, &CancelFlag::new())
        .unwrap();

    let bels: Vec<f64> = p
        .per_mp("term.bel")
        .into_iter()
        .map(|(_, v)| v)
        .take(2)
        .collect();
    let want = (bels[0] * 100_000.0 + bels[1] * 250_000.0) / 350_000.0;
    let got = p
        .aggregate("bel_per_policy", Some("product_code=TERM_UK"))
        .unwrap();
    assert!((got - want).abs() < 1e-9, "{got} vs {want}");
}

#[test]
fn min_and_max_are_monoids_over_chunks() {
    let m = model(TERM);
    let mut run = minimal_run_file();
    run.aggregations = vec![
        agg("bel_min", &[], "bel", AggregationOp::Min),
        agg("bel_max", &[], "bel", AggregationOp::Max),
    ];
    let r = runner(&m, &run);
    let split = r
        .project(&term_chunks(1), &SerialExecutor, &CancelFlag::new())
        .unwrap();
    let whole = r
        .project(&term_chunks(1024), &SerialExecutor, &CancelFlag::new())
        .unwrap();
    assert_eq!(
        split.aggregate("bel_min", None).map(f64::to_bits),
        whole.aggregate("bel_min", None).map(f64::to_bits)
    );
    assert_eq!(
        split.aggregate("bel_max", None).map(f64::to_bits),
        whole.aggregate("bel_max", None).map(f64::to_bits)
    );
    assert!(split.aggregate("bel_min", None) < split.aggregate("bel_max", None));
}

#[test]
fn a_trapped_modelpoint_contributes_to_no_group() {
    // §9.3.1: a modelpoint dropped under `continue` is excluded from every aggregation.
    let m = model(TERM);
    let mut run = minimal_run_file();
    run.aggregations = vec![agg("n", &[], "bel", AggregationOp::Count)];
    let r = runner(&m, &run);
    let p = r
        .project(&term_chunks(2), &SerialExecutor, &CancelFlag::new())
        .unwrap();
    assert_eq!(p.aggregate("n", None), Some(4.0));
}
