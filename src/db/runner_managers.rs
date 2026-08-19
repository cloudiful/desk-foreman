use runner_protocol::RUNNER_MANAGER_HEARTBEAT_TTL_SECS;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    auth::hash_bearer_token,
    db::types::{
        CreateRunnerManagerRequest, ListRunnerManagersParams, Page, RunnerManagerRecord,
        RunnerManagerResponse, UpdateRunnerManagerRequest,
    },
};

pub async fn list_runner_managers(
    pool: &PgPool,
    params: &ListRunnerManagersParams,
) -> anyhow::Result<Page<RunnerManagerResponse>> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);
    let total: i64 = sqlx::query_scalar(include_str!("../sql/count_runner_managers.sql"))
        .bind(&params.search)
        .bind(params.enabled)
        .fetch_one(pool)
        .await?;
    let items = sqlx::query_as::<_, RunnerManagerResponse>(include_str!(
        "../sql/list_runner_managers.sql"
    ))
    .bind(RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64)
    .bind(&params.search)
    .bind(params.enabled)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(Page {
        items,
        total,
        limit,
        offset,
    })
}

pub async fn create_runner_manager(
    pool: &PgPool,
    request: &CreateRunnerManagerRequest,
) -> anyhow::Result<(RunnerManagerResponse, Option<String>)> {
    let generated = request
        .access_token
        .as_deref()
        .is_none_or(|value| value.trim().is_empty());
    let token = request
        .access_token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("dfm_{}", Uuid::new_v4().simple()));
    let record = sqlx::query_as::<_, RunnerManagerResponse>(include_str!(
        "../sql/create_runner_manager.sql"
    ))
    .bind(&request.name)
    .bind(&request.endpoint)
    .bind(hash_bearer_token(&token))
    .bind(&request.image)
    .bind(request.network_enabled)
    .bind(request.max_output_bytes)
    .bind(request.max_timeout_ms)
    .bind(request.max_sessions)
    .bind(request.pids_limit)
    .bind(&request.memory_limit)
    .bind(&request.cpu_limit)
    .bind(&request.host_workspace_root)
    .fetch_one(pool)
    .await?;
    Ok((record, generated.then_some(token)))
}

pub async fn update_runner_manager(
    pool: &PgPool,
    manager_id: i64,
    request: &UpdateRunnerManagerRequest,
) -> anyhow::Result<Option<RunnerManagerResponse>> {
    Ok(
        sqlx::query_as(include_str!("../sql/update_runner_manager.sql"))
            .bind(manager_id)
            .bind(&request.endpoint)
            .bind(request.enabled)
            .bind(&request.image)
            .bind(request.network_enabled)
            .bind(request.max_output_bytes)
            .bind(request.max_timeout_ms)
            .bind(request.max_sessions)
            .bind(request.pids_limit)
            .bind(&request.memory_limit)
            .bind(&request.cpu_limit)
            .bind(&request.host_workspace_root)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn find_runner_manager_by_token(
    pool: &PgPool,
    token: &str,
) -> anyhow::Result<Option<RunnerManagerRecord>> {
    Ok(
        sqlx::query_as(include_str!("../sql/find_runner_manager_by_token.sql"))
            .bind(hash_bearer_token(token))
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn find_enabled_runner_manager(
    pool: &PgPool,
) -> anyhow::Result<Option<RunnerManagerRecord>> {
    Ok(sqlx::query_as::<_, RunnerManagerRecord>(include_str!(
        "../sql/find_enabled_runner_manager.sql"
    ))
    .bind(RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_live_runner_manager_ids(pool: &PgPool) -> anyhow::Result<Vec<i64>> {
    Ok(
        sqlx::query_scalar(include_str!("../sql/list_live_runner_manager_ids.sql"))
            .bind(RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn touch_runner_manager(pool: &PgPool, manager_id: i64) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/touch_runner_manager.sql"))
        .bind(manager_id)
        .execute(pool)
        .await?;
    Ok(())
}
