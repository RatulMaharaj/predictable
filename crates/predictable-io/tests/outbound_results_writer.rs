//! Behavioural tests for the outbound results path: what is on disk, in what order, in which
//! column, and whether a second identical run produces the same bytes.

use std::fs::File;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::Int32Type;
use arrow_array::{
    Array, BooleanArray, DictionaryArray, Float64Array, Int32Array, Int64Array, Int8Array,
    RecordBatch, StringArray, UInt32Array,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use predictable_io::outbound::*;
use predictable_ir::{Component, DType, Expr, Kind, Shape, Timing, Unit};

fn comp(name: &str, dtype: DType, shape: Shape, kind: Kind) -> Component {
    let mut c = Component::derived(name, dtype, shape, Expr::f64(0.0));
    c.kind = kind;
    if shape == Shape::Series {
        c.timing = Some(Timing::End);
    }
    c.unit = Unit::Money;
    c
}

fn schema() -> ResultsSchemaDoc {
    ResultsSchemaDoc::new(
        "all",
        vec![
            ComponentDescriptor::from_ir(
                "term",
                &comp("bel", DType::F64, Shape::Series, Kind::Output),
            ),
            ComponentDescriptor::from_ir(
                "term",
                &comp("age", DType::I64, Shape::Series, Kind::Derived),
            ),
            ComponentDescriptor::from_ir(
                "term",
                &comp("in_force", DType::Bool, Shape::PerMp, Kind::Derived),
            ),
            ComponentDescriptor::from_ir(
                "term",
                &comp(
                    "gender",
                    DType::Enum("Gender".into()),
                    Shape::PerMp,
                    Kind::Derived,
                ),
            ),
        ],
    )
    .unwrap()
}

fn mp(offset: u32, key: &str, row: u32) -> ModelpointRows {
    ModelpointRows {
        offset,
        mp_key: key.to_string(),
        mp_row: row,
        cells: vec![
            Cell::f64("term.bel", 0, 100.0 + row as f64),
            Cell::f64("term.bel", 1, 200.0 + row as f64),
            Cell::i64("term.age", 0, 40 + row as i64),
            Cell::bool("term.in_force", -1, row % 2 == 0),
            Cell::str("term.gender", -1, if row % 2 == 0 { "M" } else { "F" }),
        ],
    }
}

/// Two chunks of two modelpoints each, written in order.
fn write_portfolio(path: &std::path::Path, batch_rows: usize) -> ResultsSummary {
    let f = File::create(path).unwrap();
    let mut w = ResultsWriter::new(
        f,
        schema(),
        WriterOptions {
            batch_rows,
            compression: Codec::Uncompressed,
        },
    )
    .unwrap();
    for chunk_idx in 0..2u64 {
        let mut chunk = ResultsChunk::new(chunk_idx);
        for offset in 0..2u32 {
            let row = (chunk_idx as u32) * 2 + offset;
            chunk.push(mp(offset, &format!("POL{row:04}"), row));
        }
        w.write_chunk(&chunk).unwrap();
    }
    w.finish().unwrap()
}

fn read(path: &std::path::Path) -> Vec<RecordBatch> {
    let f = File::open(path).unwrap();
    ParquetRecordBatchReaderBuilder::try_new(f)
        .unwrap()
        .build()
        .unwrap()
        .map(|b| b.unwrap())
        .collect()
}

fn concat_strings(batches: &[RecordBatch], col: usize) -> Vec<String> {
    batches
        .iter()
        .flat_map(|b| {
            let a = b
                .column(col)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            (0..a.len())
                .map(|i| a.value(i).to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn dict_values(batches: &[RecordBatch], col: usize) -> Vec<Option<String>> {
    batches
        .iter()
        .flat_map(|b| {
            let d: &DictionaryArray<Int32Type> = b.column(col).as_dictionary();
            let vals = d.values().as_any().downcast_ref::<StringArray>().unwrap();
            (0..d.len())
                .map(|i| {
                    if d.is_null(i) {
                        None
                    } else {
                        Some(vals.value(d.keys().value(i) as usize).to_string())
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn writes_long_format_in_chunk_offset_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("results.parquet");
    let summary = write_portfolio(&path, 1024);

    assert_eq!(summary.rows, 4 * 5);
    assert_eq!(summary.modelpoints, 4);
    assert_eq!(summary.components, 4);

    let batches = read(&path);
    let arrow = results_arrow_schema();
    assert_eq!(batches[0].schema().fields(), arrow.fields());

    // (chunk_idx, offset) order == mp_row order here, and it survives the round trip.
    let keys = concat_strings(&batches, 0);
    assert_eq!(
        keys.iter().take(5).cloned().collect::<Vec<_>>(),
        vec!["POL0000"; 5]
    );
    let mp_rows: Vec<u32> = batches
        .iter()
        .flat_map(|b| {
            let a = b.column(1).as_any().downcast_ref::<UInt32Array>().unwrap();
            (0..a.len()).map(|i| a.value(i)).collect::<Vec<_>>()
        })
        .collect();
    assert!(mp_rows.windows(2).all(|w| w[0] <= w[1]), "{mp_rows:?}");
    assert_eq!(mp_rows.first(), Some(&0));
    assert_eq!(mp_rows.last(), Some(&3));
}

#[test]
fn dtype_decides_the_value_column_and_stage_comes_from_the_ir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("results.parquet");
    write_portfolio(&path, 1024);
    let batches = read(&path);
    let b = &batches[0];

    let components = dict_values(&batches, 2);
    let value = b.column(5).as_any().downcast_ref::<Float64Array>().unwrap();
    let value_i = b.column(6).as_any().downcast_ref::<Int64Array>().unwrap();
    let value_b = b.column(7).as_any().downcast_ref::<BooleanArray>().unwrap();
    let value_s = dict_values(&batches, 8);
    let stage = b.column(3).as_any().downcast_ref::<Int8Array>().unwrap();
    let t = b.column(4).as_any().downcast_ref::<Int32Array>().unwrap();

    // Row 0: term.bel @ t=0 -> `value` only.
    assert_eq!(components[0].as_deref(), Some("term.bel"));
    assert_eq!(value.value(0), 100.0);
    assert!(value_i.is_null(0) && value_b.is_null(0) && value_s[0].is_none());
    assert_eq!(stage.value(0), 1);
    assert_eq!(t.value(0), 0);

    // Row 2: term.age (i64) -> `value_i` only.
    assert_eq!(components[2].as_deref(), Some("term.age"));
    assert!(value.is_null(2));
    assert_eq!(value_i.value(2), 40);

    // Row 3: term.in_force (bool, PerMP) -> `value_b`, t = -1.
    assert!(value_b.value(3));
    assert_eq!(t.value(3), -1);

    // Row 4: term.gender (enum) -> `value_s`.
    assert_eq!(value_s[4].as_deref(), Some("M"));
    assert!(value.is_null(4) && value_i.is_null(4) && value_b.is_null(4));

    // Exactly one non-null value column per row, across the whole file.
    for b in &batches {
        for i in 0..b.num_rows() {
            let non_null = [5usize, 6, 7, 8]
                .iter()
                .filter(|c| !b.column(**c).is_null(i))
                .count();
            assert_eq!(non_null, 1, "row {i} has {non_null} value columns set");
        }
    }
}

/// Write `n_mp` modelpoints partitioned into chunks of `chunk` modelpoints each. The row
/// sequence is identical for every `chunk`; only the runner's chunking differs.
fn write_chunked(path: &std::path::Path, n_mp: u32, chunk: u32, codec: Codec) -> ResultsSummary {
    let f = File::create(path).unwrap();
    let mut w = ResultsWriter::new(
        f,
        schema(),
        WriterOptions {
            batch_rows: 64,
            compression: codec,
        },
    )
    .unwrap();
    let mut chunk_idx = 0u64;
    let mut row = 0u32;
    while row < n_mp {
        let mut c = ResultsChunk::new(chunk_idx);
        for offset in 0..chunk {
            if row >= n_mp {
                break;
            }
            c.push(mp(offset, &format!("POL{row:04}"), row));
            row += 1;
        }
        w.write_chunk(&c).unwrap();
        chunk_idx += 1;
    }
    w.finish().unwrap()
}

/// The Phase 1 gate's open defect: `results.parquet` bytes must not depend on `--chunk-size`.
/// 97 modelpoints is deliberately not a multiple of any chunk size below, so every partition
/// straddles the 64-row batch boundary differently.
#[test]
fn chunking_does_not_change_the_bytes() {
    let dir = tempfile::tempdir().unwrap();
    for codec in [Codec::Uncompressed, Codec::Snappy] {
        let mut reference: Option<(Vec<u8>, String)> = None;
        for chunk in [1u32, 3, 7, 16, 97, 4096] {
            let path = dir.path().join(format!("{codec:?}-{chunk}.parquet"));
            let summary = write_chunked(&path, 97, chunk, codec);
            assert_eq!(summary.rows, 97 * 5);
            assert_eq!(summary.modelpoints, 97);
            let bytes = std::fs::read(&path).unwrap();
            match &reference {
                None => reference = Some((bytes, summary.digest)),
                Some((want_bytes, want_digest)) => {
                    assert_eq!(
                        &summary.digest, want_digest,
                        "{codec:?}: chunk size {chunk} changed results_digest"
                    );
                    assert_eq!(
                        &bytes, want_bytes,
                        "{codec:?}: chunk size {chunk} changed the file bytes"
                    );
                }
            }
        }
    }
}

/// Determinism must not have been bought by writing one giant batch: the file really is split
/// into `batch_rows`-sized row groups, whatever the chunking.
#[test]
fn row_groups_are_batch_rows_sized_regardless_of_chunking() {
    let dir = tempfile::tempdir().unwrap();
    for chunk in [1u32, 97] {
        let path = dir.path().join(format!("rg-{chunk}.parquet"));
        write_chunked(&path, 97, chunk, Codec::Uncompressed);
        let meta = ParquetRecordBatchReaderBuilder::try_new(File::open(&path).unwrap())
            .unwrap()
            .metadata()
            .clone();
        let sizes: Vec<i64> = meta.row_groups().iter().map(|g| g.num_rows()).collect();
        // 485 rows at 64 per group: seven full groups and a 37-row tail.
        assert_eq!(sizes, vec![64, 64, 64, 64, 64, 64, 64, 37], "chunk {chunk}");
    }
}

#[test]
fn zero_batch_rows_is_refused() {
    let err = ResultsWriter::new(
        Vec::new(),
        schema(),
        WriterOptions {
            batch_rows: 0,
            compression: Codec::Uncompressed,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("batch_rows"), "{err}");
}

#[test]
fn batching_does_not_change_the_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.parquet");
    let b = dir.path().join("b.parquet");
    let sa = write_portfolio(&a, 1024);
    let sb = write_portfolio(&b, 1024);
    assert_eq!(
        sa.digest, sb.digest,
        "identical input must produce identical bytes"
    );
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());

    // A different row-group size changes the file, but not the row order or the row count.
    let c = dir.path().join("c.parquet");
    let sc = write_portfolio(&c, 3);
    assert_eq!(sc.rows, sa.rows);
    assert_eq!(concat_strings(&read(&c), 0), concat_strings(&read(&a), 0));
}

#[test]
fn out_of_order_chunks_are_refused() {
    let mut w = ResultsWriter::new(Vec::new(), schema(), WriterOptions::default()).unwrap();
    let mut c0 = ResultsChunk::new(0);
    c0.push(mp(0, "POL0000", 0));
    w.write_chunk(&c0).unwrap();

    let mut c2 = ResultsChunk::new(2);
    c2.push(mp(0, "POL0002", 2));
    let err = w.write_chunk(&c2).unwrap_err();
    assert!(
        matches!(
            err,
            IoOutError::ChunkOutOfOrder {
                expected: 1,
                got: 2
            }
        ),
        "{err}"
    );
}

#[test]
fn out_of_order_offsets_are_refused() {
    let mut w = ResultsWriter::new(Vec::new(), schema(), WriterOptions::default()).unwrap();
    let mut c = ResultsChunk::new(0);
    c.push(mp(3, "POL0003", 3));
    c.push(mp(1, "POL0001", 1));
    let err = w.write_chunk(&c).unwrap_err();
    assert!(
        matches!(
            err,
            IoOutError::OffsetOutOfOrder {
                chunk_idx: 0,
                previous: 3,
                got: 1
            }
        ),
        "{err}"
    );
}

#[test]
fn unknown_components_and_wrong_dtypes_are_refused() {
    let mut w = ResultsWriter::new(Vec::new(), schema(), WriterOptions::default()).unwrap();
    let mut c = ResultsChunk::new(0);
    c.push(ModelpointRows {
        offset: 0,
        mp_key: "POL0000".into(),
        mp_row: 0,
        cells: vec![Cell::f64("term.not_a_component", 0, 1.0)],
    });
    assert!(matches!(
        w.write_chunk(&c).unwrap_err(),
        IoOutError::UnknownComponent(id) if id == "term.not_a_component"
    ));

    let mut w = ResultsWriter::new(Vec::new(), schema(), WriterOptions::default()).unwrap();
    let mut c = ResultsChunk::new(0);
    c.push(ModelpointRows {
        offset: 0,
        mp_key: "POL0000".into(),
        mp_row: 0,
        // term.bel is f64; an i64 value is a writer bug, not a coercion.
        cells: vec![Cell::i64("term.bel", 0, 1)],
    });
    assert!(matches!(
        w.write_chunk(&c).unwrap_err(),
        IoOutError::DTypeMismatch { ref component, .. } if component == "term.bel"
    ));
}

#[test]
fn t_must_match_the_shape() {
    let mut w = ResultsWriter::new(Vec::new(), schema(), WriterOptions::default()).unwrap();
    let mut c = ResultsChunk::new(0);
    c.push(ModelpointRows {
        offset: 0,
        mp_key: "POL0000".into(),
        mp_row: 0,
        // PerMP rows carry t = -1.
        cells: vec![Cell::bool("term.in_force", 0, true)],
    });
    assert!(matches!(
        w.write_chunk(&c).unwrap_err(),
        IoOutError::BadT { .. }
    ));

    let mut w = ResultsWriter::new(Vec::new(), schema(), WriterOptions::default()).unwrap();
    let mut c = ResultsChunk::new(0);
    c.push(ModelpointRows {
        offset: 0,
        mp_key: "POL0000".into(),
        mp_row: 0,
        cells: vec![Cell::f64("term.bel", -1, 1.0)],
    });
    assert!(matches!(
        w.write_chunk(&c).unwrap_err(),
        IoOutError::BadT { .. }
    ));
}

#[test]
fn a_trapped_modelpoint_contributes_no_rows() {
    // --continue-on-trap drops the modelpoint entirely: no null rows (IR §9.3.1).
    let mut w = ResultsWriter::new(Vec::new(), schema(), WriterOptions::default()).unwrap();
    let mut c = ResultsChunk::new(0);
    c.push(mp(0, "POL0000", 0));
    c.push(ModelpointRows {
        offset: 1,
        mp_key: "POL0001".into(),
        mp_row: 1,
        cells: vec![],
    });
    c.push(mp(2, "POL0002", 2));
    w.write_chunk(&c).unwrap();
    let s = w.finish().unwrap();
    assert_eq!(s.rows, 10);
    assert_eq!(s.modelpoints, 2);
}

#[test]
fn value_is_double_not_float() {
    // Q15: `double` unconditionally; there is no f32 storage path in IR 1.0.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("results.parquet");
    write_portfolio(&path, 1024);
    let f = File::open(&path).unwrap();
    let builder = ParquetRecordBatchReaderBuilder::try_new(f).unwrap();
    let parquet_schema = builder.parquet_schema();
    let value = parquet_schema
        .columns()
        .iter()
        .find(|c| c.name() == "value")
        .unwrap();
    assert_eq!(
        value.physical_type(),
        parquet::basic::Type::DOUBLE,
        "results.value must be Parquet DOUBLE"
    );
}

#[test]
fn arrow_schema_is_the_documented_one() {
    let s = results_arrow_schema();
    let names: Vec<&str> = s.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        names,
        vec![
            "mp_key",
            "mp_row",
            "component",
            "stage",
            "t",
            "value",
            "value_i",
            "value_b",
            "value_s"
        ]
    );
    assert!(!s.field(0).is_nullable());
    assert!(s.field(5).is_nullable());
    let _ = Arc::clone(&s);
}
