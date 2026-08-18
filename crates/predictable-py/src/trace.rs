//! `Plan.explain(...)` and the [`Trace`] it returns (`04-verify.md` §3.1).
//!
//! The Python object is thin by design: the JSON is normative, the text is a
//! projection of it, and both come out of `predictable-engine`. Nothing here
//! computes a number or re-derives a rendering — this file converts, and does
//! not decide.

use predictable_engine::explain::Trace as CoreTrace;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::errors::{plain, Kind};

/// A provenance trace: `.json` (dict), `.text` (str), `.notes`, `_repr_html_`.
#[pyclass(module = "predictable_engine")]
#[derive(Debug, Clone)]
pub struct Trace {
    pub(crate) inner: CoreTrace,
}

impl Trace {
    pub(crate) fn new(inner: CoreTrace) -> Trace {
        Trace { inner }
    }
}

#[pymethods]
impl Trace {
    /// The normative trace document (`04-verify.md` §3.2) as a plain dict.
    #[getter]
    fn json<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let text = serde_json::to_string(&self.inner)
            .map_err(|e| plain(Kind::Data, format!("trace: {e}")))?;
        let json = py.import_bound("json")?;
        json.call_method1("loads", (text,))
    }

    /// The deterministic ASCII rendering — the same tree always renders
    /// identically, so a trace can be committed as a golden.
    #[getter]
    fn text(&self) -> String {
        self.inner.text()
    }

    /// The rendering at a given width (`--width`).
    #[pyo3(signature = (width = 100))]
    fn to_text(&self, width: usize) -> String {
        predictable_engine::explain_text::render_with_width(&self.inner, width)
    }

    /// The value of the cell that was explained.
    #[getter]
    fn value(&self) -> f64 {
        self.inner.root.value
    }

    /// The qualified component id.
    #[getter]
    fn component(&self) -> Option<String> {
        self.inner.root.id.clone()
    }

    /// The manifest digest of the run being explained; empty when unpinned.
    #[getter]
    fn run(&self) -> String {
        self.inner.run.clone()
    }

    /// The `N0xxx` notes, in the order they were raised.
    #[getter]
    fn notes<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let mut out = Vec::with_capacity(self.inner.notes.len());
        for note in &self.inner.notes {
            let d = PyDict::new_bound(py);
            d.set_item("code", &note.code)?;
            d.set_item("severity", &note.severity)?;
            d.set_item("path", &note.path)?;
            d.set_item("message", &note.message)?;
            d.set_item("component", &note.component)?;
            d.set_item("t", note.t)?;
            out.push(d);
        }
        Ok(out)
    }

    /// Nodes in the retained tree.
    #[getter]
    fn node_count(&self) -> usize {
        self.inner.root.count()
    }

    /// Print the rendering — the notebook's `trace.show()`.
    fn show(&self) {
        println!("{}", self.inner.text());
    }

    fn _repr_html_(&self) -> String {
        format!(
            "<pre style=\"font-family:ui-monospace,monospace;font-size:12px\">{}</pre>",
            self.inner
                .text()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        )
    }

    fn __str__(&self) -> String {
        self.inner.text()
    }

    fn __repr__(&self) -> String {
        format!(
            "<Trace {} = {} ({} node(s), {} note(s))>",
            self.inner.root.id.clone().unwrap_or_default(),
            self.inner.root.value,
            self.inner.root.count(),
            self.inner.notes.len(),
        )
    }
}
