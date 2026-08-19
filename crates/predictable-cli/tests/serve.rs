//! `predictable serve` — the command-line face of `model.show()` (05-viz.md §1.2).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

use predictable_cli::dispatch;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn cli(args: &[&str]) -> (i32, String, String) {
    dispatch(
        &args
            .iter()
            .map(|a| (*a).to_string())
            .collect::<Vec<String>>(),
    )
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("predictable-cli-serve-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["term.pir", "mp.csv", "run.pir"] {
        std::fs::copy(Path::new(FIXTURES).join(name), dir.join(name)).unwrap();
    }
    dir
}

fn p(dir: &Path, name: &str) -> String {
    dir.join(name).display().to_string()
}

/// One raw HTTP GET, so the test proves the port is actually serving.
fn get(port: u16, path: &str, token: Option<&str>) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("nothing is listening");
    let auth = token
        .map(|t| format!("X-Predictable-Token: {t}\r\n"))
        .unwrap_or_default();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{auth}\r\n")
                .as_bytes(),
        )
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn serve_starts_a_tokened_loopback_server_over_the_planners_graph() {
    let dir = scratch("basic");
    let (code, stdout, _) = cli(&[
        "serve",
        &p(&dir, "term.pir"),
        "--port",
        "0",
        "--no-block",
        "--json",
    ]);
    assert_eq!(code, 0, "{stdout}");
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["format"], "pvf/1");

    let port = doc["port"].as_u64().unwrap() as u16;
    let token = doc["token"].as_str().unwrap();
    assert_eq!(token.len(), 64, "the token must be 32 bytes");
    assert!(doc["url"]
        .as_str()
        .unwrap()
        .starts_with(&format!("http://127.0.0.1:{port}/#")));
    // No results are loaded, so the trace button must be greyed out, not dead.
    assert_eq!(doc["capabilities"]["explain"], false);

    // The graph it serves is the one `predictable graph` prints.
    let (_, graph_stdout, _) = cli(&["graph", &p(&dir, "term.pir"), "--json"]);
    let mut expected: serde_json::Value = serde_json::from_str(&graph_stdout).unwrap();
    // The CLI wraps the document in the `pvf/1` envelope; the API serves the
    // document itself. Strip the envelope and the two must be identical.
    let envelope = expected.as_object_mut().unwrap();
    envelope.remove("format");
    envelope.remove("kind");
    let response = get(port, "/api/graph", Some(token));
    let body = response.split_once("\r\n\r\n").unwrap().1;
    let served: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(served, expected);

    // And it is not open to anything that has not got the token.
    assert!(get(port, "/api/graph", None).starts_with("HTTP/1.1 401"));
}

#[test]
fn serve_refuses_a_model_that_does_not_check() {
    let dir = scratch("broken");
    std::fs::write(
        dir.join("broken.pir"),
        "format = \"pir/1\"\nmodule = \"broken\"\n\n[[component]]\nname = \"x\"\nkind = \"Output\"\ndtype = \"f64\"\nshape = \"Series\"\nunit = \"money\"\ntiming = \"end\"\nexpr = \"nope * 2\"\n",
    )
    .unwrap();
    let (code, stdout, _) = cli(&["serve", &p(&dir, "broken.pir"), "--json", "--no-block"]);
    assert_eq!(code, 2);
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["status"], "not_checked");
}

#[test]
fn a_bad_port_is_a_usage_error_not_a_default() {
    let dir = scratch("badport");
    let (code, _, stderr) = cli(&[
        "serve",
        &p(&dir, "term.pir"),
        "--port",
        "seven",
        "--no-block",
    ]);
    assert_eq!(code, 2);
    assert!(stderr.contains("port"), "{stderr}");
}
