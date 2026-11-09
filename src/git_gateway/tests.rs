use axum::http::{HeaderMap, HeaderValue, header};

use super::{
    limits::{declared_content_length, exceeds_limit},
    proxy::{GatewayRoute, parse_target},
};

#[test]
fn targets_only_append_fixed_smart_http_suffixes() {
    let info = parse_target(
        "https://git.example.com/team/repo.git",
        GatewayRoute::InfoRefs("git-upload-pack"),
    )
    .unwrap();
    assert_eq!(
        info.as_str(),
        "https://git.example.com/team/repo.git/info/refs?service=git-upload-pack"
    );

    let upload = parse_target(
        "https://git.example.com/team/repo.git/",
        GatewayRoute::UploadPack,
    )
    .unwrap();
    assert_eq!(
        upload.as_str(),
        "https://git.example.com/team/repo.git/git-upload-pack"
    );

    let receive = parse_target(
        "http://127.0.0.1:3000/git/7/origin",
        GatewayRoute::ReceivePack,
    )
    .unwrap();
    assert_eq!(
        receive.as_str(),
        "http://127.0.0.1:3000/git/7/origin/git-receive-pack"
    );
}

#[test]
fn stored_targets_reject_client_controlled_or_non_http_shapes() {
    assert!(
        parse_target("file:///tmp/repo.git", GatewayRoute::UploadPack).is_err(),
        "stored targets must be http(s)"
    );
    assert!(
        parse_target(
            "https://user:pass@git.example.com/repo.git",
            GatewayRoute::UploadPack
        )
        .is_err(),
        "stored targets must not carry userinfo"
    );
    assert!(
        parse_target(
            "https://git.example.com/repo.git?next=evil",
            GatewayRoute::UploadPack
        )
        .is_err(),
        "stored targets must not carry queries"
    );
    assert!(
        parse_target(
            "https://git.example.com/repo.git#fragment",
            GatewayRoute::UploadPack
        )
        .is_err(),
        "stored targets must not carry fragments"
    );
}

#[test]
fn declared_lengths_are_checked_before_forwarding() {
    let mut headers = HeaderMap::new();
    assert_eq!(declared_content_length(&headers), None);
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("1024"));
    assert_eq!(declared_content_length(&headers), Some(1024));
    assert!(!exceeds_limit(Some(1024), 1024));
    assert!(exceeds_limit(Some(1025), 1024));
    assert!(!exceeds_limit(None, 1));
}
