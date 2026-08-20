# Application Query 400 Fix

## Goal

Fix the admin applications list request that returns `400` when the optional
`search` query parameter is omitted, and make malformed query rejections
visible in Docker logs without logging sensitive query values.

## Background

`GET /api/admin/applications` uses `ValidatedQuery<ListApplicationsParams>`.
The `search: Option<String>` field has a custom deserializer but lacks
`serde(default)`, so the frontend's intentional omission of an empty search
term is treated as `missing field search`. The query rejection is converted
directly to `AppError::BadRequest`, whose response path currently logs only
internal errors. The same custom optional deserializer pattern is used by the
MCP-token and runner-manager list parameter types.

Database diagnostics and direct execution of the applications list SQL
succeeded before implementation; the database is not the cause of this 400.

## Baseline

- `HEAD`: `912a45ed335f1983a349b8e9a4a9ad5a338c94a9`
- Staged paths: none
- Unstaged paths: none
- Untracked paths: none
- Baseline paths are user-owned and out of scope for this task.

## Constraints

- Keep the change scoped to query deserialization and request validation
  logging.
- Do not log full query strings or parameter values; query values may contain
  user data or secrets.
- Use the existing `tracing` and validation/error patterns.
- Executors and reviewers must not commit or modify baseline paths.
- Do not stage or commit `Cargo.lock`.

## Acceptance Criteria

1. Omitting `search` from applications, MCP-token, and runner-manager list
   queries deserializes it as `None` instead of returning 400.
2. Existing supplied, blank, and non-blank search behavior remains unchanged.
3. Query-string deserialization failures produce a structured warning log with
   request method/path and the rejection message, without query values.
4. Validation failures remain HTTP 400 with the existing response shape.
5. Regression tests cover omitted optional custom-deserializer fields and the
   logging/error behavior where practical.
6. Relevant Rust formatting, tests, and checks pass.

## Phases

### Phase 1: Fix optional query fields and validation logs

- Outcome: Optional custom-deserialized query fields accept omission, and
  malformed query requests are observable in logs.
- In scope: `src/db/types.rs`, `src/api/validation.rs`, and focused Rust tests
  in those modules if needed.
- Out of scope: frontend code, SQL, migrations, Docker/Nginx configuration,
  API response shape changes, and all baseline paths.
- Dependencies: none.
- Validation: `cargo fmt --all -- --check`; focused `cargo test` for validation
  and relevant application/API tests; `cargo test` if focused coverage cannot
  exercise the full path.

## Decisions

- Use `#[serde(default, deserialize_with = ...)]` for optional fields rather
  than changing the frontend to send empty query parameters. The omission is
  the intended representation of an unset optional filter.
- Log query deserialization failures at `warn` level, not `error`, because
  malformed requests are client/input errors and can be intentionally
  generated at high volume.
- Log request method/path and the parser's rejection text; do not log the
  complete URI/query string.

## Review History

- Phase 1 executor completed the implementation in the two allowlisted source
  files. The executor initially returned no result, then confirmed completion
  with `cargo test --lib api::validation` (4/4), `cargo test --lib db::types`
  (4/4), `cargo test --lib` (53/53), and `cargo check --tests` passing.
- `cargo fmt --all -- --check` remains blocked by formatting diffs in
  out-of-scope files; the executor confirmed the two phase files are clean.
- Independent reviewer verdict: `PASS`; no P0-P2 findings.
- Phase 1 changed paths: `src/db/types.rs`, `src/api/validation.rs`.
- Phase 1 validation: `cargo check --tests`, `cargo test --lib api::validation`,
  `cargo test --lib db::types`, `cargo test --lib`, `cargo clippy --tests
  --all-features`, and `git diff --check` passed. Formatting review confirmed
  the phase files are clean; remaining formatting output is out of scope.
- Phase 1 remaining risks: none identified beyond the repository's unrelated
  pre-existing formatting/clippy warnings.

## Blocked Questions

- None.

## Final Status

`COMPLETE`
