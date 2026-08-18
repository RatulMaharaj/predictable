//! The Python error hierarchy (`03-engine.md` §8.2).
//!
//! Every failure that crosses the boundary arrives as a subclass of
//! `PredictableError`, carrying two things a traceback alone cannot: `.diagnostics`,
//! the `--json` list of `01-ir.md` §7, and `.pretty`, the already-rendered terminal
//! form. A traceback that begins with an Elm-grade rendered diagnostic is the point;
//! reconstructing one in Python from a string would not be the same thing.
//!
//! | Class | Raised when |
//! |---|---|
//! | `ParseError` | the `.pir` text does not parse (`E00xx`) |
//! | `CheckError` | it parses but does not check (`E0xxx`, `E1xxx`) |
//! | `DataError` | modelpoints, tables or run configuration are unusable |
//! | `TrapError` | the projection trapped under `on_trap = "abort"` (`E0902`) |

use predictable_diagnostics::{Diagnostic, RenderOptions, SourceMap};
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyList;

create_exception!(
    predictable_engine,
    PredictableError,
    PyException,
    "Base of every error the engine raises."
);
create_exception!(
    predictable_engine,
    ParseError,
    PredictableError,
    "The `.pir` source does not parse."
);
create_exception!(
    predictable_engine,
    CheckError,
    PredictableError,
    "The model parses but does not check."
);
create_exception!(
    predictable_engine,
    DataError,
    PredictableError,
    "Modelpoints, tables or run configuration are unusable."
);
create_exception!(
    predictable_engine,
    TrapError,
    PredictableError,
    "The projection trapped under `on_trap = \"abort\"`."
);

/// Which class a failure belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Parse,
    Check,
    Data,
    Trap,
}

/// Turn one [`Diagnostic`] into the plain `dict` of IR §7.
pub fn diagnostic_to_py<'py>(py: Python<'py>, d: &Diagnostic) -> PyResult<Bound<'py, PyAny>> {
    let json = serde_json::to_string(d)
        .map_err(|e| PredictableError::new_err(format!("diagnostic is not serialisable: {e}")))?;
    py.import_bound("json")?.call_method1("loads", (json,))
}

/// A `list[dict]` of diagnostics.
pub fn diagnostics_to_py<'py>(
    py: Python<'py>,
    diagnostics: &[Diagnostic],
) -> PyResult<Bound<'py, PyList>> {
    let mut items = Vec::with_capacity(diagnostics.len());
    for d in diagnostics {
        items.push(diagnostic_to_py(py, d)?);
    }
    PyList::new_bound(py, items).extract()
}

/// The terminal rendering of a batch — the `.pretty` attribute.
pub fn render(diagnostics: &[Diagnostic], sources: &SourceMap) -> String {
    predictable_diagnostics::render_all(diagnostics, sources, &RenderOptions::default())
}

/// Raise `kind` with `.diagnostics` and `.pretty` attached.
///
/// The message is the pretty rendering when there is one, so that the *first* line
/// of the traceback is already the diagnostic; `.diagnostics` is what a tool reads.
pub fn raise(kind: Kind, summary: &str, diagnostics: &[Diagnostic], pretty: &str) -> PyErr {
    Python::with_gil(|py| {
        let message = if pretty.is_empty() {
            summary.to_string()
        } else {
            format!("{summary}\n\n{pretty}")
        };
        let err = match kind {
            Kind::Parse => ParseError::new_err(message.clone()),
            Kind::Check => CheckError::new_err(message.clone()),
            Kind::Data => DataError::new_err(message.clone()),
            Kind::Trap => TrapError::new_err(message.clone()),
        };
        let value = err.value_bound(py);
        let attach = || -> PyResult<()> {
            value.setattr("diagnostics", diagnostics_to_py(py, diagnostics)?)?;
            value.setattr("pretty", pretty)?;
            value.setattr("summary", summary)?;
            Ok(())
        };
        // Attribute attachment cannot fail in practice; if it somehow does, the
        // error itself is still the right error, so keep it rather than replacing
        // it with the meta-failure.
        let _ = attach();
        err
    })
}

/// Raise `kind` carrying already-JSON diagnostics — the `E0902` trap envelopes of
/// IR §9.3.1, which are produced as JSON by the kernel rather than as [`Diagnostic`]s.
pub fn raise_values(kind: Kind, summary: &str, values: Vec<serde_json::Value>) -> PyErr {
    Python::with_gil(|py| {
        let rendered = values
            .iter()
            .filter_map(|v| v.get("message").and_then(|m| m.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        let message = if rendered.is_empty() {
            summary.to_string()
        } else {
            format!("{summary}\n\n{rendered}")
        };
        let err = match kind {
            Kind::Parse => ParseError::new_err(message),
            Kind::Check => CheckError::new_err(message),
            Kind::Data => DataError::new_err(message),
            Kind::Trap => TrapError::new_err(message),
        };
        let value = err.value_bound(py);
        let attach = || -> PyResult<()> {
            let json = serde_json::Value::Array(values).to_string();
            let list = py.import_bound("json")?.call_method1("loads", (json,))?;
            value.setattr("diagnostics", list)?;
            value.setattr("pretty", rendered)?;
            value.setattr("summary", summary)?;
            Ok(())
        };
        let _ = attach();
        err
    })
}

/// A failure with no diagnostics of its own — still in the hierarchy, still typed.
pub fn plain(kind: Kind, summary: impl AsRef<str>) -> PyErr {
    raise(kind, summary.as_ref(), &[], "")
}

/// Register the hierarchy on the module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("PredictableError", py.get_type_bound::<PredictableError>())?;
    m.add("ParseError", py.get_type_bound::<ParseError>())?;
    m.add("CheckError", py.get_type_bound::<CheckError>())?;
    m.add("DataError", py.get_type_bound::<DataError>())?;
    m.add("TrapError", py.get_type_bound::<TrapError>())?;
    Ok(())
}
