//! The conformance suite must pass the reference source — and, just as
//! importantly, must *fail* a source that breaks the contract. A suite that
//! cannot fail is documentation, not a test.

mod common;

use arrow_array::RecordBatch;
use predictable_viz::conformance::{run_suite, Status};
use predictable_viz::{
    conformance, Capabilities, ComponentDoc, DataSource, DataSourceError, ExplainQuery, GraphDoc,
    SeriesQuery,
};

#[test]
fn the_in_memory_source_conforms() {
    conformance::assert_conformant(&common::term_source(), Some("death_claims"), None);
}

#[test]
fn every_case_ran_and_none_was_silently_skipped_away() {
    let report = run_suite(&common::term_source(), Some("death_claims"), None);
    assert!(report.is_conformant(), "{}", report.to_text());
    let passed = report
        .cases
        .iter()
        .filter(|c| c.status == Status::Pass)
        .count();
    assert!(
        passed >= 14,
        "only {passed} cases actually asserted something:\n{}",
        report.to_text()
    );
}

/// A source that reorders series rows — the UI would index the wrong period.
#[derive(Debug)]
struct ShuffledRows(predictable_viz::InMemoryDataSource);

impl DataSource for ShuffledRows {
    fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }
    fn manifest(&self) -> predictable_viz::source::Result<serde_json::Value> {
        self.0.manifest()
    }
    fn graph(&self) -> predictable_viz::source::Result<GraphDoc> {
        self.0.graph()
    }
    fn series(&self, query: &SeriesQuery) -> predictable_viz::source::Result<RecordBatch> {
        let batch = self.0.series(query)?;
        let indices: Vec<u32> = (0..batch.num_rows() as u32).rev().collect();
        let take = arrow_array::UInt32Array::from(indices);
        let columns = batch
            .columns()
            .iter()
            .map(|c| arrow_select_take(c, &take))
            .collect::<Vec<_>>();
        RecordBatch::try_new(batch.schema(), columns)
            .map_err(|e| DataSourceError::Internal(e.to_string()))
    }
    fn explain(&self, query: &ExplainQuery) -> predictable_viz::source::Result<serde_json::Value> {
        self.0.explain(query)
    }
}

/// A tiny row-permutation helper so the test does not pull in `arrow-select`.
fn arrow_select_take(
    column: &arrow_array::ArrayRef,
    indices: &arrow_array::UInt32Array,
) -> arrow_array::ArrayRef {
    use arrow_array::{Array, Float64Array, StringArray, UInt32Array};
    use std::sync::Arc;
    let idx: Vec<usize> = (0..indices.len())
        .map(|i| indices.value(i) as usize)
        .collect();
    if let Some(a) = column.as_any().downcast_ref::<StringArray>() {
        Arc::new(StringArray::from(
            idx.iter()
                .map(|i| a.value(*i).to_string())
                .collect::<Vec<_>>(),
        ))
    } else if let Some(a) = column.as_any().downcast_ref::<UInt32Array>() {
        Arc::new(UInt32Array::from(
            idx.iter().map(|i| a.value(*i)).collect::<Vec<_>>(),
        ))
    } else if let Some(a) = column.as_any().downcast_ref::<Float64Array>() {
        Arc::new(Float64Array::from(
            idx.iter().map(|i| a.value(*i)).collect::<Vec<_>>(),
        ))
    } else {
        column.clone()
    }
}

#[test]
fn the_suite_catches_out_of_order_rows() {
    let report = run_suite(
        &ShuffledRows(common::term_source()),
        Some("death_claims"),
        None,
    );
    let failed: Vec<&str> = report.failures().iter().map(|c| c.name).collect();
    assert!(
        failed.contains(&"series.rows_sorted_by_modelpoint_then_t"),
        "the suite accepted shuffled rows:\n{}",
        report.to_text()
    );
}

/// A source that claims `explain` and then refuses it — the dead button §1.1
/// exists to prevent.
#[derive(Debug)]
struct LiesAboutExplain(predictable_viz::InMemoryDataSource);

impl DataSource for LiesAboutExplain {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            explain: true,
            recompute: true,
            sensitivity: true,
        }
    }
    fn manifest(&self) -> predictable_viz::source::Result<serde_json::Value> {
        self.0.manifest()
    }
    fn graph(&self) -> predictable_viz::source::Result<GraphDoc> {
        self.0.graph()
    }
    fn series(&self, query: &SeriesQuery) -> predictable_viz::source::Result<RecordBatch> {
        self.0.series(query)
    }
}

#[test]
fn the_suite_catches_a_dishonest_capability() {
    let report = run_suite(
        &LiesAboutExplain(common::term_source()),
        Some("death_claims"),
        None,
    );
    let failed: Vec<&str> = report.failures().iter().map(|c| c.name).collect();
    assert_eq!(
        failed,
        vec!["capabilities.explain_is_honest"],
        "{}",
        report.to_text()
    );
}

/// A source whose graph invents its own ordering instead of rendering the
/// planner's — the exact failure §2.1 is written to make impossible.
#[derive(Debug)]
struct ReorderedGraph(predictable_viz::InMemoryDataSource);

impl DataSource for ReorderedGraph {
    fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }
    fn manifest(&self) -> predictable_viz::source::Result<serde_json::Value> {
        self.0.manifest()
    }
    fn graph(&self) -> predictable_viz::source::Result<GraphDoc> {
        let mut graph = self.0.graph()?;
        graph.nodes.reverse();
        Ok(graph)
    }
    fn series(&self, query: &SeriesQuery) -> predictable_viz::source::Result<RecordBatch> {
        self.0.series(query)
    }
    fn explain(&self, query: &ExplainQuery) -> predictable_viz::source::Result<serde_json::Value> {
        self.0.explain(query)
    }
}

#[test]
fn the_suite_catches_a_graph_that_is_not_in_evaluation_order() {
    let report = run_suite(&ReorderedGraph(common::term_source()), None, None);
    let failed: Vec<&str> = report.failures().iter().map(|c| c.name).collect();
    assert!(
        failed.contains(&"graph.order_respects_edges"),
        "the suite accepted a re-sorted graph:\n{}",
        report.to_text()
    );
}

#[test]
fn component_docs_are_read_out_of_the_graphs_edges() {
    let source = common::term_source();
    let doc: ComponentDoc = source.component("term.death_claims").unwrap();
    assert!(doc.upstream.contains(&"term.qx".to_string()));
    assert!(doc.upstream.contains(&"term.num_pols_if".to_string()));
    assert_eq!(doc.downstream, vec!["term.pv_claims".to_string()]);

    // An unqualified name resolves too — it is what a user types.
    assert_eq!(
        source.component("death_claims").unwrap().node.name,
        "death_claims"
    );
    assert!(matches!(
        source.component("nope"),
        Err(DataSourceError::UnknownComponent(_))
    ));
}

#[test]
fn series_filters_periods_and_modelpoints_without_recomputing_anything() {
    use arrow_array::{Array, Float64Array, StringArray, UInt32Array};
    let source = common::term_source();
    let batch = source
        .series(&SeriesQuery {
            components: vec!["death_claims".into()],
            modelpoints: vec!["POL0002".into()],
            t_from: Some(1),
            t_to: Some(3),
            ..SeriesQuery::default()
        })
        .unwrap();
    assert_eq!(batch.num_rows(), 3);
    let mps = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let ts = batch
        .column(1)
        .as_any()
        .downcast_ref::<UInt32Array>()
        .unwrap();
    let values = batch
        .column(2)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert_eq!(
        (0..3).map(|i| ts.value(i)).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!((0..mps.len()).all(|i| mps.value(i) == "POL0002"));
    // Exactly the numbers that were put in — nothing derived, nothing rescaled.
    assert_eq!(
        (0..3).map(|i| values.value(i)).collect::<Vec<_>>(),
        vec![2.0, 3.0, 4.0]
    );
}

#[test]
fn an_unknown_modelpoint_is_a_404_not_an_empty_table() {
    let source = common::term_source();
    assert!(matches!(
        source.series(&SeriesQuery {
            components: vec!["death_claims".into()],
            modelpoints: vec!["POL9999".into()],
            ..SeriesQuery::default()
        }),
        Err(DataSourceError::UnknownModelpoint(_))
    ));
}

#[test]
fn arrow_ipc_round_trips_and_is_byte_stable() {
    let source = common::term_source();
    let query = SeriesQuery {
        components: vec!["death_claims".into(), "num_pols_if".into()],
        ..SeriesQuery::default()
    };
    let batch = source.series(&query).unwrap();
    let bytes = predictable_viz::to_ipc(&batch).unwrap();
    assert_eq!(
        bytes,
        predictable_viz::to_ipc(&source.series(&query).unwrap()).unwrap()
    );
    let back = predictable_viz::from_ipc(&bytes).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0], batch);
}
