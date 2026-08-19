//! Shared fixtures: a real model, and a blocking HTTP client so the tests can
//! talk to the server without adding an HTTP client dependency to the crate.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use predictable_check::Input;
use predictable_plan::{plan_sources, PlanOptions};
use predictable_viz::{
    graph_doc, DataSource, DataSourceError, ExplainQuery, GraphDoc, InMemoryDataSource, RunData,
    SeriesQuery,
};

/// A small but structurally complete term-assurance model: a self-referential
/// series with `init` (lag 1), a stage-1 chain, a `PerMP` reduction and a table
/// lookup, so the `GraphDoc` cases have every edge kind to look at.
pub const TERM_PIR: &str = r#"
format = "pir/1"
module = "term"

[timeline]
basis = "annual"
periods = 5
origin = "policy"
valuation_date = 2026-06-30

[[modelpoint_field]]
name = "sum_assured"
dtype = "f64"
unit = "money"
required = true

[[modelpoint_field]]
name = "entry_age"
dtype = "i64"
unit = "years"
required = true

[[assumption]]
name = "valuation_rate"
dtype = "f64"
shape = "Scalar"
unit = "rate(annual)"

[[table]]
name = "qx_table"
keys = [{ name = "age", dtype = "i64", policy = "clamp" }]
values = [{ name = "qx", dtype = "f64", unit = "prob" }]
on_missing = "error"
source = "qx.csv"

[[component]]
name = "age"
kind = "Derived"
dtype = "i64"
shape = "Series"
unit = "years"
timing = "start"
expr = "entry_age + t"

[[component]]
name = "qx"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "prob"
timing = "start"
doc = "Mortality read off the base table."
expr = "qx_table@(age)"

[[component]]
name = "num_pols_if"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "count"
timing = "start"
init = "1.0"
expr = "num_pols_if[t-1] * (1.0 - qx[t-1])"

[[component]]
name = "discount"
kind = "Derived"
dtype = "f64"
shape = "Series"
unit = "factor"
timing = "start"
expr = "1.0 / (1.0 + valuation_rate) ^ t"

[[component]]
name = "death_claims"
kind = "Output"
dtype = "f64"
shape = "Series"
unit = "money"
timing = "end"
expr = "num_pols_if * qx * sum_assured"

[[component]]
name = "pv_claims"
kind = "Output"
dtype = "f64"
shape = "PerMP"
unit = "money"
expr = "npv(death_claims, discount)"
"#;

/// The model's inputs, planned, as a `GraphDoc`.
pub fn term_graph() -> GraphDoc {
    let inputs = [Input::new("term.pir", TERM_PIR)];
    let plan = plan_sources(&inputs, &PlanOptions::default()).expect("the fixture must plan");
    graph_doc(&inputs, &plan, "sha256:fixture")
}

/// A source over the fixture with two modelpoints, five periods and one
/// pre-baked trace.
pub fn term_source() -> InMemoryDataSource {
    let claims_a = vec![10.0, 9.5, 9.0, 8.5, 8.0];
    let claims_b = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let survivors = vec![1.0, 0.99, 0.98, 0.97, 0.96];
    let run = RunData::new(5)
        .with_series("death_claims", "POL0001", claims_a)
        .with_series("num_pols_if", "POL0001", survivors.clone())
        .with_series("death_claims", "POL0002", claims_b)
        .with_series("num_pols_if", "POL0002", survivors)
        .with_trace(
            "death_claims",
            "POL0001",
            0,
            serde_json::json!({"component": "death_claims", "value": 10.0, "children": []}),
        );
    InMemoryDataSource::new(term_graph())
        .with_manifest(serde_json::json!({
            "run_id": "run/2026-06-30",
            "inputs": {"model": {"digest": "sha256:fixture"}}
        }))
        .with_run("run/2026-06-30", run)
}

// ---- a minimal blocking HTTP client ---------------------------------------

/// One response.
#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// Send one request and read the whole response. `Connection: close` keeps the
/// reader trivial: the body ends when the socket does.
pub fn request(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> HttpResponse {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nAccept: */*\r\n"
        );
        if let Some(token) = token {
            head.push_str(&format!("X-Predictable-Token: {token}\r\n"));
        }
        if let Some(body) = body {
            head.push_str("Content-Type: application/json\r\n");
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).await.unwrap();
        if let Some(body) = body {
            stream.write_all(body.as_bytes()).await.unwrap();
        }
        stream.flush().await.unwrap();

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a response with no header terminator");
        let head = String::from_utf8_lossy(&raw[..split]).to_string();
        let body = raw[split + 4..].to_vec();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .expect("a response with no status line");
        let headers = lines
            .filter_map(|l| l.split_once(": "))
            .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
            .collect();
        HttpResponse {
            status,
            headers,
            body,
        }
    })
}

/// A [`DataSource`] that talks HTTP to a running server, so the conformance
/// suite can be pointed at the transport rather than at the data.
#[derive(Debug)]
pub struct HttpDataSource {
    pub addr: std::net::SocketAddr,
    pub token: String,
}

impl HttpDataSource {
    fn get(&self, path: &str) -> HttpResponse {
        request(self.addr, "GET", path, Some(&self.token), None)
    }

    fn fail(response: &HttpResponse) -> DataSourceError {
        let body = response.json();
        let message = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("no error body")
            .to_string();
        match body.get("kind").and_then(|v| v.as_str()) {
            Some("unknown_component") => DataSourceError::UnknownComponent(message),
            Some("unknown_run") => DataSourceError::UnknownRun(message),
            Some("unknown_modelpoint") => DataSourceError::UnknownModelpoint(message),
            Some("bad_request") => DataSourceError::BadRequest(message),
            Some("unsupported") => DataSourceError::Unsupported("unsupported"),
            _ => DataSourceError::Internal(format!("HTTP {}: {message}", response.status)),
        }
    }
}

impl DataSource for HttpDataSource {
    fn capabilities(&self) -> predictable_viz::Capabilities {
        serde_json::from_slice(&self.get("/api/capabilities").body).unwrap()
    }

    fn manifest(&self) -> predictable_viz::source::Result<serde_json::Value> {
        let response = self.get("/api/manifest");
        if response.status != 200 {
            return Err(Self::fail(&response));
        }
        Ok(response.json())
    }

    fn graph(&self) -> predictable_viz::source::Result<GraphDoc> {
        let response = self.get("/api/graph");
        if response.status != 200 {
            return Err(Self::fail(&response));
        }
        serde_json::from_slice(&response.body).map_err(|e| DataSourceError::Internal(e.to_string()))
    }

    fn component(
        &self,
        name: &str,
    ) -> predictable_viz::source::Result<predictable_viz::ComponentDoc> {
        let response = self.get(&format!("/api/component/{name}"));
        if response.status != 200 {
            return Err(Self::fail(&response));
        }
        serde_json::from_slice(&response.body).map_err(|e| DataSourceError::Internal(e.to_string()))
    }

    fn series(
        &self,
        query: &SeriesQuery,
    ) -> predictable_viz::source::Result<arrow_array::RecordBatch> {
        let mut path = format!("/api/series?components={}", query.components.join(","));
        if !query.modelpoints.is_empty() {
            path.push_str(&format!("&mp={}", query.modelpoints.join(",")));
        }
        if query.t_from.is_some() || query.t_to.is_some() {
            path.push_str(&format!(
                "&t={}..{}",
                query.t_from.map(|t| t.to_string()).unwrap_or_default(),
                query.t_to.map(|t| t.to_string()).unwrap_or_default()
            ));
        }
        let response = self.get(&path);
        if response.status != 200 {
            return Err(Self::fail(&response));
        }
        let batches = predictable_viz::from_ipc(&response.body)
            .map_err(|e| DataSourceError::Internal(e.to_string()))?;
        batches
            .into_iter()
            .next()
            .ok_or_else(|| DataSourceError::Internal("empty Arrow stream".into()))
    }

    fn explain(&self, query: &ExplainQuery) -> predictable_viz::source::Result<serde_json::Value> {
        let body = serde_json::to_string(query).unwrap();
        let response = request(
            self.addr,
            "POST",
            "/api/explain",
            Some(&self.token),
            Some(&body),
        );
        if response.status != 200 {
            return Err(Self::fail(&response));
        }
        Ok(response.json())
    }
}

/// Start a server over `source` on an ephemeral port.
pub fn serve(source: impl DataSource) -> predictable_viz::VizServer {
    predictable_viz::VizServer::start(
        Arc::new(source),
        predictable_viz::ServerOptions {
            port: 0,
            ..predictable_viz::ServerOptions::default()
        },
    )
    .expect("the server must start on loopback")
}
