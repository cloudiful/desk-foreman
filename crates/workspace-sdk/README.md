# Desk Foreman Workspace SDK

`desk-foreman-workspace-sdk` exposes the reusable workspace file tooling that powers Desk Foreman's file-oriented tools.

It is intended for other Rust projects that need:

- workspace-root-relative path resolution with escape protection
- Codex patch DSL parsing and application
- file reads with line-range and byte truncation support
- directory listing
- bounded directory pages and recursive workspace traversal
- file fingerprints for optimistic concurrency checks
- path metadata inspection

This crate does not include Desk Foreman's HTTP, MCP, auth, database, actor, or runner layers.

`WorkspaceFileTools` exposes only bounded file reads, directory pages, bounded recursive
traversal, path metadata, fingerprints, and patch application. `read_file_page`,
`list_directory_page`, and `walk_workspace_page` must be used when returning workspace data
to a model; there are no unbounded file or directory APIs in the SDK.

Patch input is the Codex DSL only (`*** Begin Patch` / `*** End Patch`). It supports
add, delete, update, and move operations, context hunks, EOF markers, whitespace and
Unicode punctuation fallback matching, CRLF/BOM preservation, atomic single-file
replacement, optimistic concurrency checks, and partial success for multi-file commits.
It does not require a Git repository and does not accept unified diff or `git apply`
input. Paths are workspace-relative, traversal and symlink escapes are rejected, and
update/add/move operations require UTF-8 text; delete can remove non-UTF-8 files.
