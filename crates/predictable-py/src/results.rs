//! [`Run`] — a finished projection, as Arrow.
//!
//! The result set is the long format of `04-verify.md` §2 —
//! `(mp_key, mp_row, component, stage, t, value, value_i, value_b, value_s)` — built
//! once, in `(chunk_idx, offset)` order, and handed to Python through the Arrow C
//! Data Interface. `to_arrow()` costs no copy: `pyarrow` adopts these buffers and
//! the release callback transfers ownership.
//!
//! The long format is not a convenience: it is what makes a result set structurally
//! comparable between two runs that emitted different component sets (Q6), and what
//! lets `Series`, `PerMP` and `Scalar` values share one table without a ragged schema.

use std::sync::Arc;

use arrow_array::builder::{
    BooleanBuilder, Float64Builder, Int32Builder, Int64Builder, Int8Builder, StringBuilder,
    StringDictionaryBuilder, UInt32Builder,
};
use arrow_array::types::Int32Type;
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::SchemaRef;
use predictable_io::outbound::schema::results_arrow_schema;
use predictable_io::ComponentDescriptor;
use predictable_ir::DType;
use pyo3::prelude::*;
use pyo3::types::{PyCapsule, PyList};

use crate::arrow_ffi::{export_stream, pyarrow_table};
use crate::errors::{plain, raise_values, Kind};
use crate::plan::Projected;

/// A finished projection.
#[pyclass(module = "predictable_engine")]
#[derive(Debug)]
pub struct Run {
    schema: SchemaRef,
    batch: RecordBatch,
    outcome: String,
    exit_code: i32,
    modelpoints_projected: u64,
    modelpoints_trapped: u64,
    trap_total: u64,
    threads: usize,
    components: Vec<String>,
    traps: Vec<serde_json::Value>,
    aggregates: Vec<serde_json::Value>,
    solves: Vec<serde_json::Value>,
}

impl Run {
    pub(crate) fn build(p: Projected) -> PyResult<Run> {
        let traps: Vec<serde_json::Value> =
            p.projection.traps.iter().map(|t| t.envelope()).collect();

        // §9.3.1 / Q7: `on_trap = "abort"` produces no result set at all. Returning
        // an empty table would be a short result set that looks complete.
        if p.projection.outcome == predictable_io::outbound::Outcome::Aborted {
            return Err(raise_values(
                Kind::Trap,
                &format!(
                    "the projection trapped and `on_trap = \"abort\"`: {} trap{} over {} modelpoint{}",
                    p.projection.trap_total,
                    if p.projection.trap_total == 1 { "" } else { "s" },
                    p.projection.modelpoints_trapped,
                    if p.projection.modelpoints_trapped == 1 { "" } else { "s" },
                ),
                traps,
            ));
        }

        let batch = build_batch(&p)?;
        Ok(Run {
            schema: results_arrow_schema(),
            batch,
            outcome: outcome_name(p.projection.outcome).to_string(),
            exit_code: p.projection.exit_code(),
            modelpoints_projected: p.projection.modelpoints_projected,
            modelpoints_trapped: p.projection.modelpoints_trapped,
            trap_total: p.projection.trap_total,
            threads: p.threads,
            components: p.emitted.iter().map(|c| c.id.clone()).collect(),
            traps,
            aggregates: p
                .projection
                .aggregates
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "aggregation": a.aggregation,
                        "group_key": a.group_key,
                        "measure": a.measure,
                        "t": a.t,
                        "value": a.value,
                    })
                })
                .collect(),
            solves: p
                .solves
                .iter()
                .map(|s| serde_json::to_value(&s.outcome).unwrap_or(serde_json::Value::Null))
                .collect(),
        })
    }
}

fn outcome_name(outcome: predictable_io::outbound::Outcome) -> &'static str {
    use predictable_io::outbound::Outcome::*;
    match outcome {
        Completed => "completed",
        CompletedWithTraps => "completed_with_traps",
        Cancelled => "cancelled",
        Aborted => "aborted",
    }
}

/// Every long-format builder, in schema order.
struct Builders {
    mp_key: StringBuilder,
    mp_row: UInt32Builder,
    component: StringDictionaryBuilder<Int32Type>,
    stage: Int8Builder,
    t: Int32Builder,
    value: Float64Builder,
    value_i: Int64Builder,
    value_b: BooleanBuilder,
    value_s: StringDictionaryBuilder<Int32Type>,
}

impl Builders {
    fn new() -> Builders {
        Builders {
            mp_key: StringBuilder::new(),
            mp_row: UInt32Builder::new(),
            component: StringDictionaryBuilder::new(),
            stage: Int8Builder::new(),
            t: Int32Builder::new(),
            value: Float64Builder::new(),
            value_i: Int64Builder::new(),
            value_b: BooleanBuilder::new(),
            value_s: StringDictionaryBuilder::new(),
        }
    }

    /// One cell. The dtype decides which `value*` column it lands in (§2, Q15: `f64`
    /// storage unconditionally, so `value` is always `double`).
    fn push(
        &mut self,
        mp_key: &str,
        mp_row: u32,
        descriptor: &ComponentDescriptor,
        t: i32,
        v: f64,
    ) -> PyResult<()> {
        self.mp_key.append_value(mp_key);
        self.mp_row.append_value(mp_row);
        self.component.append_value(&descriptor.id);
        self.stage.append_value(descriptor.stage);
        self.t.append_value(t);
        match &descriptor.dtype {
            DType::F64 => {
                self.value.append_value(v);
                self.value_i.append_null();
                self.value_b.append_null();
            }
            DType::I64 | DType::Date => {
                self.value.append_null();
                self.value_i.append_value(v as i64);
                self.value_b.append_null();
            }
            DType::Bool => {
                self.value.append_null();
                self.value_i.append_null();
                self.value_b.append_value(v != 0.0);
            }
            // A `str`/`enum` lane holds a per-engine dictionary code, not text
            // (`03-engine.md` §4.2). Writing the code would put a meaningless integer
            // in the result set, so refuse rather than lie.
            DType::Str | DType::Enum(_) => {
                return Err(plain(
                    Kind::Data,
                    format!(
                        "component `{}` is `{}`; text components cannot be emitted from a run \
                         (their lane holds an engine-local dictionary code)",
                        descriptor.id, descriptor.dtype
                    ),
                ))
            }
        }
        self.value_s.append_null();
        Ok(())
    }

    fn finish(mut self) -> Vec<ArrayRef> {
        vec![
            Arc::new(self.mp_key.finish()),
            Arc::new(self.mp_row.finish()),
            Arc::new(self.component.finish()),
            Arc::new(self.stage.finish()),
            Arc::new(self.t.finish()),
            Arc::new(self.value.finish()),
            Arc::new(self.value_i.finish()),
            Arc::new(self.value_b.finish()),
            Arc::new(self.value_s.finish()),
        ]
    }
}

fn build_batch(p: &Projected) -> PyResult<RecordBatch> {
    let mut b = Builders::new();
    let solve_descriptor = |name: &str| ComponentDescriptor {
        id: name.to_string(),
        name: name.to_string(),
        kind: predictable_ir::Kind::Output,
        dtype: DType::F64,
        shape: predictable_ir::Shape::PerMp,
        unit: predictable_ir::Unit::None,
        timing: None,
        stage: 1,
        output: true,
        display: None,
    };

    for (chunk, out) in p.chunks.iter().zip(&p.projection.chunks) {
        let rows = predictable_runner::chunk::surviving_rows(chunk, out);
        for (lane, key) in out.keys.iter().enumerate() {
            let mp_row = (chunk.first_row + rows[lane] as u64) as u32;
            for column in &out.columns {
                let descriptor =
                    p.emitted
                        .iter()
                        .find(|d| d.id == column.name)
                        .ok_or_else(|| {
                            plain(
                                Kind::Data,
                                format!("component `{}` has no descriptor", column.name),
                            )
                        })?;
                let values = column.lane(lane);
                if column.stride == 1 {
                    b.push(key, mp_row, descriptor, -1, values[0])?;
                } else {
                    for (t, v) in values.iter().enumerate() {
                        b.push(key, mp_row, descriptor, t as i32, *v)?;
                    }
                }
            }
            // Q8: the solved value is an ordinary `PerMP` component of the result set,
            // so a downstream join never has to special-case a solve.
            for solve in &p.solves {
                if let Some(v) = solve.solved.get(key) {
                    b.push(key, mp_row, &solve_descriptor(&solve.outcome.name), -1, *v)?;
                }
            }
        }
    }

    RecordBatch::try_new(results_arrow_schema(), b.finish())
        .map_err(|e| plain(Kind::Data, format!("cannot build the results batch: {e}")))
}

fn to_py<'py>(py: Python<'py>, values: &[serde_json::Value]) -> PyResult<Bound<'py, PyList>> {
    let json = serde_json::Value::Array(values.to_vec()).to_string();
    py.import_bound("json")?
        .call_method1("loads", (json,))?
        .downcast_into::<PyList>()
        .map_err(|e| plain(Kind::Data, e.to_string()))
}

#[pymethods]
impl Run {
    /// `completed` | `completed_with_traps` | `cancelled` (`01-ir.md` §9.3.1).
    #[getter]
    fn outcome(&self) -> &str {
        &self.outcome
    }

    /// `0` completed, `1` completed with traps or cancelled (`04-verify.md` §7).
    #[getter]
    fn exit_code(&self) -> i32 {
        self.exit_code
    }

    /// Rows in the long-format result set.
    #[getter]
    fn rows(&self) -> usize {
        self.batch.num_rows()
    }

    /// Modelpoints that produced rows.
    #[getter]
    fn modelpoints(&self) -> u64 {
        self.modelpoints_projected
    }

    /// Modelpoints abandoned at a trap under `on_trap = "continue"`.
    #[getter]
    fn modelpoints_trapped(&self) -> u64 {
        self.modelpoints_trapped
    }

    /// Exact trap count, whether or not the report was retained under `max_errors`.
    #[getter]
    fn trap_count(&self) -> u64 {
        self.trap_total
    }

    /// Workers the executor used. Not part of any digest: `run(threads = 1)` and
    /// `run(threads = 64)` are bit-identical (§9.1).
    #[getter]
    fn threads(&self) -> usize {
        self.threads
    }

    /// The emitted component ids, in evaluation order.
    #[getter]
    fn components(&self) -> Vec<String> {
        self.components.clone()
    }

    /// The `E0902` trap envelopes of IR §9.3.1.
    #[getter]
    fn traps<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        to_py(py, &self.traps)
    }

    /// `[[aggregation]]` rows, folded in chunk index order (IR §8.3).
    #[getter]
    fn aggregates<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        to_py(py, &self.aggregates)
    }

    /// One outcome block per `[[solve]]`, in declaration order (IR §8.4.4).
    #[getter]
    fn solves<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        to_py(py, &self.solves)
    }

    /// The Arrow C stream protocol: zero copy, ownership transferred to the consumer.
    ///
    /// Any Arrow-aware library can read the results without `pyarrow` being installed
    /// and without this process serialising anything.
    #[pyo3(signature = (requested_schema = None))]
    fn __arrow_c_stream__<'py>(
        &self,
        py: Python<'py>,
        requested_schema: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyCapsule>> {
        // Schema negotiation is not supported: the results schema is normative
        // (`04-verify.md` §2), and quietly casting it would make two runs incomparable.
        if let Some(s) = requested_schema {
            if !s.is_none() {
                return Err(plain(
                    Kind::Data,
                    "the results schema is fixed by 04-verify.md §2 and cannot be re-requested",
                ));
            }
        }
        export_stream(py, self.schema.clone(), vec![self.batch.clone()])
    }

    /// A `pyarrow.Table` over the same buffers — no copy, no serialisation.
    fn to_arrow<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let me: Py<Run> = Py::new(py, self.clone_shallow())?;
        pyarrow_table(py, me.bind(py).as_any())
    }

    /// A `pandas.DataFrame`, via Arrow — one copy, on pandas' side, visible in a profile.
    fn to_pandas<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.to_arrow(py)?.call_method0("to_pandas")
    }

    fn __repr__(&self) -> String {
        format!(
            "<Run outcome={} modelpoints={} rows={} components={}>",
            self.outcome,
            self.modelpoints_projected,
            self.batch.num_rows(),
            self.components.len()
        )
    }
}

impl Run {
    /// A second handle on the same Arrow buffers (`RecordBatch` is refcounted), used
    /// so `to_arrow()` can pass *itself* to `pyarrow.table`.
    fn clone_shallow(&self) -> Run {
        Run {
            schema: self.schema.clone(),
            batch: self.batch.clone(),
            outcome: self.outcome.clone(),
            exit_code: self.exit_code,
            modelpoints_projected: self.modelpoints_projected,
            modelpoints_trapped: self.modelpoints_trapped,
            trap_total: self.trap_total,
            threads: self.threads,
            components: self.components.clone(),
            traps: self.traps.clone(),
            aggregates: self.aggregates.clone(),
            solves: self.solves.clone(),
        }
    }
}
