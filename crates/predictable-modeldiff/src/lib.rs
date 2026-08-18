//! # `predictable-modeldiff` — the structural diff of two models
//!
//! `predictable diff model A B` (`01-ir.md` §11.3). The comparison is made over
//! the IR, never over the text, so what it reports is *semantic* change:
//!
//! * components added, removed and **renamed** — a rename detected by identical
//!   normalised expression and unit, or by a preserved `meta.id`;
//! * for a changed component, the expression diff at AST level, classified as
//!   `formula`, `timing`, `unit`, `dtype`, `shape`, `init`, `kind` or
//!   `doc-only`;
//! * table declaration and digest changes, with a row-level diff of the CSV;
//! * assumption value changes;
//! * the **impact set**: the transitive downstream closure of every change, so
//!   the answer is "these 3 changes affect 14 components and 4 outputs, of which
//!   `bel` and `reserve` are Outputs".
//!
//! Three properties are worth stating because they are what make the output
//! trustworthy rather than merely plausible:
//!
//! 1. **Reformatting is not a change.** Whitespace, redundant parentheses and
//!    `1.050` written for `1.05` are gone before the comparison starts — the
//!    trees are compared, and the strings in the report are printed *from* those
//!    trees.
//! 2. **A rename is not a rewrite.** Once `a → b` is detected, every other
//!    component's references are read through the rename before being compared,
//!    so renaming one component does not report a formula change in each of its
//!    dependents.
//! 3. **Every change has a location.** Node changes are anchored at an
//!    [`ExprPath`](predictable_ir::ExprPath) (§3.0.1), which is stable under
//!    `predictable fmt`.
//!
//! ```
//! use predictable_ir::{BinaryOp, Component, DType, Expr, Module, Shape};
//! use predictable_modeldiff::{diff, ChangeClass, ModelSide};
//!
//! let build = |op| {
//!     let mut m = Module::new("m");
//!     m.components.push(Component::derived(
//!         "x",
//!         DType::F64,
//!         Shape::Series,
//!         Expr::binary(op, Expr::r#ref("a"), Expr::r#ref("b")),
//!     ));
//!     let mut side = ModelSide::new("side");
//!     side.modules.push(m);
//!     side
//! };
//!
//! let d = diff(&build(BinaryOp::Mul), &build(BinaryOp::Div));
//! assert_eq!(d.changed.len(), 1);
//! assert_eq!(d.changed[0].classes, vec![ChangeClass::Formula]);
//! assert_eq!(d.changed[0].expr_nodes[0].before, "a * b");
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(missing_debug_implementations)]

pub mod expr_diff;
pub mod impact;
pub mod render;
pub mod report;
pub mod side;

use std::collections::{BTreeMap, BTreeSet};

use predictable_ir::{Component, Expr, Kind, LitValue, PathRoot, TableDecl};
use serde::{Deserialize, Serialize};

pub use expr_diff::{diff_tree, NodeChange, NodeChangeKind};
pub use impact::ImpactSet;
pub use render::render;
pub use side::ModelSide;

/// How a changed component changed (§11.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeClass {
    /// The `expr` tree differs.
    Formula,
    /// The `init` tree differs.
    Init,
    /// `timing` differs — the most common silent one-period error.
    Timing,
    /// `unit` differs.
    Unit,
    /// `dtype` differs.
    Dtype,
    /// `shape` differs.
    Shape,
    /// `kind` differs, e.g. `Derived` became `Output`.
    ///
    /// Not one of §11.3's seven names: the IR has a `kind` field that decides
    /// emission (Q2), and a component that silently stopped being an Output
    /// would otherwise be reported as no change at all.
    Kind,
    /// Only `doc`, `tags` or non-semantic metadata differ. Never reported
    /// alongside another class — it means "nothing that computes changed".
    DocOnly,
}

impl ChangeClass {
    /// True when this class can change a number.
    pub fn is_semantic(self) -> bool {
        self != ChangeClass::DocOnly
    }
}

/// One scalar field that differs, rendered for the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldChange {
    /// The field name, e.g. `timing`.
    pub field: String,
    /// Its value on side A.
    pub before: String,
    /// Its value on side B.
    pub after: String,
}

/// A component present on both sides, with something different about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentChange {
    /// The name on side B.
    pub name: String,
    /// The name on side A, when the component was renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
    /// Every class that applies, sorted and deduplicated.
    pub classes: Vec<ChangeClass>,
    /// Scalar field differences.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldChange>,
    /// AST-level differences in `expr`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expr_nodes: Vec<NodeChange>,
    /// AST-level differences in `init`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub init_nodes: Vec<NodeChange>,
}

impl ComponentChange {
    /// True when nothing that computes changed.
    pub fn is_doc_only(&self) -> bool {
        self.classes == [ChangeClass::DocOnly]
    }
}

/// A component that exists on one side only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentSummary {
    /// The component's name.
    pub name: String,
    /// Its kind.
    pub kind: String,
    /// Its expression, canonical, or `null` for an input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
}

/// A detected rename, with the evidence that detected it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rename {
    /// The name on side A.
    pub from: String,
    /// The name on side B.
    pub to: String,
    /// `meta.id` or `expr+unit` (§11.3).
    pub evidence: String,
}

/// Row-level difference of a table's CSV.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RowDiff {
    /// Rows present only on side B, as written.
    pub added: Vec<Vec<String>>,
    /// Rows present only on side A.
    pub removed: Vec<Vec<String>>,
    /// Rows whose key tuple exists on both sides with different values.
    pub changed: Vec<RowChange>,
}

impl RowDiff {
    /// True when no row moved.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// One row that exists on both sides with different values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowChange {
    /// The key cells, in key order.
    pub key: Vec<String>,
    /// The value cells on side A.
    pub before: Vec<String>,
    /// The value cells on side B.
    pub after: Vec<String>,
}

/// What happened to one table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    /// Present on side B only.
    Added,
    /// Present on side A only.
    Removed,
    /// Present on both, different.
    Changed,
}

/// A table declaration and/or content change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableChange {
    /// Table name.
    pub name: String,
    /// Added, removed or changed.
    pub status: EntryStatus,
    /// Declaration differences: keys, values, `on_missing`, `source`, `digest`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldChange>,
    /// Row-level diff, when the rows of both sides were available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<RowDiff>,
}

/// An assumption declaration or value change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssumptionChange {
    /// Assumption name.
    pub name: String,
    /// Added, removed or changed.
    pub status: EntryStatus,
    /// The declaration, when it differs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldChange>,
    /// The value on side A, when an assumption set supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_before: Option<String>,
    /// The value on side B.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_after: Option<String>,
}

/// The counts an agent branches on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// Components added.
    pub added: usize,
    /// Components removed.
    pub removed: usize,
    /// Components renamed (whether or not they also changed).
    pub renamed: usize,
    /// Components changed.
    pub changed: usize,
    /// Of those, changed only in documentation.
    pub doc_only: usize,
    /// Tables added, removed or changed.
    pub tables: usize,
    /// Assumptions added, removed or changed.
    pub assumptions: usize,
    /// Components downstream of a change, excluding the changes themselves.
    pub components_impacted: usize,
    /// Outputs in the impact set.
    pub outputs_affected: usize,
}

/// The whole comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelDiff {
    /// Side A's label.
    pub a: String,
    /// Side B's label.
    pub b: String,
    /// Components on side B only.
    pub added: Vec<ComponentSummary>,
    /// Components on side A only.
    pub removed: Vec<ComponentSummary>,
    /// Detected renames.
    pub renamed: Vec<Rename>,
    /// Components that differ.
    pub changed: Vec<ComponentChange>,
    /// Table differences.
    pub tables: Vec<TableChange>,
    /// Assumption differences.
    pub assumptions: Vec<AssumptionChange>,
    /// The transitive downstream closure of every change.
    pub impact: ImpactSet,
    /// Counts.
    pub summary: Summary,
    /// Files on either side that did not parse.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unparsed: Vec<String>,
}

impl ModelDiff {
    /// True when the two models are semantically identical — the property that
    /// decides `predictable diff model`'s exit code.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.renamed.is_empty()
            && self.changed.is_empty()
            && self.tables.is_empty()
            && self.assumptions.is_empty()
    }

    /// True when every difference is documentation or a pure rename.
    pub fn is_semantically_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.tables.is_empty()
            && self.assumptions.is_empty()
            && self.changed.iter().all(ComponentChange::is_doc_only)
    }

    /// The `pvf/1` body of `predictable diff model --json`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("a ModelDiff serialises")
    }
}

/// Diff two models (§11.3).
pub fn diff(a: &ModelSide, b: &ModelSide) -> ModelDiff {
    let renames = detect_renames(a, b);
    // A → B name map, so side A's expressions can be read in side B's names.
    let rename_map: BTreeMap<String, String> = renames
        .iter()
        .map(|r| (r.from.clone(), r.to.clone()))
        .collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    let a_names: BTreeSet<&str> = a.components().map(|c| c.name.as_str()).collect();
    let b_names: BTreeSet<&str> = b.components().map(|c| c.name.as_str()).collect();
    let renamed_to: BTreeSet<&str> = renames.iter().map(|r| r.to.as_str()).collect();
    let renamed_from: BTreeSet<&str> = renames.iter().map(|r| r.from.as_str()).collect();

    for c in b.components() {
        if !a_names.contains(c.name.as_str()) && !renamed_to.contains(c.name.as_str()) {
            added.push(summarise(c));
        }
    }
    for c in a.components() {
        if !b_names.contains(c.name.as_str()) && !renamed_from.contains(c.name.as_str()) {
            removed.push(summarise(c));
        }
    }

    // Matched pairs: same name on both sides, plus every rename.
    let mut pairs: Vec<(&Component, &Component, Option<String>)> = Vec::new();
    for c in b.components() {
        if let Some(old) = a.component(&c.name) {
            pairs.push((old, c, None));
        }
    }
    for r in &renames {
        if let (Some(old), Some(new)) = (a.component(&r.from), b.component(&r.to)) {
            pairs.push((old, new, Some(r.from.clone())));
        }
    }

    for (old, new, renamed_from) in pairs {
        if let Some(change) = compare_component(old, new, renamed_from, &rename_map) {
            changed.push(change);
        }
    }
    changed.sort_by(|x, y| x.name.cmp(&y.name));
    added.sort_by(|x, y| x.name.cmp(&y.name));
    removed.sort_by(|x, y| x.name.cmp(&y.name));

    let tables = compare_tables(a, b);
    let assumptions = compare_assumptions(a, b);

    let seeds = impact::seeds(&added, &removed, &changed, &tables, &assumptions);
    let impact = impact::impact_set(a, b, &seeds);

    let mut unparsed = a.unparsed.clone();
    unparsed.extend(b.unparsed.iter().cloned());

    let summary = Summary {
        added: added.len(),
        removed: removed.len(),
        renamed: renames.len(),
        changed: changed.len(),
        doc_only: changed.iter().filter(|c| c.is_doc_only()).count(),
        tables: tables.len(),
        assumptions: assumptions.len(),
        components_impacted: impact.impacted.len(),
        outputs_affected: impact.outputs_affected.len(),
    };

    ModelDiff {
        a: a.label.clone(),
        b: b.label.clone(),
        added,
        removed,
        renamed: renames,
        changed,
        tables,
        assumptions,
        impact,
        summary,
        unparsed,
    }
}

fn summarise(c: &Component) -> ComponentSummary {
    ComponentSummary {
        name: c.name.clone(),
        kind: c.kind.to_string(),
        expr: c.expr.as_ref().map(render),
    }
}

/// Rewrite every reference in a tree through a rename map. This is what stops a
/// rename from being reported as a formula change in each dependent.
fn rewrite(e: &Expr, map: &BTreeMap<String, String>) -> Expr {
    if map.is_empty() {
        return e.clone();
    }
    let rename = |n: &String| map.get(n).cloned().unwrap_or_else(|| n.clone());
    match e {
        Expr::Ref { name } => Expr::Ref { name: rename(name) },
        Expr::Lag { name, k } => Expr::Lag {
            name: rename(name),
            k: *k,
        },
        Expr::At { name, k } => Expr::At {
            name: rename(name),
            k: *k,
        },
        Expr::Lit { .. } => e.clone(),
        Expr::Unary { op, operand } => Expr::Unary {
            op: *op,
            operand: Box::new(rewrite(operand, map)),
        },
        Expr::Binary { op, lhs, rhs } => Expr::Binary {
            op: *op,
            lhs: Box::new(rewrite(lhs, map)),
            rhs: Box::new(rewrite(rhs, map)),
        },
        Expr::If {
            cond,
            then,
            otherwise,
        } => Expr::If {
            cond: Box::new(rewrite(cond, map)),
            then: Box::new(rewrite(then, map)),
            otherwise: Box::new(rewrite(otherwise, map)),
        },
        Expr::Call { func, args } => Expr::Call {
            func: func.clone(),
            args: args.iter().map(|a| rewrite(a, map)).collect(),
        },
        Expr::Lookup { table, keys } => Expr::Lookup {
            table: table.clone(),
            keys: keys.iter().map(|k| rewrite(k, map)).collect(),
        },
        Expr::Agg { op, value, pred } => Expr::Agg {
            op: *op,
            value: Box::new(rewrite(value, map)),
            pred: pred.as_ref().map(|p| Box::new(rewrite(p, map))),
        },
    }
}

fn field(name: &str, before: impl ToString, after: impl ToString) -> FieldChange {
    FieldChange {
        field: name.to_string(),
        before: before.to_string(),
        after: after.to_string(),
    }
}

fn compare_component(
    old: &Component,
    new: &Component,
    renamed_from: Option<String>,
    rename_map: &BTreeMap<String, String>,
) -> Option<ComponentChange> {
    let mut classes = BTreeSet::new();
    let mut fields = Vec::new();

    if old.kind != new.kind {
        classes.insert(ChangeClass::Kind);
        fields.push(field("kind", old.kind, new.kind));
    }
    if old.dtype != new.dtype {
        classes.insert(ChangeClass::Dtype);
        fields.push(field("dtype", &old.dtype, &new.dtype));
    }
    if old.shape != new.shape {
        classes.insert(ChangeClass::Shape);
        fields.push(field("shape", old.shape, new.shape));
    }
    if old.unit != new.unit {
        classes.insert(ChangeClass::Unit);
        fields.push(field("unit", &old.unit, &new.unit));
    }
    if old.timing != new.timing {
        classes.insert(ChangeClass::Timing);
        let show = |t: Option<predictable_ir::Timing>| {
            t.map(|t| t.to_string()).unwrap_or_else(|| "none".into())
        };
        fields.push(field("timing", show(old.timing), show(new.timing)));
    }

    let expr_nodes = compare_tree(
        PathRoot::Expr,
        old.expr.as_ref(),
        new.expr.as_ref(),
        rename_map,
        &mut classes,
        ChangeClass::Formula,
        &mut fields,
        "expr",
    );
    let init_nodes = compare_tree(
        PathRoot::Init,
        old.init.as_ref(),
        new.init.as_ref(),
        rename_map,
        &mut classes,
        ChangeClass::Init,
        &mut fields,
        "init",
    );

    if classes.is_empty() {
        // Nothing that computes changed. Documentation still might have.
        let doc_changed = old.doc != new.doc
            || old.tags != new.tags
            || old.meta.doc_fingerprint() != new.meta.doc_fingerprint();
        if !doc_changed {
            return None;
        }
        classes.insert(ChangeClass::DocOnly);
        if old.doc != new.doc {
            fields.push(field(
                "doc",
                old.doc.clone().unwrap_or_default(),
                new.doc.clone().unwrap_or_default(),
            ));
        }
        if old.tags != new.tags {
            fields.push(field("tags", old.tags.join(","), new.tags.join(",")));
        }
    }

    Some(ComponentChange {
        name: new.name.clone(),
        renamed_from,
        classes: classes.into_iter().collect(),
        fields,
        expr_nodes,
        init_nodes,
    })
}

/// Compare one of the two trees a component owns, recording the class and, when
/// a tree appeared or vanished, a field change instead of node changes.
#[allow(clippy::too_many_arguments)]
fn compare_tree(
    root: PathRoot,
    old: Option<&Expr>,
    new: Option<&Expr>,
    rename_map: &BTreeMap<String, String>,
    classes: &mut BTreeSet<ChangeClass>,
    class: ChangeClass,
    fields: &mut Vec<FieldChange>,
    label: &str,
) -> Vec<NodeChange> {
    match (old, new) {
        (None, None) => Vec::new(),
        (Some(o), Some(n)) => {
            let o = rewrite(o, rename_map);
            if &o == n {
                return Vec::new();
            }
            classes.insert(class);
            diff_tree(root, &o, n)
        }
        (Some(o), None) => {
            classes.insert(class);
            fields.push(field(label, render(&rewrite(o, rename_map)), "none"));
            Vec::new()
        }
        (None, Some(n)) => {
            classes.insert(class);
            fields.push(field(label, "none", render(n)));
            Vec::new()
        }
    }
}

/// Rename detection (§11.3): a preserved `meta.id` first, then identical
/// normalised expression and unit — and only when the match is unambiguous on
/// both sides, because a guessed rename is worse than a reported add/remove.
fn detect_renames(a: &ModelSide, b: &ModelSide) -> Vec<Rename> {
    let a_names: BTreeSet<&str> = a.components().map(|c| c.name.as_str()).collect();
    let b_names: BTreeSet<&str> = b.components().map(|c| c.name.as_str()).collect();

    let mut renames: Vec<Rename> = Vec::new();
    let mut taken_from: BTreeSet<String> = BTreeSet::new();
    let mut taken_to: BTreeSet<String> = BTreeSet::new();

    // Pass 1: `meta.id`, the identity the DSL writes precisely so a rename is
    // not a guess (§11.1).
    let a_by_id: BTreeMap<&str, &Component> = a
        .components()
        .filter_map(|c| c.meta.id.as_deref().map(|id| (id, c)))
        .collect();
    for c in b.components() {
        let Some(id) = c.meta.id.as_deref() else {
            continue;
        };
        if let Some(old) = a_by_id.get(id) {
            if old.name != c.name {
                renames.push(Rename {
                    from: old.name.clone(),
                    to: c.name.clone(),
                    evidence: "meta.id".to_string(),
                });
                taken_from.insert(old.name.clone());
                taken_to.insert(c.name.clone());
            }
        }
    }

    // Pass 2: identical normalised expression and unit among the leftovers.
    let key = |c: &Component| -> Option<String> {
        c.expr
            .as_ref()
            .map(|e| format!("{}\u{1}{}\u{1}{}", render(e), c.unit, c.shape))
    };
    let mut a_left: BTreeMap<String, Vec<&Component>> = BTreeMap::new();
    for c in a.components() {
        if b_names.contains(c.name.as_str()) || taken_from.contains(&c.name) {
            continue;
        }
        if let Some(k) = key(c) {
            a_left.entry(k).or_default().push(c);
        }
    }
    let mut b_left: BTreeMap<String, Vec<&Component>> = BTreeMap::new();
    for c in b.components() {
        if a_names.contains(c.name.as_str()) || taken_to.contains(&c.name) {
            continue;
        }
        if let Some(k) = key(c) {
            b_left.entry(k).or_default().push(c);
        }
    }
    for (k, olds) in &a_left {
        let Some(news) = b_left.get(k) else { continue };
        // Ambiguity is reported as add + remove, never as a coin flip.
        if olds.len() != 1 || news.len() != 1 {
            continue;
        }
        renames.push(Rename {
            from: olds[0].name.clone(),
            to: news[0].name.clone(),
            evidence: "expr+unit".to_string(),
        });
    }
    renames.sort_by(|x, y| x.from.cmp(&y.from));
    renames
}

fn table_fields(old: &TableDecl, new: &TableDecl) -> Vec<FieldChange> {
    let mut fields = Vec::new();
    let keys = |t: &TableDecl| {
        t.keys
            .iter()
            .map(|k| format!("{}:{}:{:?}", k.name, k.dtype, k.policy))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let values = |t: &TableDecl| {
        t.values
            .iter()
            .map(|v| format!("{}:{}:{}", v.name, v.dtype, v.unit))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if keys(old) != keys(new) {
        fields.push(field("keys", keys(old), keys(new)));
    }
    if values(old) != values(new) {
        fields.push(field("values", values(old), values(new)));
    }
    if old.on_missing != new.on_missing {
        fields.push(field("on_missing", &old.on_missing, &new.on_missing));
    }
    if old.source != new.source {
        fields.push(field("source", &old.source, &new.source));
    }
    if old.digest != new.digest {
        fields.push(field(
            "digest",
            old.digest.clone().unwrap_or_else(|| "none".into()),
            new.digest.clone().unwrap_or_else(|| "none".into()),
        ));
    }
    fields
}

fn compare_tables(a: &ModelSide, b: &ModelSide) -> Vec<TableChange> {
    let a_tables: BTreeMap<&str, &TableDecl> = a.tables().map(|t| (t.name.as_str(), t)).collect();
    let b_tables: BTreeMap<&str, &TableDecl> = b.tables().map(|t| (t.name.as_str(), t)).collect();
    let mut out = Vec::new();

    for (name, new) in &b_tables {
        match a_tables.get(name) {
            None => out.push(TableChange {
                name: (*name).to_string(),
                status: EntryStatus::Added,
                fields: Vec::new(),
                rows: None,
            }),
            Some(old) => {
                let fields = table_fields(old, new);
                let rows = match (a.table_rows.get(*name), b.table_rows.get(*name)) {
                    (Some(ra), Some(rb)) => Some(row_diff(new.keys.len(), ra, rb)),
                    _ => None,
                };
                let rows_moved = rows.as_ref().is_some_and(|r| !r.is_empty());
                if !fields.is_empty() || rows_moved {
                    out.push(TableChange {
                        name: (*name).to_string(),
                        status: EntryStatus::Changed,
                        fields,
                        rows: rows.filter(|r| !r.is_empty()),
                    });
                }
            }
        }
    }
    for name in a_tables.keys() {
        if !b_tables.contains_key(name) {
            out.push(TableChange {
                name: (*name).to_string(),
                status: EntryStatus::Removed,
                fields: Vec::new(),
                rows: None,
            });
        }
    }
    out.sort_by(|x, y| x.name.cmp(&y.name));
    out
}

/// Row-level diff of two CSVs, joined on the leading `key_count` cells — which
/// is the join the table's own declaration defines (§2.9), so a reordered file
/// is not a change and a moved rate is.
fn row_diff(key_count: usize, a: &[Vec<String>], b: &[Vec<String>]) -> RowDiff {
    let index = |rows: &[Vec<String>]| -> BTreeMap<Vec<String>, Vec<String>> {
        rows.iter()
            .map(|r| {
                let n = key_count.min(r.len());
                (r[..n].to_vec(), r[n..].to_vec())
            })
            .collect()
    };
    let (ia, ib) = (index(a), index(b));
    let mut out = RowDiff::default();
    for (key, vb) in &ib {
        match ia.get(key) {
            None => out.added.push(key.iter().chain(vb).cloned().collect()),
            Some(va) if va != vb => out.changed.push(RowChange {
                key: key.clone(),
                before: va.clone(),
                after: vb.clone(),
            }),
            Some(_) => {}
        }
    }
    for (key, va) in &ia {
        if !ib.contains_key(key) {
            out.removed.push(key.iter().chain(va).cloned().collect());
        }
    }
    out
}

fn compare_assumptions(a: &ModelSide, b: &ModelSide) -> Vec<AssumptionChange> {
    let (da, db) = (a.assumption_decls(), b.assumption_decls());
    let mut names: BTreeSet<String> = da.keys().chain(db.keys()).cloned().collect();
    names.extend(a.assumption_values.keys().cloned());
    names.extend(b.assumption_values.keys().cloned());

    let show = |v: Option<&LitValue>| v.map(render::render_lit);
    let mut out = Vec::new();
    for name in names {
        let (va, vb) = (
            show(a.assumption_values.get(&name)),
            show(b.assumption_values.get(&name)),
        );
        let (ta, tb) = (da.get(&name), db.get(&name));
        let status = match (ta.is_some() || va.is_some(), tb.is_some() || vb.is_some()) {
            (true, false) => EntryStatus::Removed,
            (false, true) => EntryStatus::Added,
            _ => EntryStatus::Changed,
        };
        let mut fields = Vec::new();
        if ta != tb {
            fields.push(field(
                "declaration",
                ta.cloned().unwrap_or_else(|| "none".into()),
                tb.cloned().unwrap_or_else(|| "none".into()),
            ));
        }
        if status == EntryStatus::Changed && va == vb && fields.is_empty() {
            continue;
        }
        out.push(AssumptionChange {
            name,
            status,
            fields,
            value_before: va,
            value_after: vb,
        });
    }
    out
}

/// True when a component is emitted, i.e. is an output (§2.2).
pub(crate) fn is_output(c: &Component) -> bool {
    c.kind.is_emitted() || c.kind == Kind::Output
}

/// A cheap fingerprint of the non-semantic metadata, so "only the docs moved"
/// can be told from "nothing moved".
trait DocFingerprint {
    fn doc_fingerprint(&self) -> String;
}

impl DocFingerprint for predictable_ir::Meta {
    fn doc_fingerprint(&self) -> String {
        format!(
            "{:?}|{:?}|{:?}|{:?}",
            self.authored_by, self.source, self.overrides, self.generated_from
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use predictable_ir::{BinaryOp, DType, Module, Shape, Timing, Unit};

    fn side(label: &str, components: Vec<Component>) -> ModelSide {
        let mut m = Module::new("m");
        m.components = components;
        let mut s = ModelSide::new(label);
        s.modules.push(m);
        s
    }

    fn derived(name: &str, expr: Expr) -> Component {
        Component::derived(name, DType::F64, Shape::Series, expr)
    }

    #[test]
    fn an_identical_model_diffs_to_nothing() {
        let a = side("a", vec![derived("x", Expr::r#ref("q"))]);
        let b = side("b", vec![derived("x", Expr::r#ref("q"))]);
        let d = diff(&a, &b);
        assert!(d.is_empty(), "{:?}", d);
        assert_eq!(d.summary, Summary::default());
    }

    #[test]
    fn timing_changes_are_classified_as_timing_not_formula() {
        let mut old = derived("x", Expr::r#ref("q"));
        old.timing = Some(Timing::Start);
        let mut new = old.clone();
        new.timing = Some(Timing::End);
        let d = diff(&side("a", vec![old]), &side("b", vec![new]));
        assert_eq!(d.changed[0].classes, vec![ChangeClass::Timing]);
        assert_eq!(d.changed[0].fields[0].after, "end");
    }

    #[test]
    fn unit_and_dtype_changes_are_separate_classes() {
        let mut old = derived("x", Expr::r#ref("q"));
        old.unit = Unit::Money;
        let mut new = old.clone();
        new.unit = Unit::None;
        new.dtype = DType::I64;
        let d = diff(&side("a", vec![old]), &side("b", vec![new]));
        assert_eq!(
            d.changed[0].classes,
            vec![ChangeClass::Unit, ChangeClass::Dtype]
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_doc_change_alone_is_doc_only() {
        let old = derived("x", Expr::r#ref("q"));
        let mut new = old.clone();
        new.doc = Some("now documented".into());
        let d = diff(&side("a", vec![old]), &side("b", vec![new]));
        assert!(d.changed[0].is_doc_only());
        assert!(d.is_semantically_empty());
        assert!(d.impact.impacted.is_empty(), "doc changes impact nothing");
    }

    #[test]
    fn a_rename_is_not_a_rewrite_of_every_dependent() {
        let mut old_a = derived("prem", Expr::r#ref("q"));
        old_a.unit = Unit::Money;
        let old_b = derived(
            "total",
            Expr::binary(BinaryOp::Add, Expr::r#ref("prem"), Expr::f64(1.0)),
        );
        let mut new_a = derived("premium", Expr::r#ref("q"));
        new_a.unit = Unit::Money;
        let new_b = derived(
            "total",
            Expr::binary(BinaryOp::Add, Expr::r#ref("premium"), Expr::f64(1.0)),
        );

        let d = diff(
            &side("a", vec![old_a, old_b]),
            &side("b", vec![new_a, new_b]),
        );
        assert_eq!(d.renamed.len(), 1);
        assert_eq!(d.renamed[0].from, "prem");
        assert_eq!(d.renamed[0].to, "premium");
        assert_eq!(d.renamed[0].evidence, "expr+unit");
        assert!(d.added.is_empty() && d.removed.is_empty());
        assert!(
            d.changed.is_empty(),
            "dependents unchanged: {:?}",
            d.changed
        );
        assert!(d.impact.impacted.is_empty());
    }

    #[test]
    fn meta_id_detects_a_rename_the_heuristic_cannot() {
        let mut old = derived("prem", Expr::r#ref("q"));
        old.meta.id = Some("01J8".into());
        let mut new = derived("premium", Expr::r#ref("q2"));
        new.meta.id = Some("01J8".into());
        let d = diff(&side("a", vec![old]), &side("b", vec![new]));
        assert_eq!(d.renamed[0].evidence, "meta.id");
        // Renamed *and* changed: the formula moved too.
        assert_eq!(d.changed[0].renamed_from.as_deref(), Some("prem"));
        assert_eq!(d.changed[0].classes, vec![ChangeClass::Formula]);
    }

    #[test]
    fn an_ambiguous_rename_is_reported_as_add_and_remove() {
        let a = side(
            "a",
            vec![
                derived("p1", Expr::r#ref("q")),
                derived("p2", Expr::r#ref("q")),
            ],
        );
        let b = side(
            "b",
            vec![
                derived("r1", Expr::r#ref("q")),
                derived("r2", Expr::r#ref("q")),
            ],
        );
        let d = diff(&a, &b);
        assert!(d.renamed.is_empty());
        assert_eq!(d.added.len(), 2);
        assert_eq!(d.removed.len(), 2);
    }

    #[test]
    fn reformatting_is_not_a_change() {
        // `(a * b) + c` and `a * b + c` are one tree; the diff never sees text.
        let inner = Expr::binary(BinaryOp::Mul, Expr::r#ref("a"), Expr::r#ref("b"));
        let e = Expr::binary(BinaryOp::Add, inner, Expr::r#ref("c"));
        let d = diff(
            &side("a", vec![derived("x", e.clone())]),
            &side("b", vec![derived("x", e)]),
        );
        assert!(d.is_empty());
    }

    #[test]
    fn an_init_change_is_its_own_class() {
        let mut old = derived(
            "res",
            Expr::Lag {
                name: "res".into(),
                k: 1,
            },
        );
        old.init = Some(Expr::f64(0.0));
        let mut new = old.clone();
        new.init = Some(Expr::f64(1.0));
        let d = diff(&side("a", vec![old]), &side("b", vec![new]));
        assert_eq!(d.changed[0].classes, vec![ChangeClass::Init]);
        assert_eq!(d.changed[0].init_nodes[0].path.to_string(), "init");
        assert_eq!(d.changed[0].init_nodes[0].after, "1.0");
    }
}
