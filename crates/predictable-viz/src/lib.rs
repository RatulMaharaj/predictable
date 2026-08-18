//! # `predictable-viz` — `GraphDoc`, the `DataSource` contract and the local server
//!
//! `05-viz.md` gives the visualisation layer exactly one job: **close the
//! verification loop faster than reading numbers in a spreadsheet**, and one
//! non-negotiable constraint: **it is a pure consumer.** It reads `.pir` modules,
//! plans, results, manifests and `explain()` trees. It never computes an
//! actuarial number, and it never re-derives the evaluation order — it renders
//! the planner's, so the picture and the run cannot disagree.
//!
//! This crate is the Rust half of that layer:
//!
//! | module | what it is | spec |
//! |---|---|---|
//! | [`graphdoc`] | `GraphDoc`, built from [`predictable_plan::Plan::order`] | §2.1 |
//! | [`source`] | the `DataSource` contract every delivery mode implements | §1.1 |
//! | [`memory`] | the reference in-memory implementation | §1.1 |
//! | [`conformance`] | the suite every implementation must pass | §1.1 |
//! | [`transport`] | JSON for metadata, Arrow IPC for numbers | §1.2 |
//! | [`server`] | axum on loopback, token required, embedded SPA shell | §1.2 |
//!
//! ## Building a `GraphDoc` from a plan
//!
//! ```
//! use predictable_check::Input;
//! use predictable_plan::{plan_sources, PlanOptions};
//!
//! let source = r#"
//! format = "pir/1"
//! module = "term"
//!
//! [timeline]
//! basis = "annual"
//! periods = 5
//! origin = "policy"
//! valuation_date = 2026-06-30
//!
//! [[modelpoint_field]]
//! name = "sum_assured"
//! dtype = "f64"
//! unit = "money"
//! required = true
//!
//! [[assumption]]
//! name = "mortality_rate"
//! dtype = "f64"
//! shape = "Scalar"
//! unit = "prob"
//!
//! [[component]]
//! name = "survivors"
//! kind = "Derived"
//! dtype = "f64"
//! shape = "Series"
//! unit = "count"
//! timing = "start"
//! init = "1.0"
//! expr = "survivors[t-1] * (1.0 - mortality_rate)"
//!
//! [[component]]
//! name = "death_claims"
//! kind = "Output"
//! dtype = "f64"
//! shape = "Series"
//! unit = "money"
//! timing = "end"
//! expr = "survivors * mortality_rate * sum_assured"
//! "#;
//!
//! let inputs = [Input::new("term.pir", source)];
//! let plan = plan_sources(&inputs, &PlanOptions::default()).unwrap();
//! let doc = predictable_viz::graph_doc(&inputs, &plan, "model-digest");
//!
//! // `layers` is a view of the plan's order, never a second topological sort.
//! assert_eq!(doc.order_digest, plan.order_digest);
//! let claims = doc.node("term.death_claims").unwrap();
//! assert_eq!(claims.kind, "Output");
//! assert!(claims.depth > doc.node("term.survivors").unwrap().depth);
//! ```
//!
//! ## Serving it
//!
//! ```no_run
//! use std::sync::Arc;
//! use predictable_viz::{InMemoryDataSource, RunData, ServerOptions, VizServer};
//! # fn doc() -> predictable_viz::GraphDoc { unimplemented!() }
//!
//! let source = InMemoryDataSource::new(doc())
//!     .with_run("run/2026-06-30", RunData::new(3).with_series("death_claims", "POL1", vec![1.0, 2.0, 3.0]));
//! let server = VizServer::start(Arc::new(source), ServerOptions::default()).unwrap();
//! println!("open {}", server.url());   // http://127.0.0.1:7391/#<token>
//! ```

#![deny(missing_docs)]
#![deny(missing_debug_implementations)]
#![forbid(unsafe_code)]

pub mod assets;
pub mod conformance;
pub mod graphdoc;
pub mod memory;
pub mod server;
pub mod source;
pub mod transport;

pub use graphdoc::{graph_doc, GraphDoc, GraphEdge, GraphNode, SpanDoc};
pub use memory::{InMemoryDataSource, RunData};
pub use server::{random_token, router, ServerOptions, VizServer};
pub use source::{
    AggregateQuery, Capabilities, ComponentDoc, DataSource, DataSourceError, ExplainQuery,
    SeriesQuery,
};
pub use transport::{from_ipc, to_ipc, ARROW_STREAM};
