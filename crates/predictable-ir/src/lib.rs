//! # `predictable-ir` — the IR data model
//!
//! This crate is the in-memory form of `docs/design/01-ir.md`: the typed nodes that the Python
//! authoring DSL emits, the `.pir` parser builds, the checker validates and the engine executes.
//! It owns the data model and the lossless `pir.json` encoding, and nothing else — it does no
//! parsing of the `.pir` text (that is `predictable-syntax`), no checking (that is the checker)
//! and no evaluation.
//!
//! Three properties the types are built to preserve:
//!
//! 1. **No arbitrary code crosses the boundary.** Every formula is an [`Expr`] — data, with a
//!    closed node set and a closed builtin list.
//! 2. **The text form is the source of truth.** `pir.json` is generated from it and round-trips
//!    exactly, which the crate's tests assert construct by construct.
//! 3. **Everything checkable is representable.** Shapes, dtypes, units, timing, table key
//!    policies and missingness policies are typed fields, not conventions.
//!
//! ```
//! use predictable_ir::{Component, DType, Expr, Module, PirFile, Shape, Stage, Timing, Unit};
//!
//! let mut module = Module::new("term_assurance");
//! let mut age = Component::derived(
//!     "age",
//!     DType::I64,
//!     Shape::Series,
//!     Expr::binary(
//!         predictable_ir::BinaryOp::Add,
//!         Expr::r#ref("entry_age"),
//!         Expr::r#ref("t"),
//!     ),
//! );
//! age.unit = Unit::Years;
//! age.timing = Some(Timing::Start);
//! module.components.push(age);
//!
//! assert_eq!(module.components[0].stage(), Stage::One);
//!
//! let json = PirFile::from(module.clone()).to_json_pretty().unwrap();
//! assert_eq!(PirFile::from_json(&json).unwrap(), PirFile::from(module));
//! ```

#![deny(missing_debug_implementations)]

pub mod builtins;
pub mod error;
pub mod expr;
pub mod json;
pub mod model;
pub mod run;
pub mod types;

/// The `format` string every `.pir` and `pir.json` document of this IR version carries (§10).
pub const FORMAT: &str = "pir/1";

/// The IR version, `MAJOR.MINOR` (§10).
pub const IR_VERSION: &str = "1.0";

pub use builtins::{is_builtin, is_timing_op, BUILTINS, TIMING_OPS};
pub use error::IrError;
pub use expr::{AggOp, BinaryOp, Expr, ExprPath, LitValue, PathRoot, PathSeg, UnaryOp};
pub use json::PirFile;
pub use model::{
    is_timeline_field, AssumptionDecl, AssumptionSet, AuthoredBy, Basis, Component, EnumDecl,
    GeneratedFrom, KeyPolicy, Meta, ModelpointField, Module, OnMissing, Origin, OriginSpan,
    OverrideRef, SourceRef, Span, Stage, TableDecl, TableKey, TableSource, TableValue, Timeline,
    TIMELINE_FIELDS,
};
pub use run::{
    Aggregation, AggregationOp, Emit, ExecConfig, OnNotConverged, OnTrap, OverT, Product,
    ProductFile, Retain, RunConfig, RunFile, Solve, SolveScope, StoragePrecision,
    RUN_DIGEST_EXCLUDED,
};
pub use types::{DType, Kind, RateBasis, Shape, Timing, Unit};
