use std::collections::BTreeMap;

use runner_protocol::{
    CleanupWorkspaceBindingPrincipalsRequest, RunnerOwner, StopWorkspacePrincipalAck,
    StopWorkspacePrincipalRequest, StopWorkspacePrincipalState,
    workspace_principal::application_principal_owner,
};
use sqlx::PgPool;

use super::RunnerService;
use crate::db::principal_admission::{self, AdmissionBinding, PrincipalAdmission};

#[cfg(test)]
#[path = "principal_stop_tests.rs"]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmittedWorkCleanup {
    NoActiveAdmissions,
    ConfirmedStopped,
}

fn admission_owner(row: &PrincipalAdmission) -> anyhow::Result<RunnerOwner> {
    let owner = RunnerOwner::ApplicationPrincipal {
        application_id: row.application_id,
        workspace_binding_id: row.workspace_binding_id,
        principal_digest: row.principal_digest.clone(),
    };
    let request = StopWorkspacePrincipalRequest {
        owner: owner.clone(),
        principal_id: "validated".into(),
    };
    if !request.is_valid() || owner.stable_key() != row.owner_key {
        anyhow::bail!("invalid stored admission identity");
    }
    Ok(owner)
}

fn current_owner(binding: &AdmissionBinding) -> Option<RunnerOwner> {
    if binding.resource_kind.as_deref().is_none_or(str::is_empty)
        || binding.resource_id.as_deref().is_none_or(str::is_empty)
    {
        return None;
    }
    let lease_owner = binding.write_lease_owner.as_deref()?;
    application_principal_owner(
        binding.application_id,
        binding.workspace_binding_id,
        lease_owner,
        &binding.workspace_key,
    )
    .ok()
}

async fn mark_captured(pool: &PgPool, rows: &[PrincipalAdmission]) -> anyhow::Result<()> {
    principal_admission::mark_stopped_batch(pool, rows).await
}

pub async fn stop_owner(
    pool: &PgPool,
    runner: &dyn RunnerService,
    request: StopWorkspacePrincipalRequest,
) -> anyhow::Result<StopWorkspacePrincipalAck> {
    if !request.is_valid() {
        anyhow::bail!("invalid principal stop request");
    }
    principal_admission::fence(pool, &request.owner).await?;
    let captured = principal_admission::list_active_by_owner(pool, &request.owner).await?;
    let ack = runner.stop_workspace_principal(request).await?;
    if ack.state == StopWorkspacePrincipalState::Stopped {
        mark_captured(pool, &captured).await?;
    }
    Ok(ack)
}

pub async fn cleanup_binding_admitted_work(
    pool: &PgPool,
    runner: &dyn RunnerService,
    binding_id: i64,
) -> anyhow::Result<AdmittedWorkCleanup> {
    if binding_id <= 0 {
        anyhow::bail!("invalid workspace binding id");
    }
    let binding = principal_admission::find_binding(pool, binding_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("workspace binding not found"))?;
    if let Some(owner) = current_owner(&binding) {
        principal_admission::fence(pool, &owner).await?;
    }
    let active = principal_admission::list_active_by_binding(pool, binding_id).await?;
    let mut owners = BTreeMap::new();
    for row in &active {
        let owner = admission_owner(row)?;
        owners.insert(owner.stable_key(), owner);
    }
    for owner in owners.values() {
        principal_admission::fence(pool, owner).await?;
    }
    let captured = principal_admission::list_active_by_binding(pool, binding_id).await?;
    if captured.is_empty() {
        return Ok(AdmittedWorkCleanup::NoActiveAdmissions);
    }
    for row in &captured {
        let owner = admission_owner(row)?;
        if !owners.contains_key(&owner.stable_key())
            && current_owner(&binding).as_ref() != Some(&owner)
        {
            anyhow::bail!("new admission identity appeared during binding cleanup");
        }
    }
    let ack = runner
        .cleanup_workspace_binding_principals(CleanupWorkspaceBindingPrincipalsRequest {
            workspace_binding_id: binding_id,
        })
        .await?;
    if ack.state != StopWorkspacePrincipalState::Stopped || ack.owners.iter().any(|entry| entry.state != StopWorkspacePrincipalState::Stopped || !matches!(&entry.owner, RunnerOwner::ApplicationPrincipal { workspace_binding_id, .. } if *workspace_binding_id == binding_id) || !(StopWorkspacePrincipalRequest { owner: entry.owner.clone(), principal_id: "validated".into() }).is_valid()) {
        anyhow::bail!("principal binding cleanup unconfirmed: {:?}", ack.state);
    }
    mark_captured(pool, &captured).await?;
    Ok(AdmittedWorkCleanup::ConfirmedStopped)
}
