//! The same suite, over HTTP. Two implementations of one contract behaving
//! differently is exactly what §1.1 exists to prevent, so the transport is held
//! to the same cases as the in-memory source — including that a 404 arrives back
//! as `UnknownComponent` and not as an opaque failure.

mod common;

use predictable_viz::conformance;

#[test]
fn the_http_data_source_conforms() {
    let server = common::serve(common::term_source());
    let source = common::HttpDataSource {
        addr: server.addr(),
        token: server.token().to_string(),
    };
    conformance::assert_conformant(&source, Some("death_claims"), None);
}

#[test]
fn http_and_in_memory_return_the_same_numbers() {
    use predictable_viz::{DataSource, SeriesQuery};
    let source = common::term_source();
    let query = SeriesQuery {
        components: vec!["death_claims".into(), "num_pols_if".into()],
        ..SeriesQuery::default()
    };
    let direct = source.series(&query).unwrap();
    let server = common::serve(source);
    let over_http = common::HttpDataSource {
        addr: server.addr(),
        token: server.token().to_string(),
    }
    .series(&query)
    .unwrap();
    assert_eq!(over_http, direct);
    assert_eq!(
        predictable_viz::to_ipc(&over_http).unwrap(),
        predictable_viz::to_ipc(&direct).unwrap()
    );
}
