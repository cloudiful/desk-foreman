# Durable native task intents

An application admits one durable task intent for an authorized workspace
binding. A task is a persisted *intent*, not a running process: an admitted
task has no writer, no checkout, no mount and no container. What the control
plane guarantees is ownership — who may hold a workspace, for how long, and
what has to be proven before it is released.

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

Reads one owned task intent.

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/cancel`

Cancels one intent at the caller's current generation. A task that provably
never started — no candidate generation and no durable job referencing it — is
voided and frees its environment and workspace. Any other task is marked
`uncertain` and keeps blocking.

### `POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/release`

Releases an executed task against its own confirmed stop proof. Every field is
a fence: the proof must be this task's, from this task's original manager, at
this task's generation, recording positive quiescence. Anything else leaves the
task untouched.

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
| 409 | Fence mismatch, subject mismatch, ineligible binding, a legacy write owner, unresolved blockers, or an active-task conflict. |
| 413 | The seed request body, entry count or content size is over the accepted limit. |
| 503 | No runner manager is available to bind. |

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
`admitted`, `preparing`, `running`, `stopping` and `uncertain`. A `released`
task frees both.

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
records a durable job for the task's **original** manager to materialize the
seed into a private checkout and return a typed proof. This is control-plane
scheduling only — it dispatches nothing, mounts nothing, starts no container
and writes no canonical root.

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

### Runner-manager configuration

Preparation is **disabled by default**. Setting `RUNNER_NATIVE_TASK_ROOT` on the
original manager enables it:

| Variable | Meaning |
| --- | --- |
| `RUNNER_NATIVE_TASK_ROOT` | Absolute path to an existing, manager-owned, `0700` directory used as the checkout root. Unset or empty disables preparation. |

The manager refuses to start if the root is relative, missing or not a
directory, and refuses each checkout if the root is a symlink, not owned by the
manager's effective uid, or group/world accessible. A manager with no root
answers every preparation with a fixed unsupported failure, which the control
plane records as an `uncertain` task — never a false success. The manager never
falls back to a workspace or canonical root, and the checkout path is derived
only from the environment and task UUIDs:

```text
<root>/<environment_uuid>/<task_uuid>/tree        published checkout
<root>/<environment_uuid>/<task_uuid>/journal.json identity and phase
```

New directories are `0700` and new files `0600`, all created with `create_new`
so nothing existing is overwritten. The staged tree is hashed and compared with
the sealed manifest before it is published, and a `prepared` journal makes a
repeat call return the proof without rewriting the tree. An interrupted
`preparing` journal is retained untouched and requires explicit reconciliation:
the manager never wipes a candidate checkout and never starts a silently fresh
one.

## Non-goals in this phase

There is no writer, no checkout, no mount, no container and no job dispatch.
No API marks a task `running`, no state volume or credential is exposed, and
nothing here implies tenant isolation or real container execution. A sealed
snapshot is stored bytes in the control plane: it dispatches nothing, prepares
nothing and mounts nothing, and it never touches a legacy canonical root.
