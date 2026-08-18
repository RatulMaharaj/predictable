//! [`Program`] — a set of `.pir` sources, checked and planned (`03-engine.md` §8).
//!
//! A `Program` is *text plus provenance*, nothing more: the sources, where they came
//! from, and which of the four file kinds each one is (`01-ir.md` §6, §8.4). It is
//! the DSL's hand-off point — `predictable build` emits canonical `.pir` and this is
//! what reads it — and it is also what a notebook user writes by hand.
//!
//! Deliberately, planning is a *separate* call from construction. `check()` on a
//! program that does not compile must still return every diagnostic; if the
//! constructor planned, a broken model could not be inspected at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use predictable_check::Input;
use predictable_diagnostics::{Diagnostic, Severity, SourceMap as DiagSources};
use predictable_ir::run::RunFile;
use predictable_plan::{lower_modules, plan as build_plan, OptLevel, PlanOptions};
use predictable_syntax::ast::DocumentKind;
use predictable_syntax::SourceMap;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::errors::{diagnostics_to_py, plain, raise, render, Kind};
use crate::plan::Plan;

/// One source file and its classification.
#[derive(Debug, Clone)]
pub struct Source {
    pub name: String,
    pub text: String,
    pub kind: DocumentKind,
}

/// A set of `.pir` sources.
#[pyclass(module = "predictable_engine", frozen)]
#[derive(Debug, Clone)]
pub struct Program {
    pub(crate) sources: Vec<Source>,
    pub(crate) base: PathBuf,
}

impl Program {
    pub(crate) fn inputs(&self) -> Vec<Input> {
        self.sources
            .iter()
            .map(|s| Input::new(s.name.clone(), s.text.clone()))
            .collect()
    }

    fn source_map(&self) -> DiagSources {
        let mut map = DiagSources::new();
        for s in &self.sources {
            map.insert(s.name.clone(), s.text.clone());
        }
        map
    }

    fn of(sources: Vec<(String, String)>, base: PathBuf) -> Program {
        let sources = sources
            .into_iter()
            .map(|(name, text)| {
                let mut map = SourceMap::new();
                let parsed = predictable_syntax::parse(&mut map, name.clone(), text.clone());
                let kind = parsed.document.kind();
                Source { name, text, kind }
            })
            .collect();
        Program { sources, base }
    }

    pub(crate) fn diagnostics(&self) -> (Vec<Diagnostic>, DiagSources) {
        let result = predictable_check::check(&self.inputs());
        (result.diagnostics, self.source_map())
    }

    /// The `[run]` source, if the program carries one.
    pub(crate) fn run_source(&self) -> Option<&Source> {
        self.sources.iter().find(|s| s.kind == DocumentKind::Run)
    }

    fn assumption_sources(&self) -> impl Iterator<Item = &Source> {
        self.sources
            .iter()
            .filter(|s| s.kind == DocumentKind::AssumptionSet)
    }

    /// The typed `[run]` file: the program's own, or the spec's defaults.
    pub(crate) fn run_file(&self) -> PyResult<RunFile> {
        match self.run_source() {
            None => Ok(default_run_file()),
            Some(source) => {
                let mut map = SourceMap::new();
                let parsed =
                    predictable_syntax::parse(&mut map, source.name.clone(), source.text.clone());
                predictable_runner::config::run_file(&parsed.document)
                    .map_err(|e| plain(Kind::Data, format!("{}: {e}", source.name)))
            }
        }
    }

    /// Assumption values declared in the program's assumption sets.
    pub(crate) fn declared_assumptions(&self) -> BTreeMap<String, f64> {
        let mut out = BTreeMap::new();
        for source in self.assumption_sources() {
            let mut map = SourceMap::new();
            let parsed =
                predictable_syntax::parse(&mut map, source.name.clone(), source.text.clone());
            out.extend(predictable_runner::config::assumption_values(
                &parsed.document,
            ));
        }
        out
    }
}

/// The `[run]` block of §8.4.2 with every default applied and nothing named — what
/// an interactive `plan()` gets when the program has no run file of its own.
fn default_run_file() -> RunFile {
    RunFile {
        format: predictable_ir::FORMAT.to_string(),
        run: predictable_ir::run::RunConfig {
            product: String::new(),
            assumptions: None,
            modelpoints: String::new(),
            out: String::new(),
            emit: Default::default(),
            emit_list: Vec::new(),
            retain: Default::default(),
            storage_precision: Default::default(),
            on_trap: Default::default(),
            max_errors: 100,
            allow_table_drift: false,
            sum_kahan: false,
            tables: BTreeMap::new(),
            exec: Default::default(),
        },
        solves: Vec::new(),
        aggregations: Vec::new(),
    }
}

#[pymethods]
impl Program {
    /// Read `.pir` files from disk.
    ///
    /// `paths` may be a single path or an iterable of them. A directory is read
    /// non-recursively, in sorted order, so a program built from a folder is the
    /// same program on every machine.
    #[staticmethod]
    #[pyo3(signature = (paths, *, base_dir = None))]
    fn from_pir(paths: &Bound<'_, PyAny>, base_dir: Option<PathBuf>) -> PyResult<Program> {
        let mut files: Vec<PathBuf> = Vec::new();
        if let Ok(one) = paths.extract::<PathBuf>() {
            files.push(one);
        } else {
            for item in paths.iter()? {
                files.push(item?.extract::<PathBuf>()?);
            }
        }

        let mut expanded: Vec<PathBuf> = Vec::new();
        for path in files {
            if path.is_dir() {
                let mut entries: Vec<PathBuf> = std::fs::read_dir(&path)
                    .map_err(|e| plain(Kind::Data, format!("{}: {e}", path.display())))?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("pir"))
                    .collect();
                entries.sort();
                expanded.extend(entries);
            } else {
                expanded.push(path);
            }
        }
        if expanded.is_empty() {
            return Err(plain(Kind::Data, "no `.pir` files were given"));
        }

        let base = base_dir.unwrap_or_else(|| {
            expanded[0]
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        });

        let mut sources = Vec::with_capacity(expanded.len());
        for path in expanded {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| plain(Kind::Data, format!("{}: {e}", path.display())))?;
            sources.push((path.display().to_string(), text));
        }
        Ok(Program::of(sources, base))
    }

    /// Build from in-memory `{name: text}` sources — the DSL's emit path and the
    /// notebook path.
    #[staticmethod]
    #[pyo3(signature = (sources, *, base_dir = None))]
    fn from_sources(sources: &Bound<'_, PyDict>, base_dir: Option<PathBuf>) -> PyResult<Program> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (k, v) in sources.iter() {
            out.push((k.extract::<String>()?, v.extract::<String>()?));
        }
        // A dict preserves insertion order, but a program's identity must not: sort,
        // so `model_digest` is a function of the content and nothing else.
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Program::of(
            out,
            base_dir.unwrap_or_else(|| PathBuf::from(".")),
        ))
    }

    /// The file names in the program, in order.
    #[getter]
    fn files(&self) -> Vec<String> {
        self.sources.iter().map(|s| s.name.clone()).collect()
    }

    /// `{name: kind}` — `module` | `product` | `run` | `assumption_set` | `unknown`.
    #[getter]
    fn kinds(&self) -> BTreeMap<String, String> {
        self.sources
            .iter()
            .map(|s| (s.name.clone(), kind_name(s.kind).to_string()))
            .collect()
    }

    /// `model_digest` over the canonical text of every module (`01-ir.md` §9.6).
    #[getter]
    fn digest(&self) -> PyResult<String> {
        let files: Vec<(&str, &str)> = self
            .sources
            .iter()
            .filter(|s| matches!(s.kind, DocumentKind::Module | DocumentKind::Product))
            .map(|s| (s.name.as_str(), s.text.as_str()))
            .collect();
        predictable_fmt::digest::model_digest_of_sources(files)
            .map_err(|e| plain(Kind::Parse, format!("cannot canonicalise: {e}")))
    }

    /// Every diagnostic, as the `--json` dicts of IR §7. Never raises for a broken
    /// model: reporting the breakage *is* the job.
    fn check<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let (diagnostics, _) = self.diagnostics();
        diagnostics_to_py(py, &diagnostics)
    }

    /// Plan the program. Raises `CheckError` carrying every error diagnostic if the
    /// model does not check.
    #[pyo3(signature = (*, retain_all = false, periods = None, optimise = true))]
    fn plan(&self, retain_all: bool, periods: Option<u32>, optimise: bool) -> PyResult<Plan> {
        let (diagnostics, sources) = self.diagnostics();
        if diagnostics.iter().any(|d| d.severity == Severity::Error) {
            let errors: Vec<Diagnostic> = diagnostics
                .into_iter()
                .filter(|d| d.severity == Severity::Error)
                .collect();
            let pretty = render(&errors, &sources);
            let summary = format!(
                "{} error{} in {}",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" },
                self.sources
                    .first()
                    .map(|s| s.name.as_str())
                    .unwrap_or("model")
            );
            return Err(raise(Kind::Check, &summary, &errors, &pretty));
        }

        let run_file = self.run_file()?;
        let retain_all =
            retain_all || matches!(run_file.run.retain, predictable_ir::run::Retain::Full);
        let options = PlanOptions {
            opt: if optimise {
                OptLevel::default()
            } else {
                OptLevel::O0
            },
            retain_all,
            program_digest: self.digest()?,
            periods,
        };
        let inputs = self.inputs();
        let modules = lower_modules(&inputs);
        let plan = build_plan(&modules, &options)
            .map_err(|e| plain(Kind::Check, format!("cannot plan: {e}")))?;
        let tapes = predictable_tape::lower_plan(&plan)
            .map_err(|e| plain(Kind::Check, format!("cannot lower: {e}")))?;

        Ok(Plan {
            program: self.clone(),
            plan,
            tapes,
            modules,
            run: run_file,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "<Program files={} kinds={:?}>",
            self.sources.len(),
            self.sources
                .iter()
                .map(|s| kind_name(s.kind))
                .collect::<Vec<_>>()
        )
    }
}

fn kind_name(kind: DocumentKind) -> &'static str {
    match kind {
        DocumentKind::Module => "module",
        DocumentKind::AssumptionSet => "assumption_set",
        DocumentKind::Product => "product",
        DocumentKind::Run => "run",
        DocumentKind::Unknown => "unknown",
    }
}

/// `check(program) -> list[dict]` — the free function of §8's sketch.
#[pyfunction]
pub fn check<'py>(py: Python<'py>, program: &Program) -> PyResult<Bound<'py, PyList>> {
    program.check(py)
}
