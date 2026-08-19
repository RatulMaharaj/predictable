//! Behavioural tests for the inbound half of `predictable-io`.
//!
//! Everything here is asserted against the specs rather than against the implementation:
//! `01-ir.md` §2.10 (the schema is the IR's), §2.11 / ruling Q3 (no nulls at runtime), and
//! `03-engine.md` §6 (column pruning, deterministic chunking).

#![allow(clippy::result_large_err)]

use std::sync::Arc;

use arrow_array::{
    ArrayRef, BooleanArray, Date32Array, Float64Array, Int64Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use predictable_io::inbound::BatchReader;
use predictable_io::{
    ArrowSource, ChunkColumns, CsvSource, IoError, Lint, ModelpointSource, MpSchema, ParquetSource,
};
use predictable_ir::{
    BinaryOp, Component, DType, EnumDecl, Expr, LitValue, ModelpointField, Module, Shape, Unit,
};

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn field(name: &str, dtype: DType, required: bool, default: Option<LitValue>) -> ModelpointField {
    ModelpointField {
        name: name.to_string(),
        dtype,
        unit: Unit::None,
        required,
        key: false,
        default_value: default,
        doc: None,
    }
}

fn key_field(name: &str) -> ModelpointField {
    let mut f = field(name, DType::Str, true, None);
    f.key = true;
    f
}

fn derived(name: &str, expr: Expr) -> Component {
    Component::derived(name, DType::F64, Shape::Series, expr)
}

fn mul(a: &str, b: &str) -> Expr {
    Expr::binary(BinaryOp::Mul, Expr::r#ref(a), Expr::r#ref(b))
}

/// A schema with six declared fields of which the model reads two (plus the key), one enum, and
/// one optional field observed by `coalesce`.
fn module() -> Module {
    let mut m = Module::new("term");
    m.enums.push(EnumDecl {
        name: "Gender".to_string(),
        values: vec!["male".to_string(), "female".to_string()],
    });
    m.modelpoint_fields = vec![
        key_field("policy_id"),
        field("sum_assured", DType::F64, true, None),
        field("entry_age", DType::I64, true, None),
        field(
            "smoker_loading",
            DType::F64,
            false,
            Some(LitValue::Float(1.0)),
        ),
        field(
            "gender",
            DType::Enum("Gender".to_string()),
            false,
            Some(LitValue::Text("female".to_string())),
        ),
        field("in_force", DType::Bool, false, Some(LitValue::Bool(true))),
        field(
            "entry_date",
            DType::Date,
            false,
            Some(LitValue::Text("2000-01-01".to_string())),
        ),
        field(
            "admin_note",
            DType::Str,
            false,
            Some(LitValue::Text(String::new())),
        ),
    ];
    m.components
        .push(derived("claims", mul("sum_assured", "x")));
    m.components.push(derived(
        "loading",
        Expr::Call {
            func: "coalesce".to_string(),
            args: vec![Expr::r#ref("smoker_loading"), Expr::f64(1.0)],
        },
    ));
    m
}

fn full_schema() -> MpSchema {
    MpSchema::from_module(&module()).unwrap()
}

fn pruned_schema() -> MpSchema {
    MpSchema::pruned_for_module(&module()).unwrap()
}

// ---------------------------------------------------------------------------
// schema: the plan comes from the IR (§2.10)
// ---------------------------------------------------------------------------

#[test]
fn pruning_keeps_only_the_fields_the_model_reads_plus_the_key() {
    let schema = pruned_schema();
    let names: Vec<&str> = schema.column_names();
    assert_eq!(names, vec!["policy_id", "sum_assured", "smoker_loading"]);
    assert_eq!(full_schema().len(), 8);
    assert!(schema.is_pruned("gender"));
    assert!(!schema.is_pruned("sum_assured"));
    assert_eq!(schema.key_field().name, "policy_id");
}

#[test]
fn pruning_records_a_lint_per_dropped_column() {
    let lints = pruned_schema().pruned_lints("mp.parquet");
    let dropped: Vec<String> = lints
        .iter()
        .map(|l| match l {
            Lint::PrunedColumn { column, .. } => column.clone(),
            other => panic!("unexpected lint: {other}"),
        })
        .collect();
    assert_eq!(
        dropped,
        vec![
            "entry_age",
            "gender",
            "in_force",
            "entry_date",
            "admin_note"
        ]
    );
}

#[test]
fn a_presence_lane_exists_only_where_the_model_asks_for_one() {
    let schema = pruned_schema();
    // `coalesce(smoker_loading, 1.0)` observes a presence bit (§2.11)...
    assert!(schema.field("smoker_loading").unwrap().presence);
    // ...and nothing else does, so nothing else pays for one.
    assert!(!schema.field("sum_assured").unwrap().presence);
    assert!(!full_schema().field("gender").unwrap().presence);
}

#[test]
fn referenced_by_names_the_components_for_the_error_message() {
    let schema = pruned_schema();
    assert_eq!(
        schema.field("sum_assured").unwrap().referenced_by,
        vec!["claims".to_string()]
    );
}

#[test]
fn enum_fields_carry_their_declared_dictionary() {
    let schema = full_schema();
    assert_eq!(
        schema.field("gender").unwrap().variants,
        vec!["male".to_string(), "female".to_string()]
    );
}

#[test]
fn an_optional_field_without_a_default_is_refused() {
    let mut m = module();
    m.modelpoint_fields[3].default_value = None;
    match MpSchema::from_module(&m) {
        Err(IoError::OptionalWithoutDefault { column }) => assert_eq!(column, "smoker_loading"),
        other => panic!("expected OptionalWithoutDefault, got {other:?}"),
    }
}

#[test]
fn an_undeclared_enum_is_refused() {
    let mut m = module();
    m.enums.clear();
    assert!(matches!(
        MpSchema::from_module(&m),
        Err(IoError::UnknownEnum { .. })
    ));
}

#[test]
fn the_schema_needs_exactly_one_key_field() {
    let mut m = module();
    m.modelpoint_fields[0].key = false;
    assert!(matches!(
        MpSchema::from_module(&m),
        Err(IoError::NoKeyField)
    ));

    let mut m = module();
    m.modelpoint_fields[1].key = true;
    assert!(matches!(
        MpSchema::from_module(&m),
        Err(IoError::MultipleKeyFields(_))
    ));
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

fn csv_source(schema: MpSchema, text: &'static str) -> Result<CsvSource, IoError> {
    CsvSource::from_reader(schema, Box::new(text.as_bytes()), "mp.csv")
}

#[test]
fn csv_eliminates_nulls_at_load_and_records_the_presence_bit() {
    let schema = pruned_schema();
    let mut src = csv_source(
        schema.clone(),
        "policy_id,sum_assured,smoker_loading\nP1,100000,1.5\nP2,50000,\nP3,25000,NA\n",
    )
    .unwrap();
    let mut chunk = ChunkColumns::for_schema(&schema);
    assert_eq!(src.next_chunk(16, &mut chunk).unwrap(), 3);

    assert_eq!(
        chunk.column("smoker_loading").unwrap().as_f64().unwrap(),
        &[1.5, 1.0, 1.0]
    );
    assert_eq!(
        chunk.presence("smoker_loading").unwrap(),
        &[true, false, false]
    );
    assert!(chunk.presence("sum_assured").is_none());
    assert_eq!(
        chunk.column("policy_id").unwrap().as_str().unwrap(),
        &["P1".to_string(), "P2".to_string(), "P3".to_string()]
    );
}

#[test]
fn csv_missing_required_column_names_file_column_and_readers() {
    let err = csv_source(pruned_schema(), "policy_id,smoker_loading\nP1,1.0\n").unwrap_err();
    let text = err.to_string();
    assert!(matches!(err, IoError::MissingColumn { .. }), "{text}");
    assert!(text.contains("mp.csv"), "{text}");
    assert!(text.contains("sum_assured"), "{text}");
    assert!(text.contains("claims"), "{text}");
}

#[test]
fn csv_null_in_a_required_column_is_an_error_with_the_row() {
    let mut src = csv_source(
        pruned_schema(),
        "policy_id,sum_assured,smoker_loading\nP1,100000,1.0\nP2,,1.0\n",
    )
    .unwrap();
    let schema = pruned_schema();
    let mut chunk = ChunkColumns::for_schema(&schema);
    match src.next_chunk(16, &mut chunk) {
        Err(IoError::NullInRequired { column, row, .. }) => {
            assert_eq!(column, "sum_assured");
            assert_eq!(row, 2);
        }
        other => panic!("expected NullInRequired, got {other:?}"),
    }
}

#[test]
fn csv_an_absent_optional_column_becomes_all_defaults() {
    let schema = pruned_schema();
    let mut src = csv_source(schema.clone(), "policy_id,sum_assured\nP1,10\nP2,20\n").unwrap();
    let mut chunk = ChunkColumns::for_schema(&schema);
    assert_eq!(src.next_chunk(16, &mut chunk).unwrap(), 2);
    assert_eq!(
        chunk.column("smoker_loading").unwrap().as_f64().unwrap(),
        &[1.0, 1.0]
    );
    assert_eq!(chunk.presence("smoker_loading").unwrap(), &[false, false]);
}

#[test]
fn csv_an_undeclared_extra_column_is_accepted_with_a_lint() {
    let src = csv_source(
        pruned_schema(),
        "policy_id,sum_assured,smoker_loading,legacy_code\nP1,10,1.0,ZZ\n",
    )
    .unwrap();
    assert!(src.lints().iter().any(|l| matches!(
        l,
        Lint::UnknownColumn { column, file } if column == "legacy_code" && file == "mp.csv"
    )));
}

#[test]
fn csv_a_pruned_column_is_not_reported_as_unknown() {
    let src = csv_source(
        pruned_schema(),
        "policy_id,sum_assured,smoker_loading,gender\nP1,10,1.0,male\n",
    )
    .unwrap();
    assert!(!src
        .lints()
        .iter()
        .any(|l| matches!(l, Lint::UnknownColumn { column, .. } if column == "gender")));
}

#[test]
fn csv_reads_every_dtype() {
    let schema = full_schema();
    let mut src = csv_source(
        schema.clone(),
        "policy_id,sum_assured,entry_age,smoker_loading,gender,in_force,entry_date,admin_note\n\
         P1,100000,42,1.25,male,yes,1982-03-04,ok\n",
    )
    .unwrap();
    let mut chunk = ChunkColumns::for_schema(&schema);
    assert_eq!(src.next_chunk(4, &mut chunk).unwrap(), 1);
    assert_eq!(chunk.column("entry_age").unwrap().as_i64().unwrap(), &[42]);
    assert_eq!(chunk.column("gender").unwrap().as_enum().unwrap(), &[0]);
    assert_eq!(
        chunk.column("in_force").unwrap().as_bool().unwrap(),
        &[true]
    );
    // 1982-03-04 is 4445 days after the Unix epoch.
    assert_eq!(
        chunk.column("entry_date").unwrap().as_date().unwrap(),
        &[4445]
    );
    assert_eq!(
        chunk.column("admin_note").unwrap().as_str().unwrap(),
        &["ok".to_string()]
    );
}

#[test]
fn csv_rejects_a_cell_that_is_not_its_dtype() {
    let schema = pruned_schema();
    let mut src = csv_source(
        schema.clone(),
        "policy_id,sum_assured,smoker_loading\nP1,not-a-number,1.0\n",
    )
    .unwrap();
    let mut chunk = ChunkColumns::for_schema(&schema);
    match src.next_chunk(4, &mut chunk) {
        Err(IoError::BadValue {
            column, row, text, ..
        }) => {
            assert_eq!(
                (column.as_str(), row, text.as_str()),
                ("sum_assured", 1, "not-a-number")
            );
        }
        other => panic!("expected BadValue, got {other:?}"),
    }
}

#[test]
fn csv_rejects_an_undeclared_enum_variant() {
    let schema = full_schema();
    let mut src = csv_source(
        schema.clone(),
        "policy_id,sum_assured,entry_age,gender\nP1,10,42,unspecified\n",
    )
    .unwrap();
    let mut chunk = ChunkColumns::for_schema(&schema);
    match src.next_chunk(4, &mut chunk) {
        Err(e @ IoError::UnknownVariant { .. }) => {
            let text = e.to_string();
            assert!(text.contains("unspecified"), "{text}");
            assert!(text.contains("male, female"), "{text}");
        }
        other => panic!("expected UnknownVariant, got {other:?}"),
    }
}

#[test]
fn csv_duplicate_column_is_refused() {
    let err = csv_source(
        pruned_schema(),
        "policy_id,sum_assured,sum_assured\nP1,10,20\n",
    )
    .unwrap_err();
    assert!(matches!(err, IoError::DuplicateColumn { .. }));
}

// ---------------------------------------------------------------------------
// chunking is a function of C and the row count only
// ---------------------------------------------------------------------------

const TEN_ROWS: &str = "policy_id,sum_assured,smoker_loading\n\
    P0,0,1.0\nP1,1,1.0\nP2,2,1.0\nP3,3,1.0\nP4,4,1.0\n\
    P5,5,1.0\nP6,6,1.0\nP7,7,1.0\nP8,8,1.0\nP9,9,1.0\n";

fn drain(source: &mut dyn ModelpointSource, n: usize) -> Vec<(u64, u64, Vec<f64>)> {
    let schema = source.schema().clone();
    let mut chunk = ChunkColumns::for_schema(&schema);
    let mut out = Vec::new();
    loop {
        let rows = source.next_chunk(n, &mut chunk).unwrap();
        if rows == 0 {
            break;
        }
        out.push((
            chunk.chunk_idx(),
            chunk.first_row(),
            chunk
                .column("sum_assured")
                .unwrap()
                .as_f64()
                .unwrap()
                .to_vec(),
        ));
    }
    out
}

#[test]
fn chunk_size_changes_the_cuts_but_never_the_rows_or_their_order() {
    let mut a = csv_source(pruned_schema(), TEN_ROWS).unwrap();
    let mut b = csv_source(pruned_schema(), TEN_ROWS).unwrap();
    let mut c = csv_source(pruned_schema(), TEN_ROWS).unwrap();

    let by_1 = drain(&mut a, 1);
    let by_4 = drain(&mut b, 4);
    let by_1024 = drain(&mut c, 1024);

    let flat = |v: &[(u64, u64, Vec<f64>)]| -> Vec<f64> {
        v.iter().flat_map(|(_, _, xs)| xs.clone()).collect()
    };
    let expected: Vec<f64> = (0..10).map(|i| i as f64).collect();
    assert_eq!(flat(&by_1), expected);
    assert_eq!(flat(&by_4), expected);
    assert_eq!(flat(&by_1024), expected);

    assert_eq!(by_1.len(), 10);
    assert_eq!(by_1024.len(), 1);
    // Chunk indices and first rows are the writer's ordering key (IR §9.1).
    assert_eq!(
        by_4.iter().map(|(i, r, _)| (*i, *r)).collect::<Vec<_>>(),
        vec![(0, 0), (1, 4), (2, 8)]
    );
    assert_eq!(by_4[2].2, vec![8.0, 9.0]);
}

#[test]
fn a_chunk_buffer_from_a_different_schema_is_refused() {
    let mut src = csv_source(pruned_schema(), TEN_ROWS).unwrap();
    let mut wrong = ChunkColumns::for_schema(&full_schema());
    assert!(matches!(
        src.next_chunk(4, &mut wrong),
        Err(IoError::Backend { .. })
    ));
}

// ---------------------------------------------------------------------------
// Arrow
// ---------------------------------------------------------------------------

fn arrow_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("policy_id", DataType::Utf8, false),
        Field::new("sum_assured", DataType::Float64, false),
        Field::new("entry_age", DataType::Int64, false),
        Field::new("smoker_loading", DataType::Float64, true),
        Field::new("gender", DataType::Utf8, true),
        Field::new("in_force", DataType::Boolean, true),
        Field::new("entry_date", DataType::Date32, true),
        Field::new("admin_note", DataType::Utf8, true),
    ]))
}

fn batch(ids: &[&str], sums: &[f64], loadings: &[Option<f64>]) -> RecordBatch {
    let n = ids.len();
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(ids.to_vec())),
        Arc::new(Float64Array::from(sums.to_vec())),
        Arc::new(Int64Array::from(vec![40i64; n])),
        Arc::new(Float64Array::from(loadings.to_vec())),
        Arc::new(StringArray::from(vec![Some("male"); n])),
        Arc::new(BooleanArray::from(vec![Some(true); n])),
        Arc::new(Date32Array::from(vec![Some(4445i32); n])),
        Arc::new(StringArray::from(vec![Some("note"); n])),
    ];
    RecordBatch::try_new(arrow_schema(), columns).unwrap()
}

#[test]
fn arrow_batches_are_recut_across_their_own_boundaries() {
    let batches = vec![
        batch(&["P0", "P1", "P2"], &[0.0, 1.0, 2.0], &[Some(1.0); 3]),
        batch(&["P3", "P4"], &[3.0, 4.0], &[Some(1.0); 2]),
        batch(&["P5"], &[5.0], &[None]),
    ];
    let schema = pruned_schema();
    let reader = BatchReader::new(arrow_schema(), batches);
    let mut src = ArrowSource::new(schema.clone(), Box::new(reader), "handover.arrow").unwrap();

    // C = 4 straddles all three incoming batches.
    let rows = drain(&mut src, 4);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].2, vec![0.0, 1.0, 2.0, 3.0]);
    assert_eq!(rows[1].2, vec![4.0, 5.0]);
    assert_eq!(rows[1].1, 4);
}

#[test]
fn arrow_nulls_are_replaced_by_the_declared_default() {
    let schema = pruned_schema();
    let reader = BatchReader::new(
        arrow_schema(),
        vec![batch(&["P0", "P1"], &[0.0, 1.0], &[None, Some(2.0)])],
    );
    let mut src = ArrowSource::new(schema.clone(), Box::new(reader), "handover.arrow").unwrap();
    let mut chunk = ChunkColumns::for_schema(&schema);
    src.next_chunk(8, &mut chunk).unwrap();
    assert_eq!(
        chunk.column("smoker_loading").unwrap().as_f64().unwrap(),
        &[1.0, 2.0]
    );
    assert_eq!(chunk.presence("smoker_loading").unwrap(), &[false, true]);
}

#[test]
fn arrow_type_mismatch_is_caught_before_row_one() {
    let bad = Arc::new(Schema::new(vec![
        Field::new("policy_id", DataType::Utf8, false),
        Field::new("sum_assured", DataType::Utf8, false),
        Field::new("smoker_loading", DataType::Float64, true),
    ]));
    let empty = RecordBatch::new_empty(Arc::clone(&bad));
    let reader = BatchReader::new(bad, vec![empty]);
    match ArrowSource::new(pruned_schema(), Box::new(reader), "handover.arrow") {
        Err(IoError::TypeMismatch { column, found, .. }) => {
            assert_eq!(column, "sum_assured");
            assert_eq!(found, "Utf8");
        }
        other => panic!("expected TypeMismatch, got {other:?}"),
    }
}

#[test]
fn arrow_widens_int_columns_into_f64_lanes() {
    let schema_in = Arc::new(Schema::new(vec![
        Field::new("policy_id", DataType::Utf8, false),
        Field::new("sum_assured", DataType::Int64, false),
        Field::new("smoker_loading", DataType::Float64, true),
    ]));
    let b = RecordBatch::try_new(
        Arc::clone(&schema_in),
        vec![
            Arc::new(StringArray::from(vec!["P0"])),
            Arc::new(Int64Array::from(vec![7i64])),
            Arc::new(Float64Array::from(vec![Some(1.0)])),
        ],
    )
    .unwrap();
    let schema = pruned_schema();
    let reader = BatchReader::new(schema_in, vec![b]);
    let mut src = ArrowSource::new(schema.clone(), Box::new(reader), "handover.arrow").unwrap();
    let mut chunk = ChunkColumns::for_schema(&schema);
    src.next_chunk(4, &mut chunk).unwrap();
    assert_eq!(
        chunk.column("sum_assured").unwrap().as_f64().unwrap(),
        &[7.0]
    );
}

// ---------------------------------------------------------------------------
// Parquet
// ---------------------------------------------------------------------------

fn write_parquet(batches: &[RecordBatch]) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new()
        .suffix(".parquet")
        .tempfile()
        .unwrap();
    let mut writer =
        parquet::arrow::ArrowWriter::try_new(file.reopen().unwrap(), batches[0].schema(), None)
            .unwrap();
    for b in batches {
        writer.write(b).unwrap();
    }
    writer.close().unwrap();
    file
}

#[test]
fn parquet_reads_only_the_projected_columns() {
    let file = write_parquet(&[batch(&["P0", "P1"], &[10.0, 20.0], &[Some(1.5), None])]);
    let schema = pruned_schema();
    let mut src = ParquetSource::open(schema.clone(), file.path()).unwrap();

    // The load plan — and therefore the Parquet projection — is 3 of the file's 8 columns.
    assert_eq!(src.schema().len(), 3);
    let mut chunk = ChunkColumns::for_schema(&schema);
    assert_eq!(src.next_chunk(8, &mut chunk).unwrap(), 2);
    assert_eq!(chunk.columns().len(), 3);
    assert!(chunk.column("gender").is_none());
    assert_eq!(
        chunk.column("sum_assured").unwrap().as_f64().unwrap(),
        &[10.0, 20.0]
    );
    // Null elimination happens identically whatever the backend (§2.11).
    assert_eq!(
        chunk.column("smoker_loading").unwrap().as_f64().unwrap(),
        &[1.5, 1.0]
    );
    assert_eq!(chunk.presence("smoker_loading").unwrap(), &[true, false]);
    // Declared-but-unread columns are pruned, not "unknown".
    assert!(src
        .lints()
        .iter()
        .any(|l| matches!(l, Lint::PrunedColumn { column, .. } if column == "gender")));
    assert!(!src
        .lints()
        .iter()
        .any(|l| matches!(l, Lint::UnknownColumn { .. })));
}

#[test]
fn parquet_row_groups_do_not_leak_into_chunk_boundaries() {
    let file = write_parquet(&[
        batch(&["P0", "P1", "P2"], &[0.0, 1.0, 2.0], &[Some(1.0); 3]),
        batch(&["P3", "P4"], &[3.0, 4.0], &[Some(1.0); 2]),
    ]);
    let mut src = ParquetSource::open(pruned_schema(), file.path()).unwrap();
    let chunks = drain(&mut src, 2);
    assert_eq!(
        chunks.iter().map(|(i, r, _)| (*i, *r)).collect::<Vec<_>>(),
        vec![(0, 0), (1, 2), (2, 4)]
    );
    assert_eq!(chunks[1].2, vec![2.0, 3.0]);
}

#[test]
fn parquet_missing_required_column_fails_before_the_first_row_group() {
    let slim = Arc::new(Schema::new(vec![
        Field::new("policy_id", DataType::Utf8, false),
        Field::new("smoker_loading", DataType::Float64, true),
    ]));
    let b = RecordBatch::try_new(
        Arc::clone(&slim),
        vec![
            Arc::new(StringArray::from(vec!["P0"])),
            Arc::new(Float64Array::from(vec![Some(1.0)])),
        ],
    )
    .unwrap();
    let file = write_parquet(&[b]);
    let err = ParquetSource::open(pruned_schema(), file.path()).unwrap_err();
    let text = err.to_string();
    assert!(matches!(err, IoError::MissingColumn { .. }), "{text}");
    assert!(
        text.contains("sum_assured") && text.contains("claims"),
        "{text}"
    );
}

#[test]
fn every_source_agrees_row_for_row() {
    let schema = pruned_schema();
    let rows = batch(
        &["P0", "P1", "P2"],
        &[1.0, 2.0, 3.0],
        &[Some(0.5), None, Some(1.5)],
    );
    let file = write_parquet(std::slice::from_ref(&rows));

    let mut parquet = ParquetSource::open(schema.clone(), file.path()).unwrap();
    let mut arrow = ArrowSource::new(
        schema.clone(),
        Box::new(BatchReader::new(arrow_schema(), vec![rows])),
        "handover.arrow",
    )
    .unwrap();
    let mut csv = csv_source(
        schema.clone(),
        "policy_id,sum_assured,smoker_loading\nP0,1,0.5\nP1,2,\nP2,3,1.5\n",
    )
    .unwrap();

    let expected = vec![(0u64, 0u64, vec![1.0, 2.0, 3.0])];
    assert_eq!(drain(&mut parquet, 8), expected);
    assert_eq!(drain(&mut arrow, 8), expected);
    assert_eq!(drain(&mut csv, 8), expected);
}
