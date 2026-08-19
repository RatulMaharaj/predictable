//! `GraphDoc` is a *view of the plan*, not a second opinion about it (§2.1).

mod common;

use predictable_check::Input;
use predictable_plan::{plan_sources, PlanOptions};
use predictable_viz::graph_doc;

fn plan_fixture() -> (Vec<Input>, predictable_plan::Plan) {
    let inputs = vec![Input::new("term.pir", common::TERM_PIR)];
    let plan = plan_sources(&inputs, &PlanOptions::default()).unwrap();
    (inputs, plan)
}

#[test]
fn node_order_is_the_plans_order_not_a_new_sort() {
    let (inputs, plan) = plan_fixture();
    let doc = graph_doc(&inputs, &plan, "sha256:fixture");

    let planned: Vec<String> = plan
        .order()
        .iter()
        .map(|id| plan.info(*id).name.clone())
        .collect();
    // Tables are drawn as their own leaf nodes and come first; everything the
    // planner evaluates follows in exactly the planner's order.
    let rendered: Vec<String> = doc
        .nodes
        .iter()
        .filter(|n| n.kind != "Table")
        .map(|n| n.name.clone())
        .collect();
    assert_eq!(rendered, planned);
    assert_eq!(doc.nodes[0].name, "qx_table");
    // Inputs and timeline fields are nodes too, so no edge dangles.
    assert!(doc.node("term.sum_assured").is_some());
    assert_eq!(
        doc.node("term.sum_assured").unwrap().kind,
        "InputModelpoint"
    );
    assert!(doc.node("<timeline>.t").is_some());
    assert_eq!(doc.order_digest, plan.order_digest);
    assert_eq!(doc.plan_digest, plan.digest);
}

#[test]
fn layers_partition_the_nodes_by_depth() {
    let (inputs, plan) = plan_fixture();
    let doc = graph_doc(&inputs, &plan, "sha256:fixture");

    let flat: Vec<&String> = doc.layers.iter().flatten().collect();
    assert_eq!(flat.len(), doc.nodes.len());
    for (depth, layer) in doc.layers.iter().enumerate() {
        for name in layer {
            let node = doc.nodes.iter().find(|n| &n.name == name).unwrap();
            assert_eq!(node.depth, depth, "`{name}` is in the wrong layer");
        }
    }
}

#[test]
fn a_dependency_is_shallower_than_its_dependant() {
    let (inputs, plan) = plan_fixture();
    let doc = graph_doc(&inputs, &plan, "sha256:fixture");

    let depth = |id: &str| doc.node(id).unwrap().depth;
    assert!(depth("term.age") < depth("term.qx"));
    assert!(depth("term.qx") < depth("term.death_claims"));
    assert!(depth("term.death_claims") < depth("term.pv_claims"));
}

#[test]
fn edges_carry_the_ir_semantics_the_renderer_styles_on() {
    let (inputs, plan) = plan_fixture();
    let doc = graph_doc(&inputs, &plan, "sha256:fixture");

    // Self-reference at lag 1 — drawn dashed and labelled `t-1` (§2.2).
    let self_edge = doc
        .edges
        .iter()
        .find(|e| e.from == "term.num_pols_if" && e.to == "term.num_pols_if")
        .expect("the survivorship recurrence must appear as an edge");
    assert_eq!(self_edge.lag, Some(1));
    assert!(!self_edge.at);

    // A lagged read of another component.
    let lagged = doc
        .edges
        .iter()
        .find(|e| e.from == "term.qx" && e.to == "term.num_pols_if")
        .unwrap();
    assert_eq!(lagged.lag, Some(1));

    // A same-period read.
    let current = doc
        .edges
        .iter()
        .find(|e| e.from == "term.qx" && e.to == "term.death_claims")
        .unwrap();
    assert_eq!(current.lag, Some(0));

    // A table dependency has no lag at all — it is a different kind of edge.
    let table = doc
        .edges
        .iter()
        .find(|e| e.to == "term.qx" && e.lag.is_none())
        .expect("the lookup must appear as a table edge");
    assert_eq!(table.from, "term.qx_table");
}

#[test]
fn nodes_carry_the_authors_own_expression_text() {
    let (inputs, plan) = plan_fixture();
    let doc = graph_doc(&inputs, &plan, "sha256:fixture");

    let survivors = doc.node("term.num_pols_if").unwrap();
    // Re-printing from the tree would lose the parentheses; the inspector shows
    // what the author wrote.
    assert_eq!(
        survivors.expr.as_deref(),
        Some("num_pols_if[t-1] * (1.0 - qx[t-1])")
    );
    assert_eq!(survivors.init.as_deref(), Some("1.0"));
    assert_eq!(survivors.retention.as_deref(), Some("ring(2)"));
    assert_eq!(survivors.shape, "Series");
    assert_eq!(survivors.timing.as_deref(), Some("start"));

    let qx = doc.node("term.qx").unwrap();
    assert_eq!(
        qx.doc.as_deref(),
        Some("Mortality read off the base table.")
    );
    assert_eq!(qx.unit, "prob");

    let pv = doc.node("term.pv_claims").unwrap();
    assert_eq!(pv.shape, "PerMP");
    assert_eq!(pv.kind, "Output");
}

#[test]
fn spans_point_back_at_the_source() {
    let (inputs, plan) = plan_fixture();
    let doc = graph_doc(&inputs, &plan, "sha256:fixture");

    let span = doc.node("term.death_claims").unwrap().span.clone().unwrap();
    assert_eq!(span.file, "term.pir");
    assert!(span.byte_end > span.byte_start);
    let text = &common::TERM_PIR[span.byte_start as usize..span.byte_end as usize];
    assert!(text.contains("death_claims"), "span points at `{text}`");
}

#[test]
fn the_document_is_a_pure_function_of_the_sources() {
    let (inputs, plan) = plan_fixture();
    let once = graph_doc(&inputs, &plan, "sha256:fixture");
    let (inputs2, plan2) = plan_fixture();
    let twice = graph_doc(&inputs2, &plan2, "sha256:fixture");
    assert_eq!(once, twice);
    assert_eq!(
        serde_json::to_string(&once).unwrap(),
        serde_json::to_string(&twice).unwrap()
    );
}

#[test]
fn the_document_round_trips_through_json() {
    let (inputs, plan) = plan_fixture();
    let doc = graph_doc(&inputs, &plan, "sha256:fixture");
    let text = serde_json::to_string(&doc).unwrap();
    let back: predictable_viz::GraphDoc = serde_json::from_str(&text).unwrap();
    assert_eq!(back, doc);
}
