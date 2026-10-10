# Execution environment runner primitives (issues 739/740/742/749 P1)

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

- `ExecutionEnvironmentProfile` (`runtime/execution_environment/profile`):
  `enabled=false`, `state_mount_target=/state`, `stop_timeout=20s`,
  `operation_deadline=60s`, `volume_uid/gid=None` (blocked until explicit),
  `command`/`state_workdir` unset (blocked until explicit), `state_dir=None`
  (blocked until explicit).
- Enable only via process env (`RUNNER_EXECUTION_ENVIRONMENTS_ENABLED`,
  `..._STATE_MOUNT_TARGET`, `..._STOP_TIMEOUT_SECS`,
  `..._OPERATION_DEADLINE_SECS`, `..._VOLUME_UID/GID`,
  `..._STATE_DIR`, `..._COMMAND`, `..._STATE_WORKDIR`). The operation
  deadline accepts 1–3600s and falls back to the 60s default outside that
  range; a non-positive configured value is rejected before any work.
  Image, `pids/memory/cpu`, `runtime_class`, and network come from the
  existing manager config; requests cannot override them.
- Configuration is optional and never fatal. A missing optional setting
  leaves the profile unset for ensure; an explicitly malformed setting is
  retained as a degraded profile instead of silently falling back to a
  default. Every refusal names the setting and the violated rule, never the
  configured value.
- `..._COMMAND` is a JSON argv array, parsed with `serde_json`, not a shell
  string: there is no eval, and each token keeps its exact bytes, including
  embedded spaces and shell metacharacters. It must be a non-empty array of
  at most 64 arguments totalling at most 16KiB, with no NUL byte and a
  non-empty executable as element 0. An ordinary empty argument after the
  executable stays valid.
- `..._STATE_WORKDIR` is required for ensure and is validated purely
  lexically against `..._STATE_MOUNT_TARGET`: absolute, no control
  characters, no `.`/`..` components, no repeated or trailing slash, and a
  strict descendant at a component boundary (so `/stateful` is not inside
  `/state`). No host filesystem canonicalization happens; the root
  describes the container mount, not a manager-local path.
- `state_dir` is read only from `RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR`.
  It is unset/disabled by default and must be a dedicated, persistent,
  manager-controlled root that is never mounted into the agent workspace.
  With no configured root, environment operations fail closed before any
  Docker call; there is no ephemeral fallback.
- Volume name is `df-env-<uuid-simple>` (UUID only, stable across
  generations). Creation requires explicit uid/gid with `uid > 0`; guessing
  blocks, and no agent or probe ever runs as root.

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
  `-v <volume>:/state`, `--workdir <state workdir>`,
- `--read-only`, `--tmpfs /tmp:rw,nosuid,size=256m`, `--cap-drop ALL`,
  `no-new-privileges`, `--pids-limit/--memory/--cpus`,
  `--runtime` when configured, `--network none` unless the trusted
  profile enables it, then `<image>` followed by the configured command
  argv, token for token.

The configured command is the container `CMD`, so the image's own
`ENTRYPOINT` still decides what actually runs: the configured image must
have an entrypoint that accepts a command of this shape. No entrypoint
override is invented, and no request argument controls the executable,
the image, or any host path.

No `-p`/`--publish`, no host workspace bind, no global root mount.
Task workspaces stay unmounted; `docker volume create` runs only through
runtime code/tests (fake harness), never as a live CLI here.

## Optional private service profile (issue 749 P2a)

A served environment additionally joins one trusted Docker network and reads
one per-generation credential from an environment variable. This sub-phase
only makes the launch prerequisite available: no service is dialed, no
socket is opened, and no relay exists yet.

- Four settings, all-or-none:
  `RUNNER_EXECUTION_ENVIRONMENT_SERVICE_NETWORK`,
  `..._SERVICE_PORT`, `..._SERVICE_AUTH_USERNAME`,
  `..._SERVICE_AUTH_ENV_NAME`. None set leaves the environment exactly as
  it was before service support. Some but not all is a degraded profile:
  manager startup is never fatal, but ensure refuses before any Docker work
  rather than silently launching something that cannot serve.
- The network must be a bounded ASCII Docker name, the port `1..=65535`,
  the basic-auth username bounded printable ASCII with no colon, and the
  variable name a POSIX identifier. Diagnostics name the setting, never the
  value.
- `docker run` gains exactly `--network <name>` and
  `--env-file <host path>`. No port is published for the agent, the
  credential file is never mounted, and the credential value never appears
  in an argv. The manager deployment must already join the network; nothing
  here creates or attaches to one.
- The credential is a UUIDv4 from the workspace `uuid` generator, stored per
  `(environment, generation)` at `<state_dir>/<environment>/service/<generation>.env`
  beside the durable barrier, mode `0600` in an owner-only directory, written
  through a `create_new` temporary file with fsync and atomic rename. It is
  stable for a generation, so a restart reuses it, and differs across
  generations and environments, so no two users ever share one.
- Ownership is judged by the process's **effective** uid, never the real uid
  and never a directory owner. If procfs cannot supply it the check fails
  closed — an unavailable owner check is never treated as a passing one.
- The directory chain is re-validated on every create and every read: each
  component must be a real directory, not a symlink, owned by the manager and
  not group or world accessible. A directory that is too open is refused and
  left as it is, not quietly tightened.
- The credential file is re-checked on every read from the **opened handle**,
  so a path swapped after the open cannot change what is accepted or read:
  regular file, exactly `0600`, owned by the effective uid, bounded size,
  read through a capped reader, and holding exactly the configured
  `NAME=token` line. A symlink is refused before it is opened. Anything that
  fails is refused and left untouched: never chmod-ed, rewritten, or deleted.
  Reuse and restart require that existing credential and fail closed without
  it, rather than minting a new value under a container that is already
  running. Only creation mints, under the existing admission lock after the
  barrier and before the container run.
- **Stated assumption:** the state root is a trusted, manager-owned directory
  tree that no other local account can write. Std offers no `O_NOFOLLOW` open
  and no resolved-handle comparison, so a fully race-free check-then-open
  would need a public `libc`/`rustix` dependency, which is not an available
  workspace dependency. Under the stated assumption the pre-open checks plus
  handle-based validation hold; this is not claimed to close the
  check-then-open window itself.
- The agent image must read the configured variable and enforce basic auth
  itself. The manager stores no agent credential in DF, in any DTO, label,
  volume, or diagnostic; this phase builds no Basic header, which arrives
  with the relay client in a later sub-phase.

## Same-container state probe

A `running` acknowledgment claims more than a container status: the trusted
image must really let the configured non-root user write inside the state
mount.

- `docker exec --user <uid:gid> --workdir <state workdir> <exact owner
  container> sh -c <fixed script>`. No helper container, no root
  privileges, no new mount, no new port, no second writer.
- The script is a fixed manager constant that creates a private `mktemp`
  file in the working directory, writes to it, removes it, and removes it
  again on any exit. That is a real create/write/unlink observation, not a
  recursive tree walk or an ownership mutation. It assumes the trusted
  image supplies a POSIX `sh` and `mktemp`; no host binary is injected.
- The container name, user, and working directory travel as separate argv
  entries and are never interpolated into the script, so no configured
  string can reach the shell. Probe output never enters a manager
  diagnostic.
- The probe is an ordinary bounded CLI call, is not claimed to be
  cancellation-safe if the CLI times out, and is not a stop proof. The
  existing stop barrier remains the only owner of termination.
- A successful probe is a writability observation, not a security boundary
  and not a guarantee that image copy-up preserves ownership.

## Ensure

- Validates owner, then the trusted process inputs — enabled profile,
  explicit non-root ownership, a valid command, and a valid state working
  directory — before any disk or Docker work. An absent or malformed input
  refuses every ensure branch, including reuse, while stop keeps working.
  It then derives the volume, locks the environment in-process, takes the
  cross-process reservation, and refuses a closed or lower generation from
  the durable barrier before any Docker call. It persists this attempt as
  the new high-water, creates the volume, and scans
  `docker ps --filter label=desk-foreman.execution-environment=<id>`
  — all inside the operation deadline.
- A durably stopped generation (`stopped_generation >= request`) or a
  generation below an already attempted high-water refuses before Docker,
  so a late ensure cannot restart a generation that stop closed.
- Same generation `running` → the labels must match the requested owner,
  then the state probe must confirm writability, then idempotent success
  (no `run`). Same generation stopped → `start`, an owner recheck, the
  probe, and only then the acknowledgment.
- Any other generation `running`/`restarting`/`paused`/`unknown`
  (anything except `exited`/`dead`/`removed`) refuses creation.
  Never deletes a container to force through.
- Creates with the hardened argv, then inspects and requires an owner
  match plus a `running` state (and after any corrective `start`), then
  runs the probe before the acknowledgment.
- A probe failure is uncertainty, not a stop proof: nothing is removed,
  stopped, or retried, the durable high-water stays as written, and no
  higher generation is admitted while that candidate may still be running.
  The controller keeps the uncertain candidate until an explicit
  owner-verified stop.

## Stop

- Validates owner, requires a configured `state_dir`, and locks on
  `environment_id`; unlike ensure, stop does not require the enabled
  switch, the command, the working directory, or the ownership, so an
  existing environment stays stoppable even when creation is disabled or
  misconfigured. Stop never runs the state probe.
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
- Docker daemon access is denied in development. A lifecycle
  acknowledgment is not runtime readiness: the probe is a fake-CLI and
  filesystem observation, so real volume permissions, copy-up ownership,
  the configured image's `sh`/`mktemp`/entrypoint assumptions, and
  cross-process runtime tests gate later enablement. No live routing or
  native writes are enabled here.
