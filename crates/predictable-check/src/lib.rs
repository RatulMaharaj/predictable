//! # `predictable-check` — the checker
//!
//! The six passes of `01-ir.md` §7, plus the run-scoped seventh, over a set of
//! parsed `.pir` files. Its output is a list of
//! [`predictable_diagnostics::Diagnostic`] values and nothing else: the checker
//! decides whether a model is well formed, and says so in a way a human can act
//! on and an agent can apply.
//!
//! | Pass | What it decides | Codes |
//! |---|---|---|
//! | 1 parse | what the file says (`predictable-syntax`) | `E00xx` |
//! | 2 resolve | every name binds, and binds once | `E0203`, `E0204`, `E0205` |
//! | 3 cycles | `G₀` and `G_init` are acyclic | `E0201`, `E0202` |
//! | 4 shapes | the broadcast lattice; narrowing is explicit | `E0301` |
//! | 5 types | dtypes, units, timing tags, lookups, presence bits | `E05xx`, `E0602`, `E0801`, `E1402` |
//! | 6 lints | what is legal but probably wrong | `W01xx` |
//! | 7 run | `emit_list`, `[[aggregation]]`, `[[solve]]` | `E0106`, `E04xx` |
//!
//! Three properties are load-bearing, and each is tested against the
//! conformance corpus rather than against this implementation:
//!
//! 1. **Every diagnostic carries a suggested edit.** Not prose describing a fix:
//!    a literal byte-range replacement, so the migration loop of §7 can apply it
//!    and re-check without a human in the middle.
//! 2. **Lints run only on a model that is otherwise clean.** A wall of warnings
//!    under a real error is how a toolchain teaches people to ignore warnings.
//! 3. **Nothing is reported twice.** Pass 2 binds an unresolved name to nothing
//!    and the later passes treat it as unknown rather than re-reporting it, so
//!    one typo is one diagnostic.
//!
//! ```
//! use predictable_check::{check, Input};
//!
//! let result = check(&[Input::new(
//!     "model.pir",
//!     r#"
//! format = "pir/1"
//! module = "m"
//!
//! [timeline]
//! basis = "annual"
//! periods = 10
//! origin = "policy"
//! valuation_date = 2026-06-30
//!
//! [[modelpoint_field]]
//! name = "sum_assured"
//! dtype = "f64"
//! unit = "money"
//! required = true
//!
//! [[component]]
//! name = "bel"
//! kind = "Output"
//! dtype = "f64"
//! shape = "PerMP"
//! unit = "money"
//! expr = "sum_asured * 0.1"
//! "#,
//! )]);
//!
//! let diagnostic = &result.diagnostics[0];
//! assert_eq!(diagnostic.code, "E0203");
//! assert_eq!(
//!     diagnostic.suggestions[0].edits[0].replacement,
//!     "sum_assured"
//! );
//! ```

#![deny(missing_debug_implementations)]

pub mod decls;
pub mod emit;
pub mod graph;
pub mod infer;
pub mod lints;
pub mod lower;
pub mod resolve;
pub mod runcheck;
pub mod world;

use std::collections::BTreeSet;

use predictable_diagnostics::{Diagnostic, Severity, SourceMap as DiagSources};
use predictable_syntax::ast::DocumentKind;
use predictable_syntax::expr::Lag;
use predictable_syntax::raw::RawDocument;
use predictable_syntax::source::{FileId, SourceMap};
use predictable_syntax::PirDocument;

pub use world::World;

/// One `.pir` file to check: the name diagnostics will use, and its text.
#[derive(Debug, Clone)]
pub struct Input {
    pub name: String,
    pub text: String,
}

impl Input {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Input {
        Input {
            name: name.into(),
            text: text.into(),
        }
    }
}

/// What a check produced.
#[derive(Debug)]
pub struct CheckResult {
    /// Every diagnostic, in `(file, offset, code)` order.
    pub diagnostics: Vec<Diagnostic>,
    /// The sources, for rendering.
    pub sources: DiagSources,
}

impl CheckResult {
    /// True when nothing blocks a run. Lints do not.
    pub fn is_ok(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
    }

    /// The process exit code implied: 0 clean, 1 lints only, 2 errors.
    pub fn exit_code(&self) -> i32 {
        predictable_diagnostics::exit_code(&self.diagnostics)
    }
}

/// Check a set of `.pir` files.
///
/// The set may mix module, product and run files; each is classified by its own
/// content (`DocumentKind`), never by its file name. A product file switches on
/// the §8.4.1 manifest checks; a run file switches on the seventh pass.
pub fn check(inputs: &[Input]) -> CheckResult {
    let mut sources = SourceMap::new();
    let mut files = Vec::new();
    for input in inputs {
        files.push(sources.add(input.name.clone(), input.text.clone()));
    }
    check_files(&sources, &files)
}

/// Check files already registered in a [`SourceMap`].
pub fn check_files(sources: &SourceMap, files: &[FileId]) -> CheckResult {
    let mut out = Vec::new();
    let mut documents: Vec<(FileId, PirDocument, RawDocument)> = Vec::new();
    let mut superseded = decls::Superseded::default();

    // ---- pass 1: parse ---------------------------------------------------
    let mut parse_diagnostics = Vec::new();
    for &file in files {
        let parsed = predictable_syntax::parse_file(sources, file);
        let mut raw_diags = predictable_syntax::Diagnostics::new();
        let raw = predictable_syntax::parse_raw(sources.text(file), file, &mut raw_diags);
        parse_diagnostics.extend(parsed.diagnostics.into_vec());
        documents.push((file, parsed.document, raw));
    }

    // Declaration-level pass-5 rules that need raw spans. They run before the
    // parser's own diagnostics are folded in, because the checker owns the
    // pass-5 reading of a timing tag and its report supersedes pass 1's.
    for (_, doc, raw) in &documents {
        if doc.kind() == DocumentKind::Module {
            decls::timing(sources, raw, &mut out, &mut superseded);
            decls::table_sources(sources, raw, &mut out);
        }
    }
    for d in &parse_diagnostics {
        if d.primary_span().is_some_and(|s| superseded.covers(s)) {
            continue;
        }
        out.push(emit::from_syntax(sources, d));
    }

    let modules: Vec<(usize, &PirDocument)> = documents
        .iter()
        .enumerate()
        .filter(|(_, (_, d, _))| matches!(d.kind(), DocumentKind::Module | DocumentKind::Unknown))
        .map(|(i, (_, d, _))| (i, d))
        .collect();

    {
        // The parser recovers: a bad line becomes an `Expr::Error` node and the
        // rest of the document still lowers, so the later passes run over what
        // did parse. An `Error` node resolves to nothing and infers as unknown,
        // which is what keeps one syntax error from becoming five.
        let world = world::build(&modules);

        // ---- pass 2: resolve ---------------------------------------------
        resolve::shadowing(sources, &modules, &mut out);
        resolve::check(sources, &world, &modules, &mut out);

        // ---- pass 3: cycles ----------------------------------------------
        graph::check(sources, &world, &modules, &mut out);

        // ---- passes 4 and 5: shapes, dtypes, units, lookups --------------
        for (_, doc) in &modules {
            for component in &doc.components {
                let ctx = infer::Ctx {
                    map: sources,
                    world: &world,
                    doc,
                    component,
                };
                for (root, id) in [(false, component.expr), (true, component.init)] {
                    let Some(id) = id else { continue };
                    let info = infer::infer(&ctx, id, &mut out);
                    if !root {
                        let raw = documents
                            .iter()
                            .find(|(_, d, _)| std::ptr::eq(d, *doc))
                            .map(|(_, _, r)| r);
                        shapes(&ctx, raw, component, id, &info, &mut out);
                    }
                }
            }
        }

        // ---- product and run ---------------------------------------------
        for (i, (_, doc, raw)) in documents.iter().enumerate() {
            let _ = i;
            match doc.kind() {
                DocumentKind::Product => {
                    runcheck::product(sources, &world, &modules, doc, raw, &mut out)
                }
                DocumentKind::Run => runcheck::run(sources, &world, doc, raw, &mut out),
                _ => {}
            }
        }

        // ---- pass 6: lints -----------------------------------------------
        if !out.iter().any(|d| d.severity == Severity::Error) {
            let mut reads = BTreeSet::new();
            for (_, doc) in &modules {
                for c in &doc.components {
                    for (root, id) in [("expr", c.expr), ("init", c.init)] {
                        let Some(id) = id else { continue };
                        for r in doc.arena.references(id, root) {
                            if !matches!(r.lag, Lag::Table) {
                                reads.insert(r.name);
                            }
                        }
                    }
                }
            }
            for (_, doc, _) in &documents {
                if doc.kind() == DocumentKind::Run {
                    runcheck::run_reads(doc, &mut reads);
                }
            }
            let raws: Vec<(usize, &RawDocument)> = documents
                .iter()
                .enumerate()
                .map(|(i, (_, _, raw))| (i, raw))
                .collect();
            lints::check(
                sources,
                &world,
                &modules,
                &raws,
                &lints::Reads(reads),
                &mut out,
            );
        }
    }

    // ---- ordering and rendering data -------------------------------------
    let mut diag_sources = DiagSources::new();
    for &file in files {
        diag_sources.insert(sources.name(file), sources.text(file));
    }
    for d in &mut out {
        d.resolve_positions(&diag_sources);
    }
    out.sort_by(|a, b| {
        let key = |d: &Diagnostic| {
            let span = d.primary_span();
            (
                span.map(|s| s.file.clone()).unwrap_or_default(),
                span.map(|s| s.start).unwrap_or(0),
                d.code.clone(),
            )
        };
        key(a).cmp(&key(b))
    });

    CheckResult {
        diagnostics: out,
        sources: diag_sources,
    }
}

/// Pass 4: the broadcast lattice. Widening is free; narrowing is never implicit
/// and requires an explicit `Agg` (§2.3).
fn shapes(
    ctx: &infer::Ctx<'_>,
    raw: Option<&RawDocument>,
    component: &predictable_syntax::Component,
    expr: predictable_syntax::ExprId,
    info: &infer::Info,
    out: &mut Vec<Diagnostic>,
) {
    use infer::ShapeJoin;
    let shape_span =
        raw.and_then(|r| decls::value_span(r, "component", &component.name.value, "shape"));
    if info.shape.rank() <= component.shape.rank() {
        return;
    }
    let span = ctx.doc.arena.span(expr);
    let text = infer::snippet(ctx.map, span);
    let suggestion = match component.shape {
        predictable_syntax::Shape::PerMP => "sum",
        _ => "last",
    };
    out.push(
        emit::diagnostic(
            "E0301",
            format!(
                "cannot narrow `{}` to `{}` without an aggregate",
                info.shape, component.shape
            ),
        )
        .span(emit::primary(
            ctx.map,
            span,
            format!(
                "this expression is a `{}`, but `{}` is declared `{}`",
                info.shape, component.name.value, component.shape
            ),
        ))
        .note(
            "Widening is implicit and free; narrowing never is (01-ir.md §2.3). Choose the \
             reduction you mean: `sum`, `npv`, `last`, `first`, `at(x, k)`, `max_over`, \
             `min_over` or `count_while`.",
        )
        .suggestion(emit::maybe(emit::replace(
            ctx.map,
            span,
            format!("{suggestion}({text})"),
            format!("reduce the series with `{suggestion}(...)`"),
        )))
        .suggestion(emit::maybe(emit::replace(
            ctx.map,
            shape_span.unwrap_or_else(|| emit::inner(component.name.span)),
            info.shape.to_string(),
            format!("or declare `{}` a `{}`", component.name.value, info.shape),
        ))),
    );
}
