# Durable environment-operation records and dispatch (internal)

Internal persistence and assigned-manager dispatch slice for environment
operations (issues 741/742). It stores operation intent, claim and result
durably, dispatches pending records to the assigned manager over the existing
authenticated runner-manager transport, and keeps execution-environment owners
out of the legacy workspace path. No manager stop barrier, lifecycle
orchestration, or production enablement lives here.

## Store

`execution_environment_jobs` holds one row per operation intent: a `ensure` or
`stop` for one exact generation of one execution environment.

| Column | Meaning |
| --- | --- |
| `job_id` | UUID primary key, stable wire identity |
| `token` | opaque caller-chosen replay key, globally unique |
| `environment_id`, `application_id` | owner pair, guaranteed consistent by the composite FK |
| `assigned_manager_id` | immutable manager assigned at enqueue (`RESTRICT`) |
| `job_kind` | checked `ensure` or `stop` |
| `target_generation` | `>= 0` |
| `state` | checked `pending`/`claimed`/`succeeded`/`failed`/`uncertain` |
| `claimed_at`, `deadline_at`, `terminal_at` | lifecycle timestamps |
| `result` | typed JSON operation result, never a host path or secret |
| `error_summary` | bounded, redacted diagnostic string |

Two partial unique indexes keep at most one active `ensure` and one active
`stop` per environment across **all** generations. An active `ensure` and an
active `stop` may coexist: they are durable records, not runnable concurrent
operations.

## State machine

- `pending` -> `claimed` by the assigned manager, before the deadline, exactly
  once.
- `claimed` -> `succeeded`/`failed` by the assigned claimant, before the
  deadline.
- `pending`/`claimed` past the deadline -> `uncertain`, fail closed. `uncertain`
  is terminal, is never claimable again, and stays an active blocker; no API
  clears or retries it.

## Operations

- `enqueue_execution_environment_job`: validates the owner pair and an existing
  enabled manager inside SQL, then inserts. A same-token identical intent
  replays the original row; a same-owner token with different intent returns
  `TokenConflict`; an active operation of the same kind returns
  `ActiveConflict` without exposing the existing row; a foreign owner pair or
  missing/disabled manager returns `EnvironmentNotFound`.
- `find_execution_environment_job_by_token`, `find_active_execution_environment_job`:
  owner-scoped reads; the active lookup returns the pending/claimed/uncertain
  blocker.
- `claim_execution_environment_job`: one atomic compare-and-set; exactly one
  concurrent caller wins, wrong managers and expired jobs never claim.
- `complete_execution_environment_job`: atomic compare-and-set on job id, owner
  pair, token, assigned claimant, `claimed` state and an unexpired deadline. An
  identical terminal replay returns the recorded outcome; a conflicting replay
  is refused. Completion after the deadline never succeeds.
- `expire_execution_environment_jobs`: atomically moves expired `pending` and
  `claimed` rows of one owned environment to `uncertain`.
- `claim_next_assigned_manager_environment_job`: manager-only claim of the
  oldest pending row assigned to the authenticated manager via one atomic
  update using `FOR UPDATE SKIP LOCKED`; expired, claimed and `uncertain` rows
  are never claimable.
- `find_execution_environment_job_by_id` and
  `complete_assigned_manager_environment_job`: manager-only trusted lookup by
  stable UUID and completion, used exclusively by the authenticated
  runner-manager result path.

## Assigned-manager dispatch

Durable jobs use one reserved opaque wire id, `envjob:<UUID>`, so they never
collide with the legacy broker's plain-UUID ids:

- `GET /api/internal/runner-manager/jobs/next` first tries the durable claim for
  the authenticated manager. A claimed row is returned as an existing
  `RunnerJob` with kind `ensure_execution_environment` /
  `stop_execution_environment` and an owner-only payload, exactly what the
  runner-manager worker already deserializes. When no durable job is claimable
  the route keeps the unchanged legacy broker behavior.
- `POST /api/internal/runner-manager/jobs/result` routes a reserved `envjob:`
  id exclusively to the durable handler; a malformed prefixed id is rejected
  and never falls back to the legacy broker. Plain-UUID results still route to
  the legacy broker.

The row is the authority, never the client: after parsing the id the handler
looks the row up, verifies the authenticated manager equals `assigned_manager_id`,
and derives application, environment, token and owner from the row. A reported
ack must deserialize into the existing `EnsureExecutionEnvironmentAck` /
`StopExecutionEnvironmentAck`, match the exact `RunnerOwner` and derived
container name, and satisfy its kind's success rule — `ensure` succeeds only
with `running`; `stop` succeeds only with `confirmed=true` and observed state
`absent`/`exited`/`dead` (`removed` and arbitrary strings are not accepted).
Terminal outcomes:

- consistent success -> `succeeded` with a typed result
  (`ensure_running`, `stop_confirmed`);
- a valid unconfirmed stop or a diagnosed failure -> `uncertain` with a typed
  result (`stop_unconfirmed`, `ensure_uncertain`) and a fixed redacted
  diagnostic; the manager's raw error text is never persisted;
- a malformed or inconsistent ack, a wrong manager, or an unknown row mutates
  nothing and is refused before the store.

An identical terminal replay returns the recorded outcome; a conflicting replay
is refused; a late success cannot clear `uncertain`. The existing completion
compare-and-set keeps that boundary, including its unexpired `claimed`
predicate, so an expired transport timeout stays unreclaimed rather than being
silently cleared.

## Tenancy and legacy exclusion

Ownership is enforced in SQL, not trusted from the caller: every operation
scopes `application_id` and `environment_id` together, the insert selects from
`execution_environments` filtered by the owning application, and a composite
foreign key (backed by a new additive unique constraint on
`execution_environments (environment_id, application_id)`) rejects a foreign
pair. No other application's row is ever returned, including on a globally
colliding token.

`DockerRunnerBackend::ensure_runner` rejects any execution-environment owner
before it builds a mount or runs a Docker command, so an environment owner cannot
enter the legacy workspace/user path. Existing internal-user and workspace
behavior is unchanged.

## Contract notes

- Static SQL lives under `src/sql/execution_environment_jobs/` and is loaded with
  `query_file!`/`query_file_as!`; no runtime static SQL strings. `.sqlx`
  metadata is regenerated with `cargo sqlx prepare --workspace -- --all-targets`
  against an isolated initialized PostgreSQL.
- `GET /api/internal/application/capabilities` keeps
  `execution_environments_ready: false`, and neither flow enables environment
  creation: the dispatch layer only transports durable records to and from an
  already assigned manager.
- The store provides durable operation records only; it does not provide
  distributed process fencing, cannot declare a manager missing, and never
  discards a late running job, resets `uncertain`, or releases a volume.
