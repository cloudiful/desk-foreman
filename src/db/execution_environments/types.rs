//! Focused types for durable execution environments (issue 737 P1).
//!
//! The row is the durable identity for one `(application_id,
//! external_user_id)` pair. `runtime_kind` stays opaque and agent-neutral:
//! DF never branches on it. Generation and `state_volume_ref` are
//! bookkeeping references, not security credentials.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

/// Durable execution environment row and API response shape.
///
/// `environment_id` is stable across replacement; `generation` increments
/// on every invalidate; `state` is one of the checked lifecycle values;
/// `state_volume_ref` is the inert DF-derived token `df-env-<uuid-simple>`.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct ExecutionEnvironment {
    pub environment_id: Uuid,
    pub application_id: i64,
    pub external_user_id: String,
    pub runtime_kind: String,
    pub generation: i64,
    pub state: String,
    pub state_volume_ref: String,
    pub last_active_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Allowed lifecycle states for the P1 slice.
pub const EXECUTION_ENVIRONMENT_STATES: [&str; 6] = [
    "absent",
    "starting",
    "ready",
    "stopping",
    "replaced",
    "unreachable",
];

/// Initial state for a newly ensured environment.
pub const EXECUTION_ENVIRONMENT_INITIAL_STATE: &str = "starting";

/// State set by a successful invalidate.
pub const EXECUTION_ENVIRONMENT_INVALIDATED_STATE: &str = "stopping";

/// True when `state` is one of the checked lifecycle values.
pub fn execution_state_is_valid(state: &str) -> bool {
    EXECUTION_ENVIRONMENT_STATES.contains(&state)
}

/// Derive the inert state-volume reference from the environment UUID.
///
/// The token is never provisioned in this phase; it is a DF-derived name
/// of the form `df-env-<uuid-simple>` where `<uuid-simple>` is the
/// lowercase hyphenless UUID.
pub fn derive_state_volume_ref(environment_id: &Uuid) -> String {
    format!("df-env-{}", environment_id.to_string().replace('-', ""))
}

/// True when `runtime_kind` is a usable opaque value: non-blank after
/// trim and bounded length. No allowlist: DF stays agent-neutral.
pub fn runtime_kind_is_valid(runtime_kind: &str) -> bool {
    let trimmed = runtime_kind.trim();
    !trimmed.is_empty() && trimmed.len() <= 64
}

/// True when `external_user_id` is usable: non-blank after trim and
/// bounded length. The value stays opaque to DF.
pub fn external_user_id_is_valid(external_user_id: &str) -> bool {
    let trimmed = external_user_id.trim();
    !trimmed.is_empty() && trimmed.len() <= 128
}
