//! Compiled lookup structures and their behaviour — `03-engine.md` §6.

mod common;

use common::{csv, decl, key, value};
use predictable_ir::{DType, KeyPolicy, LitValue, OnMissing};
use predictable_tables::{
    load_bytes, KeyArg, KeyIndexKind, LoadOptions, Outcome, TableError, CARTESIAN_LIMIT,
};

fn load(d: &predictable_ir::TableDecl, text: &str) -> predictable_tables::CompiledTable {
    load_bytes(d, csv(text), &LoadOptions::default()).expect("table should compile")
}

// ---------------------------------------------------------------- structures

#[test]
fn a_contiguous_integer_key_compiles_to_a_dense_vector() {
    let d = decl(
        "qx",
        vec![key("age", DType::I64, KeyPolicy::Exact)],
        vec![value("qx", DType::F64)],
    );
    let mut text = String::from("age,qx\n");
    for age in 20..=120 {
        text.push_str(&format!("{age},{}\n", age as f64 / 100_000.0));
    }
    let table = load(&d, &text);

    assert_eq!(table.stats().key_kinds, vec![KeyIndexKind::DenseInt]);
    assert_eq!(table.rows(), 101);
    assert_eq!(
        table.lookup_f64(&[KeyArg::Int(65)], 0),
        Outcome::Hit(0.00065)
    );
    // Outside the declared ages: `exact` means miss, and `error` means trap.
    assert!(matches!(
        table.lookup_f64(&[KeyArg::Int(19)], 0),
        Outcome::Trap(_)
    ));
}

#[test]
fn a_sparse_integer_key_compiles_to_a_perfect_hash() {
    let d = decl(
        "band",
        vec![key("sum_assured", DType::I64, KeyPolicy::Exact)],
        vec![value("factor", DType::F64)],
    );
    let keys: Vec<i64> = (0..400).map(|i| 1_000 + i * 7_919).collect();
    let mut text = String::from("sum_assured,factor\n");
    for (i, k) in keys.iter().enumerate() {
        text.push_str(&format!("{k},{}\n", i as f64));
    }
    let table = load(&d, &text);

    assert_eq!(table.stats().key_kinds, vec![KeyIndexKind::PerfectHash]);
    // Every key resolves to its own row, and nothing else does.
    for (i, k) in keys.iter().enumerate() {
        assert_eq!(
            table.lookup_f64(&[KeyArg::Int(*k)], 0),
            Outcome::Hit(i as f64),
            "key {k}"
        );
    }
    for probe in [0i64, 999, 1_001, -7, i64::MAX] {
        assert!(
            matches!(table.lookup_f64(&[KeyArg::Int(probe)], 0), Outcome::Trap(_)),
            "key {probe} should miss"
        );
    }
}

#[test]
fn the_dense_or_hash_choice_follows_density() {
    // 3 keys spread over 3001 slots: density 0.001, so a dense vector would be
    // 3000 holes. The compiler picks the hash.
    let d = decl(
        "sparse",
        vec![key("k", DType::I64, KeyPolicy::Exact)],
        vec![value("v", DType::F64)],
    );
    let table = load(&d, "k,v\n0,1.0\n1500,2.0\n3000,3.0\n");
    assert_eq!(table.stats().key_kinds, vec![KeyIndexKind::PerfectHash]);

    // 3 of 4 slots: density 0.75, dense wins.
    let table = load(&d, "k,v\n10,1.0\n11,2.0\n13,3.0\n");
    assert_eq!(table.stats().key_kinds, vec![KeyIndexKind::DenseInt]);
    assert_eq!(table.lookup_f64(&[KeyArg::Int(13)], 0), Outcome::Hit(3.0));
    assert!(matches!(
        table.lookup_f64(&[KeyArg::Int(12)], 0),
        Outcome::Trap(_)
    ));
}

#[test]
fn an_enum_key_compiles_to_a_dictionary_and_compares_by_equality() {
    let d = decl(
        "qx",
        vec![
            key("age", DType::I64, KeyPolicy::Exact),
            key("gender", DType::Enum("Gender".into()), KeyPolicy::Exact),
        ],
        vec![value("qx", DType::F64)],
    );
    let table = load(
        &d,
        "age,gender,qx\n40,M,0.0011\n40,F,0.0009\n41,M,0.0012\n41,F,0.0010\n",
    );
    assert_eq!(
        table.stats().key_kinds,
        vec![KeyIndexKind::DenseInt, KeyIndexKind::Dictionary]
    );
    assert_eq!(
        table.lookup_f64(&[KeyArg::Int(41), KeyArg::Str("F")], 0),
        Outcome::Hit(0.0010)
    );
    // Declaration order of an enum is presentational; an unknown member misses.
    assert!(matches!(
        table.lookup_f64(&[KeyArg::Int(41), KeyArg::Str("X")], 0),
        Outcome::Trap(_)
    ));
}

#[test]
fn the_mortality_shape_is_one_cartesian_dense_array() {
    let d = decl(
        "sa8990",
        vec![
            key("age", DType::I64, KeyPolicy::Clamp),
            key("gender", DType::Enum("Gender".into()), KeyPolicy::Exact),
            key("smoker", DType::Bool, KeyPolicy::Exact),
        ],
        vec![value("qx", DType::F64)],
    );
    let mut text = String::from("age,gender,smoker,qx\n");
    for age in 0..=120 {
        for gender in ["M", "F"] {
            for smoker in ["false", "true"] {
                let q = age as f64 / 1_000.0
                    + if gender == "M" { 0.0001 } else { 0.0 }
                    + if smoker == "true" { 0.001 } else { 0.0 };
                text.push_str(&format!("{age},{gender},{smoker},{q}\n"));
            }
        }
    }
    let table = load(&d, &text);
    let stats = table.stats();

    assert_eq!(stats.rows, 121 * 2 * 2);
    assert_eq!(stats.index, "cartesian dense array");
    assert_eq!(stats.cells, 121 * 2 * 2);
    assert_eq!(stats.fill, 1.0);
    assert!(stats.cells < CARTESIAN_LIMIT);

    let probe = [KeyArg::Int(65), KeyArg::Str("M"), KeyArg::Bool(true)];
    assert_eq!(
        table.lookup_f64(&probe, 0),
        Outcome::Hit(0.065 + 0.0001 + 0.001)
    );
    // `age` clamps, so 130 reads the top row rather than missing.
    assert_eq!(
        table.lookup_f64(&[KeyArg::Int(130), KeyArg::Str("M"), KeyArg::Bool(true)], 0),
        table.lookup_f64(&[KeyArg::Int(120), KeyArg::Str("M"), KeyArg::Bool(true)], 0)
    );
}

#[test]
fn a_ragged_grid_leaves_holes_that_still_miss() {
    let d = decl(
        "ragged",
        vec![
            key("a", DType::I64, KeyPolicy::Exact),
            key("b", DType::I64, KeyPolicy::Exact),
        ],
        vec![value("v", DType::F64)],
    );
    let table = load(&d, "a,b,v\n1,1,10.0\n1,2,20.0\n2,1,30.0\n");
    let stats = table.stats();
    assert_eq!(stats.index, "cartesian dense array");
    assert_eq!(stats.cells, 4);
    assert_eq!(stats.rows, 3);
    assert_eq!(
        table.lookup_f64(&[KeyArg::Int(2), KeyArg::Int(1)], 0),
        Outcome::Hit(30.0)
    );
    assert!(matches!(
        table.lookup_f64(&[KeyArg::Int(2), KeyArg::Int(2)], 0),
        Outcome::Trap(_)
    ));
}

#[test]
fn a_product_over_the_limit_falls_back_to_sorted_tuples() {
    // Three keys with 300 distinct values each: 2.7e7 cells, over 2^24, so the
    // dense array would be 108 MB for 300 rows. Sorted tuples instead.
    let d = decl(
        "wide",
        vec![
            key("a", DType::I64, KeyPolicy::Exact),
            key("b", DType::I64, KeyPolicy::Exact),
            key("c", DType::I64, KeyPolicy::Exact),
        ],
        vec![value("v", DType::F64)],
    );
    let mut text = String::from("a,b,c,v\n");
    for i in 0..300 {
        text.push_str(&format!("{i},{},{},{}\n", i + 1000, i + 5000, i as f64));
    }
    let table = load(&d, &text);
    assert_eq!(table.stats().index, "sorted tuples + binary search");
    assert_eq!(table.stats().cells, 300);

    for i in 0..300i64 {
        assert_eq!(
            table.lookup_f64(
                &[KeyArg::Int(i), KeyArg::Int(i + 1000), KeyArg::Int(i + 5000)],
                0
            ),
            Outcome::Hit(i as f64)
        );
    }
    // A tuple whose parts all exist but whose combination does not.
    assert!(matches!(
        table.lookup_f64(&[KeyArg::Int(0), KeyArg::Int(1001), KeyArg::Int(5000)], 0),
        Outcome::Trap(_)
    ));
}

// ------------------------------------------------------------------ policies

#[test]
fn clamp_holds_the_end_values() {
    let d = decl(
        "rates",
        vec![key("term", DType::I64, KeyPolicy::Clamp)],
        vec![value("rate", DType::F64)],
    );
    let table = load(&d, "term,rate\n5,0.02\n10,0.03\n20,0.04\n");
    assert_eq!(table.stats().key_kinds, vec![KeyIndexKind::SortedInt]);
    assert_eq!(table.lookup_f64(&[KeyArg::Int(1)], 0), Outcome::Hit(0.02));
    assert_eq!(table.lookup_f64(&[KeyArg::Int(10)], 0), Outcome::Hit(0.03));
    assert_eq!(table.lookup_f64(&[KeyArg::Int(99)], 0), Outcome::Hit(0.04));
    // Inside the range but off the grid is still a miss under `clamp`.
    assert!(matches!(
        table.lookup_f64(&[KeyArg::Int(12)], 0),
        Outcome::Trap(_)
    ));
}

#[test]
fn step_returns_the_predecessor() {
    let d = decl(
        "bands",
        vec![key("policy_year", DType::I64, KeyPolicy::Step)],
        vec![value("lapse", DType::F64)],
    );
    let table = load(&d, "policy_year,lapse\n1,0.12\n5,0.08\n10,0.05\n");
    assert_eq!(table.lookup_f64(&[KeyArg::Int(1)], 0), Outcome::Hit(0.12));
    assert_eq!(table.lookup_f64(&[KeyArg::Int(4)], 0), Outcome::Hit(0.12));
    assert_eq!(table.lookup_f64(&[KeyArg::Int(5)], 0), Outcome::Hit(0.08));
    assert_eq!(table.lookup_f64(&[KeyArg::Int(40)], 0), Outcome::Hit(0.05));
    assert!(matches!(
        table.lookup_f64(&[KeyArg::Int(0)], 0),
        Outcome::Trap(_)
    ));
}

#[test]
fn interpolate_lerps_inside_the_range_and_is_flat_outside_it() {
    let d = decl(
        "curve",
        vec![key("term", DType::F64, KeyPolicy::Interpolate)],
        vec![value("spot", DType::F64)],
    );
    let table = load(&d, "term,spot\n1.0,0.02\n2.0,0.03\n5.0,0.045\n");
    assert_eq!(table.stats().key_kinds, vec![KeyIndexKind::SortedFloat]);

    assert_eq!(
        table.lookup_f64(&[KeyArg::Float(1.0)], 0),
        Outcome::Hit(0.02)
    );
    assert_eq!(
        table.lookup_f64(&[KeyArg::Float(1.5)], 0),
        Outcome::Hit(0.025)
    );
    // 3.0 is a third of the way from 2.0 to 5.0.
    let Outcome::Hit(v) = table.lookup_f64(&[KeyArg::Float(3.0)], 0) else {
        panic!("expected a hit")
    };
    assert!((v - 0.035).abs() < 1e-12, "{v}");
    // Flat extrapolation: interpolation never misses inside or outside.
    assert_eq!(
        table.lookup_f64(&[KeyArg::Float(0.1)], 0),
        Outcome::Hit(0.02)
    );
    assert_eq!(
        table.lookup_f64(&[KeyArg::Float(9.0)], 0),
        Outcome::Hit(0.045)
    );
}

#[test]
fn on_missing_interpolate_names_the_key_that_blends() {
    let mut d = decl(
        "curve",
        vec![key("term", DType::F64, KeyPolicy::Clamp)],
        vec![value("spot", DType::F64)],
    );
    d.on_missing = OnMissing::Interpolate("term".into());
    let table = load(&d, "term,spot\n1.0,0.02\n2.0,0.03\n");
    assert_eq!(
        table.lookup_f64(&[KeyArg::Float(1.25)], 0),
        Outcome::Hit(0.0225)
    );

    d.on_missing = OnMissing::Interpolate("tenor".into());
    let err = load_bytes(&d, csv("term,spot\n1.0,0.02\n"), &LoadOptions::default()).unwrap_err();
    assert!(
        matches!(err, TableError::UnknownInterpolationKey { .. }),
        "{err}"
    );
}

#[test]
fn interpolation_composes_with_an_exact_key() {
    let d = decl(
        "curve",
        vec![
            key("currency", DType::Enum("Ccy".into()), KeyPolicy::Exact),
            key("term", DType::F64, KeyPolicy::Interpolate),
        ],
        vec![value("spot", DType::F64)],
    );
    let table = load(
        &d,
        "currency,term,spot\nGBP,1.0,0.02\nGBP,2.0,0.03\nUSD,1.0,0.04\nUSD,2.0,0.05\n",
    );
    assert_eq!(
        table.lookup_f64(&[KeyArg::Str("USD"), KeyArg::Float(1.5)], 0),
        Outcome::Hit(0.045)
    );
    assert!(matches!(
        table.lookup_f64(&[KeyArg::Str("EUR"), KeyArg::Float(1.5)], 0),
        Outcome::Trap(_)
    ));
}

// ---------------------------------------------------------------- on_missing

#[test]
fn on_missing_error_traps_and_names_the_keys() {
    let d = decl(
        "qx",
        vec![
            key("age", DType::I64, KeyPolicy::Exact),
            key("smoker", DType::Bool, KeyPolicy::Exact),
        ],
        vec![value("qx", DType::F64)],
    );
    let table = load(&d, "age,smoker,qx\n40,false,0.001\n40,true,0.003\n");
    assert!(!table.has_presence_bit());

    let Outcome::Trap(miss) = table.lookup_f64(&[KeyArg::Int(41), KeyArg::Bool(true)], 0) else {
        panic!("expected a trap")
    };
    assert_eq!(miss.table, "qx");
    assert_eq!(miss.keys, vec!["41", "true"]);
    assert_eq!(miss.to_string(), "no row in table `qx` for (41, true)");

    let diag = predictable_tables::lookup_miss_trap(&miss);
    assert_eq!(diag.code, "E0902");
    assert!(diag.message.contains("no row in table `qx`"));
}

#[test]
fn on_missing_default_substitutes_and_lights_the_presence_bit() {
    let mut d = decl(
        "loading",
        vec![key("occupation", DType::Str, KeyPolicy::Exact)],
        vec![value("factor", DType::F64)],
    );
    d.on_missing = OnMissing::Default(LitValue::Float(1.0));
    let table = load(&d, "occupation,factor\nMINER,2.5\nPILOT,1.75\n");

    assert!(table.has_presence_bit());
    let hit = table.lookup_f64(&[KeyArg::Str("MINER")], 0);
    assert_eq!(hit, Outcome::Hit(2.5));
    assert!(hit.is_hit());

    let missed = table.lookup_f64(&[KeyArg::Str("CLERK")], 0);
    assert_eq!(missed, Outcome::Substituted(1.0));
    assert!(!missed.is_hit());

    // The presence bit `is_null` observes.
    assert!(table.contains(&[KeyArg::Str("PILOT")]));
    assert!(!table.contains(&[KeyArg::Str("CLERK")]));
}

#[test]
fn a_default_that_does_not_fit_the_value_dtype_is_refused() {
    let mut d = decl(
        "counts",
        vec![key("k", DType::I64, KeyPolicy::Exact)],
        vec![value("n", DType::I64)],
    );
    d.on_missing = OnMissing::Default(LitValue::Float(0.5));
    let err = load_bytes(&d, csv("k,n\n1,10\n"), &LoadOptions::default()).unwrap_err();
    assert!(matches!(err, TableError::DefaultDtype { .. }), "{err}");
}

#[test]
fn non_float_value_columns_round_trip() {
    let d = decl(
        "meta",
        vec![key("code", DType::Str, KeyPolicy::Exact)],
        vec![
            value("count", DType::I64),
            value("active", DType::Bool),
            value("label", DType::Str),
        ],
    );
    let table = load(
        &d,
        "code,count,active,label\nA,3,true,alpha\nB,7,false,beta\n",
    );
    assert_eq!(table.value_position("label"), Some(2));
    assert_eq!(table.lookup_i64(&[KeyArg::Str("B")], 0), Outcome::Hit(7));
    assert_eq!(
        table.lookup_bool(&[KeyArg::Str("A")], 1),
        Outcome::Hit(true)
    );
    assert_eq!(
        table.lookup_str(&[KeyArg::Str("B")], 2),
        Outcome::Hit("beta")
    );
    // `on_missing = "error"`, so a miss traps whatever the value dtype is.
    assert!(matches!(
        table.lookup_str(&[KeyArg::Str("Z")], 2),
        Outcome::Trap(_)
    ));

    // A default is one literal for the whole table, so it substitutes into
    // every value column — and a single-dtype table is where that fits.
    let mut counts = decl(
        "counts",
        vec![key("code", DType::Str, KeyPolicy::Exact)],
        vec![value("count", DType::I64)],
    );
    counts.on_missing = OnMissing::Default(LitValue::Int(0));
    let counts = load(&counts, "code,count\nA,3\n");
    assert_eq!(
        counts.lookup_i64(&[KeyArg::Str("Z")], 0),
        Outcome::Substituted(0)
    );
}

// -------------------------------------------------------- content validation

#[test]
fn duplicate_key_tuples_name_both_rows() {
    let d = decl(
        "qx",
        vec![key("age", DType::I64, KeyPolicy::Exact)],
        vec![value("qx", DType::F64)],
    );
    let err = load_bytes(
        &d,
        csv("age,qx\n40,0.001\n41,0.002\n40,0.003\n"),
        &LoadOptions::default(),
    )
    .unwrap_err();
    let TableError::DuplicateKey {
        first,
        second,
        tuple,
        ..
    } = &err
    else {
        panic!("expected a duplicate, got {err}")
    };
    assert_eq!((*first, *second), (1, 3));
    assert_eq!(tuple, "40");
}

#[test]
fn a_missing_declared_column_names_what_was_present() {
    let d = decl(
        "qx",
        vec![key("age", DType::I64, KeyPolicy::Exact)],
        vec![value("qx", DType::F64)],
    );
    let err = load_bytes(
        &d,
        csv("age,mortality\n40,0.001\n"),
        &LoadOptions::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("column `qx` is declared"));
    assert!(err.to_string().contains("age, mortality"));
}

#[test]
fn a_bad_cell_names_the_row_and_the_column() {
    let d = decl(
        "qx",
        vec![key("age", DType::I64, KeyPolicy::Exact)],
        vec![value("qx", DType::F64)],
    );
    let err = load_bytes(
        &d,
        csv("age,qx\n40,0.001\n41,not-a-number\n"),
        &LoadOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(err, TableError::BadCell { .. }), "{err}");
    assert!(err.to_string().contains("row 2, column `qx`"), "{err}");
    assert!(err.to_string().contains("not a valid f64"), "{err}");
}

#[test]
fn empty_and_headerless_sources_are_refused() {
    let d = decl(
        "qx",
        vec![key("age", DType::I64, KeyPolicy::Exact)],
        vec![value("qx", DType::F64)],
    );
    let err = load_bytes(&d, csv("age,qx\n"), &LoadOptions::default()).unwrap_err();
    assert!(matches!(err, TableError::Empty { .. }), "{err}");
    let err = load_bytes(&d, csv(""), &LoadOptions::default()).unwrap_err();
    assert!(matches!(err, TableError::NoHeader { .. }), "{err}");
}

#[test]
fn the_csv_reader_handles_quotes_comments_and_blank_lines() {
    let d = decl(
        "loading",
        vec![key("occupation", DType::Str, KeyPolicy::Exact)],
        vec![value("factor", DType::F64)],
    );
    let table = load(
        &d,
        "# sourced from the 2024 review\noccupation,factor\n\"MINER, DEEP\",2.5\n\nPILOT,1.75\n",
    );
    assert_eq!(table.rows(), 2);
    assert_eq!(
        table.lookup_f64(&[KeyArg::Str("MINER, DEEP")], 0),
        Outcome::Hit(2.5)
    );
}

#[test]
fn extra_source_columns_are_ignored() {
    let d = decl(
        "qx",
        vec![key("age", DType::I64, KeyPolicy::Exact)],
        vec![value("qx", DType::F64)],
    );
    let table = load(&d, "note,age,qx,unused\nx,40,0.001,9\ny,41,0.002,9\n");
    assert_eq!(table.rows(), 2);
    assert_eq!(table.lookup_f64(&[KeyArg::Int(41)], 0), Outcome::Hit(0.002));
}

// ------------------------------------------------------- declaration guards

#[test]
fn unordered_keys_support_exact_only() {
    for dtype in [DType::Enum("Gender".into()), DType::Str, DType::Bool] {
        for policy in [KeyPolicy::Clamp, KeyPolicy::Step, KeyPolicy::Interpolate] {
            let d = decl(
                "t",
                vec![key("k", dtype.clone(), policy)],
                vec![value("v", DType::F64)],
            );
            let err = load_bytes(&d, csv("k,v\nA,1.0\n"), &LoadOptions::default()).unwrap_err();
            assert!(
                matches!(err, TableError::UnorderedKeyPolicy { .. }),
                "{dtype} / {policy:?}: {err}"
            );
        }
    }
}

#[test]
fn there_are_no_float_keys_in_the_index_path() {
    let d = decl(
        "t",
        vec![key("x", DType::F64, KeyPolicy::Exact)],
        vec![value("v", DType::F64)],
    );
    let err = load_bytes(&d, csv("x,v\n1.5,1.0\n"), &LoadOptions::default()).unwrap_err();
    assert!(matches!(err, TableError::FloatExactKey { .. }), "{err}");
    assert!(err.to_string().contains("clamp"));
}

#[test]
fn at_most_one_key_may_interpolate() {
    let d = decl(
        "t",
        vec![
            key("x", DType::F64, KeyPolicy::Interpolate),
            key("y", DType::F64, KeyPolicy::Interpolate),
        ],
        vec![value("v", DType::F64)],
    );
    let err = load_bytes(&d, csv("x,y,v\n1.0,1.0,1.0\n"), &LoadOptions::default()).unwrap_err();
    assert!(
        matches!(err, TableError::MultipleInterpolatedKeys { .. }),
        "{err}"
    );
}

#[test]
fn only_f64_values_can_be_interpolated() {
    let d = decl(
        "t",
        vec![key("x", DType::F64, KeyPolicy::Interpolate)],
        vec![value("n", DType::I64)],
    );
    let err = load_bytes(&d, csv("x,n\n1.0,1\n"), &LoadOptions::default()).unwrap_err();
    assert!(
        matches!(err, TableError::NonNumericInterpolation { .. }),
        "{err}"
    );
}

#[test]
fn arity_mismatch_at_lookup_is_a_miss_not_a_panic() {
    let d = decl(
        "qx",
        vec![key("age", DType::I64, KeyPolicy::Exact)],
        vec![value("qx", DType::F64)],
    );
    let table = load(&d, "age,qx\n40,0.001\n");
    assert!(matches!(
        table.lookup_f64(&[KeyArg::Int(40), KeyArg::Int(1)], 0),
        Outcome::Trap(_)
    ));
    assert!(matches!(table.lookup_f64(&[], 0), Outcome::Trap(_)));
}

#[test]
fn compilation_is_deterministic() {
    // Same content, different row order in the file: the compiled answers are
    // identical, because the domains are sorted and the index is built from them.
    let d = decl(
        "qx",
        vec![key("age", DType::I64, KeyPolicy::Exact)],
        vec![value("qx", DType::F64)],
    );
    let a = load(&d, "age,qx\n40,0.001\n41,0.002\n42,0.003\n");
    let b = load(&d, "age,qx\n42,0.003\n40,0.001\n41,0.002\n");
    for age in 39..44 {
        assert_eq!(
            a.lookup_f64(&[KeyArg::Int(age)], 0),
            b.lookup_f64(&[KeyArg::Int(age)], 0),
            "age {age}"
        );
    }
    assert_eq!(a.stats().key_kinds, b.stats().key_kinds);
    // The digest is over the bytes, so the two files are *not* the same table.
    assert_ne!(a.digest, b.digest);
}
