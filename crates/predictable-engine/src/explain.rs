//! `explain()` — the recording evaluator (`04-verify.md` §3, `01-ir.md` §11.2).
//!
//! Tracing is a **replay of one modelpoint**, not a tape of every value of every
//! run (§3.6). The hot path is untouched: a run that never calls `explain()`
//! pays nothing, and a run that does pays one single-lane projection plus tree
//! allocation.
//!
//! Three properties define the contract, and each one is a defence against a
//! particular way a provenance tool can lie:
//!
//! * **Leaves come out of the kernel's own buffers.** A `Ref`, `Lag` or `At`
//!   node reports the value the vectorised kernel stored, read through the same
//!   [`crate::reduce::value_at`] the stage-2 reductions use. Interior arithmetic
//!   is recomputed — the kernel keeps no sub-expression values — but it is
//!   recomputed with [`crate::ops`], the single definition of what `ln` means
//!   that the hot loop itself calls.
//! * **The recomputation is checked, not trusted.** Every component node's
//!   replayed value is compared bit-for-bit with the slot the kernel wrote. A
//!   mismatch is `E0901`, an engine bug, and it is raised rather than rendered.
//! * **A `Ref` expands into the referenced component's tree** (§3.3), so a full
//!   trace bottoms out entirely in `Input` and `Lit` leaves and the arithmetic
//!   in the tree reproduces the value exactly.
//!
//! The JSON ([`Trace`]) is normative; [`crate::explain_text`] is a pure
//! projection of it and never carries a fact the JSON lacks.

use std::collections::{BTreeMap, BTreeSet};

use predictable_diagnostics::Diagnostic;
use predictable_ir::{
    AggOp, BinaryOp, DType, Expr, KeyPolicy, Kind, LitValue, Module, Shape, Span, Timing, UnaryOp,
};
use predictable_plan::{SlotId, Space};
use predictable_tables::{KeyIndex, KeyResolve, Outcome as TableOutcome};
use predictable_tape::TimeField;
use serde::{Deserialize, Serialize};

use crate::buffers::ChunkBuffers;
use crate::layout::Place;
use crate::ops;
use crate::run::{bind_table, ChunkInput, ChunkOutput, Engine, EngineError, RunConfig};
use crate::terms::{self, Term, DEFAULT_MAX_TERMS};
use crate::traps::TrapPolicy;

/// The document format tag every predictable JSON artefact carries.
pub const FORMAT: &str = "pvf/1";

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Knobs on a trace. None of them can change a value — they choose how much of
/// the tree is retained, never how it is computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplainOptions {
    /// Levels of children to expand; `-1` is "all the way down to leaves".
    pub depth: i32,
    /// Component names expanded to full depth regardless of `depth`.
    pub expand: BTreeSet<String>,
    /// Drop spans and type metadata for a compact trace.
    pub values_only: bool,
    /// `--trace-max-terms`: the `Agg` term budget (`01-ir.md` §11.2).
    pub max_terms: usize,
    /// Hard ceiling on retained nodes, so `depth = -1` on a diamond-shaped graph
    /// cannot allocate without bound. Elision is counted, never silent.
    pub max_nodes: usize,
}

impl Default for ExplainOptions {
    fn default() -> ExplainOptions {
        ExplainOptions {
            depth: 2,
            expand: BTreeSet::new(),
            values_only: false,
            max_terms: DEFAULT_MAX_TERMS,
            max_nodes: 100_000,
        }
    }
}

impl ExplainOptions {
    /// Expand every `Ref` to a leaf.
    pub fn full() -> ExplainOptions {
        ExplainOptions {
            depth: -1,
            ..ExplainOptions::default()
        }
    }
}

// ---------------------------------------------------------------------------
// The trace document
// ---------------------------------------------------------------------------

/// A provenance trace: a tree of [`Node`]s plus the notes it collected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    /// Always `"pvf/1"`.
    pub format: String,
    /// Always `"trace"`.
    pub kind: String,
    /// Manifest digest of the run being explained; empty when unpinned.
    pub run: String,
    pub root: Node,
    #[serde(default)]
    pub notes: Vec<Note>,
    pub truncated: Truncated,
}

/// What the depth and node budgets removed. Truncation is never silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Truncated {
    pub depth: i32,
    pub elided_nodes: u32,
}

/// A coded observation attached to a node path (`04-verify.md` §3.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub code: String,
    pub severity: String,
    pub path: String,
    pub message: String,
    pub component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t: Option<i64>,
}

/// Where a modelpoint value came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modelpoint {
    pub key: String,
    pub row: u64,
}

/// The `source` block of an `Input` leaf.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assumption_set: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

impl Source {
    fn is_empty(&self) -> bool {
        *self == Source::default()
    }
}

/// One key of a `Lookup` node, with the policy that fired.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookupKey {
    pub name: String,
    /// The key as the model computed it.
    pub requested: String,
    /// The domain value the index actually used.
    pub resolved: String,
    /// `exact` | `clamp` | `step` | `interpolate`.
    pub policy: String,
    /// True when the policy changed the key — the silent-wrongness bit.
    pub fired: bool,
}

/// The `over` block of an `Agg` node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggOver {
    pub component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<String>,
    pub t_from: u32,
    pub t_to: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exponent_rule: Option<String>,
}

/// The closed node-variant set of `04-verify.md` §3.2, one-to-one with
/// `Expr` (`01-ir.md` §2.6) plus the `Component` root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Component,
    Binary,
    Unary,
    Ref,
    Lag,
    At,
    Input,
    Lit,
    If,
    Lookup,
    Call,
    Agg,
}

impl NodeKind {
    /// The tag as it appears in the JSON and in the text rendering.
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Component => "Component",
            NodeKind::Binary => "Binary",
            NodeKind::Unary => "Unary",
            NodeKind::Ref => "Ref",
            NodeKind::Lag => "Lag",
            NodeKind::At => "At",
            NodeKind::Input => "Input",
            NodeKind::Lit => "Lit",
            NodeKind::If => "If",
            NodeKind::Lookup => "Lookup",
            NodeKind::Call => "Call",
            NodeKind::Agg => "Agg",
        }
    }
}

/// One node of the trace tree. Every node has `node`, `value` and `path`; the
/// rest is per-variant and omitted when it does not apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub node: NodeKind,
    /// Position of this node in its tree — `root.children[0].children[2]`. The
    /// join key `notes` and `explain_diff()` align on.
    pub path: String,
    pub value: f64,

    /// `Component`: the qualified id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `Ref` / `Lag` / `At` / `Input` / `Agg`: what is being read.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// `Binary` / `Unary`: the operator symbol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// `Call`: the builtin's name.
    #[serde(rename = "fn", skip_serializing_if = "Option::is_none")]
    pub func: Option<String>,
    /// `Lookup`: the table.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// `Input`: `Modelpoint` | `Assumption` | `Timeline`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The period this node was evaluated at; `-1` and below is pre-origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lag: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<u32>,
    /// `If`: `then` or `else`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taken: Option<String>,
    /// `Ref` / `Lag` / `At`: `computed` | `init` | `pre_origin_default` (§3.3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// The `init` expression, when `resolution` is `init`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init_expr: Option<String>,
    /// The note code raised at this node, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A `str` or `enum` value's text. The lane carries a dictionary code
    /// (`03-engine.md` §4.2), and `3` is not what the model author wrote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub dtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<u8>,
    /// `Component`: the canonical rendering of the expression it evaluated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modelpoint: Option<Modelpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<LookupKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_digest: Option<String>,

    /// `Agg` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub over: Option<AggOver>,
    /// `Agg` only: the per-`t` contributions, required by Q11.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terms: Vec<Term>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term_count: Option<usize>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terms_truncated: bool,
    /// `retime` only: the timing cast that changed nothing but the tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_timing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_timing: Option<String>,

    /// Children not retained under the depth or node budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elided: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

impl Node {
    fn new(kind: NodeKind, path: &str, value: f64) -> Node {
        Node {
            node: kind,
            path: path.to_string(),
            value,
            id: None,
            reference: None,
            op: None,
            func: None,
            table: None,
            kind: None,
            t: None,
            lag: None,
            at: None,
            taken: None,
            resolution: None,
            init_expr: None,
            note: None,
            text: None,
            dtype: None,
            unit: None,
            timing: None,
            shape: None,
            stage: None,
            expr: None,
            span: None,
            modelpoint: None,
            source: None,
            keys: Vec::new(),
            row: None,
            table_digest: None,
            over: None,
            terms: Vec::new(),
            term_count: None,
            terms_truncated: false,
            from_timing: None,
            to_timing: None,
            elided: None,
            children: Vec::new(),
        }
    }

    /// Depth-first walk, including `self`.
    pub fn walk(&self, f: &mut dyn FnMut(&Node)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }

    /// Nodes in the subtree rooted here.
    pub fn count(&self) -> usize {
        let mut n = 0;
        self.walk(&mut |_| n += 1);
        n
    }

    /// The node at `path`, if it is in this subtree.
    pub fn find(&self, path: &str) -> Option<&Node> {
        if self.path == path {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(path))
    }
}

impl Trace {
    /// The JSON document, pretty-printed and stable across runs.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("trace serialises")
    }

    /// The deterministic ASCII rendering — a pure projection of this document.
    pub fn text(&self) -> String {
        crate::explain_text::render(self)
    }

    /// Every note with a given code.
    pub fn notes_with(&self, code: &str) -> Vec<&Note> {
        self.notes.iter().filter(|n| n.code == code).collect()
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A replay that did not reproduce the run — always an engine bug (`E0901`).
#[derive(Debug, Clone, PartialEq)]
pub struct Divergence {
    pub component: String,
    pub t: Option<i64>,
    pub replayed: f64,
    pub recorded: f64,
    pub path: String,
    pub diagnostic: Diagnostic,
    pub trace: Trace,
}

/// Why a trace could not be produced.
#[derive(Debug, Clone, PartialEq)]
pub enum ExplainError {
    Engine(EngineError),
    /// A series slot is stored in a ring buffer, so history the trace needs has
    /// already wrapped away. Plan with `retain_all` to explain.
    NotRetained(Vec<String>),
    UnknownComponent(String),
    /// `t` is required for a `Series` component.
    NeedsT(String),
    /// `t` must be `None` for a `Scalar` or `PerMP` component.
    UnexpectedT(String),
    TOutOfRange {
        t: u32,
        periods: u32,
    },
    /// One modelpoint, exactly. A trace is a single-lane replay by definition.
    NotOneLane(usize),
    /// [`Explainer::load`] has not been called.
    NotLoaded,
    /// `E0901`.
    ReplayDiverged(Box<Divergence>),
}

impl std::fmt::Display for ExplainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExplainError::Engine(e) => write!(f, "{e}"),
            ExplainError::NotRetained(names) => write!(
                f,
                "explain needs full retention; {} slot(s) are ring-buffered (first: {})",
                names.len(),
                names.first().map(String::as_str).unwrap_or("-")
            ),
            ExplainError::UnknownComponent(n) => write!(f, "no component named `{n}`"),
            ExplainError::NeedsT(n) => write!(f, "`{n}` is a Series: explain needs a `t`"),
            ExplainError::UnexpectedT(n) => {
                write!(f, "`{n}` is not a Series: explain takes no `t`")
            }
            ExplainError::TOutOfRange { t, periods } => {
                write!(f, "t = {t} is outside the projection 0..={periods}")
            }
            ExplainError::NotOneLane(n) => {
                write!(f, "a trace replays exactly one modelpoint, not {n}")
            }
            ExplainError::NotLoaded => f.write_str("no modelpoint has been loaded"),
            ExplainError::ReplayDiverged(d) => write!(f, "{}", d.diagnostic.message),
        }
    }
}

impl std::error::Error for ExplainError {}

impl From<EngineError> for ExplainError {
    fn from(e: EngineError) -> ExplainError {
        ExplainError::Engine(e)
    }
}

// ---------------------------------------------------------------------------
// Trace context — the provenance the kernel does not carry
// ---------------------------------------------------------------------------

/// Provenance the kernel has no reason to know: which file a span points into,
/// which assumption set was used, where the modelpoints came from.
///
/// It is data, not a computation: nothing here can change a number.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TraceContext {
    /// Manifest digest of the run being explained.
    pub run: String,
    /// Component name → its `.pir` declaration span.
    pub spans: BTreeMap<String, Span>,
    /// Component name → its `doc`.
    pub docs: BTreeMap<String, String>,
    pub modelpoint_file: Option<String>,
    pub assumption_set: Option<String>,
    pub assumption_file: Option<String>,
}

impl TraceContext {
    /// Harvest the spans and docs of every component in the model.
    pub fn from_modules(modules: &[Module]) -> TraceContext {
        let mut ctx = TraceContext::default();
        for module in modules {
            for c in &module.components {
                if let Some(span) = c.meta.span.clone() {
                    ctx.spans.insert(c.name.clone(), span);
                }
                if let Some(doc) = c.doc.clone() {
                    ctx.docs.insert(c.name.clone(), doc);
                }
            }
        }
        ctx
    }
}

// ---------------------------------------------------------------------------
// The explainer
// ---------------------------------------------------------------------------

/// A single-modelpoint replay, and the traces taken from it.
///
/// The engine inside is a *separate* one-lane instance: `explain()` never
/// perturbs a live run, and it is the reason there is no "tracing mode" flag on
/// a run at all (§3.6).
#[derive(Debug)]
pub struct Explainer<'a> {
    engine: Engine<'a>,
    bufs: ChunkBuffers,
    ctx: TraceContext,
    loaded: Option<Loaded>,
}

#[derive(Debug)]
struct Loaded {
    key: String,
    row: u64,
    output: ChunkOutput,
}

impl<'a> Explainer<'a> {
    /// Bind a plan and its tapes for replay.
    ///
    /// The plan must have been built with full retention (`--retain-all`, or a
    /// model whose every series is `Full` anyway): a ring buffer has thrown away
    /// the history a trace at `t` needs, and reading it would report a value
    /// from a different period as if it were this one.
    pub fn new(
        plan: &'a predictable_plan::Plan,
        program: &'a predictable_tape::TapeProgram,
        tables: Vec<predictable_tables::CompiledTable>,
        timeline: &predictable_ir::Timeline,
        ctx: TraceContext,
    ) -> Result<Explainer<'a>, ExplainError> {
        let config = RunConfig {
            chunk: 1,
            // A trapping modelpoint is exactly the one worth explaining, so the
            // replay survives it and reports the poison value the run produced.
            on_trap: TrapPolicy::Continue,
            max_errors: 100,
        };
        let engine = Engine::new(plan, program, tables, timeline, config)?;
        let ring: Vec<String> = plan
            .series
            .iter()
            .filter(|s| {
                matches!(
                    engine.layout().place(s.info.id),
                    Place::Series { full: false, .. }
                )
            })
            .map(|s| s.info.qualified_id())
            .collect();
        if !ring.is_empty() {
            return Err(ExplainError::NotRetained(ring));
        }
        let bufs = engine.buffers();
        Ok(Explainer {
            engine,
            bufs,
            ctx,
            loaded: None,
        })
    }

    /// Scalars and hoisted series, once.
    pub fn prepare(&mut self, assumptions: &BTreeMap<String, f64>) -> Result<(), ExplainError> {
        self.engine.prepare(assumptions, &mut self.bufs)?;
        Ok(())
    }

    /// Project the one modelpoint whose values the traces will report.
    pub fn load(&mut self, chunk: &ChunkInput) -> Result<(), ExplainError> {
        if chunk.len() != 1 {
            return Err(ExplainError::NotOneLane(chunk.len()));
        }
        let output = self.engine.run_chunk(&mut self.bufs, chunk)?;
        self.loaded = Some(Loaded {
            key: chunk.keys[0].clone(),
            row: chunk.first_row,
            output,
        });
        Ok(())
    }

    /// The engine behind the replay, for interning a chunk's string columns
    /// before [`Explainer::load`]. It is the same dictionary the trace's `Lit`
    /// nodes decode against, which is why the chunk must be bound to *this*
    /// engine and not to the one the run used.
    pub fn engine_mut(&mut self) -> &mut Engine<'a> {
        &mut self.engine
    }

    /// The kernel's own output for the loaded modelpoint — what `E0901` checks
    /// the replay against.
    pub fn output(&self) -> Option<&ChunkOutput> {
        self.loaded.as_ref().map(|l| &l.output)
    }

    /// Trace one cell.
    ///
    /// `t` is required for a `Series` component and must be `None` otherwise.
    /// Returns [`ExplainError::ReplayDiverged`] — `E0901` — if the replayed
    /// value is not bit-identical to the one the vectorised kernel stored.
    pub fn explain(
        &self,
        component: &str,
        t: Option<u32>,
        options: &ExplainOptions,
    ) -> Result<Trace, ExplainError> {
        let trace = self.explain_unchecked(component, t, options)?;
        match self.divergence(&trace) {
            None => Ok(trace),
            Some(mut d) => {
                d.trace = trace;
                Err(ExplainError::ReplayDiverged(Box::new(d)))
            }
        }
    }

    /// The trace without the `E0901` assertion — for tests and for rendering a
    /// divergence that has already been detected.
    pub fn explain_unchecked(
        &self,
        component: &str,
        t: Option<u32>,
        options: &ExplainOptions,
    ) -> Result<Trace, ExplainError> {
        let loaded = self.loaded.as_ref().ok_or(ExplainError::NotLoaded)?;
        let plan = self.engine.plan();
        let slot = self
            .find_slot(component)
            .ok_or_else(|| ExplainError::UnknownComponent(component.to_string()))?;
        let space = plan.slot_refs[slot.index()].space;
        let periods = self.engine.layout().periods;
        let when = match (space, t) {
            (Space::Series, Some(t)) if t > periods => {
                return Err(ExplainError::TOutOfRange { t, periods })
            }
            (Space::Series, Some(t)) => When::Cur(t as i64),
            (Space::Series, None) => {
                return Err(ExplainError::NeedsT(plan.info(slot).qualified_id()))
            }
            (_, Some(_)) => return Err(ExplainError::UnexpectedT(plan.info(slot).qualified_id())),
            (_, None) => When::Cur(0),
        };

        let mut rec = Recorder {
            ex: self,
            key: loaded.key.clone(),
            row: loaded.row,
            options,
            notes: Vec::new(),
            elided: 0,
            budget: options.max_nodes,
        };
        let root = rec.component_node(slot, when, "root", rec.depth_budget(), true);
        let notes = rec.notes;
        let elided = rec.elided;
        Ok(Trace {
            format: FORMAT.to_string(),
            kind: "trace".to_string(),
            run: self.ctx.run.clone(),
            root,
            notes,
            truncated: Truncated {
                depth: options.depth,
                elided_nodes: elided,
            },
        })
    }

    /// Compare every `Component` node in a trace with the value the kernel
    /// stored for that slot, and build the `E0901` diagnostic if they differ.
    pub fn divergence(&self, trace: &Trace) -> Option<Divergence> {
        let mut found: Option<Divergence> = None;
        trace.root.walk(&mut |node| {
            if found.is_some() || node.node != NodeKind::Component {
                return;
            }
            let Some(id) = node.id.as_deref() else { return };
            let Some(slot) = self.find_slot(id) else {
                return;
            };
            let when = match node.t {
                Some(t) => When::Cur(t),
                None => When::Cur(0),
            };
            let recorded = self.read(slot, when);
            if same_bits(recorded, node.value) {
                return;
            }
            let message = format!(
                "trace replay of `{id}` diverged from the recorded run: replay {}, run {}",
                node.value, recorded
            );
            let mut diagnostic = Diagnostic::new("E0901", message);
            diagnostic.notes.push(
                "the projection is deterministic (01-ir.md §9), so a replay that does not \
                 reproduce the run is an engine bug, not a model problem"
                    .to_string(),
            );
            found = Some(Divergence {
                component: id.to_string(),
                t: node.t,
                replayed: node.value,
                recorded,
                path: node.path.clone(),
                diagnostic,
                trace: trace.clone(),
            });
        });
        found
    }

    // -- internals ---------------------------------------------------------

    fn find_slot(&self, name: &str) -> Option<SlotId> {
        let plan = self.engine.plan();
        let mut bare: Option<SlotId> = None;
        for r in 0..plan.slot_refs.len() {
            let info = plan.info(SlotId(r as u32));
            if info.qualified_id() == name {
                return Some(info.id);
            }
            if info.name == name && bare.is_none() {
                bare = Some(info.id);
            }
        }
        bare
    }

    /// One slot's value, from the buffers the kernel filled.
    fn read(&self, slot: SlotId, when: When) -> f64 {
        let plan = self.engine.plan();
        let info = plan.info(slot);
        if info.kind == Kind::InputTimeline {
            if let Some(field) = TimeField::from_name(&info.name) {
                let t = match when {
                    When::Cur(t) => t.max(0) as u32,
                    When::Seed => 0,
                };
                return self.engine.clock().field(field, t);
            }
        }
        match when {
            When::Seed => match self.engine.layout().place(slot) {
                Place::Series { seed, .. } => self.bufs.seeds[seed],
                Place::Hoisted { seed, .. } => self.engine.state().hoisted_seeds[seed],
                _ => self.read(slot, When::Cur(0)),
            },
            When::Cur(t) => {
                let t = t.max(0) as u32;
                let ctx = self.engine.ctx();
                crate::reduce::value_at(&ctx, self.engine.state(), &self.bufs, slot, t, 0)
            }
        }
    }
}

/// When a value is being read: at a period, or in the `init` seed section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    Cur(i64),
    Seed,
}

impl When {
    fn t(self) -> Option<i64> {
        match self {
            When::Cur(t) => Some(t),
            When::Seed => None,
        }
    }
}

// ---------------------------------------------------------------------------
// The recorder
// ---------------------------------------------------------------------------

struct Recorder<'a, 'p> {
    ex: &'a Explainer<'p>,
    key: String,
    row: u64,
    options: &'a ExplainOptions,
    notes: Vec<Note>,
    elided: u32,
    budget: usize,
}

impl Recorder<'_, '_> {
    fn depth_budget(&self) -> i32 {
        self.options.depth
    }

    fn plan(&self) -> &predictable_plan::Plan {
        self.ex.engine.plan()
    }

    fn note(&mut self, code: &str, path: &str, component: &str, t: Option<i64>, message: String) {
        let severity = if code.starts_with('N') {
            "info"
        } else {
            "warning"
        };
        self.notes.push(Note {
            code: code.to_string(),
            severity: severity.to_string(),
            path: path.to_string(),
            message,
            component: component.to_string(),
            t,
        });
    }

    /// The tree of one component at one period. `root` marks the node the user
    /// asked for, which always carries its metadata even under `values_only`.
    fn component_node(
        &mut self,
        slot: SlotId,
        when: When,
        path: &str,
        depth: i32,
        root: bool,
    ) -> Node {
        let plan = self.plan();
        let info = plan.info(slot).clone();
        let space = plan.slot_refs[slot.index()].space;
        let stored = self.ex.read(slot, when);

        if info.kind.is_input() {
            return self.input_node(slot, when, path, stored);
        }

        // `init` *is* the value at t = 0 (`01-ir.md` §2.7): the formula is not
        // evaluated there at all, and a lag below the origin reads the same seed.
        let series = plan.series_of(slot).cloned();
        let (body, is_init) = match (&series, when) {
            (Some(s), When::Seed) if s.init.is_some() => (s.init.clone(), true),
            (Some(s), When::Cur(0)) if s.init.is_some() => (s.init.clone(), true),
            (Some(s), _) => (s.expr.clone(), false),
            (None, _) => (
                match space {
                    Space::Scalar => plan.scalar_named(&info.name).and_then(|s| s.expr.clone()),
                    _ => plan.permp_named(&info.name).and_then(|s| s.expr.clone()),
                },
                false,
            ),
        };
        let inner_when = if is_init { When::Seed } else { when };

        let mut node = Node::new(NodeKind::Component, path, stored);
        node.id = Some(info.qualified_id());
        node.t = when.t().filter(|_| space == Space::Series);
        node.value = stored;
        if !self.options.values_only || root {
            node.dtype = Some(info.dtype.to_string());
            node.unit = Some(info.unit.to_string());
            node.shape = Some(
                match space {
                    Space::Scalar => Shape::Scalar,
                    Space::PerMp => Shape::PerMp,
                    Space::Series => Shape::Series,
                }
                .to_string(),
            );
            node.timing = series.as_ref().map(|s| s.timing.to_string());
            node.stage = Some(u8::from(info.stage));
            node.span = self.ex.ctx.spans.get(&info.name).cloned();
            node.expr = body.as_ref().map(expr_text);
        }
        if root {
            node.modelpoint = Some(Modelpoint {
                key: self.key.clone(),
                row: self.row,
            });
        }
        if is_init {
            node.resolution = Some("init".to_string());
            if matches!(when, When::Cur(0)) {
                node.note = Some("N0302".to_string());
                let msg = format!("`{}` used its `init` at t = 0", info.name);
                self.note("N0302", path, &info.qualified_id(), Some(0), msg);
            }
        }

        if let Some(expr) = body {
            let child_path = format!("{path}.children[0]");
            let child = self.expr_node(&expr, inner_when, slot, &child_path, depth);
            // The replay of this component's own formula, checked against the
            // slot the kernel wrote. `E0901` lives here.
            node.value = child.value;
            node.children.push(child);
        }

        // N0402: a money value far outside any plausible scale.
        if info.unit.to_string() == "money" {
            let v = node.value.abs();
            if v > 1e12 || (v > 0.0 && v < 1e-12) {
                let msg = format!(
                    "`{}` = {} is outside 1e-12 .. 1e12 while its unit is money",
                    info.name, node.value
                );
                self.note("N0402", path, &info.qualified_id(), when.t(), msg);
            }
        }
        node
    }

    fn input_node(&mut self, slot: SlotId, when: When, path: &str, value: f64) -> Node {
        let info = self.plan().info(slot).clone();
        let mut node = Node::new(NodeKind::Input, path, value);
        node.reference = Some(info.name.clone());
        node.kind = Some(
            match info.kind {
                Kind::InputModelpoint => "Modelpoint",
                Kind::InputAssumption => "Assumption",
                Kind::InputTimeline => "Timeline",
                _ => "Table",
            }
            .to_string(),
        );
        if matches!(info.dtype, DType::Str | DType::Enum(_)) {
            node.text = Some(self.ex.engine.dict().decode(value).to_string());
        }
        if !self.options.values_only {
            node.dtype = Some(info.dtype.to_string());
            node.unit = Some(info.unit.to_string());
            let mut source = Source::default();
            match info.kind {
                Kind::InputModelpoint => {
                    source.file = self.ex.ctx.modelpoint_file.clone();
                    source.row = Some(self.row);
                    source.column = Some(info.name.clone());
                }
                Kind::InputAssumption => {
                    source.assumption_set = self.ex.ctx.assumption_set.clone();
                    source.file = self.ex.ctx.assumption_file.clone();
                    source.line = self.ex.ctx.spans.get(&info.name).map(|s| s.line);
                }
                _ => {}
            }
            if !source.is_empty() {
                node.source = Some(source);
            }
            if info.kind == Kind::InputTimeline {
                node.t = when.t();
            }
        }
        node
    }

    /// One expression node. The value is always computed; `depth` only decides
    /// how much of the tree below a `Ref` is *retained*.
    fn expr_node(&mut self, e: &Expr, when: When, owner: SlotId, path: &str, depth: i32) -> Node {
        if self.budget == 0 {
            self.elided += 1;
            let mut node = Node::new(NodeKind::Lit, path, self.eval(e, when, owner));
            node.elided = Some(1);
            return node;
        }
        self.budget -= 1;
        let owner_name = self.plan().info(owner).qualified_id();
        match e {
            Expr::Lit { dtype, value } => {
                let mut node = Node::new(NodeKind::Lit, path, self.lit_value(dtype, value));
                if let LitValue::Text(text) = value {
                    node.text = Some(text.clone());
                }
                if !self.options.values_only {
                    node.dtype = Some(dtype.to_string());
                }
                node
            }
            Expr::Ref { name } => self.read_node(name, when, NodeKind::Ref, None, path, depth),
            Expr::Lag { name, k } => {
                let target = match when {
                    When::Seed => When::Seed,
                    When::Cur(t) => When::Cur(t - i64::from(*k)),
                };
                self.read_node(name, target, NodeKind::Lag, Some(*k), path, depth)
            }
            Expr::At { name, k } => self.read_node(
                name,
                When::Cur(i64::from(*k)),
                NodeKind::At,
                Some(*k),
                path,
                depth,
            ),
            Expr::Unary { op, operand } => {
                let child = self.expr_node(operand, when, owner, &child_path(path, 0), depth);
                let value = match op {
                    UnaryOp::Neg => -child.value,
                    UnaryOp::Not => f64::from(!ops::truthy(child.value)),
                };
                let mut node = Node::new(NodeKind::Unary, path, value);
                node.op = Some(
                    match op {
                        UnaryOp::Neg => "-",
                        UnaryOp::Not => "not",
                    }
                    .to_string(),
                );
                node.children.push(child);
                node
            }
            Expr::Binary { op, lhs, rhs } => {
                let a = self.expr_node(lhs, when, owner, &child_path(path, 0), depth);
                let b = self.expr_node(rhs, when, owner, &child_path(path, 1), depth);
                let value = binary(*op, a.value, b.value);
                let mut node = Node::new(NodeKind::Binary, path, value);
                node.op = Some(op.symbol().to_string());
                // N0403: a denominator that is a hair from a trap.
                if *op == BinaryOp::Div && b.value.abs() <= 1e-12 {
                    let msg = format!(
                        "denominator of `{}` is {} — within 1e-12 of zero",
                        expr_text(e),
                        b.value
                    );
                    self.note("N0403", path, &owner_name, when.t(), msg);
                }
                // N0401: the "why is my BEL zero" note.
                if *op == BinaryOp::Mul && value == 0.0 {
                    let zero = if a.value == 0.0 { lhs } else { rhs };
                    let msg = format!(
                        "value is exactly 0.0 because the factor `{}` is 0.0",
                        expr_text(zero)
                    );
                    self.note("N0401", path, &owner_name, when.t(), msg);
                }
                // N0202: adding a `start` value to an `end` value.
                if matches!(op, BinaryOp::Add | BinaryOp::Sub) {
                    if let (Some(x), Some(y)) = (self.timing_of(lhs), self.timing_of(rhs)) {
                        if x != y {
                            let msg = format!(
                                "timing-mismatched {}: `{}` is {x}, `{}` is {y}",
                                op.symbol(),
                                expr_text(lhs),
                                expr_text(rhs)
                            );
                            self.note("N0202", path, &owner_name, when.t(), msg);
                        }
                    }
                }
                node.children.push(a);
                node.children.push(b);
                node
            }
            Expr::If {
                cond,
                then,
                otherwise,
            } => {
                // Both arms are evaluated (`01-ir.md` §2.6) and both are
                // recorded; `taken` says which one the select returned.
                let c = self.expr_node(cond, when, owner, &child_path(path, 0), depth);
                let x = self.expr_node(then, when, owner, &child_path(path, 1), depth);
                let y = self.expr_node(otherwise, when, owner, &child_path(path, 2), depth);
                let taken = ops::truthy(c.value);
                let value = if taken { x.value } else { y.value };
                let mut node = Node::new(NodeKind::If, path, value);
                node.taken = Some(if taken { "then" } else { "else" }.to_string());
                // N0501: a branch that is dead for every period of this
                // projection — the arm the author thinks is live and is not.
                if let Some(dead) = self.dead_branch(cond, owner, when) {
                    let msg = format!(
                        "the `{dead}` branch of `{}` is never taken for any t in this projection",
                        expr_text(e)
                    );
                    self.note("N0501", path, &owner_name, None, msg);
                }
                node.children.push(c);
                node.children.push(x);
                node.children.push(y);
                node
            }
            Expr::Call { func, args } => self.call_node(func, args, when, owner, path, depth),
            Expr::Lookup { table, keys } => self.lookup_node(table, keys, when, owner, path, depth),
            Expr::Agg { op, value, pred } => {
                self.agg_node(*op, value, pred.as_deref(), owner, path)
            }
        }
    }

    /// A `Ref`, `Lag` or `At`: the value comes from the kernel's buffers, and
    /// the children are the referenced component's own tree (§3.3).
    fn read_node(
        &mut self,
        name: &str,
        target: When,
        kind: NodeKind,
        k: Option<u32>,
        path: &str,
        depth: i32,
    ) -> Node {
        let Some(slot) = self.ex.find_slot(name) else {
            return Node::new(NodeKind::Lit, path, 0.0);
        };
        let plan = self.plan();
        let info = plan.info(slot).clone();
        let space = plan.slot_refs[slot.index()].space;
        let series = plan.series_of(slot).cloned();

        // Below the origin: `init`, else the zero of the dtype (§2.7, §3.3).
        let (resolution, effective) = match (space, target) {
            (Space::Series, When::Cur(t)) if t < 0 => match series.as_ref().map(|s| &s.init) {
                Some(Some(_)) => ("init", When::Seed),
                _ => ("pre_origin_default", When::Cur(t)),
            },
            (Space::Series, When::Seed) => match series.as_ref().map(|s| &s.init) {
                Some(Some(_)) => ("init", When::Seed),
                _ => ("computed", When::Cur(0)),
            },
            _ => ("computed", target),
        };

        let value = if resolution == "pre_origin_default" {
            0.0
        } else {
            self.ex.read(slot, effective)
        };

        if info.kind.is_input() {
            let mut node = self.input_node(slot, effective, path, value);
            node.lag = k.filter(|_| kind == NodeKind::Lag);
            node.at = k.filter(|_| kind == NodeKind::At);
            if kind != NodeKind::Ref && space == Space::Series {
                node.t = target.t();
            }
            return node;
        }

        let mut node = Node::new(kind, path, value);
        node.reference = Some(info.qualified_id());
        // Only a series has a period. A `Ref` to a `Scalar` or `PerMP` is the
        // same value at every `t`, and stamping one on it would invite a reader
        // to believe it varies.
        node.t = target.t().filter(|_| space == Space::Series);
        node.lag = k.filter(|_| kind == NodeKind::Lag);
        node.at = k.filter(|_| kind == NodeKind::At);
        node.resolution = Some(resolution.to_string());
        if !self.options.values_only {
            node.kind = Some(info.kind.to_string());
            node.unit = Some(info.unit.to_string());
            node.timing = series.as_ref().map(|s| s.timing.to_string());
            node.span = self.ex.ctx.spans.get(&info.name).cloned();
        }
        match resolution {
            "init" => {
                node.init_expr = series.as_ref().and_then(|s| s.init.as_ref()).map(expr_text);
            }
            "pre_origin_default" => {
                node.note = Some("N0301".to_string());
                let msg = format!(
                    "`{}` has no `init`, so the read below the origin used the zero of {}",
                    info.name, info.dtype
                );
                self.note("N0301", path, &info.qualified_id(), target.t(), msg);
                return node;
            }
            _ => {}
        }

        // Expansion. `expand` overrides the depth budget for named components.
        let expanded = self.options.expand.contains(&info.name)
            || self.options.expand.contains(&info.qualified_id());
        if depth == 0 && !expanded {
            self.elided += 1;
            node.elided = Some(1);
            return node;
        }
        let next = if depth < 0 || expanded { -1 } else { depth - 1 };
        let inner = self.component_node(slot, effective, &child_path(path, 0), next, false);
        node.children.push(inner);
        node
    }

    fn call_node(
        &mut self,
        func: &str,
        args: &[Expr],
        when: When,
        owner: SlotId,
        path: &str,
        depth: i32,
    ) -> Node {
        // The timing builtins of §2.5 are series-level, not lanewise: lowering
        // re-expresses them as time-indexed loads, and so does the recorder, so
        // `shift`/`diff` are bit-identical to the lag they are sugar for.
        match (func, args.first()) {
            ("shift", Some(Expr::Ref { name })) => {
                let k = int_lit(args.get(1)).unwrap_or(0);
                let target = match when {
                    When::Seed => When::Seed,
                    When::Cur(t) => When::Cur(t - k),
                };
                let kind = if k == 0 { NodeKind::Ref } else { NodeKind::Lag };
                let mut node =
                    self.read_node(name, target, kind, Some(k.max(0) as u32), path, depth);
                node.func = Some("shift".to_string());
                return node;
            }
            ("retime", Some(inner)) => {
                // Timing is a tag, not a value: `retime` lowers to its own
                // argument, and the audit-relevant fact is that it happened.
                let child = self.expr_node(inner, when, owner, &child_path(path, 0), depth);
                let mut node = Node::new(NodeKind::Call, path, child.value);
                node.func = Some("retime".to_string());
                node.from_timing = self.timing_of(inner).map(|t| t.to_string());
                node.to_timing = args.get(1).map(expr_text);
                node.note = Some("N0201".to_string());
                let owner_name = self.plan().info(owner).qualified_id();
                let msg = format!(
                    "`retime` cast `{}` from {} to {} — the value is unchanged",
                    expr_text(inner),
                    node.from_timing.clone().unwrap_or_else(|| "?".into()),
                    node.to_timing.clone().unwrap_or_else(|| "?".into()),
                );
                self.note("N0201", path, &owner_name, when.t(), msg);
                node.children.push(child);
                return node;
            }
            ("diff", Some(Expr::Ref { name })) => {
                let cur =
                    self.read_node(name, when, NodeKind::Ref, None, &child_path(path, 0), depth);
                let prev_when = match when {
                    When::Seed => When::Seed,
                    When::Cur(t) => When::Cur(t - 1),
                };
                let prev = self.read_node(
                    name,
                    prev_when,
                    NodeKind::Lag,
                    Some(1),
                    &child_path(path, 1),
                    depth,
                );
                let mut node = Node::new(NodeKind::Call, path, cur.value - prev.value);
                node.func = Some("diff".to_string());
                node.children.push(cur);
                node.children.push(prev);
                return node;
            }
            ("cum", Some(Expr::Ref { name })) => {
                // `cum` is the running total *including* t; the kernel carries
                // it in an accumulator, the recorder re-adds it left to right in
                // the same order, which is the same sum.
                let t_now = match when {
                    When::Cur(t) => t.max(0) as u32,
                    When::Seed => 0,
                };
                let slot = self.ex.find_slot(name);
                let mut node = Node::new(NodeKind::Agg, path, 0.0);
                node.func = Some("cum".to_string());
                node.op = Some("cum".to_string());
                if let Some(slot) = slot {
                    let mut acc = 0.0;
                    let mut retained = Vec::new();
                    for t in 0..=t_now {
                        let x = self.ex.read(slot, When::Cur(i64::from(t)));
                        acc += x;
                        retained.push(Term {
                            t,
                            value: x,
                            disc: None,
                            contribution: x,
                            included: true,
                            stopped_here: false,
                        });
                    }
                    node.value = acc;
                    node.reference = Some(self.plan().info(slot).qualified_id());
                    node.term_count = Some(retained.len());
                    node.over = Some(AggOver {
                        component: self.plan().info(slot).qualified_id(),
                        timing: self.plan().series_of(slot).map(|s| s.timing.to_string()),
                        t_from: 0,
                        t_to: t_now,
                        discount: None,
                        exponent_rule: None,
                    });
                    let trimmed = truncate_terms(retained, self.options.max_terms);
                    node.terms_truncated = trimmed.1;
                    node.terms = trimmed.0;
                }
                return node;
            }
            _ => {}
        }

        let mut children = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            children.push(self.expr_node(a, when, owner, &child_path(path, i), depth));
        }
        let values: Vec<f64> = children.iter().map(|c| c.value).collect();
        let value = builtin(func, &values);
        let mut node = Node::new(NodeKind::Call, path, value);
        node.func = Some(func.to_string());
        node.children = children;
        node
    }

    fn lookup_node(
        &mut self,
        table: &str,
        keys: &[Expr],
        when: When,
        owner: SlotId,
        path: &str,
        depth: i32,
    ) -> Node {
        let mut children = Vec::with_capacity(keys.len());
        for (i, k) in keys.iter().enumerate() {
            children.push(self.expr_node(k, when, owner, &child_path(path, i), depth));
        }
        let raw: Vec<f64> = children.iter().map(|c| c.value).collect();

        let owner_name = self.plan().info(owner).qualified_id();
        let mut node = Node::new(NodeKind::Lookup, path, f64::NAN);
        node.table = Some(table.to_string());

        let Ok(binding) = bind_table(self.ex.engine.tables(), table) else {
            node.children = children;
            return node;
        };
        let compiled = &self.ex.engine.tables()[binding.table];
        let dict = self.ex.engine.dict();
        let args = crate::exec::key_args(compiled, &raw, dict);
        let outcome = compiled.lookup_f64(&args, binding.value);
        let (value, substituted) = match outcome {
            TableOutcome::Hit(v) => (v, false),
            TableOutcome::Substituted(v) => (v, true),
            TableOutcome::Trap(_) => (f64::NAN, false),
        };
        node.value = value;
        node.table_digest = Some(compiled.digest.clone());
        if let predictable_tables::RowHit::Row(r) = compiled.probe(&args) {
            node.row = Some(r);
        }

        // Key resolution: what the model asked for, what the table used, and
        // whether the policy changed it. This is where silent wrongness lives.
        for (i, arg) in args.iter().enumerate() {
            let index = compiled.key_index(i);
            let name = compiled
                .key_names()
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("key{i}"));
            let policy = key_policy(index);
            let requested = arg.render();
            let (resolved, fired, code) = match index.resolve(*arg) {
                KeyResolve::Slot(s) => {
                    let resolved = domain_value(index, s).unwrap_or_else(|| requested.clone());
                    let fired = resolved != requested;
                    let code = if !fired {
                        None
                    } else if policy == "clamp" {
                        Some("N0101")
                    } else {
                        Some("N0102")
                    };
                    (resolved, fired, code)
                }
                KeyResolve::Blend { lo, hi, w } => (
                    format!(
                        "{} .. {} (w = {w})",
                        domain_value(index, lo).unwrap_or_default(),
                        domain_value(index, hi).unwrap_or_default()
                    ),
                    true,
                    Some("N0103"),
                ),
                KeyResolve::Miss => (requested.clone(), false, None),
            };
            if let Some(code) = code {
                let msg = format!(
                    "lookup on {table} {} key {name} {requested} → {resolved}",
                    match code {
                        "N0101" => "clamped",
                        "N0102" => "stepped",
                        _ => "interpolated",
                    }
                );
                self.note(code, path, &owner_name, when.t(), msg);
            }
            node.keys.push(LookupKey {
                name,
                requested,
                resolved,
                policy: policy.to_string(),
                fired,
            });
        }
        if substituted {
            let msg = format!("lookup on {table} missed and used `on_missing = default(...)`");
            self.note("N0104", path, &owner_name, when.t(), msg);
            node.note = Some("N0104".to_string());
        }
        node.children = children;
        node
    }

    /// A stage-2 reduction, with the per-`t` contributions Q11 makes mandatory.
    fn agg_node(
        &mut self,
        op: AggOp,
        value: &Expr,
        pred: Option<&Expr>,
        owner: SlotId,
        path: &str,
    ) -> Node {
        let mut node = Node::new(NodeKind::Agg, path, 0.0);
        node.op = Some(terms::agg_name(op).to_string());
        let Some(series) = ref_name(value).and_then(|n| self.ex.find_slot(&n)) else {
            return node;
        };
        let t_max = self.ex.engine.layout().periods;
        let ctx = self.ex.engine.ctx();
        let state = self.ex.engine.state();
        let bufs = &self.ex.bufs;
        let series_timing = self.plan().series_of(series).map(|s| s.timing);
        node.reference = Some(self.plan().info(series).qualified_id());

        let (v, retained, disc_name, timing_used) = if op == AggOp::Npv {
            let Some(disc) = pred.and_then(ref_name).and_then(|n| self.ex.find_slot(&n)) else {
                return node;
            };
            let timing = series_timing.unwrap_or(Timing::End);
            let mut get = |t: u32| crate::reduce::value_at(&ctx, state, bufs, series, t, 0);
            let mut disc_at =
                |t: u32| crate::reduce::npv_factor(&ctx, state, bufs, disc, timing, t, 0);
            let (v, retained) = terms::record_npv(t_max, &mut get, &mut disc_at);
            (
                v,
                retained,
                Some(self.plan().info(disc).qualified_id()),
                Some(timing),
            )
        } else {
            let pred_slot = pred.and_then(ref_name).and_then(|n| self.ex.find_slot(&n));
            let mut get = |t: u32| crate::reduce::value_at(&ctx, state, bufs, series, t, 0);
            let mut keep = |t: u32| match pred_slot {
                None => true,
                Some(p) => ops::truthy(crate::reduce::value_at(&ctx, state, bufs, p, t, 0)),
            };
            let (v, retained) =
                terms::record_reduce(op, t_max, &mut get, &mut keep, pred_slot.is_some());
            (v, retained, None, None)
        };

        node.value = v;
        node.term_count = Some(retained.len());
        node.over = Some(AggOver {
            component: self.plan().info(series).qualified_id(),
            timing: series_timing.map(|t| t.to_string()),
            t_from: 0,
            t_to: t_max,
            discount: disc_name,
            exponent_rule: timing_used.map(exponent_rule),
        });
        let (kept, truncated) = truncate_terms(retained, self.options.max_terms);
        node.terms = kept;
        node.terms_truncated = truncated;
        if truncated {
            self.elided += 1;
        }
        let _ = owner;
        node
    }

    // -- helpers -----------------------------------------------------------

    /// Evaluate without recording — used by the `N0501` dead-branch probe.
    fn eval(&mut self, e: &Expr, when: When, owner: SlotId) -> f64 {
        match e {
            Expr::Lit { dtype, value } => self.lit_value(dtype, value),
            Expr::Ref { name } | Expr::Lag { name, .. } | Expr::At { name, .. } => {
                let target = match (e, when) {
                    (Expr::Lag { k, .. }, When::Cur(t)) => When::Cur(t - i64::from(*k)),
                    (Expr::At { k, .. }, _) => When::Cur(i64::from(*k)),
                    _ => when,
                };
                match self.ex.find_slot(name) {
                    None => 0.0,
                    Some(slot) => {
                        if let (Space::Series, When::Cur(t)) =
                            (self.plan().slot_refs[slot.index()].space, target)
                        {
                            if t < 0 {
                                return match self
                                    .plan()
                                    .series_of(slot)
                                    .and_then(|s| s.init.as_ref())
                                {
                                    Some(_) => self.ex.read(slot, When::Seed),
                                    None => 0.0,
                                };
                            }
                        }
                        self.ex.read(slot, target)
                    }
                }
            }
            Expr::Unary { op, operand } => {
                let x = self.eval(operand, when, owner);
                match op {
                    UnaryOp::Neg => -x,
                    UnaryOp::Not => f64::from(!ops::truthy(x)),
                }
            }
            Expr::Binary { op, lhs, rhs } => {
                let a = self.eval(lhs, when, owner);
                let b = self.eval(rhs, when, owner);
                binary(*op, a, b)
            }
            Expr::If {
                cond,
                then,
                otherwise,
            } => {
                let c = self.eval(cond, when, owner);
                let x = self.eval(then, when, owner);
                let y = self.eval(otherwise, when, owner);
                if ops::truthy(c) {
                    x
                } else {
                    y
                }
            }
            _ => {
                // Calls, lookups and aggregates are recorded rather than
                // re-derived: the probe only needs conditions, which are
                // arithmetic over reads.
                let path = "probe".to_string();
                let saved_notes = self.notes.len();
                let node = self.expr_node(e, when, owner, &path, 0);
                self.notes.truncate(saved_notes);
                node.value
            }
        }
    }

    /// `N0501`: is one arm of this `If` dead for every `t` in the projection?
    fn dead_branch(&mut self, cond: &Expr, owner: SlotId, when: When) -> Option<&'static str> {
        when.t()?;
        if self.plan().slot_refs[owner.index()].space != Space::Series {
            return None;
        }
        let t_max = self.ex.engine.layout().periods;
        let mut any_true = false;
        let mut any_false = false;
        for t in 0..=t_max {
            if ops::truthy(self.eval(cond, When::Cur(i64::from(t)), owner)) {
                any_true = true;
            } else {
                any_false = true;
            }
            if any_true && any_false {
                return None;
            }
        }
        Some(if any_true { "else" } else { "then" })
    }

    fn lit_value(&self, dtype: &DType, value: &LitValue) -> f64 {
        let _ = dtype;
        match value {
            LitValue::Float(x) => *x,
            LitValue::Int(i) => *i as f64,
            LitValue::Bool(b) => f64::from(*b),
            LitValue::Text(s) => self.ex.engine.dict().code_of(s),
        }
    }

    /// The timing tag of a direct series read, for `N0202` and `retime`.
    fn timing_of(&self, e: &Expr) -> Option<Timing> {
        let name = match e {
            Expr::Ref { name } | Expr::Lag { name, .. } | Expr::At { name, .. } => name,
            _ => return None,
        };
        let slot = self.ex.find_slot(name)?;
        self.plan().series_of(slot).map(|s| s.timing)
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

fn child_path(path: &str, i: usize) -> String {
    format!("{path}.children[{i}]")
}

fn same_bits(a: f64, b: f64) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

fn ref_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Ref { name } => Some(name.clone()),
        _ => None,
    }
}

fn int_lit(e: Option<&Expr>) -> Option<i64> {
    match e {
        Some(Expr::Lit {
            value: LitValue::Int(k),
            ..
        }) => Some(*k),
        _ => None,
    }
}

fn exponent_rule(timing: Timing) -> String {
    match timing {
        Timing::Start | Timing::Point => "v^t".to_string(),
        Timing::End => "v^(t+1)".to_string(),
        Timing::Mid => "v^(t+0.5)".to_string(),
    }
}

fn truncate_terms(terms: Vec<Term>, max: usize) -> (Vec<Term>, bool) {
    if max == 0 || terms.len() <= max {
        return (terms, false);
    }
    let half = max / 2;
    let tail_start = terms.len() - (max - half);
    let mut kept: Vec<Term> = terms[..half].to_vec();
    kept.extend_from_slice(&terms[tail_start..]);
    (kept, true)
}

fn key_policy(index: &KeyIndex) -> &'static str {
    match index {
        KeyIndex::SortedInt { policy, .. } | KeyIndex::SortedFloat { policy, .. } => match policy {
            KeyPolicy::Exact => "exact",
            KeyPolicy::Clamp => "clamp",
            KeyPolicy::Step => "step",
            KeyPolicy::Interpolate => "interpolate",
        },
        _ => "exact",
    }
}

fn domain_value(index: &KeyIndex, slot: u32) -> Option<String> {
    match index {
        KeyIndex::SortedInt { domain, .. } => domain.get(slot as usize).map(|v| v.to_string()),
        KeyIndex::SortedFloat { domain, .. } => domain.get(slot as usize).map(|v| v.to_string()),
        KeyIndex::Dictionary { domain } => domain.get(slot as usize).cloned(),
        // A dense or hashed index is exact by construction: the slot *is* the
        // key, so the resolved value is whatever was asked for.
        KeyIndex::DenseInt { .. } | KeyIndex::PerfectHash(_) => None,
    }
}

/// The scalar semantics of a binary operator, delegating every trapping case to
/// [`crate::ops`] so the recorder cannot disagree with the kernel.
fn binary(op: BinaryOp, a: f64, b: f64) -> f64 {
    use predictable_tape::CmpOp;
    match op {
        BinaryOp::Add => a + b,
        BinaryOp::Sub => a - b,
        BinaryOp::Mul => a * b,
        BinaryOp::Div => ops::div(a, b).0,
        BinaryOp::Pow => ops::pow(a, b).0,
        BinaryOp::Eq => ops::cmp(CmpOp::Eq, a, b),
        BinaryOp::Ne => ops::cmp(CmpOp::Ne, a, b),
        BinaryOp::Lt => ops::cmp(CmpOp::Lt, a, b),
        BinaryOp::Le => ops::cmp(CmpOp::Le, a, b),
        BinaryOp::Gt => ops::cmp(CmpOp::Gt, a, b),
        BinaryOp::Ge => ops::cmp(CmpOp::Ge, a, b),
        BinaryOp::And => f64::from(ops::truthy(a) && ops::truthy(b)),
        BinaryOp::Or => f64::from(ops::truthy(a) || ops::truthy(b)),
    }
}

/// The builtins, by the same dispatch the tape lowering uses.
fn builtin(func: &str, args: &[f64]) -> f64 {
    use predictable_tape::{CmpOp, Fn1, Fn2, FnN};
    let a = args.first().copied().unwrap_or(0.0);
    let b = args.get(1).copied().unwrap_or(0.0);
    match func {
        "eq" => return ops::cmp(CmpOp::Eq, a, b),
        "ne" => return ops::cmp(CmpOp::Ne, a, b),
        "lt" => return ops::cmp(CmpOp::Lt, a, b),
        "le" => return ops::cmp(CmpOp::Le, a, b),
        "gt" => return ops::cmp(CmpOp::Gt, a, b),
        "ge" => return ops::cmp(CmpOp::Ge, a, b),
        "and" => return f64::from(ops::truthy(a) && ops::truthy(b)),
        "or" => return f64::from(ops::truthy(a) || ops::truthy(b)),
        "not" => return f64::from(!ops::truthy(a)),
        "pow" => return ops::pow(a, b).0,
        "clamp" => return ops::applyn(FnN::Clamp, args).0,
        "year_frac" => return ops::applyn(FnN::YearFrac, args).0,
        _ => {}
    }
    let fn1 = match func {
        "abs" => Some(Fn1::Abs),
        "floor" => Some(Fn1::Floor),
        "ceil" => Some(Fn1::Ceil),
        "sign" => Some(Fn1::Sign),
        "exp" => Some(Fn1::Exp),
        "ln" => Some(Fn1::Ln),
        "sqrt" => Some(Fn1::Sqrt),
        "to_monthly" => Some(Fn1::ToMonthly),
        "to_annual" => Some(Fn1::ToAnnual),
        "v_from_i" => Some(Fn1::VFromI),
        "i_from_v" => Some(Fn1::IFromV),
        "year" => Some(Fn1::Year),
        "month" => Some(Fn1::Month),
        "day" => Some(Fn1::Day),
        "is_null" => Some(Fn1::IsNull),
        _ => None,
    };
    if let Some(f) = fn1 {
        return ops::apply1(f, a).0;
    }
    let fn2 = match func {
        "min" => Some(Fn2::Min),
        "max" => Some(Fn2::Max),
        "round" => Some(Fn2::Round),
        "nominal_to_periodic" => Some(Fn2::NominalToPeriodic),
        "annuity_factor" => Some(Fn2::AnnuityFactor),
        "compound" => Some(Fn2::Compound),
        "coalesce" => Some(Fn2::Coalesce),
        "add_months" => Some(Fn2::AddMonths),
        "months_between" => Some(Fn2::MonthsBetween),
        _ => None,
    };
    match fn2 {
        Some(f) => ops::apply2(f, a, b).0,
        None => f64::NAN,
    }
}

// ---------------------------------------------------------------------------
// Expression text
// ---------------------------------------------------------------------------

/// The canonical infix rendering of an expression (`01-ir.md` §4.1), used for a
/// component node's `expr` and for the labels in the text rendering.
pub fn expr_text(e: &Expr) -> String {
    render_expr(e, 0)
}

fn precedence(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => 1,
        BinaryOp::And => 2,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            3
        }
        BinaryOp::Add | BinaryOp::Sub => 4,
        BinaryOp::Mul | BinaryOp::Div => 5,
        BinaryOp::Pow => 6,
    }
}

fn render_expr(e: &Expr, outer: u8) -> String {
    match e {
        Expr::Lit { value, .. } => match value {
            LitValue::Text(s) => format!("\"{s}\""),
            other => other.to_string(),
        },
        Expr::Ref { name } => name.clone(),
        Expr::Lag { name, k } => format!("{name}[t-{k}]"),
        Expr::At { name, k } => format!("{name}[{k}]"),
        Expr::Unary { op, operand } => match op {
            UnaryOp::Neg => format!("-{}", render_expr(operand, 7)),
            UnaryOp::Not => format!("not {}", render_expr(operand, 7)),
        },
        Expr::Binary { op, lhs, rhs } => {
            let p = precedence(*op);
            let text = format!(
                "{} {} {}",
                render_expr(lhs, p),
                op.symbol(),
                render_expr(rhs, p + 1)
            );
            if p < outer {
                format!("({text})")
            } else {
                text
            }
        }
        Expr::If {
            cond,
            then,
            otherwise,
        } => format!(
            "if({}, {}, {})",
            render_expr(cond, 0),
            render_expr(then, 0),
            render_expr(otherwise, 0)
        ),
        Expr::Call { func, args } => format!(
            "{func}({})",
            args.iter()
                .map(|a| render_expr(a, 0))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Lookup { table, keys } => format!(
            "{table}@({})",
            keys.iter()
                .map(|k| render_expr(k, 0))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Agg { op, value, pred } => {
            let name = terms::agg_name(*op);
            match pred {
                Some(p) => format!("{name}({}, {})", render_expr(value, 0), render_expr(p, 0)),
                None => format!("{name}({})", render_expr(value, 0)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expression_text_parenthesises_by_precedence() {
        let e = Expr::binary(
            BinaryOp::Mul,
            Expr::binary(BinaryOp::Add, Expr::r#ref("a"), Expr::r#ref("b")),
            Expr::r#ref("c"),
        );
        assert_eq!(expr_text(&e), "(a + b) * c");

        let flat = Expr::binary(
            BinaryOp::Add,
            Expr::binary(BinaryOp::Mul, Expr::r#ref("a"), Expr::r#ref("b")),
            Expr::r#ref("c"),
        );
        assert_eq!(expr_text(&flat), "a * b + c");
    }

    #[test]
    fn truncation_keeps_both_ends_and_says_so() {
        let terms: Vec<Term> = (0..10)
            .map(|t| Term {
                t,
                value: t as f64,
                disc: None,
                contribution: t as f64,
                included: true,
                stopped_here: false,
            })
            .collect();
        let (kept, truncated) = truncate_terms(terms, 4);
        assert!(truncated);
        assert_eq!(kept.len(), 4);
        assert_eq!(kept[0].t, 0);
        assert_eq!(kept[3].t, 9);
    }

    #[test]
    fn binary_delegates_traps_to_the_kernels_own_ops() {
        assert!(binary(BinaryOp::Div, 1.0, 0.0).is_nan());
        assert_eq!(binary(BinaryOp::Pow, 2.0, 10.0), 1024.0);
    }
}
