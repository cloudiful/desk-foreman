//! Mandatory named integration gate for native-task preparation
//! (issues 753 and 764 P2b, `#[cfg(test)]`).
//!
//! Real isolated PostgreSQL, the real DF router, and the real manager dispatch
//! helper and filesystem store, with no Docker anywhere. What this proves is
//! the whole manager leg: a durable preparation is polled, its seed is fetched
//! over the manager's own bearer, materialized to a real local checkout, and
//! acknowledged — while an unauthorized manager, an unavailable job, or a
//! disabled root can never produce a false success.
//!
//! PG-gated by design: every case is `#[ignore]` and the mandatory gate runs
//! `DF_ENV_TEST_DATABASE_URL="$TASK_PG_URL" cargo test -p
//! desk-foreman-runner-manager native_task_preparation -- --ignored
//! --test-threads=1`. A missing URL fails the run instead of silently passing.

#[cfg(test)]
#[path = "native_task_preparation/support.rs"]
pub(super) mod support;

#[cfg(test)]
#[path = "native_task_preparation/cases.rs"]
mod cases;
