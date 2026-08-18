//! The local server (§1.2): loopback only, token on every API call, JSON for
//! metadata and Arrow IPC for numbers.

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use arrow_array::{Array, Float64Array, UInt32Array};
use predictable_viz::{ServerOptions, VizServer};

#[test]
fn the_api_refuses_a_request_with_no_token() {
    let server = common::serve(common::term_source());
    let response = common::request(server.addr(), "GET", "/api/graph", None, None);
    assert_eq!(response.status, 401);
    assert!(response.json()["error"].as_str().unwrap().contains("token"));
}

#[test]
fn the_api_refuses_the_wrong_token() {
    let server = common::serve(common::term_source());
    let wrong = "0".repeat(server.token().len());
    let response = common::request(server.addr(), "GET", "/api/graph", Some(&wrong), None);
    assert_eq!(response.status, 401);

    // A prefix of the real token is not enough either.
    let prefix = &server.token()[..8];
    let response = common::request(server.addr(), "GET", "/api/graph", Some(prefix), None);
    assert_eq!(response.status, 401);
}

#[test]
fn the_token_may_travel_as_a_query_parameter() {
    let server = common::serve(common::term_source());
    let path = format!("/api/graph?token={}", server.token());
    let response = common::request(server.addr(), "GET", &path, None, None);
    assert_eq!(response.status, 200);
}

#[test]
fn the_shell_is_served_without_a_token_because_it_holds_no_data() {
    let server = common::serve(common::term_source());
    let response = common::request(server.addr(), "GET", "/", None, None);
    assert_eq!(response.status, 200);
    assert!(response.headers["content-type"].starts_with("text/html"));
    let html = String::from_utf8_lossy(&response.body);
    assert!(html.contains("<!doctype html>"));
    // The shell must be self-contained: a governance pack fetches nothing.
    //
    // The check is on the constructs that *cause* a fetch, not on the string
    // `http://` — the real bundle contains XML namespace URIs
    // (`http://www.w3.org/2000/svg`, ELK's `.../elk/ElkGraph`) and a
    // documentation link in a React error message, none of which is ever
    // requested. Anything that would actually hit the network has to appear as
    // an external `src`/`href`, an `@import`, or a literal absolute URL handed
    // to `fetch`/`importScripts`.
    for pattern in [
        "src=\"http",
        "src='http",
        "href=\"http",
        "href='http",
        "@import",
        "importScripts(\"http",
        "fetch(\"http",
        "fetch('http",
    ] {
        assert!(
            !html.contains(pattern),
            "the shell fetches something remote: found `{pattern}`"
        );
    }
    // And it must read the token from the fragment, not embed one.
    assert!(html.contains("location.hash"));
    assert!(!html.contains(server.token()));
}

#[test]
fn health_answers_without_a_token_and_says_nothing_about_the_run() {
    let server = common::serve(common::term_source());
    let response = common::request(server.addr(), "GET", "/api/health", None, None);
    assert_eq!(response.status, 200);
    assert_eq!(response.json()["status"], "ok");
}

#[test]
fn graph_over_http_is_the_document_the_source_holds() {
    let source = common::term_source();
    let expected = predictable_viz::DataSource::graph(&source).unwrap();
    let server = common::serve(source);
    let response = common::request(
        server.addr(),
        "GET",
        "/api/graph",
        Some(server.token()),
        None,
    );
    assert_eq!(response.status, 200);
    let doc: predictable_viz::GraphDoc = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(doc, expected);
}

#[test]
fn numbers_come_back_as_an_arrow_stream() {
    let server = common::serve(common::term_source());
    let response = common::request(
        server.addr(),
        "GET",
        "/api/series?components=death_claims&mp=POL0002&t=1..3",
        Some(server.token()),
        None,
    );
    assert_eq!(response.status, 200);
    assert_eq!(
        response.headers["content-type"],
        "application/vnd.apache.arrow.stream"
    );
    let batches = predictable_viz::from_ipc(&response.body).unwrap();
    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 3);
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
    assert_eq!(
        (0..3).map(|i| values.value(i)).collect::<Vec<_>>(),
        vec![2.0, 3.0, 4.0]
    );
}

#[test]
fn errors_carry_the_status_the_ui_branches_on() {
    let server = common::serve(common::term_source());
    let token = Some(server.token());

    let unknown = common::request(server.addr(), "GET", "/api/component/nope", token, None);
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.json()["kind"], "unknown_component");

    let bad_range = common::request(
        server.addr(),
        "GET",
        "/api/series?components=death_claims&t=notarange",
        token,
        None,
    );
    assert_eq!(bad_range.status, 400);
    assert_eq!(bad_range.json()["kind"], "bad_request");

    let no_components = common::request(server.addr(), "GET", "/api/series", token, None);
    assert_eq!(no_components.status, 400);

    let unknown_mp = common::request(
        server.addr(),
        "GET",
        "/api/series?components=death_claims&mp=POL9999",
        token,
        None,
    );
    assert_eq!(unknown_mp.status, 404);
    assert_eq!(unknown_mp.json()["kind"], "unknown_modelpoint");

    // A capability this source does not have is 501, never a dead 200.
    let diff = common::request(
        server.addr(),
        "GET",
        "/api/diff?a=run/a&b=run/b",
        token,
        None,
    );
    assert_eq!(diff.status, 501);
    assert_eq!(diff.json()["kind"], "unsupported");
}

#[test]
fn explain_is_a_post_and_returns_the_pre_baked_trace() {
    let server = common::serve(common::term_source());
    let response = common::request(
        server.addr(),
        "POST",
        "/api/explain",
        Some(server.token()),
        Some(r#"{"component":"death_claims","modelpoint":"POL0001","t":0}"#),
    );
    assert_eq!(response.status, 200);
    assert_eq!(response.json()["value"], 10.0);

    // A cell that was not baked in says so, rather than inventing a trace.
    let missing = common::request(
        server.addr(),
        "POST",
        "/api/explain",
        Some(server.token()),
        Some(r#"{"component":"death_claims","modelpoint":"POL0001","t":4}"#),
    );
    assert_eq!(missing.status, 400);
    assert!(missing.json()["error"]
        .as_str()
        .unwrap()
        .contains("frozen export"));
}

#[test]
fn capabilities_are_served_so_the_ui_can_grey_buttons_out() {
    let server = common::serve(common::term_source());
    let response = common::request(
        server.addr(),
        "GET",
        "/api/capabilities",
        Some(server.token()),
        None,
    );
    let caps = response.json();
    assert_eq!(caps["explain"], true);
    assert_eq!(caps["recompute"], false);
    assert_eq!(caps["sensitivity"], false);
}

#[test]
fn the_server_refuses_to_leave_loopback() {
    let error = VizServer::start(
        Arc::new(common::term_source()),
        ServerOptions {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 0,
            token: None,
        },
    )
    .expect_err("0.0.0.0 must be refused");
    assert!(error.to_string().contains("loopback only"));
}

#[test]
fn the_url_carries_the_token_in_the_fragment() {
    let server = common::serve(common::term_source());
    let url = server.url();
    assert!(url.starts_with("http://127.0.0.1:"));
    let (before, fragment) = url.split_once('#').expect("no fragment");
    assert_eq!(fragment, server.token());
    assert_eq!(fragment.len(), 64, "the token must be 32 bytes");
    assert!(!before.contains(server.token()));
}

#[test]
fn a_generated_token_is_fresh_every_time() {
    assert_ne!(
        predictable_viz::random_token(),
        predictable_viz::random_token()
    );
}

#[test]
fn dropping_the_server_stops_it() {
    let server = common::serve(common::term_source());
    let addr = server.addr();
    assert_eq!(
        common::request(addr, "GET", "/api/health", None, None).status,
        200
    );
    drop(server);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let closed = runtime.block_on(async move { tokio::net::TcpStream::connect(addr).await });
    // Either the connect fails or the port has been handed to nobody; what must
    // not happen is a live `predictable` API on a port the user thinks is gone.
    if let Ok(mut stream) = closed {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let alive = runtime.block_on(async move {
            stream
                .write_all(b"GET /api/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .await
                .ok();
            let mut buffer = Vec::new();
            stream.read_to_end(&mut buffer).await.ok();
            buffer
        });
        assert!(alive.is_empty(), "the server is still answering after drop");
    }
}
