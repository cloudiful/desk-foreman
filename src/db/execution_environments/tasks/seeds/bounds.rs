//! Control-plane shim over the shared seed rules (issue 753 P2b).
//!
//! The rules themselves — path normalization, the limits, the ancestor and
//! duplicate checks and the v1 manifest digest — live in
//! `runner_protocol::task_seed`, because the runner manager must apply the
//! exact same ones when it materializes a checkout. This module only re-exports
//! them under the name the P2a control-plane code already calls, so there is
//! one implementation and no copy that could drift.

pub use runner_protocol::validate_seed_bundle as plan_task_seed;
