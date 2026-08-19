//! The local server — `05-viz.md` §1.2.
//!
//! `model.show()` / `results.show()` start an **axum** server in a background
//! thread of the process that already holds the engine. It is a thin adapter: it
//! owns no data, computes no number, and knows nothing about which delivery mode
//! is behind it — it serves whatever [`DataSource`] it was handed.
//!
//! Two constraints are enforced here rather than documented:
//!
//! - **Loopback only.** [`ServerOptions::bind`] must be a loopback address;
//!   anything else is refused at start, not silently accepted.
//! - **A 32-byte token on every API call.** The token travels in the URL
//!   *fragment* (never sent to a server by a browser, never logged by a proxy)
//!   and the SPA replays it as `?token=` or `X-Predictable-Token`. It is not
//!   authentication; it is enough to stop a drive-by from another local process.
//!
//! The static shell is served without a token: it is a few hundred bytes of HTML
//! with no data in it, and it is what reads the fragment in the first place.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::{header, request::Parts, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::source::{AggregateQuery, DataSource, DataSourceError, ExplainQuery, SeriesQuery};
use crate::transport::{to_ipc, ARROW_STREAM};

/// The header the SPA may use instead of `?token=`.
pub const TOKEN_HEADER: &str = "x-predictable-token";

/// How to start the server.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Address to bind. Must be loopback.
    pub bind: IpAddr,
    /// Port; `0` asks the OS for a free one (what the tests use).
    pub port: u16,
    /// Token to require. `None` generates a fresh 32-byte one.
    pub token: Option<String>,
}

impl Default for ServerOptions {
    fn default() -> ServerOptions {
        ServerOptions {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            // 7391 is the documented default (§1.2).
            port: 7391,
            token: None,
        }
    }
}

/// A 32-byte token, hex encoded.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("the OS refused to provide randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Shared handler state.
#[derive(Clone)]
struct AppState {
    source: Arc<dyn DataSource>,
    token: Arc<String>,
}

/// Proof that the request carried the token.
struct Authorized;

impl FromRequestParts<AppState> for Authorized {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Authorized, Response> {
        let from_header = parts
            .headers
            .get(TOKEN_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let from_query = parts.uri.query().and_then(|q| {
            q.split('&')
                .find_map(|pair| pair.strip_prefix("token=").map(|v| v.to_string()))
        });
        let presented = from_header.or(from_query).unwrap_or_default();
        // Length-independent comparison: the token is fixed length, so a
        // difference in length is itself a mismatch.
        let expected = state.token.as_bytes();
        let ok = presented.len() == expected.len()
            && presented
                .as_bytes()
                .iter()
                .zip(expected)
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0;
        if ok {
            Ok(Authorized)
        } else {
            Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "missing or invalid token",
                    "hint": "the token is in the URL fragment; send it as ?token= or the X-Predictable-Token header"
                })),
            )
                .into_response())
        }
    }
}

impl IntoResponse for DataSourceError {
    fn into_response(self) -> Response {
        let status = match &self {
            DataSourceError::UnknownComponent(_)
            | DataSourceError::UnknownRun(_)
            | DataSourceError::UnknownModelpoint(_) => StatusCode::NOT_FOUND,
            DataSourceError::BadRequest(_) => StatusCode::BAD_REQUEST,
            DataSourceError::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
            DataSourceError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let kind = match &self {
            DataSourceError::UnknownComponent(_) => "unknown_component",
            DataSourceError::UnknownRun(_) => "unknown_run",
            DataSourceError::UnknownModelpoint(_) => "unknown_modelpoint",
            DataSourceError::BadRequest(_) => "bad_request",
            DataSourceError::Unsupported(_) => "unsupported",
            DataSourceError::Internal(_) => "internal",
        };
        (
            status,
            Json(serde_json::json!({"error": self.to_string(), "kind": kind})),
        )
            .into_response()
    }
}

/// The API, mounted under `/`. Exposed so a test (or an embedder) can serve it
/// on its own listener.
pub fn router(source: Arc<dyn DataSource>, token: String) -> Router {
    let state = AppState {
        source,
        token: Arc::new(token),
    };
    Router::new()
        .route("/", get(shell))
        .route("/index.html", get(shell))
        .route("/api/health", get(health))
        .route("/api/capabilities", get(capabilities))
        .route("/api/manifest", get(manifest))
        .route("/api/graph", get(graph))
        .route("/api/component/{name}", get(component))
        .route("/api/series", get(series))
        .route("/api/aggregate", get(aggregate))
        .route("/api/explain", post(explain))
        .route("/api/diff", get(diff))
        .with_state(state)
}

async fn shell() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        crate::assets::SHELL_HTML,
    )
        .into_response()
}

/// Unauthenticated liveness probe: it says the server is up and nothing else.
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok", "ir_version": predictable_ir::FORMAT}))
}

async fn capabilities(_: Authorized, State(state): State<AppState>) -> Response {
    Json(state.source.capabilities()).into_response()
}

async fn manifest(_: Authorized, State(state): State<AppState>) -> Response {
    match state.source.manifest() {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn graph(_: Authorized, State(state): State<AppState>) -> Response {
    match state.source.graph() {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn component(
    _: Authorized,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    match state.source.component(&name) {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

fn arrow_response(batch: crate::source::Result<arrow_array::RecordBatch>) -> Response {
    match batch {
        Err(e) => e.into_response(),
        Ok(batch) => match to_ipc(&batch) {
            Ok(bytes) => ([(header::CONTENT_TYPE, ARROW_STREAM)], bytes).into_response(),
            Err(e) => DataSourceError::Internal(e.to_string()).into_response(),
        },
    }
}

async fn series(
    _: Authorized,
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let query = match parse_series_query(&params) {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };
    arrow_response(state.source.series(&query))
}

/// `?run=<id>&components=a,b&mp=POL1,POL2&t=0..40` (§1.2).
fn parse_series_query(
    params: &std::collections::HashMap<String, String>,
) -> crate::source::Result<SeriesQuery> {
    let list = |key: &str| -> Vec<String> {
        params
            .get(key)
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut query = SeriesQuery {
        run: params.get("run").cloned(),
        components: list("components"),
        modelpoints: {
            let mut mps = list("mp");
            mps.extend(list("modelpoints"));
            mps
        },
        t_from: None,
        t_to: None,
    };
    if let Some(range) = params.get("t") {
        let (from, to) = range.split_once("..").ok_or_else(|| {
            DataSourceError::BadRequest(format!("`t={range}` is not a range like `0..40`"))
        })?;
        let parse = |s: &str, what: &str| -> crate::source::Result<Option<u32>> {
            if s.is_empty() {
                return Ok(None);
            }
            s.parse::<u32>().map(Some).map_err(|_| {
                DataSourceError::BadRequest(format!("`{what}` in `t={range}` is not a period"))
            })
        };
        query.t_from = parse(from, from)?;
        query.t_to = parse(to, to)?;
    }
    Ok(query)
}

async fn aggregate(
    _: Authorized,
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(measure) = params.get("measure").cloned() else {
        return DataSourceError::BadRequest("`measure` is required".into()).into_response();
    };
    let query = AggregateQuery {
        run: params.get("run").cloned(),
        measure,
        group_by: params
            .get("group_by")
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    };
    arrow_response(state.source.aggregate(&query))
}

async fn explain(
    _: Authorized,
    State(state): State<AppState>,
    Json(query): Json<ExplainQuery>,
) -> Response {
    match state.source.explain(&query) {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn diff(
    _: Authorized,
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let (Some(a), Some(b)) = (params.get("a"), params.get("b")) else {
        return DataSourceError::BadRequest("`a` and `b` run ids are required".into())
            .into_response();
    };
    match state.source.diff(a, b) {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

/// A running server. Dropping it shuts the server down.
#[derive(Debug)]
pub struct VizServer {
    addr: SocketAddr,
    token: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl VizServer {
    /// Start the server on a background thread and return once it is listening.
    pub fn start(
        source: Arc<dyn DataSource>,
        options: ServerOptions,
    ) -> Result<VizServer, std::io::Error> {
        if !options.bind.is_loopback() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "predictable-viz binds loopback only; `{}` would expose the run to the network",
                    options.bind
                ),
            ));
        }
        let token = options.token.unwrap_or_else(random_token);
        let app = router(source, token.clone());
        let bind_addr = SocketAddr::new(options.bind, options.port);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let listener = runtime.block_on(tokio::net::TcpListener::bind(bind_addr))?;
        let addr = listener.local_addr()?;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("predictable-viz".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let _ = axum::serve(listener, app)
                        .with_graceful_shutdown(async {
                            let _ = rx.await;
                        })
                        .await;
                });
            })?;
        Ok(VizServer {
            addr,
            token,
            shutdown: Some(tx),
            thread: Some(thread),
        })
    }

    /// Where it is listening.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The token every API call must carry.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The URL to open: the token is in the fragment, so it never leaves the
    /// browser as part of a request line.
    pub fn url(&self) -> String {
        format!("http://{}/#{}", self.addr, self.token)
    }

    /// The base every API path hangs off.
    pub fn api_base(&self) -> String {
        format!("http://{}/api", self.addr)
    }

    /// Shut down and wait for the thread.
    pub fn stop(mut self) {
        self.shutdown_now();
    }

    fn shutdown_now(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for VizServer {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}
