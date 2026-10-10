//! Sealing one immutable snapshot (issue 753 P2a).
//!
//! One owner-scoped transaction. Lock order is the P1 order — environment
//! `FOR UPDATE`, then task, then binding — never reversed, because task
//! admission and the legacy guards take the same locks in that order.
//!
//! The snapshot is written only for a task that is still the environment's
//! active admitted intent with no candidate and no dispatched job reference:
//! `preparing`/`running`/`stopping`/`uncertain`/`released` are all refused, so
//! a seal can never land after a writer could exist.

use sqlx::PgPool;
use uuid::Uuid;

use super::queries::{find_seed_files, find_seed_header, insert_seed_file, insert_seed_header};
use super::types::internal;
use super::types::{SeedDescriptor, SeedError, SeedFile, SeedPlan};
use crate::db::execution_environments::tasks::queries::{
    count_job_references, lock_binding, lock_environment, lock_task,
};

/// Seal `plan` for `task_id`, or return the existing identical snapshot.
pub(super) async fn seal(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    expected_generation: i64,
    plan: &SeedPlan,
) -> Result<SeedDescriptor, SeedError> {
    let mut tx = pool.begin().await.map_err(internal)?;

    let environment = lock_environment(&mut tx, application_id, environment_id)
        .await
        .map_err(internal)?
        .ok_or(SeedError::NotFound)?;
    if expected_generation != environment.generation {
        return Err(SeedError::StaleGeneration);
    }

    let task = lock_task(&mut tx, application_id, environment_id, task_id)
        .await
        .map_err(internal)?
        .ok_or(SeedError::NotFound)?;

    // Only a still-admitted intent with no candidate and no job reference may
    // receive its input. Anything else could already have a writer.
    if task.state != "admitted" || environment.active_native_task_id != Some(task_id) {
        return Err(SeedError::TaskNotAdmitted);
    }
    if task.container_generation.is_some() || environment.container_generation.is_some() {
        return Err(SeedError::CandidatePresent);
    }
    if count_job_references(&mut tx, task_id)
        .await
        .map_err(internal)?
        > 0
    {
        return Err(SeedError::JobReferencePresent);
    }

    let binding = lock_binding(&mut tx, task.workspace_binding_id)
        .await
        .map_err(internal)?
        .ok_or(SeedError::NotFound)?;
    if binding.application_id != application_id || !binding.is_active_code_project() {
        return Err(SeedError::BindingIneligible);
    }

    // An existing snapshot is immutable: an identical seal replays its
    // descriptor, and any difference is a conflict. It is never overwritten.
    if let Some(existing) = find_seed_header(&mut tx, task_id).await.map_err(internal)? {
        let descriptor = existing.into_descriptor();
        if !descriptor_matches(&descriptor, plan) {
            return Err(SeedError::DifferentContent);
        }
        tx.commit().await.map_err(internal)?;
        return Ok(descriptor);
    }

    let inserted = insert_seed_header(
        &mut tx,
        task_id,
        &plan.manifest_sha256,
        plan.file_count,
        plan.total_bytes,
    )
    .await
    .map_err(internal)?;

    let Some(inserted) = inserted else {
        // A racing seal won: re-read and compare rather than raising.
        let winner = find_seed_header(&mut tx, task_id)
            .await
            .map_err(internal)?
            .ok_or(SeedError::Internal)?
            .into_descriptor();
        if !descriptor_matches(&winner, plan) {
            return Err(SeedError::DifferentContent);
        }
        tx.commit().await.map_err(internal)?;
        return Ok(winner);
    };

    for file in &plan.files {
        insert_seed_file(&mut tx, task_id, &file.path, &file.content)
            .await
            .map_err(internal)?;
    }

    // Read the files back so a storage-level surprise (a duplicate path that
    // slipped past the planning checks) surfaces as a rollback, not a bad row.
    let stored = find_seed_files(&mut tx, task_id).await.map_err(internal)?;
    if !files_match(&stored, &plan.files) {
        return Err(SeedError::Internal);
    }
    let _ = &stored;

    let descriptor = inserted.into_descriptor();
    if !descriptor_matches(&descriptor, plan) {
        return Err(SeedError::Internal);
    }

    tx.commit().await.map_err(internal)?;
    Ok(descriptor)
}

/// True when the stored descriptor is exactly the supplied plan's identity.
fn descriptor_matches(descriptor: &SeedDescriptor, plan: &SeedPlan) -> bool {
    descriptor.manifest_sha256 == plan.manifest_sha256
        && descriptor.file_count == plan.file_count
        && descriptor.total_bytes == plan.total_bytes
}

/// True when the stored files are exactly the planned files, in order.
fn files_match(stored: &[SeedFile], planned: &[SeedFile]) -> bool {
    stored.len() == planned.len()
        && stored
            .iter()
            .zip(planned.iter())
            .all(|(left, right)| left == right)
}
