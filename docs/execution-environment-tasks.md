# Durable native task intents

See [selected retained task exports](execution-environment-task-exports.md) for
immutable output selection and download.

An application admits one durable task intent for an authorized workspace
binding. Its state progresses from admission and preparation through a fenced
start and a task-aware stop. The control plane records original-manager jobs,
changes the task to `running` only after a matching typed start acknowledgment,
and keeps the workspace reserved until a typed stop proves both environment and
preparation quiescence and the application explicitly releases the task.

```
application ──HTTPS──▶ control plane ──▶ durable task row
                         │
                         └─ owner-scoped fences; one active task per environment and per workspace
```

The task UUID is the owner. It is *not* an authorization generation and not a
mounted container generation: a generation is a fence that may advance while a
task stays active, so an `uncertain` task keeps blocking new intents across
every generation until an operator resolves it.

## Endpoints

Every route resolves the application identity from the bearer token, never from
the body, and refuses unknown fields.

### `POST /api/internal/execution-environments/{environment_id}/tasks/admit`

Admits one task intent, or idempotently returns the existing task when the same
operation token is replayed with an identical intent.

| Field | Meaning |
| --- | --- |
| `task_id` | Caller-known stable task UUID. An identifier, not a credential. |
| `workspace_binding_id` | The authorized workspace binding to bind. |
| `subject_external_user_id` | Must equal the environment's own subject. |
| `expected_generation` | The environment's current authorization generation. |
| `operation_token` | The idempotency key, bounded to 128 bytes. |

### `GET /api/internal/execution-environments/{environment_id}/tasks/{task_id}`

Reads one owned task and its current state, candidate generation and optional
`start_job_id` / `stop_job_id` anchors. The response also carries
`workspace_directory`, the binding's checkout location inside its container —
see [native task workspace directories](native-task-workspace-directory.md).

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/cancel`

Cancels one intent at the caller's current generation. A task that provably
never started — no candidate generation and no durable job referencing it — is
voided and frees its environment and workspace. Any other task is marked
`uncertain` and keeps blocking.

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/release`

Releases a stopped task against its own confirmed stop proof. Every field is a
fence: the proof must be this task's, from this task's original manager, at this
task's candidate generation, and record positive environment confirmation plus
`preparation_quiescent: true`. Stop never releases the task automatically.

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/seed`

Seals one immutable snapshot of the initial files the application authorizes
for an admitted task, or idempotently returns the existing identical snapshot.

| Field | Meaning |
| --- | --- |
| `expected_generation` | The environment's current authorization generation. |
| `files` | The authorized files: `path` (relative POSIX) and `content` (UTF-8). |

A response carries identity only — `task_id`, `manifest_sha256`, `file_count`,
`total_bytes` — and never a path, a credential or a host detail.

### `GET /api/internal/execution-environments/{environment_id}/tasks/{task_id}/seed`

Returns the sealed snapshot with its contents, in canonical path order. The
snapshot is artifact history, so it is returned unchanged in any task state,
including `released` and `uncertain`.

### Response shape

A task descriptor carries identity, binding, generation and state only. It
never contains a host path, state volume, image, port, credential, manager
endpoint or the subject.

| Status | Meaning |
| --- | --- |
| 200 | The task, in the state the operation produced or found. |
| 400 | A missing or oversized operation token. |
| 401 | No application bearer. |
| 404 | A foreign or unknown environment, binding, task or proof. A foreign resource is indistinguishable from an absent one. |
| 409 | Fence mismatch, subject mismatch, ineligible binding, a legacy write owner, unresolved blockers, token conflict or active-task conflict. |
| 413 | The seed request body, entry count or content size is over the accepted limit. |
| 503 | The original runner manager is unavailable for admission, start or stop. |

## Admission rules

A task is admitted only when all of the following hold. The environment row and
then the workspace binding row are locked, in that order, and held until the
intent commits.

- The environment is owned by the calling application and its generation
  matches the caller's fence.
- The subject matches the environment's own subject. The resource binding's
  fixed external-user marker is never compared against it: authorization is the
  authenticated application's delegation, and the marker exists only to keep the
  shared row unique.
- The binding is an active `code_project` resource workspace, and its lifecycle
  admits admission.
- No legacy write-lease owner is present, **even an expired one**. An expiry is
  a cleanup timer, not evidence that no dispatched process is still writing.
- No unresolved ensure or stop operation exists for this environment.
- Neither the environment nor the binding already carries an active task.

The manager a task binds to is the environment's immutable binding, or the same
internal selection first provision uses. A task never re-binds to a replacement
manager, even if that manager goes offline.

## Exclusivity

Two partial unique indexes, and matching admission guards, enforce one active
task per environment and one active task per workspace binding, across
`admitted`, `preparing`, `prepared`, `starting`, `running`, `stopping` and
`uncertain`. A `released` task frees both.

## Legacy fenced writers

A durable task and a legacy workspace writer would write the same workspace
with no shared fence, so every legacy path refuses while a task is active:

- fenced write admission (`lock_workspace_binding_for_write`);
- legacy write-lease acquire, release and force-taking;
- admin archive, restore and reset;
- git/sync through the resource workspace route;
- the archived-workspace janitor sweep, which skips instead of deleting.

Each of those re-reads the native-active guard *after* it holds its own binding
row lock, inside the same transaction. A check evaluated before the lock would
read a snapshot that predates a task admitted while it waited — exactly the
interleaving the guard closes. A task admission takes the same lock, so either
it committed first (and the legacy path sees it) or it waits (and sees the
legacy transition).

Metadata invalidation and the legacy environment provision, replace and stop
routes also refuse while the active-task anchor is present. Status may read the
current environment/job descriptor but never reconciles task-linked start/stop
rows through the legacy lifecycle path.

With no native task, every legacy path behaves exactly as before.

## Immutable task input snapshot

An admitted task may carry one sealed snapshot of the files the application
authorized for it. The snapshot is immutable: there is no overwrite or delete
path, and a second seal with different content is a conflict.

### Bounds

| Limit | Value |
| --- | --- |
| Request body (transport cap, before decoding) | 16 MiB |
| Entries | 4096 |
| Path (UTF-8 bytes) | 512 |
| Single content (UTF-8 bytes) | 1 MiB |
| Aggregate content (UTF-8 bytes) | 8 MiB |

An empty file list is a valid empty workspace.

### Path rules

Paths are normalized relative POSIX paths. Refused: absolute paths, Windows
drive prefixes, backslashes, NUL and other control characters, empty `.`/`..`
components, a trailing slash, duplicate paths, a file that is also a directory
ancestor of another, and any component named `.git` in any case. A path over
512 bytes and every other shape rule is a `400`; the request body, the entry
count and the content/total sizes are `413`. Content containing a NUL is
refused before any SQL, because Postgres `TEXT` cannot store it.

### Manifest

The digest is a fixed, versioned encoding over the file bytes only, so it is
independent of JSON whitespace, request order, task id and generation — the
property that lets a later phase CAS the content against a canonical project:

```
"df-native-task-seed-v1" NUL
for each file in lexicographic path order:
    path_byte_length  u64 big-endian
    path bytes
    content_byte_length  u64 big-endian
    content bytes
```

The result is the lowercase hex SHA-256 of those bytes. Because the task id and
the generation are excluded, the same content hashes identically everywhere.

### Seal rules

The snapshot is written only for a task that is still the environment's active
`admitted` intent with no candidate container generation and no durable job
reference; the environment row lock serializes seals, so concurrent seals of
one task leave exactly one durable input. The seal transaction locks the
environment, then the task, then the binding — the same order every other task
mutation uses.

## Original-manager preparation

An admitted task with a sealed seed may be *prepared*: the control plane
records and dispatches a durable job to the task's **original** manager, which
materializes the seed into a private checkout and returns a typed proof.
Preparation does not start a container or write a canonical root.

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/prepare`

Schedules (or idempotently replays) one preparation.

| Field | Meaning |
| --- | --- |
| `expected_generation` | The environment's current authorization generation. |
| `operation_token` | The idempotency key, bounded to 128 bytes. |

The response is the durable descriptor: `task_id`, `task_state`, `job_id`,
`job_state`, `target_generation`, `active`, `released`. No host path, manager
endpoint or seed content is included.

### `GET /api/internal/execution-environments/{environment_id}/tasks/{task_id}/prepare`

Returns the same descriptor for one owned task. Absent or foreign is `404`.

### `GET /api/internal/runner-managers/{manager_id}/environment-jobs/{job_id}/task-seed`

The manager's own bearer fetches the sealed seed bytes for a job it is assigned.
It is returned only when the authenticated manager is the job's assigned
manager, the job is a live `prepare_native_task` claim, and the task is still
`preparing` on the generation the job targets. Anything else is `404`.

### Preparation rules

Scheduling locks the environment, then the task, then the binding — the same
order every other task mutation uses — and requires all of:

- the task is the environment's active `admitted` intent, with no candidate
  container generation;
- the task's binding is a live code project workspace;
- a sealed seed exists (a preparation with nothing to materialize is refused);
- no unresolved `ensure`/`stop` operation exists;
- the task's **original** manager is still enabled, online and recently seen —
  a preparation never fails over to another manager.

The task moves to `preparing` and links its job in one commit. An identical
token replay returns the existing job with its current status; a different
token is a conflict. A successful, exactly-matching ack marks the task
`prepared` — never the environment `ready`, and never a container generation. A
failed, timed-out or mismatched preparation marks the linked task `uncertain`,
keeping its exclusive environment and workspace anchors; it is never
re-dispatched or reassigned, and a late ack after a cancel never re-marks a
terminal task.

`prepared` is an active task state: like `preparing`, it occupies the
environment and the workspace binding exclusively, so no new task and no legacy
writer can take them.

## Native task start and stop

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/start`

Schedules or replays one start for a prepared task. The strict request contains
only `expected_generation` and an opaque `operation_token`. A start is accepted
only when the task is the environment's active `prepared` task, its linked
preparation job succeeded for the sealed seed, the task and environment fences
match, any previous candidate has a matching positive stop proof, and the
original manager is still enabled, online and recently seen. It does not fail
over to another manager.

The response carries the environment and task state, job identity and
`native_task_id`. Scheduling atomically sets task and environment to `starting`,
records the candidate and links the job. This is a candidate, not running
confirmation. The manager request contains only the stored task identity
(`task_id`, binding, seed manifest and preparation generation) and the
server-derived environment owner. It never accepts a caller path, image,
command or URL.

Only a matching typed result from the original manager, for the current claimed
job and generation, can atomically set the task to `running`, the environment to
`ready`, and the job to `succeeded`. Failure, timeout, cancellation or stale
acknowledgment retains the candidate and both exclusive anchors in `uncertain`.

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/stop`

Schedules or replays a task-aware stop with the same two request fields. It
accepts prepared, preparing, starting, running, stopping and uncertain tasks.
The first stop advances the authorization generation once while retaining the
potential candidate; a recovery stop does not advance it again. The route
returns a stop job and cancels relayed service streams only after the stop intent
commits. It never releases the task.

The manager receives the `stop_native_task` wire kind and the stored task
identity. A successful stop proof requires both a confirmed `absent`, `exited`
or `dead` environment observation and `preparation_quiescent: true`. The
control plane atomically records the positive proof and marks the environment
`replaced`, while the task remains `stopping` and continues to own its workspace.
A failed, expired or non-quiescent stop leaves the task and environment
`uncertain`, with no release proof. A later application release must still pass
the current generation, candidate generation and exact stop-job fences.

The original manager executes typed native starts for prepared tasks when both
native roots and the existing execution-environment profile are configured. It
mounts only the persistent state volume and the UUID-derived task tree at the
binding's checkout directory, then verifies the exact task labels, user, workdir
and two mounts before probing state and task-tree writability. See
[native task workspace directories](native-task-workspace-directory.md). A
`running` journal allows a validated replay without recreating the container or
comparing edited task files to the seed. An interrupted `starting` journal is
retained and never retried automatically.

Native stop persists the generation barrier before Docker inspection, then waits
for the shared environment and task-workspace locks before claiming preparation
quiescence. It validates the manager-owned journal roots without reading or
rewriting task-created descendants, checks the native task labels and mounts, and
stops/removes and reinspects a present candidate. Only a confirmed `absent`,
`exited` or `dead` observation after those checks produces a positive result.
Uncertain Docker outcomes or identity mismatches fail closed with the barrier
retained. Stop preserves the task checkout and state volume; task release remains
a separate explicit operation. `execution_environments_ready` remains false and
service routing does not switch to native task execution.

### Runner-manager configuration

Preparation is **disabled by default**. Native start additionally requires the
manager's existing execution-environment profile and `RUNNER_NATIVE_TASK_HOST_ROOT`:

| Variable | Meaning |
| --- | --- |
| `RUNNER_NATIVE_TASK_ROOT` | Absolute path to an existing, manager-owned, `0700` local checkout root. Unset disables native preparation, start and stop. |
| `RUNNER_NATIVE_TASK_HOST_ROOT` | Optional normalized absolute path visible to the Docker daemon; required for native start and to validate a present stop candidate, never inferred from another root. |

The local root's original path is retained and revalidated at use, so symlink
aliases are refused. A configured host root is not canonicalized or required to
exist on the manager. Neither root falls back to a workspace or canonical
project. The local checkout and daemon bind source are derived only from trusted
environment and task UUIDs:

```text
<local-root>/<environment_uuid>/<task_uuid>/tree
<host-root>/<environment_uuid>/<task_uuid>/tree -> /workspace/bindings/<workspace_binding_id>
```

New directories are `0700` and new files `0600`, all created with `create_new`.
The staged tree is hashed against the sealed manifest before publication. A
`prepared` journal is atomically advanced to `starting` before any Docker start;
only a validated container and successful state/task write probes advance it to
`running`. An interrupted `preparing` or `starting` journal is retained and
requires explicit reconciliation. A running replay verifies the journal,
container identity and mounts without rewriting or rehashing task-edited files.

## Non-goals in this phase

Task artifact cleanup/export, readiness activation and default service routing
remain out of scope; stop never releases a task automatically. This phase keeps
`execution_environments_ready` false and does not switch default service routing.
Manager start/stop coverage uses a fake Docker CLI; no live Docker or remote-runtime
acceptance is claimed. The application API exposes no state volume, credential,
host path, image, command or URL, and no operation writes a legacy canonical
root.
