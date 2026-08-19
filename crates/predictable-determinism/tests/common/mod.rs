//! Shared plumbing: build a one-off model, feed it exact modelpoint values, read lanes back.

use predictable_determinism::corpus::{Case, Loaded, RunOutput};
use predictable_determinism::{load, project};
use predictable_runner::SerialExecutor;

/// A model built from source and run against an explicit CSV, so a test can choose the exact
/// bit patterns that go in. `None` uses the harness's synthetic modelpoints.
pub fn run(name: &str, source: &str, csv: Option<&str>) -> (Loaded, RunOutput) {
    let case = Case::from_source(name, source);
    let mut loaded = load(&case).unwrap_or_else(|e| panic!("{name}: {e}"));
    if let Some(csv) = csv {
        loaded.modelpoints = (format!("<{name}>"), csv.to_string());
    }
    let out = project(&loaded, 1024, &SerialExecutor).unwrap_or_else(|e| panic!("{name}: {e}"));
    (loaded, out)
}

/// One lane of one emitted component.
pub fn lane(out: &RunOutput, component: &str, lane: usize) -> Vec<f64> {
    let chunk = &out.projection.chunks[0];
    let column = chunk
        .column(component)
        .unwrap_or_else(|| panic!("no column {component}"));
    column.lane(lane).to_vec()
}

/// The single value of a `PerMP` component in lane 0.
pub fn scalar(out: &RunOutput, component: &str) -> f64 {
    lane(out, component, 0)[0]
}
