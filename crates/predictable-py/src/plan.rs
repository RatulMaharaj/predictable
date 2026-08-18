//! [`Plan`] — a checked, ordered, lowered model, and `plan.run(...)`.
//!
//! Everything expensive and everything deterministic happens here: slot allocation,
//! the evaluation order and its `order_digest`, tape lowering. A `Plan` is reusable
//! — running it twice with different modelpoints does not re-plan — which is what
//! makes a sensitivity fan cheap and what makes `order_digest` a stable identity.
//!
//! `run()` releases the GIL for the whole projection (`03-engine.md` §8.2). It can,
//! because IR rule 1 forbids a Python callback during projection: nothing inside the
//! `t` loop can call back into the interpreter, so there is no re-entrancy to guard.
//! The progress callback is invoked *between chunk groups*, with the GIL retaken,
//! at a rate no faster than every 100 ms, and it receives counts only.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use predictable_engine::explain::{ExplainError, ExplainOptions, Explainer, TraceContext};
use predictable_io::{ArrowSource, CsvSource, ModelpointSource, MpSchema, ParquetSource};
use predictable_ir::run::RunFile;
use predictable_ir::Module;
use predictable_plan::Plan as CorePlan;
use predictable_runner::cancel::CancelFlag;
use predictable_runner::chunk::{load_chunks, Chunk};
use predictable_runner::executor::{
    ChunkExecutor, ChunkResult, LocalExecutor, SerialExecutor, WorkerFactory,
};
use predictable_runner::pipeline::{RunInputs, Runner};
use predictable_runner::solver::{self, SolveResult};
use predictable_tables::{CompiledTable, FsResolver, LoadOptions};
use predictable_tape::TapeProgram;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::errors::{plain, Kind};
use crate::program::Program;
use crate::results::Run;
use crate::trace::Trace;

/// A checked model, ordered and lowered, ready to be run.
#[pyclass(module = "predictable_engine")]
#[derive(Debug)]
pub struct Plan {
    pub(crate) program: Program,
    pub(crate) plan: CorePlan,
    pub(crate) tapes: TapeProgram,
    pub(crate) modules: Vec<Module>,
    pub(crate) run: RunFile,
}

/// A chunk executor that lets Python breathe between groups of chunks.
///
/// It delegates every chunk to `inner` — the numbers are the inner executor's, and
/// so is the order — and only interposes at group boundaries, where it retakes the
/// GIL to run Python's signal handlers (so `Ctrl-C` sets the cancel flag rather than
/// needing `SIGKILL`) and to call the progress callback.
struct GilExecutor {
    inner: Box<dyn ChunkExecutor>,
    progress: Option<Py<PyAny>>,
    group: usize,
    done: AtomicU64,
    total: u64,
    last_tick: Mutex<Instant>,
    interrupted: Arc<AtomicBool>,
}

impl ChunkExecutor for GilExecutor {
    fn map_chunks<'w>(
        &self,
        chunks: &[Chunk],
        make_worker: &WorkerFactory<'w>,
        cancel: &CancelFlag,
    ) -> Vec<ChunkResult> {
        let mut out = Vec::with_capacity(chunks.len());
        for group in chunks.chunks(self.group.max(1)) {
            out.extend(self.inner.map_chunks(group, make_worker, cancel));
            let done =
                self.done.fetch_add(group.len() as u64, Ordering::SeqCst) + group.len() as u64;
            let due = {
                let mut last = self.last_tick.lock().expect("progress clock");
                if last.elapsed().as_millis() >= 100 || done >= self.total {
                    *last = Instant::now();
                    true
                } else {
                    false
                }
            };
            let interrupted = Python::with_gil(|py| {
                // §8.2: `Ctrl-C` sets the cancel flag; the runner stops after the
                // current chunk and the manifest says `cancelled` rather than
                // pretending a truncated result set is complete.
                if py.check_signals().is_err() {
                    return true;
                }
                if due {
                    if let Some(cb) = self.progress.as_ref().map(|cb| cb.bind(py)) {
                        // A progress callback that raises is a cancellation request,
                        // not a run failure: it cannot influence the numbers either way.
                        if cb.call1((done, self.total)).is_err() {
                            return true;
                        }
                    }
                }
                false
            });
            if interrupted {
                self.interrupted.store(true, Ordering::SeqCst);
                cancel.cancel();
            }
            if cancel.is_cancelled() {
                // Report the untouched tail honestly rather than silently short.
                for _ in out.len()..chunks.len() {
                    out.push(ChunkResult::Cancelled);
                }
                break;
            }
        }
        out
    }

    fn threads(&self) -> usize {
        self.inner.threads()
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }
}

impl Plan {
    /// The one module that declares the modelpoint schema.
    fn schema_module(&self) -> PyResult<&Module> {
        let mut with_fields = self
            .modules
            .iter()
            .filter(|m| !m.modelpoint_fields.is_empty());
        let first = with_fields.next().ok_or_else(|| {
            plain(
                Kind::Data,
                "no module declares any `[[modelpoint_field]]`, so there is no modelpoint schema to read against",
            )
        })?;
        if let Some(second) = with_fields.next() {
            return Err(plain(
                Kind::Data,
                format!(
                    "two modules declare a modelpoint schema (`{}` and `{}`); a run reads one file against one schema",
                    first.module, second.module
                ),
            ));
        }
        Ok(first)
    }

    fn tables(&self, allow_drift: bool) -> PyResult<Vec<CompiledTable>> {
        let resolver = FsResolver::new();
        let options = LoadOptions {
            allow_table_drift: allow_drift,
        };
        let mut out = Vec::new();
        for module in &self.modules {
            for decl in &module.tables {
                // The `[run.tables]` override replaces `source`, never `digest` (§2.9.1).
                let mut decl = decl.clone();
                if let Some(source) = self.run.run.tables.get(&decl.name) {
                    decl.source = predictable_ir::TableSource::File(source.clone());
                }
                out.push(
                    predictable_tables::load(&decl, &self.program.base, &resolver, &options)
                        .map_err(|e| plain(Kind::Data, format!("table `{}`: {e}", decl.name)))?,
                );
            }
        }
        Ok(out)
    }
}

#[pymethods]
impl Plan {
    /// `sha256(program_digest ‖ run_config ‖ engine major)` — the plan's identity.
    #[getter]
    fn digest(&self) -> String {
        self.plan.digest.clone()
    }

    /// The hash of the evaluation order (`03-engine.md` §3.2).
    #[getter]
    fn order_digest(&self) -> String {
        self.plan.order_digest.clone()
    }

    /// `T` from `[timeline].periods`; the projection runs `t = 0..=periods`.
    #[getter]
    fn periods(&self) -> u32 {
        self.plan.periods
    }

    /// The qualified component ids the result set will carry, in evaluation order.
    #[getter]
    fn components(&self) -> PyResult<Vec<String>> {
        let runner = self.bind_runner(Vec::new())?;
        Ok(runner.emitted().iter().map(|c| c.id.clone()).collect())
    }

    /// Project modelpoints.
    ///
    /// `modelpoints` is a path (`.csv` / `.parquet`), any object exporting
    /// `__arrow_c_stream__` (a `pyarrow.Table`, a `RecordBatchReader`), or a pandas
    /// DataFrame; `None` uses `[run].modelpoints`.
    #[pyo3(signature = (
        modelpoints = None,
        *,
        assumptions = None,
        threads = None,
        chunk_size = None,
        progress = None,
        allow_table_drift = false,
    ))]
    #[allow(clippy::too_many_arguments)] // one keyword-only argument per §8 knob
    fn run(
        &self,
        py: Python<'_>,
        modelpoints: Option<&Bound<'_, PyAny>>,
        assumptions: Option<&Bound<'_, PyDict>>,
        threads: Option<usize>,
        chunk_size: Option<usize>,
        progress: Option<&Bound<'_, PyAny>>,
        allow_table_drift: bool,
    ) -> PyResult<Run> {
        let chunk_size = chunk_size.unwrap_or(self.run.run.exec.chunk_size.max(1) as usize);
        if chunk_size == 0 {
            return Err(plain(Kind::Data, "chunk_size must be at least 1"));
        }

        // ---- inputs, all of which need the GIL --------------------------------
        let module = self.schema_module()?.clone();
        let schema =
            MpSchema::from_module(&module).map_err(|e| plain(Kind::Data, format!("{e}")))?;
        let key_field = schema.key_field().name.clone();
        let mut source = self.open_source(py, modelpoints, schema)?;

        let mut values = self.program.declared_assumptions();
        if let Some(dict) = assumptions {
            for (k, v) in dict.iter() {
                values.insert(k.extract::<String>()?, v.extract::<f64>()?);
            }
        }
        let tables = self.tables(allow_table_drift || self.run.run.allow_table_drift)?;

        let progress: Option<Py<PyAny>> = progress.map(|p| p.clone().unbind());
        let interrupted = Arc::new(AtomicBool::new(false));
        let interrupted_inner = Arc::clone(&interrupted);
        let has_progress = progress.is_some();
        let plan = self.plan.clone();
        let tapes = self.tapes.clone();
        let modules = self.modules.clone();
        let run_file = self.run.clone();
        let cancel = CancelFlag::new();

        let inner: Box<dyn ChunkExecutor> =
            match threads.or(self.run.run.exec.threads.map(|t| t as usize)) {
                None | Some(0) => Box::new(LocalExecutor::new(0)),
                Some(1) => Box::new(SerialExecutor),
                Some(n) => Box::new(LocalExecutor::new(n)),
            };

        // ---- the projection, with the GIL released ---------------------------
        let outcome = py.allow_threads(move || -> Result<Projected, String> {
            let chunks =
                load_chunks(&mut *source, &key_field, chunk_size).map_err(|e| e.to_string())?;
            let total = chunks.len() as u64;
            let executor = GilExecutor {
                inner,
                progress,
                group: if has_progress { 1 } else { 16 },
                done: AtomicU64::new(0),
                total,
                last_tick: Mutex::new(Instant::now() - std::time::Duration::from_secs(1)),
                interrupted: interrupted_inner,
            };

            let mut runner = Runner::new(RunInputs {
                modules: &modules,
                plan: &plan,
                tapes: &tapes,
                tables,
                assumptions: values,
                run: &run_file,
            })
            .map_err(|e| e.to_string())?;

            let mut chunks = chunks;
            let mut solves: Vec<SolveResult> = Vec::new();
            // Solves first, in declaration order, so the final projection is *the
            // solved projection* and not an approximation of it (IR §8.4.4).
            for solve in runner.solves().to_vec() {
                let result = solver::solve(&mut runner, &solve, &mut chunks, &executor, &cancel)
                    .map_err(|e| e.to_string())?;
                result.check(&solve).map_err(|e| e.to_string())?;
                solves.push(result);
            }

            let projection = runner
                .project(&chunks, &executor, &cancel)
                .map_err(|e| e.to_string())?;
            Ok(Projected {
                emitted: runner.emitted().to_vec(),
                projection,
                solves,
                threads: executor.threads(),
                chunks,
            })
        });

        // A `Ctrl-C` during the projection stopped the run cleanly; the caller asked
        // to be interrupted, so give them the interruption, not a short result set.
        if interrupted.load(Ordering::SeqCst) {
            return Err(pyo3::exceptions::PyKeyboardInterrupt::new_err(
                "run cancelled",
            ));
        }
        let projected = outcome.map_err(|e| plain(Kind::Data, e))?;
        Run::build(projected)
    }

    /// Replay one modelpoint and trace one cell (`04-verify.md` §3.1).
    ///
    /// `t` is required for a `Series` component and must be `None` otherwise.
    /// The replay is a *separate*, single-lane projection: it never perturbs a
    /// run, which is why there is no "tracing mode" flag on `run()` at all.
    ///
    /// ```python
    /// trace = plan.explain("bel", "TA00001")
    /// print(trace.text)          # the deterministic ASCII rendering
    /// trace.json["root"]["value"]
    /// ```
    ///
    /// Raises if the replayed value is not bit-identical to the one the
    /// vectorised kernel produced — `E0901`, which is an engine bug.
    #[pyo3(signature = (
        component,
        modelpoint,
        t = None,
        *,
        depth = 2,
        expand = None,
        values_only = false,
        max_terms = None,
        modelpoints = None,
        assumptions = None,
        allow_table_drift = false,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn explain(
        &self,
        py: Python<'_>,
        component: &str,
        modelpoint: &str,
        t: Option<u32>,
        depth: i32,
        expand: Option<Vec<String>>,
        values_only: bool,
        max_terms: Option<usize>,
        modelpoints: Option<&Bound<'_, PyAny>>,
        assumptions: Option<&Bound<'_, PyDict>>,
        allow_table_drift: bool,
    ) -> PyResult<Trace> {
        // A trace at `t` reads history a ring buffer has already overwritten, so
        // the replay is planned with full retention. Retention is a storage
        // decision and cannot change a value (`03-engine.md` §4.3) — which is
        // what makes the `E0901` assertion below meaningful rather than circular.
        let options = predictable_plan::PlanOptions {
            opt: self.plan.opt,
            retain_all: true,
            program_digest: String::new(),
            periods: Some(self.plan.periods),
        };
        let core = predictable_plan::plan(&self.modules, &options)
            .map_err(|e| plain(Kind::Data, e.to_string()))?;
        let tapes = predictable_tape::lower_plan(&core)
            .map_err(|e| plain(Kind::Data, format!("lowering: {e}")))?;

        let module = self.schema_module()?.clone();
        let schema =
            MpSchema::from_module(&module).map_err(|e| plain(Kind::Data, format!("{e}")))?;
        let key_field = schema.key_field().name.clone();
        let mut source = self.open_source(py, modelpoints, schema)?;
        let chunk_size = self.run.run.exec.chunk_size.max(1) as usize;
        let mut chunks = load_chunks(&mut *source, &key_field, chunk_size)
            .map_err(|e| plain(Kind::Data, e.to_string()))?;

        let mut values = self.program.declared_assumptions();
        if let Some(dict) = assumptions {
            for (k, v) in dict.iter() {
                values.insert(k.extract::<String>()?, v.extract::<f64>()?);
            }
        }
        let tables = self.tables(allow_table_drift || self.run.run.allow_table_drift)?;

        // A `[[solve]]` model's modelpoints are not the ones on disk: the solver
        // wrote the solved `vary` value back into the columns, and replaying the
        // raw file would explain a projection nobody ran.
        if !self.run.solves.is_empty() {
            let mut runner = Runner::new(RunInputs {
                modules: &self.modules,
                plan: &core,
                tapes: &tapes,
                tables: tables.clone(),
                assumptions: values.clone(),
                run: &self.run,
            })
            .map_err(|e| plain(Kind::Data, e.to_string()))?;
            let cancel = CancelFlag::new();
            for solve in runner.solves().to_vec() {
                solver::solve(&mut runner, &solve, &mut chunks, &SerialExecutor, &cancel)
                    .map_err(|e| plain(Kind::Data, e.to_string()))?;
            }
        }

        let single = single_lane(&chunks, modelpoint).ok_or_else(|| {
            plain(
                Kind::Data,
                format!("no modelpoint `{modelpoint}` in the modelpoint set"),
            )
        })?;

        let timeline = self
            .modules
            .iter()
            .find_map(|m| m.timeline.clone())
            .ok_or_else(|| plain(Kind::Data, "the model declares no [timeline]"))?;
        let mut ctx = TraceContext::from_modules(&self.modules);
        ctx.assumption_set = Some("base".to_string());

        let mut explainer = Explainer::new(&core, &tapes, tables, &timeline, ctx)
            .map_err(|e| plain(Kind::Data, e.to_string()))?;
        explainer
            .prepare(&values)
            .map_err(|e| plain(Kind::Data, e.to_string()))?;
        let input = single.bind(explainer.engine_mut());
        explainer
            .load(&input)
            .map_err(|e| plain(Kind::Data, e.to_string()))?;

        let trace_options = ExplainOptions {
            depth,
            expand: expand.unwrap_or_default().into_iter().collect(),
            values_only,
            max_terms: max_terms.unwrap_or(predictable_engine::DEFAULT_MAX_TERMS),
            ..ExplainOptions::default()
        };
        match explainer.explain(component, t, &trace_options) {
            Ok(trace) => Ok(Trace::new(trace)),
            Err(ExplainError::ReplayDiverged(d)) => Err(crate::errors::raise_values(
                Kind::Trap,
                &d.diagnostic.message,
                vec![serde_json::to_value(&d.diagnostic).unwrap_or_default()],
            )),
            Err(e) => Err(plain(Kind::Data, e.to_string())),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "<Plan periods={} slots={} digest={}>",
            self.plan.periods,
            self.plan.series.len(),
            &self.plan.digest[..self.plan.digest.len().min(12)]
        )
    }
}

/// What the GIL-free section produced.
pub(crate) struct Projected {
    pub emitted: Vec<predictable_io::ComponentDescriptor>,
    pub projection: predictable_runner::pipeline::Projection,
    pub solves: Vec<SolveResult>,
    pub threads: usize,
    pub chunks: Vec<Chunk>,
}

impl Plan {
    /// Bind a runner without data — used to answer questions about the result set.
    fn bind_runner(&self, tables: Vec<CompiledTable>) -> PyResult<Runner<'_>> {
        Runner::new(RunInputs {
            modules: &self.modules,
            plan: &self.plan,
            tapes: &self.tapes,
            tables,
            assumptions: BTreeMap::new(),
            run: &self.run,
        })
        .map_err(|e| plain(Kind::Data, e.to_string()))
    }

    fn open_source(
        &self,
        py: Python<'_>,
        modelpoints: Option<&Bound<'_, PyAny>>,
        schema: MpSchema,
    ) -> PyResult<Box<dyn ModelpointSource>> {
        let from_run_file = || -> PyResult<PathBuf> {
            if self.run.run.modelpoints.is_empty() {
                return Err(plain(
                    Kind::Data,
                    "no modelpoints given and the program has no `[run].modelpoints`",
                ));
            }
            Ok(self.program.base.join(&self.run.run.modelpoints))
        };

        let path: Option<PathBuf> = match modelpoints {
            None => Some(from_run_file()?),
            Some(obj) => {
                if obj.is_instance_of::<pyo3::types::PyString>()
                    || obj.hasattr("__fspath__").unwrap_or(false)
                {
                    Some(obj.extract::<PathBuf>()?)
                } else {
                    None
                }
            }
        };

        if let Some(path) = path {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            return match ext.as_str() {
                "parquet" => ParquetSource::open(schema, &path)
                    .map(|s| Box::new(s) as Box<dyn ModelpointSource>)
                    .map_err(|e| plain(Kind::Data, e.to_string())),
                "csv" | "txt" | "" => CsvSource::open(schema, &path)
                    .map(|s| Box::new(s) as Box<dyn ModelpointSource>)
                    .map_err(|e| plain(Kind::Data, e.to_string())),
                other => Err(plain(
                    Kind::Data,
                    format!("`.{other}` is not a modelpoint format; use `.csv` or `.parquet`"),
                )),
            };
        }

        let obj = modelpoints.expect("path branch returned");
        let reader = crate::arrow_ffi::import_stream(obj)?;
        let _ = py;
        ArrowSource::new(schema, reader, "<arrow>")
            .map(|s| Box::new(s) as Box<dyn ModelpointSource>)
            .map_err(|e| plain(Kind::Data, e.to_string()))
    }
}

/// The one-lane chunk holding `key`, keeping the modelpoint's real file row so a
/// trace's `Input` leaves point at the row a reviewer can open.
fn single_lane(chunks: &[Chunk], key: &str) -> Option<Chunk> {
    for chunk in chunks {
        let lane = chunk.keys.iter().position(|k| k == key)?;
        let mut columns = std::collections::BTreeMap::new();
        for (name, column) in &chunk.columns {
            let one = match column {
                predictable_runner::ChunkColumn::Num(v) => {
                    predictable_runner::ChunkColumn::Num(vec![v[lane]])
                }
                predictable_runner::ChunkColumn::Text(v) => {
                    predictable_runner::ChunkColumn::Text(vec![v[lane].clone()])
                }
            };
            columns.insert(name.clone(), one);
        }
        return Some(Chunk {
            index: chunk.index,
            first_row: chunk.first_row + lane as u64,
            keys: vec![key.to_string()],
            columns,
        });
    }
    None
}
