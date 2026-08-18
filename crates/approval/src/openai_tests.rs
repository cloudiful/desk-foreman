use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use super::{OpenAiReviewer, OpenAiReviewerConfig, extract_output_text};
use crate::{ApprovalReviewer, ReviewDecision, ReviewDecisionKind, ReviewRequest, ReviewRisk};
use axum::{Json, Router, http::StatusCode, routing::post};
use serde_json::json;
use tokio::sync::Mutex;

#[test]
fn output_text_is_extracted_without_strong_response_deserialization() {
    let response = json!({
        "service_tier": "standard",
        "output": [{
            "type": "message",
            "content": [{"type": "output_text", "text": "{\"decision\":\"deny\"}"}]
        }]
    });
    assert_eq!(
        extract_output_text(&response),
        Some("{\"decision\":\"deny\"}")
    );
}

#[test]
fn only_low_and_medium_allow() {
    let mut decision = ReviewDecision {
        decision: ReviewDecisionKind::Allow,
        risk: ReviewRisk::Low,
        reason_code: "workspace_local".to_string(),
        rationale: "stays in workspace".to_string(),
        safer_alternative: None,
    };
    assert!(decision.permits_execution());
    decision.risk = ReviewRisk::High;
    assert!(!decision.permits_execution());
}

#[tokio::test]
async fn reviewer_sends_strict_schema_and_accepts_compatible_response_fields() {
    let received = Arc::new(Mutex::new(None));
    let received_for_handler = received.clone();
    let app = Router::new().route(
        "/responses",
        post(move |Json(body): Json<serde_json::Value>| {
            let received = received_for_handler.clone();
            async move {
                *received.lock().await = Some(body);
                Json(json!({
                    "service_tier": "standard",
                    "output": [{
                        "type": "message",
                        "content": [{
                            "type": "output_text",
                            "text": "{\"decision\":\"allow\",\"risk\":\"low\",\"reason_code\":\"workspace_local\",\"rationale\":\"safe\",\"safer_alternative\":null}"
                        }]
                    }]
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let reviewer = OpenAiReviewer::new(OpenAiReviewerConfig {
        api_base: format!("http://{address}"),
        api_key: Some("test-key".to_string()),
        model: "reviewer".to_string(),
        timeout: Duration::from_secs(2),
        max_input_bytes: 32 * 1024,
        max_concurrent: 1,
        max_output_tokens: 1024,
    })
    .expect("reviewer");
    let decision = reviewer
        .review(&ReviewRequest::shell(
            "git status --short",
            None,
            json!({"workspace_scoped": true}),
        ))
        .await
        .expect("decision");
    assert!(decision.permits_execution());
    let body = received.lock().await.clone().expect("request body");
    assert_eq!(body["model"], "reviewer");
    assert_eq!(body["max_output_tokens"], 1024);
    assert_eq!(body["text"]["format"]["type"], "json_schema");
    assert_eq!(body["text"]["format"]["strict"], true);
    server.abort();
}

#[tokio::test]
async fn reviewer_rejects_input_before_network_request() {
    let reviewer = OpenAiReviewer::new(OpenAiReviewerConfig {
        api_base: "http://127.0.0.1:1".to_string(),
        api_key: None,
        model: "reviewer".to_string(),
        timeout: Duration::from_secs(1),
        max_input_bytes: 1,
        max_concurrent: 1,
        max_output_tokens: 1024,
    })
    .expect("reviewer");

    let error = reviewer
        .review(&ReviewRequest::shell("pwd", None, json!({})))
        .await
        .expect_err("input should exceed the configured limit");
    assert!(matches!(error, crate::ApprovalError::InputTooLarge));
}

#[tokio::test]
async fn reviewer_timeout_covers_the_http_request() {
    let app = Router::new().route(
        "/responses",
        post(|| async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Json(json!({"output_text": "{}"}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let reviewer = OpenAiReviewer::new(OpenAiReviewerConfig {
        api_base: format!("http://{address}"),
        api_key: None,
        model: "reviewer".to_string(),
        timeout: Duration::from_millis(20),
        max_input_bytes: 32 * 1024,
        max_concurrent: 1,
        max_output_tokens: 1024,
    })
    .expect("reviewer");
    let error = reviewer
        .review(&ReviewRequest::shell("pwd", None, json!({})))
        .await
        .expect_err("review should time out");
    assert!(matches!(error, crate::ApprovalError::TimedOut));
    server.abort();
}

#[tokio::test]
async fn reviewer_does_not_retry_http_errors() {
    let requests = Arc::new(AtomicUsize::new(0));
    let requests_for_handler = requests.clone();
    let app = Router::new().route(
        "/responses",
        post(move || {
            let requests = requests_for_handler.clone();
            async move {
                requests.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "temporary"})),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let reviewer = OpenAiReviewer::new(OpenAiReviewerConfig {
        api_base: format!("http://{address}"),
        api_key: None,
        model: "reviewer".to_string(),
        timeout: Duration::from_secs(1),
        max_input_bytes: 32 * 1024,
        max_concurrent: 1,
        max_output_tokens: 1024,
    })
    .expect("reviewer");
    let error = reviewer
        .review(&ReviewRequest::shell("pwd", None, json!({})))
        .await
        .expect_err("HTTP error should fail closed");
    assert!(matches!(error, crate::ApprovalError::Unavailable));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn reviewer_rejects_invalid_json_response() {
    let app = Router::new().route(
        "/responses",
        post(|| async { Json(json!({"output_text": "not-json"})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let reviewer = OpenAiReviewer::new(OpenAiReviewerConfig {
        api_base: format!("http://{address}"),
        api_key: None,
        model: "reviewer".to_string(),
        timeout: Duration::from_secs(1),
        max_input_bytes: 32 * 1024,
        max_concurrent: 1,
        max_output_tokens: 1024,
    })
    .expect("reviewer");
    let error = reviewer
        .review(&ReviewRequest::shell("pwd", None, json!({})))
        .await
        .expect_err("invalid JSON should fail closed");
    assert!(matches!(error, crate::ApprovalError::InvalidResponse));
    server.abort();
}
