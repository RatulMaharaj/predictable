//! # `predictable-py` — the PyO3 boundary (`03-engine.md` §8)
//!
//! A deliberately small surface: the Python DSL does authoring and validation, the
//! engine does compilation and execution.
//!
//! ```python
//! from predictable_engine import Program, check
//!
//! prog   = Program.from_pir(["term.pir", "run.pir"])
//! diags  = check(prog)                       # list[dict] — the --json of IR §7
//! plan   = prog.plan()                       # raises CheckError with rendered diagnostics
//! result = plan.run("modelpoints.csv", assumptions={"valuation_rate": 0.035})
//! result.to_arrow()                          # pyarrow.Table, zero-copy
//! ```
//!
//! Four properties are the whole design:
//!
//! 1. **No Python callback runs during projection** (IR rule 1), so [`Plan::run`]
//!    releases the GIL for the entire projection and re-takes it only between chunk
//!    groups, for progress and for `Ctrl-C`.
//! 2. **Nothing is copied at the boundary.** Results leave as an `arrow_array_stream`
//!    PyCapsule; modelpoints enter through the same interface in reverse. `pyarrow`
//!    is optional at runtime — the capsule protocol needs no library at all.
//! 3. **Every error is a rendered diagnostic.** Failures raise a `PredictableError`
//!    subclass carrying `.diagnostics` (IR §7 JSON) and `.pretty`.
//! 4. **Nothing here computes a number.** Every value comes from the kernel; this
//!    crate converts, and does not decide.

#![deny(missing_debug_implementations)]
// `pyo3::create_exception!` emits `cfg(feature = "gil-refs")` probes for the 0.21
// migration feature this crate does not enable.
#![allow(unexpected_cfgs)]
// `#[pymethods]` generates `PyResult -> PyResult` conversions in its wrappers; the
// lint fires on generated code this crate does not write.
#![allow(clippy::useless_conversion)]

mod arrow_ffi;
mod errors;
mod plan;
mod program;
mod results;
mod trace;

use pyo3::prelude::*;

/// Version string of the underlying Rust engine.
#[pyfunction]
fn engine_version() -> &'static str {
    ::predictable_engine::version()
}

/// IR schema version this engine build accepts.
#[pyfunction]
fn ir_schema_version() -> u32 {
    ::predictable_engine::IR_SCHEMA_VERSION
}

/// The canonical form of one `.pir` source (`01-ir.md` §4.1) — what every digest is
/// computed over, and what `predictable fmt` writes.
#[pyfunction]
fn format_pir(name: &str, text: &str) -> PyResult<String> {
    predictable_fmt::format_source(name, text)
        .map_err(|e| errors::plain(errors::Kind::Parse, format!("{name}: {e}")))
}

#[pymodule]
fn predictable_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add(
        "__doc__",
        "The predictable projection engine (Rust), exposed to Python.",
    )?;
    m.add("__engine_version__", ::predictable_engine::version())?;
    errors::register(m)?;
    m.add_class::<program::Program>()?;
    m.add_class::<plan::Plan>()?;
    m.add_class::<results::Run>()?;
    m.add_class::<trace::Trace>()?;
    m.add_function(wrap_pyfunction!(program::check, m)?)?;
    m.add_function(wrap_pyfunction!(engine_version, m)?)?;
    m.add_function(wrap_pyfunction!(ir_schema_version, m)?)?;
    m.add_function(wrap_pyfunction!(format_pir, m)?)?;
    Ok(())
}
