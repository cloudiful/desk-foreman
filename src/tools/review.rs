use std::{sync::Arc, time::Instant};

use desk_foreman_approval::{ApprovalFuture, ApprovalReviewer, ReviewDecision, ReviewRequest};
use serde_json::json;

use crate::{
    AppState,
    actor::ActorContext,
    tools::common::{ToolError, sha256_hex, spawn_tool_audit},
};

pub(super) async fn reviewer_for_request(
    state: &AppState,
    actor: &ActorContext,
    request: &ReviewRequest,
) -> Result<Option<Arc<dyn ApprovalReviewer>>, ToolError> {
    match state.approval.reviewer_for_actor(state, actor).await {
        Ok(reviewer) => Ok(reviewer),
        Err(error) => {
            spawn_tool_audit(
                state,
                actor,
                "approval.review",
                json!({
                    "status": "failed",
                    "reason": "reviewer_configuration_unavailable",
                    "action": request.action,
                    "input_sha256": sha256_hex(&request.input),
                    "input_bytes": request.input.len(),
                }),
            );
            tracing::warn!(%error, "approval reviewer configuration unavailable");
            Err(ToolError::Forbidden(
                "approval reviewer unavailable".to_string(),
            ))
        }
    }
}

pub(super) async fn ensure_review(
    state: &AppState,
    actor: &ActorContext,
    request: &ReviewRequest,
) -> Result<Option<ReviewDecision>, ToolError> {
    let started = Instant::now();
    let Some(reviewer) = reviewer_for_request(state, actor, request).await? else {
        return Ok(None);
    };
    let provider = reviewer.provider_name();
    let model = reviewer.model_identifier().map(str::to_string);
    let endpoint = reviewer.endpoint_identifier().map(str::to_string);
    let decision = match reviewer.review(request).await {
        Ok(decision) => decision,
        Err(error) => {
            spawn_tool_audit(
                state,
                actor,
                "approval.review",
                json!({
                    "status": "failed",
                    "reason": error.to_string(),
                    "provider": provider,
                    "model": model,
                    "endpoint": endpoint,
                    "action": request.action,
                    "input_sha256": sha256_hex(&request.input),
                    "input_bytes": request.input.len(),
                    "duration_ms": started.elapsed().as_millis(),
                }),
            );
            return Err(ToolError::Forbidden(
                "approval reviewer unavailable".to_string(),
            ));
        }
    };
    let allowed = decision.permits_execution();
    spawn_tool_audit(
        state,
        actor,
        "approval.review",
        json!({
            "status": if allowed { "allowed" } else { "denied" },
            "provider": provider,
            "model": model,
            "endpoint": endpoint,
            "action": request.action,
            "input_sha256": sha256_hex(&request.input),
            "input_bytes": request.input.len(),
            "decision": decision.decision,
            "risk": decision.risk,
            "reason_code": decision.reason_code,
            "rationale": decision.rationale,
            "duration_ms": started.elapsed().as_millis(),
        }),
    );
    if !allowed {
        return Err(ToolError::Forbidden(format!(
            "operation rejected by approval reviewer ({})",
            decision.reason_code
        )));
    }
    Ok(Some(decision))
}

pub(super) struct AuditedReviewer {
    inner: Arc<dyn ApprovalReviewer>,
    state: AppState,
    actor: ActorContext,
}

struct ReviewMetadata {
    provider: &'static str,
    model: Option<String>,
    endpoint: Option<String>,
}

impl AuditedReviewer {
    pub(super) fn new(
        inner: Arc<dyn ApprovalReviewer>,
        state: &AppState,
        actor: &ActorContext,
    ) -> Self {
        Self {
            inner,
            state: state.clone(),
            actor: actor.clone(),
        }
    }
}

impl ApprovalReviewer for AuditedReviewer {
    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }

    fn model_identifier(&self) -> Option<&str> {
        self.inner.model_identifier()
    }

    fn endpoint_identifier(&self) -> Option<&str> {
        self.inner.endpoint_identifier()
    }

    fn review<'a>(&'a self, request: &'a ReviewRequest) -> ApprovalFuture<'a> {
        let inner = self.inner.clone();
        let state = self.state.clone();
        let actor = self.actor.clone();
        let metadata = ReviewMetadata {
            provider: inner.provider_name(),
            model: inner.model_identifier().map(str::to_string),
            endpoint: inner.endpoint_identifier().map(str::to_string),
        };
        Box::pin(async move {
            let started = Instant::now();
            let result = inner.review(request).await;
            match &result {
                Ok(decision) => {
                    audit_review_result(&state, &actor, request, metadata, decision, started)
                }
                Err(error) => spawn_tool_audit(
                    &state,
                    &actor,
                    "approval.review",
                    json!({
                        "status": "failed",
                        "provider": metadata.provider,
                        "model": metadata.model,
                        "endpoint": metadata.endpoint,
                        "action": request.action,
                        "input_sha256": sha256_hex(&request.input),
                        "input_bytes": request.input.len(),
                        "reason": error.to_string(),
                        "duration_ms": started.elapsed().as_millis(),
                    }),
                ),
            }
            result
        })
    }
}

fn audit_review_result(
    state: &AppState,
    actor: &ActorContext,
    request: &ReviewRequest,
    metadata: ReviewMetadata,
    decision: &ReviewDecision,
    started: Instant,
) {
    spawn_tool_audit(
        state,
        actor,
        "approval.review",
        json!({
            "status": if decision.permits_execution() { "allowed" } else { "denied" },
            "provider": metadata.provider,
            "model": metadata.model,
            "endpoint": metadata.endpoint,
            "action": request.action,
            "input_sha256": sha256_hex(&request.input),
            "input_bytes": request.input.len(),
            "decision": decision.decision,
            "risk": decision.risk,
            "reason_code": decision.reason_code,
            "rationale": decision.rationale,
            "duration_ms": started.elapsed().as_millis(),
        }),
    );
}
