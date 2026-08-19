//! `predictable-syntax` — the front end of the `.pir` IR text format.
//!
//! This crate turns the bytes of a `.pir` file into a typed, spanned document
//! plus a list of diagnostics. It is pass 1 of the six checker passes in
//! `01-ir.md` §7 and nothing more: it decides what a file *says*, never whether
//! what it says makes sense. Name resolution, cycles, shapes, units and lints
//! all run over the output of this crate.
//!
//! ```
//! use predictable_syntax::{parse, SourceMap};
//!
//! let mut sources = SourceMap::new();
//! let parsed = parse(
//!     &mut sources,
//!     "model.pir",
//!     r#"
//! format = "pir/1"
//! module = "term_assurance"
//!
//! [[component]]
//! name = "deaths"
//! kind = "Derived"
//! dtype = "f64"
//! shape = "Series"
//! unit = "count"
//! timing = "end"
//! expr = "num_pols_if * qx"
//! "#,
//! );
//!
//! assert!(parsed.diagnostics.is_empty());
//! let deaths = parsed.document.component("deaths").unwrap();
//! let refs = parsed.document.arena.references(deaths.expr.unwrap(), "expr");
//! assert_eq!(refs[0].name, "num_pols_if");
//! assert_eq!(refs[0].path, "expr.lhs");
//! ```
//!
//! # Structure
//!
//! | module | job |
//! |---|---|
//! | [`source`] | files, byte [`Span`]s, line/column resolution |
//! | [`diagnostic`] | parser-owned `{code, severity, message, spans, suggestions, doc_url}` |
//! | [`raw`] | the restricted TOML dialect → a spanned key/value tree |
//! | [`expr`] | the infix formula grammar (§4.2) → the [`ExprArena`] |
//! | [`ast`] | the typed document: components, tables, timeline, product, run |
//! | [`lower`] | raw tree → typed document |
//!
//! # Diagnostic codes
//!
//! Syntax codes are `E00xx` and are owned by this crate:
//!
//! | range | meaning |
//! |---|---|
//! | `E0001`–`E0014` | lexical and TOML-structural errors |
//! | `E0020`–`E0032` | expression grammar errors |
//! | `E0041`–`E0053` | block schema errors (missing/unknown/ill-typed keys) |
//!
//! A handful of codes fixed by the spec are emitted here because they are
//! decidable without resolution: `E0101` (a run setting a timeline field),
//! `E0108` (`storage_precision != "f64"`) and `E0305` (a non-`exact` policy on
//! an enum key).
//!
//! # Error recovery
//!
//! Parsing never stops at the first error. A bad line is reported once and
//! skipped to the next newline; a bad section header resynchronises on the next
//! header; a bad expression yields an [`expr::Expr::Error`] node and the rest of
//! the document still lowers. One call to [`parse`] therefore reports every
//! independent syntax error in the file, which is what the migration loop in
//! `01-ir.md` §7 needs in order to converge.

pub mod ast;
pub mod diagnostic;
pub mod expr;
pub mod lower;
pub mod raw;
pub mod source;

pub use ast::{
    Aggregation, AssumptionDecl, Component, DType, DocumentKind, EnumDecl, Kind, ModelpointField,
    OnMissing, PirDocument, Product, Run, Shape, Solve, TableDecl, Timeline, Timing, Unit,
};
pub use diagnostic::{Diagnostic, Diagnostics, Label, Severity, Suggestion};
pub use expr::{
    is_agg_fn, is_builtin, BinOp, Expr, ExprArena, ExprId, Lag, Lit, Reference, UnaryOp,
};
pub use raw::{parse_raw, RawDocument, Value};
pub use source::{FileId, Location, SourceMap, Span, Spanned};

/// The result of parsing one `.pir` file.
pub struct Parsed {
    /// The file this parse registered in the [`SourceMap`].
    pub file: FileId,
    /// The typed document. Present even when there are errors: it holds
    /// everything that did parse.
    pub document: PirDocument,
    pub diagnostics: Diagnostics,
}

impl Parsed {
    /// `true` if any diagnostic is an error, i.e. the document is incomplete.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.has_errors()
    }
}

/// Parse one `.pir` file: register it in `sources`, lex, parse, and lower.
///
/// Never fails and never panics on malformed input — every problem comes back as
/// a [`Diagnostic`] with a span.
pub fn parse(sources: &mut SourceMap, name: impl Into<String>, text: impl Into<String>) -> Parsed {
    let text = text.into();
    let file = sources.add(name, text.clone());
    parse_file(sources, file)
}

/// Parse a file already registered in `sources`.
pub fn parse_file(sources: &SourceMap, file: FileId) -> Parsed {
    let mut diagnostics = Diagnostics::new();
    let raw = parse_raw(sources.text(file), file, &mut diagnostics);
    let document = lower::lower(&raw, &mut diagnostics);
    Parsed {
        file,
        document,
        diagnostics,
    }
}

/// Parse a bare expression string — the entry point `explain()`, the REPL and
/// the DSL's round-trip tests need. `origin` names the pseudo-file it registers.
pub fn parse_expression(
    sources: &mut SourceMap,
    origin: impl Into<String>,
    text: impl Into<String>,
) -> (ExprArena, ExprId, Diagnostics) {
    let text = text.into();
    let file = sources.add(origin, text.clone());
    let mut arena = ExprArena::new();
    let mut diags = Diagnostics::new();
    let id = expr::parse_expr(&text, file, 0, &mut arena, &mut diags);
    (arena, id, diags)
}
