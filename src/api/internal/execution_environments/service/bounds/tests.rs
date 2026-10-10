//! Bounds rule tests (issue 749 P2b, `#[cfg(test)]`).
//!
//! Split from [`super::bounds`] so the rule set and its tests each stay
//! within the size budget. Covers path, query, header and body bounds plus
//! the relayed method set: pure values, no transport.

use axum::http::{HeaderMap, Method};

use super::{
    BoundsRefusal, MAX_SERVICE_HEADER_VALUE_BYTES, bounded_service_request,
    path::MAX_SERVICE_PATH_BYTES, permitted_request_headers, query::MAX_SERVICE_QUERY_BYTES,
};

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    headers
}

fn bounds(path: &str) -> Result<super::ServiceRequestBounds, BoundsRefusal> {
    bounded_service_request(
        &Method::POST,
        path,
        None,
        &headers(&[("content-length", "0")]),
    )
}

#[test]
fn an_ordinary_relative_path_is_accepted_verbatim() {
    assert_eq!(bounds("v1/models").unwrap().path, "v1/models");
    assert_eq!(
        bounds("a/b/c-d_e.json").unwrap().path,
        "a/b/c-d_e.json",
        "the path is opaque text, not a normalised route"
    );
}

#[test]
fn a_path_that_could_escape_or_retarget_is_refused() {
    for path in [
        "",
        "/absolute",
        "../up",
        "a/../b",
        "a//b",
        "a/./b",
        "a/",
        "back\\slash",
        "per%2Fcent",
        "http://elsewhere/v1",
        "a?b",
        "a#b",
        "with\nnewline",
    ] {
        assert_eq!(
            bounds(path),
            Err(BoundsRefusal::InvalidPath),
            "{path:?} must be refused"
        );
    }
}

#[test]
fn an_oversized_path_is_refused_before_anything_else_is_read() {
    let long = "a".repeat(MAX_SERVICE_PATH_BYTES + 1);
    assert_eq!(bounds(&long), Err(BoundsRefusal::InvalidPath));
}

#[test]
fn a_query_is_opaque_bounded_text() {
    let bounded = bounded_service_request(
        &Method::GET,
        "v1/models",
        Some("a=1&b=%20&c"),
        &headers(&[("content-length", "0")]),
    )
    .unwrap();
    assert_eq!(bounded.query.as_deref(), Some("a=1&b=%20&c"));
    let long = "q".repeat(MAX_SERVICE_QUERY_BYTES + 1);
    assert_eq!(
        bounded_service_request(
            &Method::GET,
            "v1/models",
            Some(&long),
            &headers(&[("content-length", "0")]),
        )
        .map(|bounded| bounded.query),
        Err(BoundsRefusal::InvalidQuery)
    );
}

#[test]
fn only_content_type_and_accept_cross_the_relay() {
    let headers = headers(&[
        ("content-type", "application/json"),
        ("accept", "text/event-stream"),
        ("authorization", "Bearer application-secret"),
        ("cookie", "session=secret"),
        ("x-forwarded-for", "203.0.113.9"),
        ("x-forwarded-host", "elsewhere.invalid"),
        ("forwarded", "for=203.0.113.9"),
    ]);
    let permitted = permitted_request_headers(&headers).unwrap();
    assert_eq!(
        permitted,
        vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("accept".to_string(), "text/event-stream".to_string()),
        ],
        "the caller's own credentials and hop headers never cross the relay"
    );
}

#[test]
fn an_oversized_forwarded_header_is_refused() {
    let oversized = "v".repeat(MAX_SERVICE_HEADER_VALUE_BYTES + 1);
    let headers = headers(&[("content-type", oversized.as_str())]);
    assert_eq!(
        permitted_request_headers(&headers),
        Err(BoundsRefusal::InvalidHeader)
    );
}

#[test]
fn the_declared_body_length_must_be_present_and_bounded() {
    assert_eq!(
        bounded_service_request(&Method::POST, "v1/models", None, &HeaderMap::new()),
        Err(BoundsRefusal::InvalidBodyLength),
        "an undeclared body cannot be relayed byte-exactly"
    );
    assert_eq!(
        bounded_service_request(
            &Method::POST,
            "v1/models",
            None,
            &headers(&[("content-length", "not-a-number")]),
        ),
        Err(BoundsRefusal::InvalidBodyLength),
        "a malformed length is a malformed request, not a size refusal"
    );
    assert_eq!(
        bounded_service_request(
            &Method::POST,
            "v1/models",
            None,
            &headers(&[("content-length", "")]),
        ),
        Err(BoundsRefusal::InvalidBodyLength),
        "an empty length is malformed, not over the bound"
    );
    assert_eq!(
        bounded_service_request(
            &Method::POST,
            "v1/models",
            None,
            &headers(&[("content-length", "-1")]),
        ),
        Err(BoundsRefusal::InvalidBodyLength),
        "a negative length is malformed"
    );
    assert_eq!(
        bounded_service_request(
            &Method::POST,
            "v1/models",
            None,
            &headers(&[("content-length", "2097153")]),
        ),
        Err(BoundsRefusal::BodyTooLarge),
        "a well-formed length over the bound is a size refusal (413), \
         never conflated with a malformed one (400)"
    );
    assert_eq!(
        bounded_service_request(
            &Method::POST,
            "v1/models",
            None,
            &headers(&[(
                "content-length",
                &runner_protocol::service_relay::MAX_BODY_BYTES.to_string()
            )]),
        )
        .unwrap()
        .body_length,
        runner_protocol::service_relay::MAX_BODY_BYTES,
        "the bound itself is admissible"
    );
    assert_eq!(
        bounded_service_request(
            &Method::POST,
            "v1/models",
            None,
            &headers(&[("content-length", "11")]),
        )
        .unwrap()
        .body_length,
        11
    );
}

#[test]
fn only_the_declared_methods_are_relayed() {
    for method in [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::HEAD,
        Method::OPTIONS,
    ] {
        assert!(
            bounded_service_request(
                &method,
                "v1/models",
                None,
                &headers(&[("content-length", "0")]),
            )
            .is_ok(),
            "{method} must be relayed"
        );
    }
    for method in [Method::CONNECT, Method::TRACE] {
        assert_eq!(
            bounded_service_request(
                &method,
                "v1/models",
                None,
                &headers(&[("content-length", "0")]),
            ),
            Err(BoundsRefusal::UnsupportedMethod)
        );
    }
}
