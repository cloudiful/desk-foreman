-- Insert one durable task intent (issue 753 P1).
--
-- `ON CONFLICT DO NOTHING` with no conflict target covers *every* uniqueness
-- constraint on the row: the `task_id` primary key and the `operation_token`
-- unique index. Naming only `task_id` would let a racing insert that collides
-- on `operation_token` raise a unique violation, which surfaces as an internal
-- error instead of a typed refusal.
--
-- A conflict returns no row, and the caller resolves it in the same
-- transaction: an identical intent replays its own existing task, and anything
-- else is a typed conflict. A concurrent uncommitted conflicting insert makes
-- this statement wait, so the caller always observes the committed winner.
INSERT INTO execution_environment_tasks (
    task_id,
    operation_token,
    environment_id,
    application_id,
    workspace_binding_id,
    subject_external_user_id,
    target_generation,
    assigned_manager_id,
    state
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'admitted')
ON CONFLICT DO NOTHING
RETURNING
    task_id,
    operation_token,
    environment_id,
    application_id,
    workspace_binding_id,
    subject_external_user_id,
    target_generation,
    assigned_manager_id,
    state,
    container_generation,
    prepare_job_id,
    created_at,
    updated_at,
    terminal_at;
