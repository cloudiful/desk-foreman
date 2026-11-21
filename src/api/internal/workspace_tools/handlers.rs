use super::types::{
    IdentityHeaders, InputRequest, OutputQuery, StartFailure, StartRequest, StartResponse,
    StartServiceUnavailable,
};
use crate::{
    AppState,
    actor::{ActorContext, McpActor},
    api::validation::{ValidatedJson, ValidatedQuery},
    error::AppError,
    tools::{
        self, ToolError,
        params::{
            ApplyPatchParams, GlobParams, GrepParams, ReadParams, ShellParams, StatPathParams,
        },
        readonly::types::{GlobOutput, GrepOutput, ReadOutput, StatPathOutput},
        shared::PatchResultOutput,
    },
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

async fn actor(state: &AppState, headers: &HeaderMap) -> Result<ActorContext, AppError> {
    for name in ["x-df-external-user-id", "x-df-workspace-key"] {
        let value = headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| AppError::unauthorized("workspace identity headers required"))?;
        if value.len() > 1024 {
            return Err(AppError::unauthorized("invalid workspace identity"));
        }
    }
    let actor = crate::auth::identity::mcp_actor_from_bearer(state, headers, false)
        .await?
        .ok_or_else(|| AppError::unauthorized("application bearer required"))?;
    if !matches!(actor, McpActor::ApplicationSubject { .. }) {
        return Err(AppError::unauthorized("application bearer required"));
    }
    crate::actor::actor_from_mcp_actor(state, actor)
}

fn error(error: ToolError) -> AppError {
    match error {
        ToolError::InvalidInput(_) => AppError::bad_request("invalid workspace tool request"),
        ToolError::NotFound(_) => AppError::not_found("resource not found"),
        ToolError::Forbidden(message)
            if message == "execution is still running" || message == "execution is not running" =>
        {
            AppError::conflict("execution is not in the required state")
        }
        ToolError::Forbidden(_) => AppError::forbidden("workspace operation not permitted"),
        ToolError::Internal(_) => AppError::service_unavailable("workspace runner unavailable"),
    }
}

#[utoipa::path(post, path="/api/internal/workspace-tools/shell", operation_id="workspace_tools_shell", tag="workspace-tools", params(IdentityHeaders), request_body=StartRequest, security(("application_bearer"=[])), responses((status=500, body=crate::error::ErrorResponse), (status=403, body=crate::error::ErrorResponse), (status=401, body=crate::error::ErrorResponse), (status=200, body=StartResponse), (status=503, body=StartServiceUnavailable), (status=400, body=crate::error::ErrorResponse)))]
pub(crate) async fn shell(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidatedJson(input): ValidatedJson<StartRequest>,
) -> Response {
    async fn inner(
        state: &AppState,
        headers: &HeaderMap,
        input: StartRequest,
    ) -> Result<Response, AppError> {
        if input.command.len() > 32768 {
            return Err(AppError::bad_request("invalid command"));
        }
        let actor = actor(state, headers).await?;
        let params = ShellParams {
            command: input.command,
            workdir: input.workdir,
            timeout: input.timeout_ms,
            max_output_tokens: None,
        };
        match tools::execution::start(state, &actor, &params).await {
            Ok((execution, output)) => {
                Ok(Json(StartResponse { execution, output }).into_response())
            }
            Err(failure) => match failure.handle {
                Some(execution_id) => Ok((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(StartFailure {
                        error: "execution outcome uncertain".into(),
                        execution_id,
                        state: "uncertain".into(),
                    }),
                )
                    .into_response()),
                None => Err(error(failure.error)),
            },
        }
    }
    inner(&state, &headers, input)
        .await
        .unwrap_or_else(IntoResponse::into_response)
}

#[utoipa::path(post, path="/api/internal/workspace-tools/write-stdin", operation_id="workspace_tools_write_stdin", tag="workspace-tools", params(IdentityHeaders), request_body=InputRequest, security(("application_bearer"=[])), responses((status=500, body=crate::error::ErrorResponse), (status=403, body=crate::error::ErrorResponse), (status=401, body=crate::error::ErrorResponse), (status=200, body=tools::execution::Descriptor), (status=400, body=crate::error::ErrorResponse), (status=404, body=crate::error::ErrorResponse), (status=409, body=crate::error::ErrorResponse), (status=503, body=crate::error::ErrorResponse)))]
pub(crate) async fn write_stdin(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidatedJson(input): ValidatedJson<InputRequest>,
) -> Result<Json<tools::execution::Descriptor>, AppError> {
    if input.chars.len() > 65536 {
        return Err(AppError::bad_request("invalid input"));
    }
    let actor = actor(&state, &headers).await?;
    tools::execution::input(&state, &actor, &input.execution_id, input.chars)
        .await
        .map_err(error)?;
    Ok(Json(
        tools::execution::current(&state, &actor, &input.execution_id).map_err(error)?,
    ))
}

#[utoipa::path(get, path="/api/internal/workspace-tools/executions/{execution_id}", operation_id="workspace_tools_status", tag="workspace-tools", params(IdentityHeaders, ("execution_id"=String, Path)), security(("application_bearer"=[])), responses((status=500, body=crate::error::ErrorResponse), (status=403, body=crate::error::ErrorResponse), (status=401, body=crate::error::ErrorResponse), (status=200, body=tools::execution::Descriptor), (status=404, body=crate::error::ErrorResponse), (status=503, body=crate::error::ErrorResponse)))]
pub(crate) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<tools::execution::Descriptor>, AppError> {
    let actor = actor(&state, &headers).await?;
    Ok(Json(
        tools::execution::status(&state, &actor, &id)
            .await
            .map_err(error)?,
    ))
}

#[utoipa::path(get, path="/api/internal/workspace-tools/executions/{execution_id}/output", operation_id="workspace_tools_output", tag="workspace-tools", params(IdentityHeaders, ("execution_id"=String, Path), ("cursor"=Option<String>, Query, min_length=1, max_length=128, description="Opaque cursor, at most 128 bytes"), ("max_bytes"=Option<usize>, Query, minimum=1, maximum=1048576, description="Maximum decoded output bytes")), security(("application_bearer"=[])), responses((status=500, body=crate::error::ErrorResponse), (status=403, body=crate::error::ErrorResponse), (status=401, body=crate::error::ErrorResponse), (status=200, body=tools::execution::Output), (status=400, body=crate::error::ErrorResponse), (status=404, body=crate::error::ErrorResponse), (status=409, body=crate::error::ErrorResponse), (status=503, body=crate::error::ErrorResponse)))]
pub(crate) async fn output(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    ValidatedQuery(query): ValidatedQuery<OutputQuery>,
) -> Result<Json<tools::execution::Output>, AppError> {
    if query.cursor.as_ref().is_some_and(|v| v.len() > 128) {
        return Err(AppError::bad_request("invalid cursor"));
    }
    let actor = actor(&state, &headers).await?;
    let result = tools::execution::output(
        &state,
        &actor,
        &id,
        query.cursor,
        query.max_bytes.unwrap_or(1048576),
    )
    .await
    .map_err(|e| match e {
        tools::execution::OutputError::Unavailable => {
            AppError::conflict("execution output unavailable")
        }
        tools::execution::OutputError::Tool(e) => error(e),
    })?;
    Ok(Json(result))
}

#[utoipa::path(get, path="/api/internal/workspace-tools/executions/{execution_id}/final", operation_id="workspace_tools_final", tag="workspace-tools", params(IdentityHeaders, ("execution_id"=String, Path)), security(("application_bearer"=[])), responses((status=500, body=crate::error::ErrorResponse), (status=403, body=crate::error::ErrorResponse), (status=401, body=crate::error::ErrorResponse), (status=200, body=tools::execution::Descriptor), (status=404, body=crate::error::ErrorResponse), (status=409, body=crate::error::ErrorResponse), (status=503, body=crate::error::ErrorResponse)))]
pub(crate) async fn final_result(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<tools::execution::Descriptor>, AppError> {
    let actor = actor(&state, &headers).await?;
    Ok(Json(
        tools::execution::final_result(&state, &actor, &id)
            .await
            .map_err(error)?,
    ))
}

#[utoipa::path(post, path="/api/internal/workspace-tools/executions/{execution_id}/cancel", operation_id="workspace_tools_cancel", tag="workspace-tools", params(IdentityHeaders, ("execution_id"=String, Path)), security(("application_bearer"=[])), responses((status=500, body=crate::error::ErrorResponse), (status=403, body=crate::error::ErrorResponse), (status=401, body=crate::error::ErrorResponse), (status=200, body=tools::execution::Descriptor), (status=404, body=crate::error::ErrorResponse), (status=409, body=crate::error::ErrorResponse), (status=503, body=crate::error::ErrorResponse)))]
pub(crate) async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<tools::execution::Descriptor>, AppError> {
    let actor = actor(&state, &headers).await?;
    Ok(Json(
        tools::execution::cancel(&state, &actor, &id)
            .await
            .map_err(error)?,
    ))
}

#[utoipa::path(post, path="/api/internal/workspace-tools/apply-patch", operation_id="workspace_tools_apply_patch", tag="workspace-tools", params(IdentityHeaders), request_body=ApplyPatchParams, security(("application_bearer"=[])), responses((status=500, body=crate::error::ErrorResponse), (status=403, body=crate::error::ErrorResponse), (status=401, body=crate::error::ErrorResponse), (status=503, body=crate::error::ErrorResponse), (status=404, body=crate::error::ErrorResponse), (status=200, body=PatchResultOutput), (status=400, body=crate::error::ErrorResponse)))]
pub(crate) async fn apply_patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<ApplyPatchParams>,
) -> Result<Json<PatchResultOutput>, AppError> {
    let actor = actor(&state, &headers).await?;
    Ok(Json(
        tools::shared::apply_patch(&state, &actor, &params)
            .await
            .map_err(error)?,
    ))
}
#[utoipa::path(post, path="/api/internal/workspace-tools/read", operation_id="workspace_tools_read", tag="workspace-tools", params(IdentityHeaders), request_body=ReadParams, security(("application_bearer"=[])), responses((status=500, body=crate::error::ErrorResponse), (status=403, body=crate::error::ErrorResponse), (status=401, body=crate::error::ErrorResponse), (status=503, body=crate::error::ErrorResponse), (status=404, body=crate::error::ErrorResponse), (status=200, body=ReadOutput), (status=400, body=crate::error::ErrorResponse)))]
pub(crate) async fn read(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<ReadParams>,
) -> Result<Json<ReadOutput>, AppError> {
    let actor = actor(&state, &headers).await?;
    Ok(Json(
        tools::shared::read(&state, &actor, &params).map_err(error)?,
    ))
}
#[utoipa::path(post, path="/api/internal/workspace-tools/glob", operation_id="workspace_tools_glob", tag="workspace-tools", params(IdentityHeaders), request_body=GlobParams, security(("application_bearer"=[])), responses((status=500, body=crate::error::ErrorResponse), (status=403, body=crate::error::ErrorResponse), (status=401, body=crate::error::ErrorResponse), (status=503, body=crate::error::ErrorResponse), (status=404, body=crate::error::ErrorResponse), (status=200, body=GlobOutput), (status=400, body=crate::error::ErrorResponse)))]
pub(crate) async fn glob(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<GlobParams>,
) -> Result<Json<GlobOutput>, AppError> {
    let actor = actor(&state, &headers).await?;
    Ok(Json(
        tools::shared::glob(&state, &actor, &params)
            .await
            .map_err(error)?,
    ))
}
#[utoipa::path(post, path="/api/internal/workspace-tools/grep", operation_id="workspace_tools_grep", tag="workspace-tools", params(IdentityHeaders), request_body=GrepParams, security(("application_bearer"=[])), responses((status=500, body=crate::error::ErrorResponse), (status=403, body=crate::error::ErrorResponse), (status=401, body=crate::error::ErrorResponse), (status=503, body=crate::error::ErrorResponse), (status=404, body=crate::error::ErrorResponse), (status=200, body=GrepOutput), (status=400, body=crate::error::ErrorResponse)))]
pub(crate) async fn grep(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<GrepParams>,
) -> Result<Json<GrepOutput>, AppError> {
    let actor = actor(&state, &headers).await?;
    Ok(Json(
        tools::shared::grep(&state, &actor, &params)
            .await
            .map_err(error)?,
    ))
}
#[utoipa::path(post, path="/api/internal/workspace-tools/stat", operation_id="workspace_tools_stat", tag="workspace-tools", params(IdentityHeaders), request_body=StatPathParams, security(("application_bearer"=[])), responses((status=500, body=crate::error::ErrorResponse), (status=403, body=crate::error::ErrorResponse), (status=401, body=crate::error::ErrorResponse), (status=503, body=crate::error::ErrorResponse), (status=404, body=crate::error::ErrorResponse), (status=200, body=StatPathOutput), (status=400, body=crate::error::ErrorResponse)))]
pub(crate) async fn stat(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidatedJson(params): ValidatedJson<StatPathParams>,
) -> Result<Json<StatPathOutput>, AppError> {
    let actor = actor(&state, &headers).await?;
    Ok(Json(
        tools::shared::stat_path(&state, &actor, &params).map_err(error)?,
    ))
}
