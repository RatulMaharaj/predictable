//! The Arrow C Data Interface, both ways (`03-engine.md` §8.1).
//!
//! Nothing is serialised and nothing is copied at the boundary. Outbound, the
//! results batch is handed over as an `arrow_array_stream` PyCapsule: `pyarrow`
//! adopts the buffers this process allocated and the release callback transfers
//! ownership. Inbound, any object implementing `__arrow_c_stream__` (a
//! `pyarrow.Table`, a `RecordBatchReader`, a polars frame) is imported as an
//! [`ArrowArrayStreamReader`] and read straight into [`predictable_io::ArrowSource`].
//!
//! The capsule protocol is deliberately preferred over `pyarrow`'s
//! `_import_from_c`: it is the standard PyCapsule interface, it works with any
//! Arrow-compatible library, and it keeps `pyarrow` a *runtime-optional*
//! dependency — the engine imports it only when the user calls `to_arrow()`.

use std::ffi::CString;

use arrow::ffi_stream::{ArrowArrayStreamReader, FFI_ArrowArrayStream};
use arrow_array::{RecordBatch, RecordBatchIterator, RecordBatchReader};
use arrow_schema::SchemaRef;
use pyo3::prelude::*;
use pyo3::types::PyCapsule;

use crate::errors::{plain, Kind};

/// The capsule name the C Data Interface reserves for a stream.
const STREAM_CAPSULE: &str = "arrow_array_stream";

/// Export batches as an `arrow_array_stream` PyCapsule.
///
/// The batches are moved into the stream; the consumer owns them once the capsule
/// is consumed. Calling this twice on the same data is a copy the caller asked for,
/// not one the boundary imposed.
pub fn export_stream<'py>(
    py: Python<'py>,
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
) -> PyResult<Bound<'py, PyCapsule>> {
    let reader = RecordBatchIterator::new(batches.into_iter().map(Ok), schema);
    let stream = FFI_ArrowArrayStream::new(Box::new(reader));
    let name = CString::new(STREAM_CAPSULE).expect("capsule name has no NUL");
    PyCapsule::new_bound(py, stream, Some(name))
}

/// Import anything implementing `__arrow_c_stream__` as a `RecordBatchReader`.
///
/// A `pandas.DataFrame` is converted by `pyarrow` first — one copy, on the user's
/// side, visible in a profile. That is deliberate: the docs steer large portfolios
/// at Parquet paths or Arrow tables precisely so this copy never happens.
pub fn import_stream(obj: &Bound<'_, PyAny>) -> PyResult<Box<dyn RecordBatchReader + Send>> {
    let source = if obj.hasattr("__arrow_c_stream__")? {
        obj.clone()
    } else {
        to_arrow_table(obj)?
    };
    let capsule = source.call_method1("__arrow_c_stream__", (source.py().None(),))?;
    let capsule = capsule.downcast_into::<PyCapsule>().map_err(|_| {
        plain(
            Kind::Data,
            "__arrow_c_stream__ did not return a PyCapsule; the object does not implement the Arrow C stream interface",
        )
    })?;
    // SAFETY: the capsule is named `arrow_array_stream`, so by the C Data Interface
    // contract its pointer is a `FFI_ArrowArrayStream` the producer has released to
    // us. `from_raw` moves the struct out and marks the original released, so the
    // capsule's own destructor becomes a no-op and there is no double free.
    let reader = unsafe {
        let ptr = capsule.pointer() as *mut FFI_ArrowArrayStream;
        if ptr.is_null() {
            return Err(plain(Kind::Data, "empty Arrow stream capsule"));
        }
        ArrowArrayStreamReader::from_raw(ptr)
    }
    .map_err(|e| plain(Kind::Data, format!("cannot import Arrow stream: {e}")))?;
    Ok(Box::new(reader))
}

/// Convert a non-Arrow object (a pandas DataFrame, a dict of columns) with `pyarrow`.
fn to_arrow_table<'py>(obj: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let py = obj.py();
    let pa = py.import_bound("pyarrow").map_err(|_| {
        plain(
            Kind::Data,
            "modelpoints must be a path, or an Arrow object implementing __arrow_c_stream__; \
             converting anything else (a pandas DataFrame, a dict) needs pyarrow installed",
        )
    })?;
    let table = pa.getattr("Table")?;
    if obj.hasattr("dtypes")? && obj.hasattr("columns")? {
        return table.call_method1("from_pandas", (obj,));
    }
    table.call_method1("from_pydict", (obj,))
}

/// `pyarrow.table(obj)` over an object that exports `__arrow_c_stream__`.
pub fn pyarrow_table<'py>(
    py: Python<'py>,
    exporter: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let pa = py.import_bound("pyarrow").map_err(|_| {
        plain(
            Kind::Data,
            "to_arrow() needs pyarrow installed; the zero-copy capsule is available without it \
             as __arrow_c_stream__",
        )
    })?;
    pa.call_method1("table", (exporter,))
}
