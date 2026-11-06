# Resource workspace binding resolution

[简体中文](resource-workspace-binding.zh-CN.md)

An application resolves the workspace binding of one resource workspace without
taking a legacy write lease.

```
application ──HTTPS──▶ control plane ──▶ workspace_bindings row
                         │
                         └─ binding identity only; no lease, no native task, no execution
```

The endpoint is application-authenticated and bodyless. It reuses the same
find-or-create resolver as the resource-lease endpoints, so the binding identity
is stable: the same application, resource kind and resource id always resolve
to the same binding, and the first resolution materializes the resource
workspace directory through that resolver.

## `POST /api/internal/resource-workspaces/{resource_kind}/{resource_id}/binding`

Resolves, and creates on first access, the active binding of one resource
workspace for the calling application.

| Path part | Meaning |
| --- | --- |
| `resource_kind` | Resource workspace kind, for example `code_project`. Any currently valid kind is accepted. |
| `resource_id` | Resource workspace identifier. |

The response is the binding row: its `workspace_binding_id`, application id,
`workspace_key`, resource identity, workspace paths and the unchanged
`write_lease_owner` / `write_lease_acquired_at` / `write_lease_expires_at`
fields.

| Status | Meaning |
| --- | --- |
| `200` | The active binding was resolved. |
| `400` | A path component is not a valid resource key. |
| `401` | Missing or wrong bearer token, or the binding is not active. |
| `404` | The calling application does not exist. |
| `500` | The workspace could not be materialized. |

## Principal-isolated execution

Authenticated application resource requests can select isolated execution with `X-DF-Runner-Isolation: principal`. Supply one `X-DF-External-User-Id`, `X-DF-Workspace-Key` and `X-DF-Lease-Owner`; the external user and lease owner must match and the workspace key must identify a resource. Existing write-lease and scope checks still apply. Execution requires a Docker-backed runner manager. A principal stop fence is permanent for that identity; a new lifecycle needs a fresh identity. A resource workspace request without the isolation header is rejected with `403` (`resource workspace requires an isolated principal`). Personal, non-resource application workspaces retain their ownership behavior. Workspace reconstruction is explicit; authentication never resets or rewrites workspace contents.

## Principal stop and protocol discovery

`GET /api/internal/principal-workspace-capabilities` requires an application bearer and returns `protocol_version: 1`; it describes the server protocol, not runner availability.

`POST /api/internal/resource-workspaces/{resource_kind}/{resource_id}/workspace-sessions/stop` requires the application bearer, `workspace.shell` scope, `X-DF-Runner-Isolation: principal`, and matching external-user, workspace-key and lease-owner headers. Its JSON body is `{ "expected_lease_owner": "..." }`, matching the external-user and lease-owner headers. A successful response confirms that principal stopped and reports whether its lease was released. A different or absent current lease is not changed. An unconfirmed stop returns a retryable error; the permanent principal fence may persist even when the lease is retained. Start a new lifecycle with a fresh identity.

## Git sync

`POST /api/internal/resource-workspaces/{resource_kind}/{resource_id}/git/sync` requires the resource write lease, shell scope, and network access. Supply the legacy scoped broker `remote_url`, the matching `code_project_id`, a non-nil `authorization_generation`, and optionally `branch`. The server splits the scoped URL privately and never sends the scoped password through model commands, job payloads, database rows, results, errors, or repository configuration.

An existing checkout with `preserve_existing: true` is inspected with a fixed credential-free sandbox probe: no fetch, pull, network, configuration write, or new job occurs. Repositories with includes, unsafe config shape, external object directories, or credential-bearing legacy origin URLs return `409` with `legacy_git_cleanup_required` and are left unchanged; that response preserves work but is not a legacy migration.

The default `preserve_existing: false` path performs a clean fast-forward on an existing checkout. The request must name the canonical branch explicitly, the probe must report it clean and attached, and the actual HEAD becomes the job's expected base. A dirty, detached, branch-mismatched, or otherwise unsafe checkout returns `409` unchanged. The trusted manager holds the binding write fence and refuses live bound Shell sessions; it imports only the already-verified bundle, publishes the exact descendant with a compare-and-set ref update and `read-tree`, and preserves unrelated refs and history. A non-fast-forward or mismatched-base change is refused with the refs and worktree unchanged. A non-empty non-repository directory still returns `409`.

A durable first clone passes no expected base and keeps its existing behavior. If a job is `pending` or `claimed`, the route polls that same job within the original wait budget and never admits a fresh job, resets the credential deadline, or replays work. If the job is `uncertain`, reconciliation is proof-only and read-only: the manager reads its completed, published, cleanup-confirmed receipt and independently inspects the clean actual HEAD. Only a matching proof lets the server compare-and-set that uncertain row to succeeded using the strict stored result; a missing, malformed, mismatched, expired, busy, or dirty proof is an explicit `503` and never triggers an external Git write, fetch, pull, push, replay, credential registration, or new job.

An absent `.git` in an empty target admits one durable first-clone job for the manager already observed serving that binding; `branch` is required and must be canonical. With no live observation the route performs one credential-free bootstrap probe and re-reads the observation once. Zero or ambiguous live managers return `503`. Successful first clone returns `cloned: true`, `dirty: false`, the canonical branch, and the verified head. Pending, uncertain, or timed-out work is never replayed.

## Ownership boundary

Resolving a binding grants no permission at all:

- it does not acquire, renew, clear or take over a write lease — the resource
  lease endpoints remain the only legacy lease authority, and an existing owner
  is returned untouched;
- it does not admit, dispatch or release a native task, and an active native
  task keeps owning its workspace afterwards;
- it infers no container, manager or generation state.

Native admission stays the authority that refuses a binding carrying any legacy
write owner, including an expired one, and that keeps one active task per
workspace. A resolved binding may still be refused by admission.

The response carries internal workspace paths and is an
application-to-application API, not user-facing data.