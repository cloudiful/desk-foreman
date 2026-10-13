# Cold native task start

A native task can start from a genuinely cold environment: a public `ensure`
that has never named a manager, no warm provisioning, and no manual write to an
environment row.

## The cold shape

A public `ensure` creates an absent environment with
`assigned_manager_id = NULL` and no candidate generation. That row is what this
path is about; nothing else in the product may write a manager onto it before
the native start.

| Step | What it decides about the manager |
| --- | --- |
| `ensure` | Nothing. The environment stays unbound. |
| `admit` | Pins one usable manager on the **task** row. The environment is untouched. |
| `seed` | Nothing. |
| `prepare` | Schedules against the task's pin and accepts that manager's acknowledgment. The environment stays unbound. |
| `start` | Binds the environment to the task's own manager, atomically. |

## What the two predicates allow

Both gates share one rule and nothing else. A cold binding is accepted when the
environment has no manager **and** is still `absent` with no candidate
generation; an already-bound environment is accepted only when it names this
exact manager. Everything else is a conflict, and no other manager is ever
substituted.

- The native start (`src/sql/execution_environments/tasks/start/mark_environment_starting.sql`)
  writes `assigned_manager_id = COALESCE(assigned_manager_id, $6)` and guards with
  `(assigned_manager_id = $6 OR (assigned_manager_id IS NULL AND state = 'absent' AND container_generation IS NULL))`.
- The preparation completion (`src/db/execution_environments/tasks/prepare/complete/transaction.rs`)
  applies the same rule to the manager's acknowledgment, and keeps its
  no-candidate requirement in both branches.

A foreign manager is never retargeted: a non-NULL mismatch is always a conflict,
at both gates.

## Atomicity and rollback

The start binds the manager in the **same statement and the same transaction**
as the `starting` candidate, so the job insert, the task transition and the
environment assignment either all commit or none do. Every other proof is
checked first — binding eligibility, preparation proof, environment candidate,
legacy blockers and manager usability — and nothing is persisted before them. If
the environment update refuses or fails, the transaction rolls back and no job,
no task state and no binding survive.

## What is unchanged

Job kind, operation token, expected generation, task UUID and the manager's
directory profile are identical to the warm path. The token replay path, the
existing original-manager completion checks and the service generation gate are
untouched. No admission, lifecycle, stop, release or seed policy changes, and no
schema, migration, API field or dependency changes: the exported OpenAPI
document is byte-identical to the previous release.

## Related

- [Durable native task intents](execution-environment-tasks.md)
- [Resource workspace bindings](resource-workspace-binding.md)
