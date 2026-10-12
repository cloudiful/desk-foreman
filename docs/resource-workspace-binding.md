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