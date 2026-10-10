//! Database fixture helpers for the native-task suite (issue 753 P1).
//!
//! Split so each file stays inside the size budget: this one owns the isolated
//! manager, environment and workspace seeding, while `admin_state.rs` owns the
//! admin-bootstrapped state, `task_seed.rs` owns the executed-task seeds and
//! `support.rs` owns the HTTP caller and the task fixture.
//!
//! Every fixture builds rows straight through the project's own queries against
//! its own isolated database; the shared database is never reset or truncated.

use desk_foreman::db::{queries::create_runner_manager, types::CreateRunnerManagerRequest};
use sqlx::PgPool;
use uuid::Uuid;

use super::support::RESOURCE_MARKER;

pub async fn create_manager(pool: &PgPool, name: &str) -> (i64, String) {
    let (record, token) = create_runner_manager(
        pool,
        &CreateRunnerManagerRequest {
            name: name.to_string(),
            endpoint: format!("http://{name}.invalid"),
            access_token: None,
            image: "test-image".to_string(),
            network_enabled: false,
            max_output_bytes: 65_536,
            max_timeout_ms: 600_000,
            max_sessions: 2,
            pids_limit: 64,
            memory_limit: "256m".to_string(),
            cpu_limit: "1".to_string(),
            host_workspace_root: None,
        },
    )
    .await
    .expect("create runner manager");
    let manager_id = record.runner_manager_id;
    sqlx::query("UPDATE runner_managers SET enabled = TRUE, status = 'online', last_seen_at = NOW() WHERE runner_manager_id = $1")
        .bind(manager_id)
        .execute(pool)
        .await
        .expect("mark manager online");
    (manager_id, token.expect("generated bearer token"))
}

/// Create one environment row straight through the metadata query.
pub async fn create_environment(
    pool: &PgPool,
    application_id: i64,
    external_user_id: &str,
) -> Uuid {
    let outcome = desk_foreman::db::queries::ensure_execution_environment(
        pool,
        application_id,
        external_user_id,
        "opencode",
    )
    .await
    .expect("ensure environment");
    match outcome {
        desk_foreman::db::execution_environments::EnsureOutcome::Created(environment)
        | desk_foreman::db::execution_environments::EnsureOutcome::Existing(environment) => {
            environment.environment_id
        }
        desk_foreman::db::execution_environments::EnsureOutcome::RuntimeConflict { .. } => {
            panic!("ensure must not conflict for a fresh external user")
        }
    }
}

/// Pin an environment into the exact idle shape a task intent may bind to:
/// absent candidate, no unresolved lifecycle blocker, bound manager.
pub async fn set_idle_fence(pool: &PgPool, environment_id: Uuid, manager_id: i64, generation: i64) {
    sqlx::query(
        "UPDATE execution_environments \
         SET generation = $2, container_generation = NULL, assigned_manager_id = $3, \
             state = 'absent', stop_proof_job_id = NULL, stop_proof_generation = NULL, \
             active_native_task_id = NULL \
         WHERE environment_id = $1",
    )
    .bind(environment_id)
    .bind(generation)
    .bind(manager_id)
    .execute(pool)
    .await
    .expect("write environment fence");
}

/// Create one live code-project resource workspace binding.
pub async fn create_code_project_binding(
    pool: &PgPool,
    application_id: i64,
    workspace_root: &str,
    key: &str,
) -> i64 {
    // `key` is the workspace key (`code_project:<id>`); the resource routes
    // address the resource by its bare id, so the two must not be conflated.
    let resource_id = key.strip_prefix("code_project:").unwrap_or(key).to_string();
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO workspace_bindings ( \
             application_id, external_user_id, workspace_key, external_user_hash, \
             workspace_root, is_active, lifecycle_state, resource_kind, resource_id \
         ) VALUES ($1, $2, $3, $4, $5, TRUE, 'active', 'code_project', $6) \
         RETURNING workspace_binding_id",
    )
    .bind(application_id)
    .bind(RESOURCE_MARKER)
    .bind(key)
    .bind(hash_marker(key))
    .bind(workspace_root)
    .bind(resource_id)
    .fetch_one(pool)
    .await
    .expect("create code project binding");
    row.0
}

/// Bind a workspace to an active legacy write owner.
///
/// Used to prove that an expired lease name still blocks a native intent: an
/// expiry is a cleanup timer, never evidence that no process is writing.
pub async fn set_legacy_lease_owner(pool: &PgPool, binding_id: i64, owner: &str, expired: bool) {
    sqlx::query(
        "UPDATE workspace_bindings \
         SET write_lease_owner = $2, \
             write_lease_acquired_at = NOW() - INTERVAL '2 hours', \
             write_lease_expires_at = $3 \
         WHERE workspace_binding_id = $1",
    )
    .bind(binding_id)
    .bind(owner)
    .bind(if expired {
        chrono::Utc::now() - chrono::Duration::hours(1)
    } else {
        chrono::Utc::now() + chrono::Duration::hours(1)
    })
    .execute(pool)
    .await
    .expect("write legacy lease owner");
}

fn hash_marker(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
