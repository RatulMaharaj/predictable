//! [`InMemoryDataSource`] — the reference [`DataSource`].
//!
//! It holds a [`GraphDoc`], a manifest and already-computed series in memory. It
//! is what the inline (static export) delivery mode reads, what the tests and
//! the conformance suite run against, and the worked example in the docs. It
//! computes nothing: every number it returns was put into it.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema};

use crate::graphdoc::GraphDoc;
use crate::source::{Capabilities, DataSource, DataSourceError, ExplainQuery, Result, SeriesQuery};

/// One run's numbers.
#[derive(Debug, Clone, Default)]
pub struct RunData {
    /// Modelpoint keys, in the source's stable order.
    modelpoints: Vec<String>,
    /// Number of periods held (the length every series must have).
    periods: u32,
    /// `(component, modelpoint) -> values`.
    series: BTreeMap<(String, String), Vec<f64>>,
    /// Pre-baked traces, keyed by `(component, modelpoint, t)` (§4.3).
    traces: BTreeMap<(String, String, u32), serde_json::Value>,
}

impl RunData {
    /// An empty run holding `periods` periods.
    pub fn new(periods: u32) -> RunData {
        RunData {
            periods,
            ..RunData::default()
        }
    }

    /// Add one component's series for one modelpoint.
    ///
    /// Panics if the length does not match `periods` — a data source that
    /// returns ragged series would make every row index in the UI a lie.
    pub fn with_series(
        mut self,
        component: &str,
        modelpoint: &str,
        values: impl Into<Vec<f64>>,
    ) -> RunData {
        let values = values.into();
        assert_eq!(
            values.len() as u32,
            self.periods,
            "series `{component}` for `{modelpoint}` has {} values, expected {}",
            values.len(),
            self.periods
        );
        if !self.modelpoints.iter().any(|m| m == modelpoint) {
            self.modelpoints.push(modelpoint.to_string());
        }
        self.series
            .insert((component.to_string(), modelpoint.to_string()), values);
        self
    }

    /// Add a pre-baked `explain()` trace.
    pub fn with_trace(
        mut self,
        component: &str,
        modelpoint: &str,
        t: u32,
        trace: serde_json::Value,
    ) -> RunData {
        self.traces
            .insert((component.to_string(), modelpoint.to_string(), t), trace);
        self
    }

    /// Modelpoint keys, in stable order.
    pub fn modelpoints(&self) -> &[String] {
        &self.modelpoints
    }
}

/// A [`DataSource`] over in-memory documents and arrays.
#[derive(Debug, Clone)]
pub struct InMemoryDataSource {
    graph: GraphDoc,
    manifest: serde_json::Value,
    runs: BTreeMap<String, RunData>,
    default_run: Option<String>,
}

impl InMemoryDataSource {
    /// A source over `graph`, with an empty manifest and no runs.
    pub fn new(graph: GraphDoc) -> InMemoryDataSource {
        InMemoryDataSource {
            graph,
            manifest: serde_json::json!({}),
            runs: BTreeMap::new(),
            default_run: None,
        }
    }

    /// Attach the run manifest (`04-verify.md` §6).
    pub fn with_manifest(mut self, manifest: serde_json::Value) -> InMemoryDataSource {
        self.manifest = manifest;
        self
    }

    /// Attach a run. The first run added becomes the default.
    pub fn with_run(mut self, id: &str, run: RunData) -> InMemoryDataSource {
        if self.default_run.is_none() {
            self.default_run = Some(id.to_string());
        }
        self.runs.insert(id.to_string(), run);
        self
    }

    fn run(&self, id: Option<&str>) -> Result<(&str, &RunData)> {
        let id = match id.or(self.default_run.as_deref()) {
            Some(id) => id,
            None => return Err(DataSourceError::UnknownRun("<none>".into())),
        };
        match self.runs.get_key_value(id) {
            Some((k, v)) => Ok((k.as_str(), v)),
            None => Err(DataSourceError::UnknownRun(id.to_string())),
        }
    }
}

impl DataSource for InMemoryDataSource {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // Honest: `explain` works exactly as far as the pre-baked traces go.
            explain: self.runs.values().any(|r| !r.traces.is_empty()),
            recompute: false,
            sensitivity: false,
        }
    }

    fn manifest(&self) -> Result<serde_json::Value> {
        Ok(self.manifest.clone())
    }

    fn graph(&self) -> Result<GraphDoc> {
        Ok(self.graph.clone())
    }

    fn series(&self, query: &SeriesQuery) -> Result<RecordBatch> {
        if query.components.is_empty() {
            return Err(DataSourceError::BadRequest(
                "at least one component is required".into(),
            ));
        }
        let (run_id, run) = self.run(query.run.as_deref())?;
        for component in &query.components {
            if self.graph.node(component).is_none()
                && !self.graph.nodes.iter().any(|n| &n.name == component)
            {
                return Err(DataSourceError::UnknownComponent(component.clone()));
            }
        }
        let modelpoints: Vec<String> = if query.modelpoints.is_empty() {
            run.modelpoints.clone()
        } else {
            for mp in &query.modelpoints {
                if !run.modelpoints.iter().any(|m| m == mp) {
                    return Err(DataSourceError::UnknownModelpoint(mp.clone()));
                }
            }
            query.modelpoints.clone()
        };

        let last = run.periods.saturating_sub(1);
        let from = query.t_from.unwrap_or(0);
        let to = query.t_to.unwrap_or(last).min(last);
        if run.periods > 0 && from > to {
            return Err(DataSourceError::BadRequest(format!(
                "empty period range t={from}..{to}"
            )));
        }
        let periods: Vec<u32> = if run.periods == 0 {
            Vec::new()
        } else {
            (from..=to).collect()
        };

        let rows = modelpoints.len() * periods.len();
        let mut mp_col: Vec<String> = Vec::with_capacity(rows);
        let mut t_col: Vec<u32> = Vec::with_capacity(rows);
        let mut value_cols: Vec<Vec<f64>> = vec![Vec::with_capacity(rows); query.components.len()];
        for mp in &modelpoints {
            for t in &periods {
                mp_col.push(mp.clone());
                t_col.push(*t);
                for (c, component) in query.components.iter().enumerate() {
                    let values = run
                        .series
                        .get(&(component.clone(), mp.clone()))
                        .ok_or_else(|| {
                            DataSourceError::BadRequest(format!(
                                "run `{run_id}` holds no series `{component}` for modelpoint `{mp}`"
                            ))
                        })?;
                    value_cols[c].push(values[*t as usize]);
                }
            }
        }

        let mut fields: Vec<Field> = vec![
            Field::new("modelpoint", DataType::Utf8, false),
            Field::new("t", DataType::UInt32, false),
        ];
        let mut arrays: Vec<arrow_array::ArrayRef> = vec![
            Arc::new(StringArray::from(mp_col)),
            Arc::new(UInt32Array::from(t_col)),
        ];
        for (component, values) in query.components.iter().zip(value_cols) {
            fields.push(Field::new(component, DataType::Float64, false));
            arrays.push(Arc::new(Float64Array::from(values)));
        }
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| DataSourceError::Internal(e.to_string()))
    }

    fn explain(&self, query: &ExplainQuery) -> Result<serde_json::Value> {
        if !self.capabilities().explain {
            return Err(DataSourceError::Unsupported("explain"));
        }
        let (_, run) = self.run(query.run.as_deref())?;
        run.traces
            .get(&(query.component.clone(), query.modelpoint.clone(), query.t))
            .cloned()
            .ok_or_else(|| {
                DataSourceError::BadRequest(format!(
                    "no pre-baked trace for `{}` at `{}` t={} — this is a frozen export",
                    query.component, query.modelpoint, query.t
                ))
            })
    }
}
