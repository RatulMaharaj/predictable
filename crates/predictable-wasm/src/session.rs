//! A model, compiled once and projected many times, with no host underneath.
//!
//! A [`Session`] is the wasm build's whole state: sources checked, planned and
//! lowered; tables resolved from embedded bytes and digest-checked; modelpoints
//! read into lanes. Everything after that — [`Session::project`],
//! [`Session::explain`] — is the ordinary engine, and is deliberately reached
//! through the same types the CLI uses so there is no second implementation to
//! keep in step.
//!
//! Two choices that differ from a native run, both forced and neither able to
//! change a number:
//!
//! * the plan is built with `retain_all` (`03-engine.md` §4.3), because the page
//!   may explain any cell at any `t` and a ring buffer has already overwritten
//!   the history such a trace asks about;
//! * chunking is capped at [`MAX_CHUNK`] rather than 1024, because `wasm32` is a
//!   32-bit address space (§9's 512 MB budget). §7 clause 1 is what makes that
//!   free of consequence: `run(C=1) ≡ run(C=1024)`.

use std::collections::BTreeMap;
use std::path::Path;

use predictable_check::Input;
use predictable_diagnostics::Diagnostic;
use predictable_engine::explain::{ExplainOptions, Explainer, Trace, TraceContext};
use predictable_engine::{ChunkInput, Engine, RunConfig, TrapPolicy};
use predictable_ir::model::{TableSource, Timeline};
use predictable_ir::TableDecl;
use predictable_ir::{DType, Module};
use predictable_plan::{plan_sources, OptLevel, Plan, PlanOptions};
use predictable_tables::{
    CompiledTable, InlineResolver, LoadOptions, ResolveError, TableFormat, TableResolver,
};
use predictable_tape::{lower_plan, TapeProgram};

/// Modelpoints per chunk in the browser (`03-engine.md` §9: `C` defaults to 256
/// on `wasm32`). Any value gives the same answer.
pub const MAX_CHUNK: usize = 256;

/// One `.pir` file, as text.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SourceFile {
    /// Display path — what diagnostics point at, and the order `model_digest`
    /// hashes in.
    pub name: String,
    /// The file's text.
    pub text: String,
}

/// One table's bytes, keyed by the `source` string its declaration carries.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TableBytes {
    /// `tables/mortality.csv` for a `File` source, or the bare name for
    /// `resource:<name>`.
    pub name: String,
    /// The file's text. CSV unless `format` says otherwise.
    pub text: String,
}

/// Everything a session needs. There is no path in here, by design.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct Inputs {
    /// The model's `.pir` files, in the order `model_digest` hashes them.
    pub sources: Vec<SourceFile>,
    /// Assumption values, by name.
    #[serde(default)]
    pub assumptions: BTreeMap<String, f64>,
    /// Table bytes, by `source` string.
    #[serde(default)]
    pub tables: Vec<TableBytes>,
    /// The modelpoint CSV.
    #[serde(default)]
    pub modelpoints: String,
    /// Accept a table whose bytes no longer match its declared digest. Off by
    /// default: a pack whose tables drifted is a pack that cannot reproduce its
    /// own numbers.
    #[serde(default)]
    pub allow_table_drift: bool,
}

/// What went wrong. Every variant is something a caller can act on; none of
/// them is "the engine is unavailable", which is the failure a browser build is
/// most tempted to invent.
#[derive(Debug)]
pub enum SessionError {
    /// The model does not check. Carries the checker's own diagnostics, so the
    /// page shows the message the CLI would print (§9: "the same messages").
    Check(Vec<Diagnostic>),
    /// Planning, lowering, table loading or projection failed.
    Engine(String),
    /// The model declares no `[timeline]`, so there is nothing to project.
    NoTimeline,
    /// A modelpoint key that is not in the embedded file.
    UnknownModelpoint(String),
    /// A component that the model does not emit.
    UnknownComponent(String),
    /// The modelpoint CSV is missing a declared column.
    MissingColumn(String),
    /// A cell in the modelpoint CSV is not a value of its declared type.
    BadValue {
        /// Column name.
        field: String,
        /// Row index in the file, 0-based, excluding the header.
        row: usize,
        /// The text that would not parse.
        text: String,
    },
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Check(d) => write!(f, "the model does not check ({} error(s))", d.len()),
            SessionError::Engine(e) => f.write_str(e),
            SessionError::NoTimeline => f.write_str("the model declares no [timeline]"),
            SessionError::UnknownModelpoint(k) => write!(f, "no modelpoint `{k}` in this pack"),
            SessionError::UnknownComponent(c) => write!(f, "`{c}` is not an output of this model"),
            SessionError::MissingColumn(c) => write!(f, "the modelpoint file has no `{c}` column"),
            SessionError::BadValue { field, row, text } => {
                write!(f, "row {row}: `{text}` is not a valid `{field}`")
            }
        }
    }
}

impl std::error::Error for SessionError {}

/// The resolver a pack uses: table bytes by the `source` string, no filesystem.
///
/// `MapResolver` answers `resource:` only; a pack embeds tables that were
/// declared as ordinary relative paths, so those must resolve too — by name,
/// never by touching a path. `inline` delegates to [`InlineResolver`] so an
/// inline table hashes exactly as `predictable fmt` writes it.
#[derive(Debug, Clone, Default)]
pub struct PackResolver {
    entries: BTreeMap<String, Vec<u8>>,
}

impl PackResolver {
    /// An empty resolver.
    pub fn new() -> PackResolver {
        PackResolver::default()
    }

    /// Add one table's bytes under the `source` string that names it.
    pub fn insert(&mut self, name: impl Into<String>, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.entries.insert(name.into(), bytes.into());
        self
    }
}

impl TableResolver for PackResolver {
    fn resolve(
        &self,
        decl: &TableDecl,
        base: &Path,
    ) -> Result<predictable_tables::TableBytes, ResolveError> {
        let name = match &decl.source {
            TableSource::File(p) => p.clone(),
            TableSource::Resource(n) => n.clone(),
            TableSource::Inline => return InlineResolver.resolve(decl, base),
        };
        let bytes = self
            .entries
            .get(&name)
            .ok_or_else(|| ResolveError::UnknownResource {
                table: decl.name.clone(),
                name: name.clone(),
            })?;
        Ok(predictable_tables::TableBytes {
            bytes: bytes.clone(),
            origin: format!("pack:{name}"),
            format: TableFormat::Csv,
        })
    }
}

/// One modelpoint column, in the two forms the kernel accepts.
#[derive(Debug, Clone, PartialEq)]
enum Column {
    Num(Vec<f64>),
    Text(Vec<String>),
}

/// A compiled model plus its data, ready to project or trace.
#[derive(Debug)]
pub struct Session {
    plan: Plan,
    tapes: TapeProgram,
    modules: Vec<Module>,
    tables: Vec<CompiledTable>,
    timeline: Timeline,
    assumptions: BTreeMap<String, f64>,
    keys: Vec<String>,
    columns: BTreeMap<String, Column>,
    model_digest: String,
}

impl Session {
    /// Check, plan, lower, load tables, read modelpoints. Everything that can
    /// fail, fails here rather than on the first `explain()` in the page.
    pub fn new(inputs: &Inputs) -> Result<Session, SessionError> {
        let checked: Vec<Input> = inputs
            .sources
            .iter()
            .map(|s| Input::new(s.name.clone(), s.text.clone()))
            .collect();
        let result = predictable_check::check(&checked);
        if !result.is_ok() {
            return Err(SessionError::Check(result.errors().cloned().collect()));
        }

        let model_digest = model_digest(&checked);
        let options = PlanOptions {
            opt: OptLevel::O1,
            // §4.3: a trace at `t` reads history a ring buffer has overwritten.
            retain_all: true,
            program_digest: model_digest.clone(),
            periods: None,
        };
        let plan =
            plan_sources(&checked, &options).map_err(|e| SessionError::Engine(e.to_string()))?;
        let tapes =
            lower_plan(&plan).map_err(|e| SessionError::Engine(format!("lowering: {e}")))?;
        let modules = predictable_plan::lower_modules(&checked);

        let mut resolver = PackResolver::new();
        for table in &inputs.tables {
            resolver.insert(table.name.clone(), table.text.clone().into_bytes());
        }
        let load_options = LoadOptions {
            allow_table_drift: inputs.allow_table_drift,
        };
        let mut tables = Vec::new();
        for module in &modules {
            for decl in &module.tables {
                tables.push(
                    predictable_tables::load(decl, Path::new("."), &resolver, &load_options)
                        .map_err(|e| SessionError::Engine(format!("table `{}`: {e}", decl.name)))?,
                );
            }
        }

        let timeline = modules
            .iter()
            .find_map(|m| m.timeline.clone())
            .ok_or(SessionError::NoTimeline)?;

        let (keys, columns) = read_modelpoints(&modules, &inputs.modelpoints)?;

        Ok(Session {
            plan,
            tapes,
            modules,
            tables,
            timeline,
            assumptions: inputs.assumptions.clone(),
            keys,
            columns,
            model_digest,
        })
    }

    /// `model_digest` over the canonical text of every source.
    pub fn model_digest(&self) -> &str {
        &self.model_digest
    }

    /// `plan_digest` — the pack's proof that the page planned what the run
    /// planned.
    pub fn plan_digest(&self) -> &str {
        &self.plan.digest
    }

    /// `order_digest`.
    pub fn order_digest(&self) -> &str {
        &self.plan.order_digest
    }

    /// Projection length `T`.
    pub fn periods(&self) -> u32 {
        self.timeline.periods
    }

    /// Modelpoint keys, in file order.
    pub fn modelpoint_keys(&self) -> &[String] {
        &self.keys
    }

    /// The output components this model emits, in plan order.
    pub fn outputs(&self) -> Vec<String> {
        self.plan
            .outputs
            .iter()
            .map(|slot| self.plan.info(*slot).qualified_id())
            .collect()
    }

    /// Project modelpoints and return the requested output columns.
    ///
    /// `overrides` is applied over the session's assumptions — that, and only
    /// that, is what a sensitivity scenario varies (`05-viz.md` §3.3). `keys`
    /// selects modelpoints; empty means all of them.
    pub fn project(
        &self,
        keys: &[String],
        overrides: &BTreeMap<String, f64>,
    ) -> Result<Projection, SessionError> {
        let lanes = self.select(keys)?;
        let mut assumptions = self.assumptions.clone();
        for (k, v) in overrides {
            assumptions.insert(k.clone(), *v);
        }

        let chunk = lanes.len().clamp(1, MAX_CHUNK);
        let mut engine = Engine::new(
            &self.plan,
            &self.tapes,
            self.tables.clone(),
            &self.timeline,
            RunConfig {
                chunk,
                on_trap: TrapPolicy::Continue,
                max_errors: 100,
            },
        )
        .map_err(|e| SessionError::Engine(e.to_string()))?;
        let mut bufs = engine.buffers();
        engine
            .prepare(&assumptions, &mut bufs)
            .map_err(|e| SessionError::Engine(e.to_string()))?;

        let mut out = Projection {
            keys: Vec::new(),
            periods: self.timeline.periods,
            columns: BTreeMap::new(),
            strides: BTreeMap::new(),
            dropped: Vec::new(),
        };
        for (index, window) in lanes.chunks(chunk).enumerate() {
            let input = self.chunk_input(&mut engine, index as u32, window);
            let output = engine
                .run_chunk(&mut bufs, &input)
                .map_err(|e| SessionError::Engine(e.to_string()))?;
            out.keys.extend(output.keys.iter().cloned());
            out.dropped.extend(output.dropped.iter().cloned());
            for column in &output.columns {
                out.strides.insert(column.name.clone(), column.stride);
                out.columns
                    .entry(column.name.clone())
                    .or_default()
                    .extend_from_slice(&column.values);
            }
        }
        Ok(out)
    }

    /// Trace one cell of one modelpoint — the call §1.5 calls the highest-value
    /// use of wasm, because it turns a frozen pack into one that answers "where
    /// did this number come from" for *any* cell, not the pre-baked few.
    pub fn explain(
        &self,
        component: &str,
        modelpoint: &str,
        t: Option<u32>,
        options: &ExplainOptions,
        overrides: &BTreeMap<String, f64>,
    ) -> Result<Trace, SessionError> {
        let lane = self
            .keys
            .iter()
            .position(|k| k == modelpoint)
            .ok_or_else(|| SessionError::UnknownModelpoint(modelpoint.to_string()))?;
        let mut assumptions = self.assumptions.clone();
        for (k, v) in overrides {
            assumptions.insert(k.clone(), *v);
        }

        let mut ctx = TraceContext::from_modules(&self.modules);
        ctx.modelpoint_file = Some("pack:modelpoints".to_string());
        let mut explainer = Explainer::new(
            &self.plan,
            &self.tapes,
            self.tables.clone(),
            &self.timeline,
            ctx,
        )
        .map_err(|e| SessionError::Engine(e.to_string()))?;
        explainer
            .prepare(&assumptions)
            .map_err(|e| SessionError::Engine(e.to_string()))?;
        let input = self.chunk_input(explainer.engine_mut(), 0, &[lane]);
        explainer
            .load(&input)
            .map_err(|e| SessionError::Engine(e.to_string()))?;
        explainer
            .explain(component, t, options)
            .map_err(|e| match e {
                predictable_engine::ExplainError::UnknownComponent(c) => {
                    SessionError::UnknownComponent(c)
                }
                other => SessionError::Engine(other.to_string()),
            })
    }

    /// Lane indices for the requested keys, in file order. Empty means all.
    fn select(&self, keys: &[String]) -> Result<Vec<usize>, SessionError> {
        if keys.is_empty() {
            return Ok((0..self.keys.len()).collect());
        }
        let wanted: std::collections::BTreeSet<&String> = keys.iter().collect();
        for key in keys {
            if !self.keys.contains(key) {
                return Err(SessionError::UnknownModelpoint(key.clone()));
            }
        }
        Ok((0..self.keys.len())
            .filter(|i| wanted.contains(&self.keys[*i]))
            .collect())
    }

    /// Build one chunk, interning text into *this* engine's dictionary — the
    /// rule `predictable-runner`'s `Chunk::bind` exists to enforce.
    fn chunk_input(&self, engine: &mut Engine<'_>, index: u32, lanes: &[usize]) -> ChunkInput {
        let mut columns = BTreeMap::new();
        for (name, column) in &self.columns {
            let values: Vec<f64> = match column {
                Column::Num(v) => lanes.iter().map(|i| v[*i]).collect(),
                Column::Text(v) => lanes.iter().map(|i| engine.intern(&v[*i])).collect(),
            };
            columns.insert(name.clone(), values);
        }
        ChunkInput {
            index,
            keys: lanes.iter().map(|i| self.keys[*i].clone()).collect(),
            first_row: lanes.first().copied().unwrap_or(0) as u64,
            columns,
        }
    }
}

/// What a projection produced, in the layout the page consumes.
#[derive(Debug, Clone, Default)]
pub struct Projection {
    /// Surviving modelpoint keys, lane order.
    pub keys: Vec<String>,
    /// `T`.
    pub periods: u32,
    /// Output name → values, lane-major.
    pub columns: BTreeMap<String, Vec<f64>>,
    /// Output name → values per lane (`T+1` for a series, 1 otherwise).
    pub strides: BTreeMap<String, usize>,
    /// Modelpoints abandoned at a trap.
    pub dropped: Vec<String>,
}

/// `model_digest` over canonical text (`01-ir.md` §9.6), or empty when a file
/// has no canonical form — which cannot happen for checked input, but is not
/// worth a panic in a browser.
fn model_digest(inputs: &[Input]) -> String {
    let mut files = Vec::with_capacity(inputs.len());
    for input in inputs {
        match predictable_fmt::canonical::format_source(&input.name, &input.text) {
            Ok(text) => files.push(predictable_fmt::digest::CanonicalFile {
                path: input.name.clone(),
                text,
            }),
            Err(_) => return String::new(),
        }
    }
    predictable_fmt::digest::model_digest(&files)
}

/// The modelpoint CSV, read against the schema the IR declares.
fn read_modelpoints(
    modules: &[Module],
    text: &str,
) -> Result<(Vec<String>, BTreeMap<String, Column>), SessionError> {
    let fields: Vec<_> = modules
        .iter()
        .flat_map(|m| m.modelpoint_fields.iter().cloned())
        .collect();
    let key_field = fields
        .iter()
        .find(|f| f.key)
        .ok_or_else(|| SessionError::MissingColumn("<key>".to_string()))?
        .name
        .clone();

    let rows = crate::csv::rows(text);
    let Some(header) = rows.first() else {
        return Ok((Vec::new(), BTreeMap::new()));
    };
    let index_of = |name: &str| header.iter().position(|h| h.trim() == name);
    let body = &rows[1..];

    let mut columns: BTreeMap<String, Column> = BTreeMap::new();
    for field in &fields {
        // Optional fields are not optional *columns*: §2.11 eliminates
        // missingness at load, so the writer of the file still has to emit the
        // column. A missing one is a schema mismatch either way.
        let col =
            index_of(&field.name).ok_or_else(|| SessionError::MissingColumn(field.name.clone()))?;
        let cells = body.iter().enumerate().map(|(row, r)| {
            (
                row,
                r.get(col)
                    .map(String::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
            )
        });
        let column = match field.dtype {
            DType::Str | DType::Enum(_) => {
                Column::Text(cells.map(|(_, text)| text).collect::<Vec<_>>())
            }
            DType::Date => {
                let mut values = Vec::new();
                for (row, text) in cells {
                    let days = predictable_engine::dates::parse_iso(&text).ok_or_else(|| {
                        SessionError::BadValue {
                            field: field.name.clone(),
                            row,
                            text: text.clone(),
                        }
                    })?;
                    values.push(days as f64);
                }
                Column::Num(values)
            }
            DType::Bool => {
                let mut values = Vec::new();
                for (row, text) in cells {
                    let v = match text.as_str() {
                        "true" | "True" | "TRUE" | "1" => 1.0,
                        "false" | "False" | "FALSE" | "0" => 0.0,
                        _ => {
                            return Err(SessionError::BadValue {
                                field: field.name.clone(),
                                row,
                                text,
                            })
                        }
                    };
                    values.push(v);
                }
                Column::Num(values)
            }
            DType::I64 => {
                let mut values = Vec::new();
                for (row, text) in cells {
                    let v: i64 = text.parse().map_err(|_| SessionError::BadValue {
                        field: field.name.clone(),
                        row,
                        text: text.clone(),
                    })?;
                    values.push(v as f64);
                }
                Column::Num(values)
            }
            DType::F64 => {
                let mut values = Vec::new();
                for (row, text) in cells {
                    let v: f64 = text.parse().map_err(|_| SessionError::BadValue {
                        field: field.name.clone(),
                        row,
                        text: text.clone(),
                    })?;
                    values.push(v);
                }
                Column::Num(values)
            }
        };
        columns.insert(field.name.clone(), column);
    }

    let keys = match columns.get(&key_field) {
        Some(Column::Text(v)) => v.clone(),
        Some(Column::Num(v)) => v.iter().map(|x| format!("{}", *x as i64)).collect(),
        None => return Err(SessionError::MissingColumn(key_field)),
    };
    Ok((keys, columns))
}
