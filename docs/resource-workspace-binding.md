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
| `resource_kind` | Resource workspace kind, for example `artifact`. Any currently valid kind is accepted. |
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

## Workspace reads

Direct file reads stay available without a write lease. A read that runs inside a runner container (`glob`, `grep`) locks the binding row before dispatch: the locked row must still match the binding and application, keep the same workspace root, and remain active and unarchived. A missing, archived, resetting, or otherwise guarded binding — including an active native task or scoped session — refuses the search with `workspace is not available for reads`. Read-only search never acquires a write lease, and the lock is held through the runner dispatch, including the first Running acknowledgement.

## Runner container workspace mount

A runner container mounts exactly one workspace directory at `config.workdir`: a strict descendant of the manager's trusted `WORKSPACE_ROOT`. The whole base itself, a parent or sibling, a path outside the root, a non-directory, a path with a symlink component, and a value containing a comma, NUL or control character are refused before any Docker command runs. The container working directory must be a normalized absolute non-root path.

Reuse and start of an existing ordinary-owner container require its inspected mount source to equal the computed child exactly. A container with a missing, whole-base, outside, or otherwise different mount is left running and refused with an incompatible-workspace-mount error; startup reconciliation also skips it. Such a container requires operator-reviewed cleanup and manual removal, with no active Shell sessions. Private execution-environment containers keep their existing reconciliation behavior.

## Principal stop and protocol discovery

`GET /api/internal/principal-workspace-capabilities` requires an application bearer and returns `protocol_version: 1`; it describes the server protocol, not runner availability.

`POST /api/internal/resource-workspaces/{resource_kind}/{resource_id}/workspace-sessions/stop` requires the application bearer, `workspace.shell` scope, `X-DF-Runner-Isolation: principal`, and matching external-user, workspace-key and lease-owner headers. Its JSON body is `{ "expected_lease_owner": "..." }`, matching the external-user and lease-owner headers. A successful response confirms that principal stopped and reports whether its lease was released. A different or absent current lease is not changed. An unconfirmed stop returns a retryable error; the permanent principal fence may persist even when the lease is retained. Start a new lifecycle with a fresh identity.

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