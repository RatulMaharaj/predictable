//! `GraphDoc` — `05-viz.md` §2.1.
//!
//! One rule governs this module: **the viz layer does not compute the topological
//! order — it renders the engine's.** `layers` is bucketed out of
//! [`predictable_plan::Plan::order`], the same sequence `order_digest` hashes and
//! the same sequence the tapes execute, so the picture and the evaluation order
//! cannot disagree. Nothing here re-derives dependencies from scratch: node order
//! is the plan's order, and `depth` is the longest path measured *along* it.

use std::collections::BTreeMap;

use predictable_check::Input;
use predictable_plan::{Plan, Retention, Space};
use predictable_syntax::expr::Lag;
use predictable_syntax::{SourceMap as SynSources, Span};
use serde::{Deserialize, Serialize};

/// A source location, as the inspector renders it and as an agent's suggested
/// edit is anchored to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanDoc {
    /// Display path of the file the span came from.
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub col: u32,
    /// Byte offset of the first byte.
    pub byte_start: u32,
    /// Byte offset one past the last byte.
    pub byte_end: u32,
}

/// One component, in the plan's own evaluation order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    /// `module.name`, the id edges refer to.
    pub id: String,
    /// Unqualified declared name.
    pub name: String,
    /// Module path the component was declared in.
    pub module: String,
    /// `Input` / `Derived` / `Output` (IR §2.2).
    pub kind: String,
    /// IR dtype.
    pub dtype: String,
    /// `Scalar` / `PerMP` / `Series`.
    pub shape: String,
    /// IR unit.
    pub unit: String,
    /// `start` / `end` for series, absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<String>,
    /// 1 or 2 (IR §2.2).
    pub stage: u8,
    /// The expression *as written*, not re-printed from the tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    /// The `init` expression as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init: Option<String>,
    /// Doc comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Declared tags.
    pub tags: Vec<String>,
    /// Where the declaration is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<SpanDoc>,
    /// Declaration index within the module — the planner's tie-break (IR §3.2).
    pub declaration_index: u32,
    /// Longest-path depth over the plan's order; the layer index.
    pub depth: usize,
    /// `full` or `ring(n)` for series slots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention: Option<String>,
    /// Whether the planner hoisted the series out of the modelpoint loop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hoistable: Option<bool>,
}

/// One dependency edge, carrying the IR semantics the renderer styles on (§2.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Qualified id of the component read.
    pub from: String,
    /// Qualified id of the component doing the reading.
    pub to: String,
    /// `k` for `x[t-k]`, `0` for a same-period read, `null` for a table read.
    pub lag: Option<u32>,
    /// True when the read was an absolute `At(x, k)` seed edge.
    pub at: bool,
    /// Stage of the reading component.
    pub stage: u8,
    /// Path to the reference inside the expression, e.g. `expr.Binary.rhs.Lag`.
    pub via: String,
    /// Where the reference is.
    pub span: SpanDoc,
}

/// The document `GET /api/graph` returns (§2.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphDoc {
    /// IR format string.
    pub ir_version: String,
    /// `model_digest` of the sources this was built from.
    pub model_digest: String,
    /// The plan's digest.
    pub plan_digest: String,
    /// The plan's ordering digest — `layers` is a view of exactly this order.
    pub order_digest: String,
    /// Module paths, in file order.
    pub modules: Vec<String>,
    /// Components, in the plan's evaluation order.
    pub nodes: Vec<GraphNode>,
    /// Dependency edges.
    pub edges: Vec<GraphEdge>,
    /// Node *names* bucketed by `depth`, in evaluation order within each bucket.
    pub layers: Vec<Vec<String>>,
}

impl GraphDoc {
    /// The node with this qualified id.
    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Every node name, in the plan's evaluation order.
    pub fn order(&self) -> Vec<&str> {
        self.nodes.iter().map(|n| n.name.as_str()).collect()
    }
}

/// Build the `GraphDoc` for already-planned sources.
///
/// `inputs` must be the same slice the plan was built from: the syntax layer is
/// re-parsed here because it is the only place that still knows the *text* of an
/// expression and the span it came from.
pub fn graph_doc(inputs: &[Input], plan: &Plan, model_digest: &str) -> GraphDoc {
    let mut syn = SynSources::new();
    let mut decls: BTreeMap<String, Decl> = BTreeMap::new();
    let mut modules: Vec<String> = Vec::new();
    let mut tables: Vec<GraphNode> = Vec::new();

    // Every name the plan knows — components, assumptions, modelpoint fields and
    // the timeline's own fields — mapped to the qualified id its node carries.
    // Edges are resolved through this map, so a reference to an input lands on
    // the input's node instead of inventing a dangling name.
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    for slot in &plan.scalars {
        ids.insert(slot.info.name.clone(), slot.info.qualified_id());
    }
    for slot in &plan.permp {
        ids.insert(slot.info.name.clone(), slot.info.qualified_id());
    }
    for slot in &plan.series {
        ids.insert(slot.info.name.clone(), slot.info.qualified_id());
    }

    for input in inputs {
        let parsed = predictable_syntax::parse(&mut syn, input.name.clone(), input.text.clone());
        // The expression *text* comes from the raw key/value tree, not from a
        // span over the parsed tree: an `ExprId`'s span covers the operands it
        // was built from, so `(1 - q)` would lose its parentheses.
        let mut raw_diags = predictable_syntax::diagnostic::Diagnostics::new();
        let raw = predictable_syntax::parse_raw(&input.text, parsed.file, &mut raw_diags);
        let mut text_of: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
        for section in raw.sections_named("component") {
            let name = match section.table.get("name").map(|v| &v.value) {
                Some(predictable_syntax::raw::Value::Str(s)) => s.clone(),
                _ => continue,
            };
            let string_at = |key: &str| match section.table.get(key).map(|v| &v.value) {
                Some(predictable_syntax::raw::Value::Str(s)) => Some(s.clone()),
                _ => None,
            };
            text_of.insert(name, (string_at("expr"), string_at("init")));
        }
        let module = parsed
            .document
            .module
            .as_ref()
            .map(|m| m.value.clone())
            .unwrap_or_default();
        if !module.is_empty() && !modules.contains(&module) {
            modules.push(module.clone());
        }
        // Tables are not slots — the planner never evaluates one — but they are
        // real dependencies and §2.2 draws them as their own node shape.
        for (index, table) in parsed.document.tables.iter().enumerate() {
            let id = if module.is_empty() {
                table.name.value.clone()
            } else {
                format!("{module}.{}", table.name.value)
            };
            ids.insert(table.name.value.clone(), id.clone());
            tables.push(GraphNode {
                id,
                name: table.name.value.clone(),
                module: module.clone(),
                kind: "Table".to_string(),
                dtype: table
                    .values
                    .first()
                    .map(|v| v.dtype.to_string())
                    .unwrap_or_default(),
                shape: "Table".to_string(),
                unit: table
                    .values
                    .first()
                    .map(|v| v.unit.to_string())
                    .unwrap_or_default(),
                timing: None,
                stage: 1,
                expr: None,
                init: None,
                doc: None,
                tags: table.keys.iter().map(|k| k.name.clone()).collect(),
                span: Some(span_doc(&syn, table.name.span)),
                declaration_index: index as u32,
                depth: 0,
                retention: None,
                hoistable: None,
            });
        }
        for component in &parsed.document.components {
            let expr_refs = component
                .expr
                .map(|id| parsed.document.arena.references(id, "expr"))
                .unwrap_or_default();
            let init_refs = component
                .init
                .map(|id| parsed.document.arena.references(id, "init"))
                .unwrap_or_default();
            let texts = text_of.get(&component.name.value);
            decls.insert(
                component.name.value.clone(),
                Decl {
                    expr: texts.and_then(|(e, _)| e.clone()),
                    init: texts.and_then(|(_, i)| i.clone()),
                    doc: component.doc.clone(),
                    tags: component.tags.clone(),
                    span: span_doc(&syn, component.name.span),
                    refs: expr_refs
                        .into_iter()
                        .chain(init_refs)
                        .map(|r| (r.name, r.lag, r.path, span_doc(&syn, r.span)))
                        .collect(),
                },
            );
        }
    }

    // Depth is longest-path over the *plan's* order: a slot sits one layer below
    // the deepest slot it reads, and a slot with no in-edges is layer 0.
    let order = plan.order();
    let mut depth: BTreeMap<String, usize> = BTreeMap::new();
    // Tables come first: they are leaves, they are drawn on the far left, and
    // putting them ahead of every reader keeps the node list readable as an
    // order.
    let mut nodes: Vec<GraphNode> = tables.clone();
    let mut edges: Vec<GraphEdge> = Vec::new();
    nodes.reserve(order.len());

    for id in &order {
        let info = plan.info(*id);
        let name = &info.name;
        let decl = decls.get(name);
        let mut d = 0usize;
        if let Some(decl) = decl {
            for (dep, lag, path, span) in &decl.refs {
                if !matches!(lag, Lag::Table) {
                    d = d.max(depth.get(dep).map(|x| x + 1).unwrap_or(0));
                }
                edges.push(GraphEdge {
                    from: qualified(&ids, dep),
                    to: info.qualified_id(),
                    lag: match lag {
                        Lag::Back(k) => Some(*k),
                        Lag::Absolute(_) | Lag::Current => Some(0),
                        Lag::Table => None,
                    },
                    at: matches!(lag, Lag::Absolute(_)),
                    stage: stage_number(info.stage),
                    via: path.clone(),
                    span: span.clone(),
                });
            }
        }
        depth.insert(name.clone(), d);

        let series = plan.series_of(*id);
        nodes.push(GraphNode {
            id: info.qualified_id(),
            name: name.clone(),
            module: info.module_path.clone(),
            kind: format!("{:?}", info.kind),
            dtype: info.dtype.to_string(),
            shape: match plan.slot_refs[id.index()].space {
                Space::Scalar => "Scalar",
                Space::PerMp => "PerMP",
                Space::Series => "Series",
            }
            .to_string(),
            unit: info.unit.to_string(),
            timing: series.map(|s| format!("{:?}", s.timing).to_lowercase()),
            stage: stage_number(info.stage),
            expr: decl.and_then(|d| d.expr.clone()),
            init: decl.and_then(|d| d.init.clone()),
            doc: decl.and_then(|d| d.doc.clone()),
            tags: decl.map(|d| d.tags.clone()).unwrap_or_default(),
            span: decl.map(|d| d.span.clone()),
            declaration_index: info.decl_index,
            depth: d,
            retention: series.map(|s| match s.retention {
                Retention::Full => "full".to_string(),
                Retention::Ring { len } => format!("ring({len})"),
            }),
            hoistable: series.map(|s| s.hoistable),
        });
    }

    let max_depth = depth.values().copied().max().unwrap_or(0);
    let mut layers: Vec<Vec<String>> = vec![Vec::new(); max_depth + 1];
    for table in &tables {
        layers[0].push(table.name.clone());
    }
    for id in &order {
        let info = plan.info(*id);
        layers[depth[&info.name]].push(info.name.clone());
    }

    GraphDoc {
        ir_version: predictable_ir::FORMAT.to_string(),
        model_digest: model_digest.to_string(),
        plan_digest: plan.digest.clone(),
        order_digest: plan.order_digest.clone(),
        modules,
        nodes,
        edges,
        layers,
    }
}

struct Decl {
    expr: Option<String>,
    init: Option<String>,
    doc: Option<String>,
    tags: Vec<String>,
    span: SpanDoc,
    refs: Vec<(String, Lag, String, SpanDoc)>,
}

fn qualified(ids: &BTreeMap<String, String>, name: &str) -> String {
    ids.get(name).cloned().unwrap_or_else(|| name.to_string())
}

fn stage_number(stage: predictable_ir::Stage) -> u8 {
    match stage {
        predictable_ir::Stage::One => 1,
        predictable_ir::Stage::Two => 2,
    }
}

fn span_doc(sources: &SynSources, span: Span) -> SpanDoc {
    let loc = sources.location(span);
    SpanDoc {
        file: loc.file,
        line: loc.line,
        col: loc.col,
        byte_start: span.start,
        byte_end: span.end,
    }
}
