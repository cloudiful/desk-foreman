# Desk Foreman

Workspace-scoped MCP service for managed Desk Foreman workspaces. It exposes a shell-first coding surface with standard `shell`, `write_stdin`, `cancel_session`, `read`, `glob`, `grep`, and `apply_patch` tools, plus a PostgreSQL-backed multi-user admin console. Each MCP token is bound to one user, and every user runs inside an isolated workspace rooted under `WORKSPACE_ROOT`.

The repository is a Cargo workspace. The root crate is `desk-foreman`, and `crates/runner-manager` is the local pull-based execution agent.

The main service is the runner control plane. Runner managers connect to it with a pull-based protocol, fetch jobs, and execute them in local Docker workspace runners.

## Workspace SDK

This workspace also includes `desk-foreman-workspace-sdk`, a reusable Rust crate for workspace-scoped file tooling. It exposes the shared path-safety, Codex patch DSL, bounded file/directory reads, recursive traversal, fingerprints, and path stat capabilities that power Desk Foreman's file-oriented tools.

`desk-foreman` itself remains the full deployable product with HTTP, MCP, auth, database, admin, and runner integration layers on top of that SDK.

## Tools

- `shell({ command, workdir?, timeout?, max_output_tokens? })`
- `write_stdin({ session_id, chars="", yield_time_ms?, max_output_tokens? })`
- `cancel_session({ session_id })`
- `read({ filePath, offset?, limit? })`
- `glob({ pattern, path="." })`
- `grep({ pattern, path=".", include? })`
- `apply_patch({ patchText })`

Use `shell` for Git commands such as `git status --short`, `git diff`, and `git show <revision>`.

Application tokens select a workspace with `X-DF-Workspace-Key` (default `default`). Workspace keys of the form `kind:id` (for example `code_project:<uuid>`) resolve to a **resource-owned shared workspace**: one directory per `(application, kind, id)` shared across all external users of the application. Resource workspaces enforce a **write lease**: mutating tools (`shell`, `apply_patch`, and `write_stdin` with input) only run when the request carries an active lease for that binding. Pass the lease owner with `X-DF-Lease-Owner` (for example an AI conversation id); leases are acquired and released by the host application through the lease endpoints. Read-only tools (`read`, `glob`, `grep`, `stat`) work without a lease.

`read` accepts a file or directory. File text output includes line numbers; directory output contains child entries with directory names ending in `/`. Both are bounded and paginated by `offset` and `limit`. All returned paths are workspace-relative.

`apply_patch` accepts one JSON field named `patchText`. Its value must contain Codex patch DSL text.

Patch application accepts only the Codex DSL (`*** Begin Patch` / `*** End Patch`), not unified diff or Git patch input. Updates support context hunks, move operations, EOF matching, whitespace-tolerant fallback matching, and Unicode punctuation normalization. All files are parsed and preflighted before writes begin; operations then commit in input order. A later write failure preserves earlier successful files and returns `partial: true` with per-file statuses. Each file is rechecked before commit so an external edit is not silently overwritten, and individual file replacements use an atomic temporary-file rename. The workspace does not need to be a Git repository.

Shell execution always runs through `runner-manager`. The standard request does not expose a shell binary, login shell, or TTY switch: login is disabled and TTY is disabled. Commands have a default timeout, bounded output, a finite session limit, and UTF-8-safe continuation metadata. Non-TTY responses expose separated `stdout` and `stderr`; TTY responses would mark legacy `output` as combined. Docker runners default to no network, read-only root filesystems, a workspace-only writable mount, a limited temporary filesystem, dropped capabilities, `no-new-privileges`, PID limits, memory limits, and CPU limits. The `direct` backend requires explicit development-only opt-in.

Workspace paths are checked for traversal, symlink escapes, protected credentials, and workspace boundaries. Dangerous host-oriented commands and Docker socket access are denied before execution. These checks are policy guards, not the isolation boundary: Docker and workspace mounts provide the runtime boundary. Tool audit records store actor/workspace/tool metadata, hashes, status, duration/size fields where available, and bounded previews; full commands, patches, stdin, and shell output are not persisted by default.

Desk Foreman can optionally review side-effecting operations with an OpenAI Responses-compatible reviewer before execution. Configure the global reviewer endpoint and model from the admin Approval page; application bindings can inherit, disable, or override that reviewer. Set `APPROVAL_API_KEY` (or `OPENAI_API_KEY`) only in the gateway environment. Reviewer failures deny execution, and reviewer requests do not persist command, stdin, patch, prompt, or model response contents.

## Endpoints

- `POST /mcp`
- `POST /api/auth/login`
- `POST /api/auth/change-password`
- `POST /api/auth/logout`
- `GET /api/auth/me`
- `GET /api/admin/users`
- `POST /api/admin/users`
- `PATCH /api/admin/users/{user_id}`
- `POST /api/admin/users/{user_id}/reset-password`
- `DELETE /api/admin/users/{user_id}`
- `GET /api/admin/mcp-tokens`
- `POST /api/admin/mcp-tokens`
- `DELETE /api/admin/mcp-tokens/{token_id}`
- `PATCH /api/admin/mcp-tokens/{token_id}`
- `GET|POST|PATCH /api/admin/applications` and `/api/admin/applications/{application_id}`
- `GET|POST|DELETE|PATCH /api/admin/application-tokens`
- `GET /api/admin/audit-logs`
- `GET /api/admin/workspace-runners`
- `GET /api/admin/runner-sessions`
- `GET /api/admin/operations/summary`
- `GET|PATCH /api/admin/approval-settings`
- `GET|POST /api/admin/runner-managers`
- `PATCH /api/admin/runner-managers/{runner_manager_id}`
- `POST /api/admin/workspace-bindings/{binding_id}/archive|restore|reset`
- `POST|DELETE /api/admin/workspace-bindings/{binding_id}/lease` (admin, web session)
- `POST|DELETE /api/internal/workspace-bindings/{binding_id}/lease` (application bearer token)
- `POST|DELETE /api/internal/resource-workspaces/{resource_kind}/{resource_id}/lease` (application bearer token; resolves or creates the shared resource workspace binding)
- `GET /healthz`
- `GET /readyz`
- `/` serves the frontend SPA when `frontend/dist` is available

## Environment

- `MCP_BIND_ADDR` default `0.0.0.0:3000`
- `MCP_ALLOWED_HOSTS` optional comma-separated Host allowlist for MCP HTTP requests; empty preserves unrestricted legacy behavior. Entries are matched against the forwarded `Host` header, and entries with a port require an exact port match.
- `DATABASE_URL` required
- `WORKSPACE_ROOT` default `/workspace`, used as the base directory for per-user workspaces such as `/workspace/users/<user_id>`
- `DEFAULT_SHELL` default `bash`
- `DESK_FOREMAN_URL` required by `runner-manager`, for example `http://desk-foreman:3000`
- `RUNNER_MANAGER_TOKEN` required by `runner-manager`; create or register it from the admin Runner managers page
- `RUNNER_HOST_WORKSPACE_ROOT` required by a containerized Docker runner-manager, and must be the host path corresponding to its `/workspace` mount
- Runner image, network access, resource limits, and enabled state are managed from the admin Runner managers page
- `SESSION_IDLE_TTL_SEC` default `1800`
- `WEB_SESSION_TTL_SEC` default `604800`
- `WEB_COOKIE_NAME` default `desk_foreman_session`
- `WEB_COOKIE_SECURE` default `false`
- `BOOTSTRAP_ADMIN_LOGIN` default `admin`
- `BOOTSTRAP_ADMIN_PASSWORD` default `admin`; the first login must change it
- `BOOTSTRAP_ADMIN_DISPLAY_NAME` optional
- `BOOTSTRAP_ADMIN_EMAIL` optional
- `BOOTSTRAP_ADMIN_TIMEZONE` default `UTC`
- `MAX_OUTPUT_BYTES` default `262144`
- `MAX_TIMEOUT_MS` default `600000`
- `MAX_FILE_BYTES` default `52428800`
- `MAX_SESSIONS` optional server-wide policy cap
- `SERVER_SCOPES` optional comma-separated server scope allowlist
- `NETWORK_ENABLED` default `true` for policy calculation; Docker runner network remains separately controlled
- `APPROVAL_API_KEY` optional gateway secret for the configured approval reviewer; `OPENAI_API_KEY` is accepted as a fallback
- `DESK_FOREMAN_SECRET_MASTER_KEY` optional base64-encoded 32-byte key used to encrypt reviewer API keys entered in the admin UI; generate one with `openssl rand -base64 32` and keep it stable across deployments
- `WORKSPACE_RETENTION_DAYS` default `30`, archived workspace retention before janitor deletion
- `FRONTEND_DIST` default `frontend/dist`

Approval reviewer settings are managed from the admin UI. Global settings apply to applications using `inherit`; applications using `enabled` have their own endpoint, model, API key, and reviewer limits. API keys are never returned by the API. The master key is required before saving a key from the UI; losing it makes stored reviewer keys unrecoverable.

## Frontend

- `frontend/` is a Vue 3 + Nuxt UI + Tailwind CSS admin SPA (dark mode by default)
- Rust exports `openapi.json` and the frontend consumes generated OpenAPI SDK stubs
- `frontend` scripts use `bun` by default
- `bun.lock` is intentionally not committed; CI resolves frontend deps at build time
- Local dev frontend runs on `5173`
- Containerized frontend is served by `nginx` on port `80`

## Run

```bash
DATABASE_URL=postgres://desk_foreman:change-me-local-only@127.0.0.1:5432/desk_foreman \
cargo run
```

```bash
DESK_FOREMAN_URL=http://127.0.0.1:3000 \
RUNNER_MANAGER_TOKEN=change-me-runner-manager-token-local-only \
RUNNER_HOST_WORKSPACE_ROOT=/absolute/path/to/workspace-root \
cargo run -p desk-foreman-runner-manager
```

```bash
docker compose --profile build-only up --build
```

Then log into the admin UI, create per-user MCP tokens there, and open `http://localhost:8080`.

The Compose defaults are local-development placeholders. Set `POSTGRES_PASSWORD`, `DATABASE_URL`, and the runner manager connection variables before exposing the service outside a local machine.

## Runner Split

- `desk-foreman`: MCP, HTTP API, auth, admin UI, audit, workspace binding, runner job broker, and runner configuration.
- `runner-manager`: pull-based runner agent responsible for local shell execution and workspace-runner lifecycle.
- `workspace-runner`: per-workspace execution runtime image.

Current compose deployment keeps `/var/run/docker.sock` only on `runner-manager`. Runner managers pull jobs from desk-foreman and no longer require PostgreSQL access.

`runner-manager` now has direct-backend HTTP integration tests covering `exec-shell`, `write-stdin`, `run-command`, and bearer-auth enforcement. Run them with `cargo test -p desk-foreman-runner-manager`.
