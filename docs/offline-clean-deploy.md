# Offline clean deployment

English | [简体中文](offline-clean-deploy.zh-CN.md)

This runbook describes a single-instance maintenance deployment that stops
admission and actors, discards an explicitly approved set of workspace and
history state, and starts clean. It is documentation only: it does not
authorize execution, and it contains no destructive commands to copy.

## Warning and approval gate

The cleanup is irreversible. Do not start until the operator has:

- a written approval identifying the exact target instance and the exact state
  to discard;
- a current database backup and a restore decision;
- a live manifest of the selected state (see below), not just this document.

This document is not a production-verified inventory. Resolve every deployment
specific value from the actual deployment, never from an example here.

## Deployment model

- The deployment uses a single-instance offline cutover; it has no online
  reset, rolling compatibility, old-state adoption, or resume path.
- The disposal scope is the managed workspace content and historical
  session/task history described below; the recorded permission remains in the
  issue record.
- Login sessions are not workspace history and stay protected.
- Unknown or unclassified state is kept, not deleted.

## State categories

### A. Approved disposable category

This category covers the managed workspace directories and the historical
conversation/session history listed as cleanable candidates below, subject to
the selection and dependency rules in this runbook. It is not a blind-wipe
scope: the exact rows, paths, volumes and tables must still be selected and
verified.

### B. Candidate tables requiring per-deployment selection

Do not treat this as an executable delete list. Each table needs an explicit
selection, row counts, retention disposition and dependency verification.

Desk Foreman workspace/runtime candidates:

- `workspace_bindings`
- `workspace_runners`
- `runner_principal_admission`
- `runner_owner_stop_fence`
- `execution_environments`
- `execution_environment_jobs`
- `execution_environment_tasks`
- `execution_environment_task_seeds`
- `execution_environment_task_seed_files`
- `execution_environment_task_exports`
- `execution_environment_task_export_selection_files`
- `execution_environment_task_export_files`
- `execution_environment_task_export_missing_paths`

Stock cleanable conversation-history candidates (schema `stock_app`):

- `ai_conversation`
- `ai_message`
- `ai_attachment`
- `ai_artifact` (may contain user deliverables; archive instead of deleting
  unless the owner approves disposal)
- `ai_tool_output`
- `ai_run`
- `ai_run_event`
- `ai_run_message`
- `ai_prompt_queue`
- `ai_pending_question`
- `ai_conversation_folder`
- `ai_conversation_memory_snapshot`
- `ai_memory_entry`

Quiesce and classify pending or running `ai_run` and `ai_prompt_queue` work
before disposing history. An enqueued item is not proof that no external effect
can still occur.

### C. HOLD: external-write, approval, or audit evidence

Keep, archive, or reconcile these records; do not include them in a bulk
deletion. Preserving identifiers and linkages is not the same as deleting
proof.

Desk Foreman:

- `audit_log`
- `workspace_git_jobs`

Stock:

- `code_project_push_evidence`
- `code_project_push_evidence_ref`
- `code_project_run_authority` (must survive even though its
  `workspace_binding_id` is nullable; it carries external-write evidence and
  has no foreign keys)
- `code_project_mutation_intent`
- `code_project_native_task_input`
- `ai_tool_approval_policy`
- `ai_tool_pending_approval`
- `ai_tool_approval_grant`
- `auth_audit_log`
- `code_project_mcp_workspace_binding` in `pending`, `active`, or `stopping`
  state: reconcile first. Only an association proven `stopped` and disposable
  is a cleanup candidate.

Also retain any approval, push receipt, authentication audit, or reconciliation
evidence connected to the discarded state.

### D. Protected: never deleted by this runbook

Desk Foreman:

- `users`
- `web_sessions`
- `mcp_tokens`
- `applications`
- `application_tokens`
- `runner_managers`

Stock:

- `users`
- `auth_passkeys`
- market data in `stock_app`, `stock_analytics`, and `stock_raw`
- `code_project`
- `code_project_run`
- `code_project_runtime_config`
- `code_project_member`
- `ai_provider_endpoint`
- `ai_model_config`
- `ai_mcp_server`
- `ai_mcp_tools`
- `ai_research_preference`
- `ai_runtime_config`

Do not bootstrap or wipe a whole business database to satisfy foreign keys.

## Required live manifest

Record and review all of the following before any deletion:

1. Target instance, running revision, database identity, backup reference, and
   the named maintenance owner.
2. Selected tables with row counts, retention disposition, and an inspected
   foreign-key graph for each selection.
3. Exact resolved canonical host and container paths, volume names, and mount
   topology, plus current task ownership and a check for sharing with
   unrelated projects.
4. The deployment's actual values for `WORKSPACE_ROOT`,
   `RUNNER_HOST_WORKSPACE_ROOT`, `RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR`,
   `RUNNER_NATIVE_TASK_ROOT`, `RUNNER_NATIVE_TASK_HOST_ROOT`,
   `RUNNER_GIT_PRIVATE_ROOT`, and `RUNNER_GIT_HOST_PRIVATE_ROOT`, and the
   per-environment UUID volumes.
5. The Stock compose project/profile names in use. `stock-code-projects` and
   `stock-ai-workspaces` are logical names only; record the actual identifiers
   from the deployment.
6. Which workspace paths are selected. A root may be the parent of private
   credentials or unrelated projects: never delete a mount parent by
   inference, only the selected workspace directories.

Keep credential archives out of workspaces, model traffic, and Shell output.
Rotate or preserve machine-managed credentials through the supported service
lifecycle; never copy secrets into command arguments or temporary files.

## Shutdown

1. Disable external ingress and admission first.
2. Stop scheduled and queued producers, including Stock orchestration that can
   submit new work.
3. Quiesce Stock consumers (`stock`, `stock-rust-tools`) before stopping the
   Desk Foreman manager and service.
4. Stop the Desk Foreman manager and service, then verify that every task,
   session, container and native child process is actually stopped. Stopping
   the manager alone can leave Docker containers or native child processes
   running.
5. Inventory every managed process and container with its ownership, volumes,
   state, and stop confirmation. Do not affect unrelated containers.
6. Keep the database available and prevent restarts, autoheal, or scheduler
   admission until every check is complete.

Any unconfirmed stop or unresolved external write blocks disposal. Never blindly
replay an uncertain Forgejo or SFTP write; preserve receipts and reconcile the
external state read-only. The stop/start ordering above is an example topology
from the source, not a production deployment command.

## Selection, archive, and deletion rules

- Archive conservatively before disposal, especially user deliverables in
  `ai_artifact` and records with approval or linkage value.
- No whole-database or whole-schema wipe, no `CASCADE`/`TRUNCATE` sweep, no
  Docker global prune, no wildcard table or path selection, and no deletion of
  unknown volumes.
- Historical session disposal never authorizes an authentication-session wipe.

Desk Foreman dependency handling, once a selection is approved:

1. The task/job/environment cycle has no plain child-first order. Before any
   deletion, clear the five task/environment reference columns
   (`execution_environment_tasks.prepare_job_id`, `start_job_id`,
   `stop_job_id`; `execution_environment_jobs.native_task_id`;
   `execution_environments.active_native_task_id`) for every selected row, then
   verify that no unselected row still references a selected row.
2. Delete the export subtree child-first:
   `execution_environment_task_export_files` and
   `execution_environment_task_export_missing_paths`, then
   `execution_environment_task_export_selection_files`, then
   `execution_environment_task_exports`.
3. Delete `execution_environment_task_seed_files`, then
   `execution_environment_task_seeds`.
4. Delete `execution_environment_tasks`, then
   `execution_environment_jobs`, then `execution_environments`.
5. `audit_log` references `workspace_bindings` with NO ACTION and blocks
   binding deletion. Do not null or delete audit rows to bypass it: archive the
   audit evidence and retain the referenced binding until a separately approved
   retention disposition exists. HOLD Git jobs and receipts that reference a
   binding have the same consequence.
6. `workspace_git_jobs` also blocks `runner_managers` and its binding. Deleting
   a `workspace_bindings` row cascades its `workspace_runners` rows through the
   single-column and composite owner foreign keys. Separately, deleting a
   `runner_managers` row only sets `workspace_runners.runner_manager_id` to
   NULL; it does not delete the runner row. `runner_owner_stop_fence` and
   `runner_principal_admission` have no foreign keys.

Stock tables need their own child-first order. The exact order is not
enumerated here and must come from read-only foreign-key introspection of the
target database before any deletion transaction is prepared. Stop when a
constraint is unknown; never invent an order or silently null records to make a
delete succeed.

## Restart and verification

1. After approved cleanup and schema/version verification, start the Desk
   Foreman service, then the runner manager, with the existing identities and
   private configuration.
2. Verify health and capability endpoints.
3. Start Stock, then reopen ingress only after fresh authenticated MCP
   bind/session and user/workspace isolation smoke checks succeed.
4. Scope purged-set assertions to the approved selection only. Do not require
   HOLD Git jobs or bindings to be empty when preserved receipts still
   reference them.
5. A fresh workspace gets a new supported binding and session. Never adopt or
   replay old queued tasks or old request identities.
6. Preserve full Git capabilities. Do not fetch, pull, or overwrite dirty
   supported live workspaces as part of this runbook.

## Failure handling

On any unexpected schema state, stop state, or error, keep admission closed and
retain the evidence. A backup restore is a separately approved, coordinated
maintenance operation; do not mix restored old actor state with new processes
or replay uncertain external writes. There is no automatic rollback or resume
mechanism.

## Open operator items

- Exact production roots, volumes, and mount topology.
- Exact Stock foreign-key deletion order from read-only target introspection.
- Classification of `workspace_git_jobs` pending/claimed rows and
  `code_project_mcp_workspace_binding` pending/active rows.
- Archive-versus-delete disposition for `ai_artifact`.

Production inspection, cleanup execution, and runtime verification each
require separate authorization before they are performed.
