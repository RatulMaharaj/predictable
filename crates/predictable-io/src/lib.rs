//! # `predictable-io` — modelpoints in, results out
//!
//! `predictable-io` owns everything the kernel refuses to (`03-engine.md` §6). It has two halves
//! that never reach into each other:
//!
//! - [`inbound`] — reading modelpoints into dense, typed chunk columns. Owned by T13, present.
//! - [`outbound`] — the long-format results schema, its Parquet writer, the aggregate/solve/table
//!   side files and `manifest.json`. Owned by T14; merged in from the temporary
//!   `predictable-io-out` crate by T15 so that outbound run IO has one home.
//!
//! ## What the inbound half guarantees
//!
//! 1. **The schema is the IR's, not the file's** (`01-ir.md` §2.10). [`MpSchema`] is built from a
//!    [`predictable_ir::Module`] and knows every column's dtype, default and presence requirement
//!    before a file is opened. A missing required column is an error that names the file, the
//!    column and the components that read it — the definition-time shape, not a `KeyError` on row
//!    40,000.
//! 2. **Column pruning** (`03-engine.md` §6). [`MpSchema::pruned_for_module`] keeps only the
//!    fields some component's `expr` or `init` actually reads (plus the results key), and
//!    [`ParquetSource`] pushes that list down as a Parquet projection: a model touching 7 of an
//!    MPF's 60 columns reads 7.
//! 3. **No nulls survive the load** (`01-ir.md` §2.11, ruling Q3). A missing or null cell in an
//!    optional field becomes its declared `default`; a null in a required field is an error. The
//!    lanes handed to the kernel are dense, with no validity bitmap. Where — and only where — the
//!    model calls `is_null`/`coalesce` on an optional field, the loader materialises a `bool`
//!    presence lane beside the value lane: opt-in, and visible in [`MpField::presence`].
//! 4. **Chunking is deterministic.** `next_chunk(n, ..)` yields exactly `n` rows until the file
//!    runs out, whatever the file's own row groups or batches look like, and stamps each chunk
//!    with its index and first row so the writer can restore `(chunk_idx, offset)` order.
//!
//! ## A load, end to end
//!
//! ```
//! use predictable_io::{ChunkColumns, CsvSource, ModelpointSource, MpSchema};
//! use predictable_ir::{DType, Expr, LitValue, ModelpointField, Module, Unit};
//!
//! let mut module = Module::new("term");
//! module.modelpoint_fields = vec![
//!     ModelpointField {
//!         name: "policy_id".into(), dtype: DType::Str, unit: Unit::None,
//!         required: true, key: true, default_value: None, doc: None,
//!     },
//!     ModelpointField {
//!         name: "sum_assured".into(), dtype: DType::F64, unit: Unit::Money,
//!         required: true, key: false, default_value: None, doc: None,
//!     },
//!     ModelpointField {
//!         name: "smoker_loading".into(), dtype: DType::F64, unit: Unit::None,
//!         required: false, key: false,
//!         default_value: Some(LitValue::Float(0.0)), doc: None,
//!     },
//! ];
//! module.components.push(predictable_ir::Component::derived(
//!     "claims", DType::F64, predictable_ir::Shape::Series,
//!     Expr::binary(
//!         predictable_ir::BinaryOp::Mul,
//!         Expr::r#ref("sum_assured"),
//!         Expr::r#ref("smoker_loading"),
//!     ),
//! ));
//!
//! let schema = MpSchema::pruned_for_module(&module).unwrap();
//! let csv = "policy_id,sum_assured,smoker_loading\nP1,100000,0.25\nP2,50000,\n";
//! let mut source =
//!     CsvSource::from_reader(schema.clone(), Box::new(csv.as_bytes()), "mp.csv").unwrap();
//!
//! let mut chunk = ChunkColumns::for_schema(&schema);
//! assert_eq!(source.next_chunk(8, &mut chunk).unwrap(), 2);
//! // The null in row 2 became the declared default — no sentinel reaches a lane.
//! assert_eq!(chunk.column("smoker_loading").unwrap().as_f64().unwrap(), &[0.25, 0.0]);
//! assert_eq!(source.next_chunk(8, &mut chunk).unwrap(), 0);
//! ```

// `IoError` is deliberately a wide enum: every variant carries the file, the column and, where it
// helps, the referencing components, because §2.10 requires the message to name them. That makes
// the `Err` arm large; it is the cold path, taken at most once per run, so the size is the right
// trade for the message quality.
#![allow(clippy::result_large_err)]
#![deny(missing_docs)]
#![deny(missing_debug_implementations)]

pub mod error;
pub mod inbound;
pub mod outbound;
pub mod schema;

pub use error::{IoError, Lint};
pub use inbound::{
    ArrowSource, BatchReader, ChunkColumns, CsvSource, ModelpointSource, MpColumn, ParquetSource,
};
pub use outbound::{
    Cell, ComponentDescriptor, Lineage, Manifest, ModelpointRows, ResultsChunk, ResultsSchemaDoc,
    ResultsWriter, RunDir, WriterOptions,
};
pub use schema::{DefaultValue, MpField, MpSchema};
