# Selected retained task exports

[简体中文](execution-environment-task-exports.zh-CN.md) · [Native tasks](execution-environment-tasks.md)

An application can authorize one immutable selection of UTF-8 files from a
retained, positively stopped native task. Export uses that task's original
manager and stopped candidate generation. It remains authorized after explicit
task release or a successor environment becomes ready; it does not change the
current environment, task state, workspace reservation, or relay streams.

The Docker manager backend captures selected files locally without Docker CLI
or socket calls. Other backends fail closed as unsupported. There is no automatic
export, release, cleanup, inventory, archive, or canonical-project import.

## Retained checkout capture

Capture requires the existing local task root, matching published preparation
journal, and persistent stopped-through barrier. It holds the environment mutex,
durable environment reservation, and task lock in that order. A newer successor
high-water does not prevent reading the older stopped task. Capture does not
restore the seed, create directories, rewrite journal/barrier/history, or require
host-root mapping, image credentials, or a service creation profile. Only the
existing lock reservation outside the checkout is created and removed.

Only selected files and their ancestors are inspected. Native-created directories
with mode 0755 and files with mode 0644 are accepted when manager-owned and not
group/world writable. Symlinks, hard links, special files, selected directories,
foreign ownership, and unsafe permissions are refused. Opened handle identity,
size and modification time are checked before and after bounded reads; protected
root identities are rechecked after selection. This uses a trusted private root
and check/open protection, not a race-free openat sandbox.

A missing selected leaf or ancestor is represented as missing; a missing task,
journal or published tree is an error. Binary, NUL-containing or oversized
selected content fails the entire export. Unselected content is not opened.
The manager bounds serialized JSON to 16 MiB before upload, including escaping
expansion. Only a 204 acknowledgement permits a summary result; refusals or lost
responses fail without automatic recapture or retry.

## Application endpoints

All endpoints resolve ownership from the application bearer. Requests reject
unknown fields and accept no host paths, manager configuration, or credentials.

### Schedule

`POST /api/internal/execution-environments/{environment_id}/tasks/{task_id}/export`

```json
{
  "operation_token": "selected-output-1",
  "stop_job_id": "123e4567-e89b-12d3-a456-426614174000",
  "paths": ["README.md", "src/main.rs", "new.txt"]
}
```

The task must be `stopping` or `released`, with a matching successful task-aware
stop job that positively confirms `absent`, `exited`, or `dead` and
`preparation_quiescent: true`. Environment, application, task, original manager,
and candidate must match the retained proof. Current environment generation and
active-task identity are not export fences.

New scheduling requires the enabled, online original manager. An exact token,
proof, task, and canonical-selection replay returns the same job even when that
manager is offline. There is one selection per task: another token or changed
selection conflicts, including after failure or expiry. Jobs are not reassigned
or automatically retried.

### Inspect and download

- `GET .../tasks/{task_id}/export/status` returns a small descriptor:
  `task_id`, `job_id`, `stop_job_id`, `state`, `selection_sha256`, and optional
  `output_manifest_sha256`.
- `GET .../tasks/{task_id}/export/content` returns committed output, or `409`
  while no successful output exists.

Content carries `task_id`, `stop_job_id`, both digests, `files` with `path` and
`content`, and `missing_paths`. Files and missing paths are disjoint, sorted,
and their union is exactly the authorized selection. An empty present file is
not a missing path. New files may be selected explicitly.

This output is not a complete project snapshot. Unselected paths are neither
read nor represented; omission outside the selection must not be interpreted
as deletion by a later importer.

## Original-manager upload

`POST /api/internal/runner-managers/{manager_id}/environment-jobs/{job_id}/task-export`

The manager bearer is authenticated before decoding the body. The path manager
must be that authenticated manager and the job must belong to it. Upload uses
strict `ExportNativeTaskUpload` with the exact polled `request` nested beside
`content`. The polled request carries the stored environment owner, task runtime
identity, stop job, canonical paths, and selection digest.

Only a live, unexpired claimed export can commit new output. Under fresh locks,
the control plane rechecks the original manager, task, header, proof, selection,
and stopped candidate. It validates and canonicalizes the selected output,
then stores files, missing paths, output metadata, and job success in one
transaction. Failure leaves no partial output or successful job.

An identical already-successful upload from the original manager is a read-only
`204`; changed terminal content is `409`. A normal job-result success without
matching committed output cannot succeed. Job-result JSON contains only the
small `ExportNativeTaskResult` identity, digests, counts, and total bytes, never
file contents. An ordinary failure can terminate the export without output;
expiry marks it `uncertain` without changing a successor's lifecycle.

## Bounds and status codes

| Limit | Value |
| --- | --- |
| Selected paths | 1–4096 |
| Path, UTF-8 bytes | 512 |
| One file's content, UTF-8 bytes | 1 MiB |
| Aggregate content, UTF-8 bytes | 8 MiB |
| Request body, before JSON decoding | 16 MiB |

Paths use the [seed path rules](execution-environment-tasks.md#path-rules):
normalized relative POSIX files, no `.git`, absolute or traversal components,
backslashes, control characters, duplicates, or ancestor collisions. Content
must be UTF-8 text without NUL. The transport limit is enforced against the
actual body as well as a declared length, including chunked bodies.

| Status | Meaning |
| --- | --- |
| 200 | Scheduled or replayed intent, descriptor, or committed content. |
| 204 | Manager output committed, or identical terminal upload/result replay. |
| 400 | Malformed or unknown fields, invalid path/content shape, or invalid token. |
| 401 | Missing or invalid bearer. |
| 404 | Foreign or absent application task/export, manager path identity, or job. |
| 409 | Proof, selection, digest, exact output set, claim, or immutable replay conflict; output not ready. |
| 413 | Request body, entry count, per-file content, or aggregate content exceeds a limit. |
| 503 | Original manager unavailable for a new intent. |

## Digest encoding

Selection SHA-256 uses `df-native-task-export-selection-v1` followed by NUL,
then each lexicographically sorted path's u64 big-endian byte length and UTF-8
bytes.

Output SHA-256 uses `df-native-task-export-v1` followed by NUL, then each
selected sorted path's u64 big-endian byte length and bytes. A present file adds
byte `1`, its u64 big-endian content length, and content bytes. A missing path
adds byte `0`. Both digests are lowercase hexadecimal and use the shared
producer/consumer validator.

The output digest is a selected-content hash, distinct from the original input
seed digest and from any full-workspace hash.
