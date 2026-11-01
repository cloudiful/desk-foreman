use chrono::{DateTime, Utc};
use runner_protocol::{RunnerOwner, StopWorkspacePrincipalRequest};
use sqlx::PgPool;

#[cfg(test)]
#[path = "principal_admission_tests.rs"]
mod tests;

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct PrincipalAdmission {
    pub owner_key: String,
    pub application_id: i64,
    pub workspace_binding_id: i64,
    pub principal_digest: String,
    pub runner_manager_id: i64,
    pub container_id: String,
    pub admitted_at: DateTime<Utc>,
    pub stopped_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct PrincipalStopFence {
    pub owner_key: String,
    pub application_id: i64,
    pub workspace_binding_id: i64,
    pub principal_digest: String,
    pub requested_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
pub struct AdmissionBinding {
    pub workspace_binding_id: i64,
    pub application_id: i64,
    pub external_user_id: String,
    pub workspace_key: String,
    pub is_active: bool,
    pub lifecycle_state: String,
    pub archived_at: Option<DateTime<Utc>>,
    pub resource_kind: Option<String>,
    pub resource_id: Option<String>,
    pub write_lease_owner: Option<String>,
    pub lease_unexpired: Option<bool>,
}

pub async fn find_binding(pool: &PgPool, id: i64) -> anyhow::Result<Option<AdmissionBinding>> {
    Ok(
        sqlx::query_as(include_str!("../sql/principal_admission/find_binding.sql"))
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdmissionOutcome {
    Admitted(PrincipalAdmission),
    Fenced,
}

fn identity(owner: &RunnerOwner) -> anyhow::Result<(String, i64, i64, &str)> {
    if !(StopWorkspacePrincipalRequest {
        owner: owner.clone(),
        principal_id: "identity".into(),
    })
    .is_valid()
    {
        anyhow::bail!("invalid principal owner");
    }
    match owner {
        RunnerOwner::ApplicationPrincipal {
            application_id,
            workspace_binding_id,
            principal_digest,
        } => Ok((
            owner.stable_key(),
            *application_id,
            *workspace_binding_id,
            principal_digest,
        )),
        _ => anyhow::bail!("invalid principal owner"),
    }
}

fn check_manager_container(manager_id: i64, container_id: &str) -> anyhow::Result<()> {
    if manager_id <= 0
        || container_id.is_empty()
        || container_id.len() > 256
        || container_id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        anyhow::bail!("invalid manager or container id");
    }
    Ok(())
}

pub async fn admit(
    pool: &PgPool,
    manager_id: i64,
    owner: &RunnerOwner,
    container_id: &str,
) -> anyhow::Result<AdmissionOutcome> {
    let (key, app, binding, digest) = identity(owner)?;
    check_manager_container(manager_id, container_id)?;
    let mut tx = pool.begin().await?;
    sqlx::query(include_str!("../sql/principal_admission/lock_owner.sql"))
        .bind(&key)
        .execute(&mut *tx)
        .await?;
    let fenced: bool = sqlx::query_scalar(include_str!("../sql/principal_admission/is_fenced.sql"))
        .bind(&key)
        .fetch_one(&mut *tx)
        .await?;
    if fenced {
        tx.commit().await?;
        return Ok(AdmissionOutcome::Fenced);
    }
    let record = sqlx::query_as::<_, PrincipalAdmission>(include_str!(
        "../sql/principal_admission/admit.sql"
    ))
    .bind(&key)
    .bind(app)
    .bind(binding)
    .bind(digest)
    .bind(manager_id)
    .bind(container_id)
    .fetch_optional(&mut *tx)
    .await?;
    let record = record
        .ok_or_else(|| anyhow::anyhow!("previously stopped container cannot be readmitted"))?;
    tx.commit().await?;
    Ok(AdmissionOutcome::Admitted(record))
}

pub async fn fence(pool: &PgPool, owner: &RunnerOwner) -> anyhow::Result<()> {
    let (key, app, binding, digest) = identity(owner)?;
    let mut tx = pool.begin().await?;
    sqlx::query(include_str!("../sql/principal_admission/lock_owner.sql"))
        .bind(&key)
        .execute(&mut *tx)
        .await?;
    sqlx::query(include_str!("../sql/principal_admission/fence.sql"))
        .bind(&key)
        .bind(app)
        .bind(binding)
        .bind(digest)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn list_active_by_owner(
    pool: &PgPool,
    owner: &RunnerOwner,
) -> anyhow::Result<Vec<PrincipalAdmission>> {
    let (key, _, _, _) = identity(owner)?;
    Ok(
        sqlx::query_as(include_str!("../sql/principal_admission/list_owner.sql"))
            .bind(key)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn list_active_by_binding(
    pool: &PgPool,
    binding_id: i64,
) -> anyhow::Result<Vec<PrincipalAdmission>> {
    if binding_id <= 0 {
        anyhow::bail!("invalid workspace binding id");
    }
    Ok(
        sqlx::query_as(include_str!("../sql/principal_admission/list_binding.sql"))
            .bind(binding_id)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn list_fences_by_binding(
    pool: &PgPool,
    binding_id: i64,
) -> anyhow::Result<Vec<PrincipalStopFence>> {
    if binding_id <= 0 {
        anyhow::bail!("invalid workspace binding id");
    }
    Ok(
        sqlx::query_as(include_str!("../sql/principal_admission/list_fences.sql"))
            .bind(binding_id)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn mark_stopped_batch(pool: &PgPool, rows: &[PrincipalAdmission]) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    for row in rows {
        let owner = RunnerOwner::ApplicationPrincipal {
            application_id: row.application_id,
            workspace_binding_id: row.workspace_binding_id,
            principal_digest: row.principal_digest.clone(),
        };
        let (key, app, binding, digest) = identity(&owner)?;
        if key != row.owner_key {
            anyhow::bail!("invalid stored admission identity");
        }
        check_manager_container(row.runner_manager_id, &row.container_id)?;
        sqlx::query(include_str!("../sql/principal_admission/mark_stopped.sql"))
            .bind(key)
            .bind(app)
            .bind(binding)
            .bind(digest)
            .bind(row.runner_manager_id)
            .bind(&row.container_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn mark_stopped(
    pool: &PgPool,
    manager_id: i64,
    owner: &RunnerOwner,
    container_id: &str,
) -> anyhow::Result<bool> {
    let (key, app, binding, digest) = identity(owner)?;
    check_manager_container(manager_id, container_id)?;
    let changed = sqlx::query(include_str!("../sql/principal_admission/mark_stopped.sql"))
        .bind(key)
        .bind(app)
        .bind(binding)
        .bind(digest)
        .bind(manager_id)
        .bind(container_id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(changed == 1)
}
