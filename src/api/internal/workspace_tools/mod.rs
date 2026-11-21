pub(crate) mod handlers;
pub(crate) mod types;

pub(super) fn router() -> axum::Router<crate::AppState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/internal/workspace-tools/shell", post(handlers::shell))
        .route(
            "/api/internal/workspace-tools/write-stdin",
            post(handlers::write_stdin),
        )
        .route(
            "/api/internal/workspace-tools/apply-patch",
            post(handlers::apply_patch),
        )
        .route("/api/internal/workspace-tools/read", post(handlers::read))
        .route("/api/internal/workspace-tools/glob", post(handlers::glob))
        .route("/api/internal/workspace-tools/grep", post(handlers::grep))
        .route("/api/internal/workspace-tools/stat", post(handlers::stat))
        .route(
            "/api/internal/workspace-tools/executions/{execution_id}",
            get(handlers::status),
        )
        .route(
            "/api/internal/workspace-tools/executions/{execution_id}/output",
            get(handlers::output),
        )
        .route(
            "/api/internal/workspace-tools/executions/{execution_id}/final",
            get(handlers::final_result),
        )
        .route(
            "/api/internal/workspace-tools/executions/{execution_id}/cancel",
            post(handlers::cancel),
        )
}
