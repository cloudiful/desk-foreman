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

Direct file reads stay available without a write lease. A read that runs inside a runner container (`glob`, `grep`) locks the binding row before dispatch: the locked row must still match the binding and application, keep the same workspace root, and remain active and unarchived. A missing, archived, resetting, or otherwise guarded binding — including an active native task, scoped session — refuses the search with `workspace is not available for reads`. Read-only search never acquires a write lease, and the lock is held through the runner dispatch, including the first Running acknowledgement.

## Runner container workspace mount

A runner container mounts exactly one workspace directory at `config.workdir`: a strict descendant of the manager's trusted `WORKSPACE_ROOT`. The whole base itself, a parent or sibling, a path outside the root, a non-directory, a path with a symlink component, and a value containing a comma, NUL or control character are refused before any Docker command runs. The container working directory must be a normalized absolute non-root path.

Reuse and start of an existing ordinary-owner container require its inspected mount source to equal the computed child exactly. A container with a missing, whole-base, outside, or otherwise different mount is left running and refused with an incompatible-workspace-mount error; startup reconciliation also skips it. Such a container requires operator-reviewed cleanup and manual removal, with no active Shell sessions. Private execution-environment containers keep their existing reconciliation behavior.

## Principal stop and protocol discovery

`GET /api/internal/principal-workspace-capabilities` requires an application bearer and returns `protocol_version: 1`; it describes the server protocol, not runner availability.

`POST /api/internal/resource-workspaces/{resource_kind}/{resource_id}/workspace-sessions/stop` requires the application bearer, `workspace.shell` scope, `X-DF-Runner-Isolation: principal`, and matching external-user, workspace-key and lease-owner headers. Its JSON body is `{ "expected_lease_owner": "..." }`, matching the external-user and lease-owner headers. A successful response confirms that principal stopped and reports whether its lease was released. A different or absent current lease is not changed. An unconfirmed stop returns a retryable error; the permanent principal fence may persist even when the lease is retained. Start a new lifecycle with a fresh identity.

## Workspace Git upstreams

Admins manage authoritative upstream metadata per workspace binding through
`GET|POST /api/admin/workspace-bindings/{binding_id}/git-upstreams`,
`PATCH|DELETE …/{upstream_id}`, and `POST …/{upstream_id}/enable|disable`.
`remote_alias` is a strict ASCII identifier (up to 64 characters, starting with
a letter or digit). `target_url` is an absolute `http` or `https` Smart HTTP URL
with a host and no user information, query string, fragment or control
characters. Listing works for archived bindings; every mutation requires an
active binding and returns `409` otherwise. Unknown or cross-binding upstream
ids return `404`, and a duplicate alias for the same binding returns `409`.

Credentials are write-only: responses expose only `has_credential`, never the
username or secret. Upstream credentials are stored as plaintext in the
database by explicit product decision; no operator key or extra configuration is
required to create or update them. Rows written by the previous encrypted
format remain detectable but are never decrypted: their upstream requests fail
closed with `credential_resupply_required` until an administrator supplies a new
credential, which replaces the legacy row. `enabled` is metadata only: it does
not assert network reachability and does not make a Git push succeed by itself.

## Native Git gateway

When `DF_GIT_PUBLIC_BASE_URL` is set, the stored upstream aliases become normal
native Git remotes inside the ordinary workspace container. `GET /git/{binding_id}/{remote_alias}/info/refs` accepts
exactly one `service=git-upload-pack|git-receive-pack` query, and
`POST /git/{binding_id}/{remote_alias}/git-upload-pack|git-receive-pack` proxies
the pack stream. The target comes only from the stored admin URL; callers cannot
choose a host, path prefix, query beyond the service discriminator, or upstream
URL.

The capability signing key is generated once and persisted in the database, so
concurrent instances share one key and a restart preserves it without operator
configuration. The runner authenticates with Basic username `df-git` and a
short-lived signed capability scoped to the workspace binding and alias
generation. Upstream
credentials are read only inside the gateway request and are never placed
in the runner, its argv, `.git/config`, or the workspace. Client cookies,
authorization, proxy and forwarding headers are stripped; redirects, upstream
error bodies, `Location`, `Set-Cookie` and `WWW-Authenticate` are never
forwarded. Request and response packs are streamed with a configurable 256 MiB
default cap (`DF_GIT_GATEWAY_MAX_BODY_BYTES`, maximum 1 GiB) and connect,
response-header and total-stream deadlines.

Each shell or `write_stdin` dispatch mints a fresh capability and writes it to a
small `noexec` tmpfs credential file. The manager configures only the enabled
DF aliases as remotes and never removes unrelated user remotes. Disable, delete,
alias/target change, credential replacement or a generation change invalidates
older capabilities on their next request. When the public base URL is absent
the gateway returns a fixed `503`, native Git stays unavailable, and normal
Shell tools continue to work. A signing-key initialization failure stops
startup instead of falling back to an unsigned gateway. `enabled` remains metadata; a successful request still
depends on the stored target being reachable by the runner.

## Internal application upstream management

An application bearer can manage its own binding's upstream metadata without an
admin browser session:

- `GET /api/internal/workspace-bindings/{binding_id}/git-upstreams`
- `POST /api/internal/workspace-bindings/{binding_id}/git-upstreams`
- `PATCH /api/internal/workspace-bindings/{binding_id}/git-upstreams/{upstream_id}`
- `DELETE /api/internal/workspace-bindings/{binding_id}/git-upstreams/{upstream_id}?lease_owner=...`

Reads use the application actor and resolve the binding for the exact
application and external-user subject; a missing or foreign binding is `404`.
Resource-owned bindings authenticate with the isolated principal headers
(`X-DF-Runner-Isolation: principal`, matching `X-DF-External-User-Id` and
`X-DF-Lease-Owner`, and the `X-DF-Workspace-Key` naming the resource). The
caller identity is the real external user; the stored `__resource__` marker is
internal and is never required from or returned to the client. The resolved
scope must name this exact binding and the caller's lease must be the current
live one; a different application, binding or principal and a stale, expired or
other-holder lease are refused on reads as well as writes. The capability
endpoint applies the same headers and authority. The external-user header is
required for every caller; the isolation, workspace-key and lease-owner headers
are optional in the generated contract because personal callers omit them.
Personal bindings keep the stored-subject check and need no write lease.
Writes require the same ownership, an existing workspace write scope and the
current write lease named by `lease_owner` in the JSON body (or the delete query);
for a resource binding it must also equal the authenticated lease. The binding
row is locked before that authority is validated and the row is changed in the
same transaction, so a lease handover racing the request cannot pass a snapshot
check. A personal binding follows the existing personal write policy; a resource
binding requires the live unexpired lease.

Create and update use the same validated alias, URL and write-only credential
DTOs as the admin API. Credentials are stored as plaintext and are never
returned; responses expose `has_credential`, the alias, target URL, `enabled`
and `capability_generation`. Every successful mutation bumps the generation, so
older capabilities for that alias stop working. Duplicate alias, inactive
binding and stale lease return `409`; a foreign application or subject returns
`403`; a cross-binding upstream id returns `404`.

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