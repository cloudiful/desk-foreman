# Offline clean deployment

English | [简体中文](offline-clean-deploy.zh-CN.md)

This runbook describes a single-instance maintenance deployment that stops
admission and actors, disposes an explicitly approved set of managed workspace
and runtime state, and starts clean. It is documentation only: it does not
authorize execution and contains no destructive command to copy.

## Warning and approval gate

The cleanup is irreversible. Do not start until the operator has:

- written approval naming the exact target instance and the state to discard;
- a current database backup and a restore decision;
- a live manifest of the selected state, not just this document.

This document is not a production-verified inventory. Resolve every
deployment-specific value from the actual deployment.

## Deployment model

- The deployment is a single-instance offline cutover with no online reset,
  rolling compatibility, adoption of old state, or resume path.
- The disposal scope is the selected managed workspace content and runtime
  history; the approval remains on the deployment record.
- Login sessions are not workspace history and stay protected.
- Unknown or unclassified state is kept, not deleted.

## State categories

### A. Approved disposable category

Managed workspace directories and selected runtime rows are cleanable only
after exact paths, rows, volumes and tables are selected and verified. This is
not a blind-wipe scope.

### B. Candidate tables requiring per-deployment selection

Each table needs an explicit selection, row counts, retention disposition and
dependency verification.

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

Quiesce and classify pending or running work before disposing runtime history.
A queued item is not proof that no external effect can still occur.

### C. HOLD: external-write, approval, or audit evidence

Keep, archive, or reconcile these records; do not include them in a bulk
deletion.

- `audit_log`
- Retained inert historical evidence whose table and rows remain preserved

Also retain any approval or reconciliation evidence connected to the selected
state.

### D. Protected: never deleted by this runbook

- `users`
- `web_sessions`
- `mcp_tokens`
- `applications`
- `application_tokens`
- `runner_managers`

Do not bootstrap or wipe a whole database to satisfy foreign keys.

## Required live manifest

Record and review all of the following before deletion:

1. Target instance, running revision, database identity, backup reference and
   named maintenance owner.
2. Selected tables with row counts, retention disposition and an inspected
   foreign-key graph for each selection.
3. Exact resolved canonical host and container paths, volume names and mount
   topology, plus current task ownership and sharing checks.
4. The deployment's actual values for `WORKSPACE_ROOT`,
   `RUNNER_HOST_WORKSPACE_ROOT`, `RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR`,
   `RUNNER_NATIVE_TASK_ROOT`, `RUNNER_NATIVE_TASK_HOST_ROOT`, and the
   per-environment UUID volumes.
5. Which workspace paths are selected. A root may be the parent of private
   credentials or unrelated projects: never delete a mount parent by
   inference, only the selected workspace directories.

Keep credential archives out of workspaces, model traffic and Shell output.
Rotate or preserve machine-managed credentials through the supported service
lifecycle; never copy secrets into command arguments or temporary files.

## Shutdown

1. Disable external ingress and admission first.
2. Stop scheduled and queued producers that can submit new work.
3. Stop the Desk Foreman manager and service, then verify every task, session,
   container and native child process is actually stopped. Stopping the manager
   alone can leave Docker containers or native child processes running.
4. Inventory every managed process and container with its ownership, volumes,
   state and stop confirmation. Do not affect unrelated containers.
5. Keep the database available and prevent restarts, autoheal or scheduler
   admission until every check is complete.

Any unconfirmed stop or unresolved external write blocks disposal. Reconcile
uncertain external state read-only and preserve receipts.

## Selection, archive and deletion rules

- Archive conservatively before disposal, especially user deliverables and
  records with approval or linkage value.
- No whole-database or whole-schema wipe, no `CASCADE`/`TRUNCATE` sweep, no
  Docker global prune, no wildcard table or path selection, and no deletion of
  unknown volumes.
- Historical session disposal never authorizes an authentication-session wipe.

Desk Foreman dependency handling, once a selection is approved:

1. The task/job/environment cycle has no plain child-first order. Clear the
   task/environment reference columns
   (`execution_environment_tasks.prepare_job_id`, `start_job_id`,
   `stop_job_id`; `execution_environment_jobs.native_task_id`;
   `execution_environments.active_native_task_id`) for every selected row, then
   verify no unselected row still references a selected row.
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
   retention disposition exists. Retained inert historical evidence no longer
   blocks binding or manager cleanup.

## Restart and verification

1. After approved cleanup and schema/version verification, start the Desk
   Foreman service, then the runner manager, with the existing identities and
   private configuration.
2. Verify health and capability endpoints, then reopen ingress only after fresh
   authenticated MCP bind/session and user/workspace isolation smoke checks.
3. Scope purged-set assertions to the approved selection only.
4. A fresh workspace gets a new supported binding and session. Never adopt or
   replay old queued tasks or old request identities.
5. Preserve generic Local Git capability. Do not fetch, pull or overwrite dirty
   supported live workspaces as part of this runbook.

## Failure handling

On any unexpected schema state, stop state or error, keep admission closed and
retain the evidence. A backup restore is a separately approved, coordinated
maintenance operation; do not mix restored old actor state with new processes
or replay uncertain external writes. There is no automatic rollback or resume.

## Open operator items

- Exact production roots, volumes and mount topology.
- Exact foreign-key deletion order from read-only target introspection.
- Archive-versus-delete disposition for user deliverables.

Production inspection, cleanup execution and runtime verification each require
separate authorization.
