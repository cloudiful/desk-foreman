# Native task workspace directories

A native task checkout is mounted inside its container at one directory derived
from the task's workspace binding:

```text
/workspace/bindings/<workspace_binding_id>
```

That path is the single definition shared by the manager launch, the container
reuse and stop mount guards, the same-container writability probe and the
control-plane task descriptor, so all four cannot drift apart. The binding id
renders as bounded decimal digits, so the derived directory can never carry a
path separator, a parent segment or a host path.

## Why one directory per binding

The checkout location is a stable identity per workspace binding. Two bindings
must therefore never share one location, and one binding must keep one location
across its tasks:

- Two bindings admitted with the same workspace key stay distinct, because the
  binding id differs even when the presented workspace does not.
- Every task of one binding resolves to the same directory, so a workspace
  identifies one location inside its container instead of a per-task one.
- The task UUID is deliberately *not* part of the path. It changes per task,
  which would make every task look like a different location.

**A stable directory is a stable location, not stable contents.** What is
mounted there is the manager-owned per-task checkout
`<host-root>/<environment_uuid>/<task_uuid>/tree`, which is materialized from
that task's own sealed seed. A later task of the same binding therefore mounts a
different host source at the same container path and starts from its own seed;
nothing carries over inside the checkout. Nothing in this change alters the
checkout lifecycle, and no shared or per-workspace tool cache is implemented.

This is a provider-owned contract. It does not reimplement any agent's own
project identity, and it creates no Git metadata: `.git` paths are rejected by
the shared seed protocol, so a native checkout carries no Git identity.

## What is not included

- `/workspace` itself is only a root guard. A checkout is never mounted at the
  root, and the persistent state mount must stay disjoint from the whole root —
  a state mount under `/workspace` is refused exactly as before.
- The directory is a *normative* path. It says where a manager mounts a
  checkout; it is not a certificate that any container is running, and it
  carries no manager address, image or credential.
- Nothing is caller-provided. Requests, descriptors and mounts never accept a
  host path or a task directory; the server always derives this value from the
  stored binding id.

## Persistence boundaries

| What | Where it lives |
| --- | --- |
| Task checkout (seeded files, task edits) | The manager-owned per-task host tree `<host-root>/<environment_uuid>/<task_uuid>/tree`, bind-mounted at the binding directory for the lifetime of that one task. Not carried into the next task. |
| Durable agent state that must outlive a task | The separate durable state volume, mounted at the configured state mount target and scoped to the environment. This is where durable state belongs. |
| Task identity, labels, generation | Environment labels and the durable task row; never encoded in the path. |

A native task stop keeps that task's checkout and the state volume; releasing a
task is a separate explicit operation. A kept checkout is the per-task host
tree, so a later task of the same binding still starts from its own seed rather
than from it. Existing checkouts from the earlier shared-root layout are not
migrated, aliased or purged: a container created against the old `/workspace`
target simply fails the current profile matcher, and its start or stop is refused
rather than adopted.

## Related

- [Durable native task intents](execution-environment-tasks.md)
- [Resource workspace bindings](resource-workspace-binding.md)
