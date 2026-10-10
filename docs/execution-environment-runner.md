# Execution environment runner primitives (issues 739/740/742 P1)

Tested Docker primitives to create an isolated persistent-state environment
and confirm a full container stop, plus a durable generation barrier and
cross-process reservation in a dedicated state root. Additive only: no
OpenCode integration, public proxy, routing, migration, or live enablement.

## Owner and DTOs

- `RunnerOwner::ExecutionEnvironment { environment_id, generation }`
  (`crates/runner-protocol`). `environment_id` is a UUID string;
  `generation` is `>= 0`.
- `stable_key()` is `execution_environment:<uuid>:<generation>`;
  `container_name()` derives from it, so generations have distinct
  containers. Locking uses `lock_key()`, which is the environment id
  alone — generation locks cannot serialize replacement.
- `EnsureExecutionEnvironmentRequest { owner }` and
  `StopExecutionEnvironmentRequest { owner }` carry only identity
  (`deny_unknown_fields`). Image, host paths, volumes, env, secrets,
  and endpoints are never accepted.
- `EnsureExecutionEnvironmentAck { owner, container_name, state }`
  echoes the request owner with observed `running`. `StopExecutionEnvironmentAck
  { owner, container_name, confirmed, observed_state }` echoes the owner
  with `absent`/`exited`/`dead`/`removed` or the observed status. No
  secrets, host paths, or volume details.

## Trusted profile (disabled by default)

- `ExecutionEnvironmentProfile` (`runtime/execution_environment`):
  `enabled=false`, `state_mount_target=/state`, `stop_timeout=20s`,
  `operation_deadline=60s`, `volume_uid/gid=None` (blocked until explicit),
  `state_dir=None` (blocked until explicit).
- Enable only via process env (`RUNNER_EXECUTION_ENVIRONMENTS_ENABLED`,
  `..._STATE_MOUNT_TARGET`, `..._STOP_TIMEOUT_SECS`,
  `..._OPERATION_DEADLINE_SECS`, `..._VOLUME_UID/GID`,
  `..._STATE_DIR`). The operation
  deadline accepts 1–3600s and falls back to the 60s default outside that
  range; a non-positive configured value is rejected before any work.
  Image, `pids/memory/cpu`, `runtime_class`, and network come from the
  existing manager config; requests cannot override them.
- `state_dir` is read only from `RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR`.
  It is unset/disabled by default and must be a dedicated, persistent,
  manager-controlled root that is never mounted into the agent workspace.
  With no configured root, environment operations fail closed before any
  Docker call; there is no ephemeral fallback.
- Volume name is `df-env-<uuid-simple>` (UUID only, stable across
  generations). Creation requires explicit uid/gid; guessing blocks.

## Deadline and ownership

- One monotonic operation deadline covers the environment lock and every
  Docker command of a single ensure/stop; each command receives only the
  remaining budget. An expired budget errors instead of becoming an
  unbounded wait, and a timeout fails closed rather than reporting
  absence. Bounded commands reuse the shared command runner, so a
  deadline kills and reaps the spawned Docker CLI.
- Every success acknowledgment and every destructive command requires the
  container's owner, environment-id, and generation labels to match the
  request. Missing, malformed, or foreign labels refuse without any
  stop/rm. `stop_timeout` still bounds the graceful `docker stop` step.

## Durable state root and generation barrier

- One dedicated root (`state_dir`) holds, per environment and derived only
  from the environment UUID, a `barrier.json` generation barrier and a
  `lock` reservation. No request can select or escape a path.
- The barrier persists a monotonic nonnegative generation high-water plus
  an optional stopped generation. Writes are atomic: a same-directory temp
  file, file sync, rename, then directory sync. Missing state is an empty
  barrier; a present but corrupt, invalid, unsupported-version, or oversize
  file fails closed and is never reset.
- The reservation is one `create_new` file with an explicit holder token.
  An existing or crash-held reservation errors instead of being stolen:
  there is no stale-PID cleanup and no `Drop`-based removal, and release
  removes the file only while it still carries the holder's own token.
  Ordinary error exits release their own reservation; a crash or
  cancellation leaves it for explicit operator recovery. Two cooperating
  managers sharing one root therefore cannot ensure/stop the same
  environment at once, and a crash-held file blocks both paths.
- The reservation is held across the barrier read/modify and the whole
  Docker operation. The existing in-process environment mutex stays as the
  first, cheaper serialization.

## Hardened containers

`build_environment_run_argv` emits only:

- `--name <generation-container>`, `--user <uid:gid>`,
  `--label desk-foreman.owner/managed/manager/execution-environment/generation`,
  `-e DF_EXECUTION_ENVIRONMENT/DF_GENERATION`,
  `-v <volume>:/state`, `-w /state`,
- `--read-only`, `--tmpfs /tmp:rw,nosuid,size=256m`, `--cap-drop ALL`,
  `no-new-privileges`, `--pids-limit/--memory/--cpus`,
  `--runtime` when configured, `--network none` unless the trusted
  profile enables it, then `<image> sleep infinity`.

No `-p`/`--publish`, no host workspace bind, no global root mount.
Task workspaces stay unmounted; `docker volume create` runs only through
runtime code/tests (fake harness), never as a live CLI here.

## Ensure

- Validates owner, requires enabled profile, explicit ownership, and a
  configured `state_dir`; derives the volume, locks the environment
  in-process, takes the cross-process reservation, and refuses a closed or
  lower generation from the durable barrier before any Docker call. It then
  persists this attempt as the new high-water, creates the volume, and
  scans `docker ps --filter label=desk-foreman.execution-environment=<id>`
  — all inside the operation deadline.
- A durably stopped generation (`stopped_generation >= request`) or a
  generation below an already attempted high-water refuses before Docker,
  so a late ensure cannot restart a generation that stop closed.
- Same generation `running` → idempotent success (no `run`) only when the
  container labels match the requested owner.
  Same generation stopped → `start` then confirm, again after an owner
  match (only when no durable barrier closed it).
- Any other generation `running`/`restarting`/`paused`/`unknown`
  (anything except `exited`/`dead`/`removed`) refuses creation.
  Never deletes a container to force through.
- Creates with the hardened argv, then inspects and requires an owner
  match before confirming `running` (and after any corrective `start`).

## Stop

- Validates owner, requires a configured `state_dir`, and locks on
  `environment_id`; unlike ensure, stop does not require the enabled
  switch, so an existing environment stays stoppable even when creation is
  disabled.
- Inspects first. A positively absent container confirms with no
  destructive command; a present container is only touched after its
  owner/environment/generation labels match. Foreign or malformed labels
  fail before any `stop`/`rm` and record no barrier.
- Once labels match (or the container is positively absent), the stopped
  generation is persisted to the durable barrier before any destructive
  command or confirmation, so a failed or timed-out stop still closes the
  old generation to a late ensure while never returning a confirmed
  acknowledgment.
- A matching, already-stopped container confirms without destruction. A
  running matching container is terminated with a bounded `stop` (bounded
  by `min(stop_timeout, remaining)`) plus `rm`/`rm -f` escalation, then
  inspected again. No `active_ops` refusal — writers are terminated, not
  skipped.
- Confirms only after a final inspect proves `exited`/`dead`/`removed` or
  positively identified absence via `No such container/object`.
  Transport, parse, timeout, or permission failures never become
  absence; still-running returns `confirmed=false`. Never `volume rm`.
- Ack owner/generation matches the request. Persisted stop intent stays
  distinct from the actual stop acknowledgment: the barrier is not a claim
  that a container was killed.

## Compatibility and limits

- Legacy `workspace_runners` and admin sessions reject/skip environment
  owners explicitly; nothing is persisted or reported there.
  `execution_environments_ready` stays `false`; `runtime_kind` stays
  opaque (DF never branches on OpenCode/ACP).
- Docker label scans are a guard, not an atomic distributed lock.
  Multiple managers or engines still need durable affinity/CAS in a
  later lifecycle phase; until then restrict environment primitives to
  a single controller and keep the capability disabled.
- The durable reservation covers cooperating processes on one shared
  state root only, never different Docker hosts. A crash-held reservation
  is fail-closed and requires explicit operator recovery; it is never
  auto-deleted from PID heuristics. The state root must be dedicated,
  persistent, manager-controlled, and outside the agent workspace mount.
- Docker daemon access is denied in development; real volume
  permissions, child termination, and cross-process runtime tests gate
  later enablement. No live routing or native writes are enabled here.
