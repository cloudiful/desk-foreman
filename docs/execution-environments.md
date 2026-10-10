# Execution environments (internal)

Internal control-plane slice for durable user execution identity (issues
737/742 P3). Additive infrastructure only: no agent endpoint, native workspace
mounts, cross-manager failover, or production migration. Async intent
acknowledgment is never a running or stop confirmation.

## Auth

All calls require an application bearer token plus `X-DF-External-User-Id`
(the existing `application_actor` contract, mirroring the lease routes).
Application identity comes solely from the bearer; a request-supplied
`application_id` is rejected. The header value is auth-only: environment
identity comes from the JSON body.

## Endpoints

### `POST /api/internal/execution-environments/ensure`

Idempotent ensure for one `(application_id, external_user_id)` pair.

Request (`EnsureExecutionEnvironmentRequest`, unknown fields denied):

```json
{"external_user_id": "user-123", "runtime_kind": "opencode"}
```

- `external_user_id`: opaque, non-blank, max 128.
- `runtime_kind`: opaque agent-neutral kind, non-blank, max 64. DF never
  branches on it.

Behavior:

- First call inserts one row with a fresh UUID `environment_id`,
  `generation` 0, `state` `starting`, and `state_volume_ref`
  `df-env-<uuid-simple>` (inert token, no storage provisioned).
- Repeated or concurrent calls return the same `environment_id`,
  `state_volume_ref`, and `generation` unchanged.
- A different `runtime_kind` for the same identity returns `409` without
  mutation.
- Mount paths, volume refs, container addresses, and `application_id`
  cannot be supplied (rejected as `400`).

Response `200`: `ExecutionEnvironment`
(`environment_id`, `application_id`, `external_user_id`, `runtime_kind`,
`generation`, `state`, `state_volume_ref`, timestamps).

### `POST /api/internal/execution-environments/{environment_id}/invalidate`

Owner-scoped atomic fencing step.

Request (`InvalidateExecutionEnvironmentRequest`, unknown fields denied):

```json
{"expected_generation": 0, "reason": "replace"}
```

- `expected_generation`: `>= 0`, compared against the stored generation.
- `reason`: optional diagnostic, max 512.

Behavior:

- On exact match: `generation = generation + 1`, `state = 'stopping'`,
  `200` with the updated row.
- On mismatch: `409` with no mutation.
- Unknown or foreign `environment_id`: `404` with no mutation.
- Invalidation does not stop a writer: this phase has no writers and no
  routing. `GET /api/internal/application/capabilities` keeps
  `execution_environments_ready: false`.

## States and generation

- `state` is checked: `absent`/`starting`/`ready`/`stopping`/`replaced`/
  `unreachable`/`uncertain`. New rows start `starting`; successful invalidates
  set `stopping`.
- `generation` is monotonic `>= 0`. Generation and `state_volume_ref`
  are references, not credentials.

## Asynchronous lifecycle

The metadata `ensure`/`invalidate` routes above keep their semantics. Four
additional authenticated routes admit lifecycle intent asynchronously and
return a public lifecycle descriptor
(`EnvironmentOperationDescriptor`: `environment_id`, `generation`, `state`,
and an optional `operation` of `job_id`/`kind`/`state`). No state-volume
reference, operation token, manager id, host endpoint, or secret is exposed.

All calls resolve the application identity from the bearer first, then run a
short owner-scoped transaction that locks the environment row `FOR UPDATE`.
Requests only carry the fenced inputs and a bounded `operation_token`
(`1..=128`, `deny_unknown_fields`); `application_id`, manager, image, path and
endpoint are never accepted.

### `POST /api/internal/execution-environments/{environment_id}/provision`

Admits one ensure intent. Requires a null provision candidate and no active
conflicting job; binds an enabled+online+recently-seen manager only on first
provision (immutably, never reassigning), records the candidate generation,
sets state `starting`, and enqueues an `ensure` job to the bound manager. A
matching token replay returns the original operation; a duplicate candidate or
a conflicting token is refused. When no manager is available it returns `503`
and binds nothing.

### `POST /api/internal/execution-environments/{environment_id}/stop`

Rejects `not_provisioned` (never fabricates a stop proof) when no candidate
exists. The first stop at the expected generation increments the authorization
generation once, preserves the candidate, clears any previous proof, sets state
`stopping`, and enqueues a `stop` job for the candidate on the bound manager. A
recovery stop after an `uncertain` stop re-admits without another increment,
candidate change, or manager reassignment; an identical token replay does not
increment again. Overdue `pending`/`claimed` jobs are expired to `uncertain`
before any decision; a manager being offline never yields a stop proof.

### `GET /api/internal/execution-environments/{environment_id}/status`

Reconciles persisted evidence only (never dispatches). It validates each job's
typed result, the job's assigned manager against the environment binding, and
the target generation against the candidate. A matching successful ensure marks
`ready` only when the authorization generation equals the candidate and no stop
is outstanding; a matching confirmed stop (`stop_confirmed`, `confirmed=true`,
observed state `absent`/`exited`/`dead`) records the proof job and stopped
generation, marks `replaced`, and supersedes matching obsolete `ensure` rows as
`superseded` with an explicit stop-proof result that is never an ensure
success; an unconfirmed stop downgrades `stopping` to `uncertain`. Repeated
reconciliation is idempotent.

### `POST /api/internal/execution-environments/{environment_id}/replace`

Admits the next ensure. The expected authorization generation, candidate, and
stop-proof job must all match; the proof row must still be a succeeded,
confirmed stop from the original bound manager; there must be no active stop
and no unresolved ensure blocker. It advances the candidate to the current
authorization generation (which must exceed the stopped candidate), consumes
the proof, sets state `starting`, and enqueues an `ensure` job to the bound
manager. A stale or already-consumed proof is refused; the manager binding is
never reassigned.

## Lifecycle persistence

A forward migration (`migrations/*_execution_environment_lifecycle_proof.sql`)
adds `assigned_manager_id` (nullable, `RESTRICT`), `container_generation`
(nullable, `>= 0`; null means no candidate), `stop_proof_job_id`,
`stop_proof_generation`, and the environment `uncertain` state. It also adds
the job `superseded` terminal state (`terminal_at` required) and narrows the
stop active index to `pending`/`claimed` so a prior `uncertain` stop stays
recorded but no longer blocks a recovery stop; the ensure active index keeps
`pending`/`claimed`/`uncertain` across all generations. No earlier migration is
edited.

## Contract notes

- OpenAPI is generated from annotations; regenerate with
  `cargo run -p desk-foreman --bin export-openapi -- openapi.json` and
  verify a second export has no diff.
- Static lifecycle SQL lives under
  `src/sql/execution_environments/lifecycle/` and is loaded with
  `query_file!`/`query_file_as!`; `.sqlx` metadata is regenerated with
  `cargo sqlx prepare --workspace -- --all-targets` against an isolated
  initialized PostgreSQL.
- `GET /api/internal/application/capabilities` keeps
  `execution_environments_ready: false`; no route enables environment creation
  or claims agent readiness.
- DF stays agent-neutral: OpenAPI contains no OpenCode/ACP/prompt/AG-UI
  type. No Docker or provisioning call is reachable from these routes.
