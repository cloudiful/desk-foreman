use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use validator::Validate;

use crate::api::validation::{
    deserialize_optional_trimmed_nonempty, validate_audit_status, validate_lifecycle_state,
    validate_non_blank, validate_sort_dir, validate_user_sort_by,
};
use crate::policy::ResourceLimits;

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct AuthLoginRequest {
    #[validate(custom(function = "validate_non_blank"))]
    pub login_name: String,
    #[validate(length(min = 1, message = "must not be empty"))]
    pub password: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct ChangePasswordRequest {
    #[validate(length(min = 1, message = "must not be empty"))]
    pub current_password: String,
    #[validate(length(min = 8, message = "must be at least 8 characters"))]
    pub new_password: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct CreateUserRequest {
    #[validate(custom(function = "validate_non_blank"))]
    pub login_name: String,
    #[validate(length(min = 1, message = "must not be empty"))]
    pub password: String,
    #[validate(custom(function = "validate_non_blank"))]
    pub display_name: String,
    #[validate(email(message = "must be a valid email address"))]
    pub email: String,
    #[validate(custom(function = "validate_non_blank"))]
    pub timezone: String,
    #[serde(default, deserialize_with = "deserialize_optional_trimmed_nonempty")]
    pub workspace_root: Option<String>,
    pub is_admin: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct UpdateUserRequest {
    #[validate(custom(function = "validate_non_blank"))]
    pub display_name: String,
    #[validate(email(message = "must be a valid email address"))]
    pub email: String,
    #[validate(custom(function = "validate_non_blank"))]
    pub timezone: String,
    pub is_admin: bool,
    pub is_active: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct ResetPasswordRequest {
    #[validate(length(min = 1, message = "must not be empty"))]
    pub password: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct UserResponse {
    pub user_id: i64,
    pub login_name: String,
    pub display_name: String,
    pub email: String,
    pub timezone: String,
    pub workspace_root: String,
    pub is_admin: bool,
    pub is_active: bool,
    pub must_change_password: bool,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct AuthMeResponse {
    pub user: UserResponse,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct UserRecord {
    pub user_id: i64,
    pub login_name: String,
    pub password_hash: String,
    pub display_name: String,
    pub email: String,
    pub timezone: String,
    pub workspace_root: Option<String>,
    pub is_admin: bool,
    pub is_active: bool,
    pub must_change_password: bool,
    pub deleted_at: Option<DateTime<Utc>>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, FromRow)]
pub struct WebSessionRecord {
    pub session_id: Uuid,
    pub user_id: i64,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct ListUsersParams {
    #[param(minimum = 1, maximum = 200)]
    #[validate(range(min = 1, max = 200))]
    pub limit: Option<i64>,
    #[param(minimum = 0)]
    #[validate(range(min = 0))]
    pub offset: Option<i64>,
    #[validate(custom(function = "validate_user_sort_by"))]
    pub sort_by: Option<String>,
    #[validate(custom(function = "validate_sort_dir"))]
    pub sort_dir: Option<String>,
    pub search: Option<String>,
    pub is_admin: Option<bool>,
    pub is_active: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct McpTokenResponse {
    pub token_id: i64,
    pub token_name: String,
    pub user_id: i64,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
    pub max_timeout_ms: Option<i64>,
    pub max_output_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
    pub max_sessions: Option<i64>,
    pub network_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct CreateMcpTokenRequest {
    #[validate(range(min = 1))]
    pub user_id: i64,
    #[validate(custom(function = "validate_non_blank"))]
    pub token_name: String,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    #[serde(default)]
    pub max_timeout_ms: Option<i64>,
    #[serde(default)]
    pub max_output_bytes: Option<i64>,
    #[serde(default)]
    pub max_file_bytes: Option<i64>,
    #[serde(default)]
    pub max_sessions: Option<i64>,
    #[serde(default)]
    pub network_enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct CreateMcpTokenResponse {
    pub token: String,
    pub metadata: McpTokenResponse,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct UpdateMcpTokenRequest {
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Option<Vec<String>>,
    pub max_timeout_ms: Option<i64>,
    pub max_output_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
    pub max_sessions: Option<i64>,
    pub network_enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct ApplicationResponse {
    pub application_id: i64,
    pub name: String,
    pub is_active: bool,
    pub workspace_template: Option<String>,
    pub default_shell: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub default_scopes: Vec<String>,
    pub max_timeout_ms: Option<i64>,
    pub max_output_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
    pub max_sessions: Option<i64>,
    pub network_enabled: bool,
    pub approval_mode: String,
    pub approval_endpoint: Option<String>,
    pub approval_model: Option<String>,
    pub approval_timeout_ms: Option<i64>,
    pub approval_max_input_bytes: Option<i64>,
    pub approval_max_concurrent: Option<i64>,
    pub approval_max_output_tokens: Option<i64>,
    pub approval_api_key_configured: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct CreateApplicationRequest {
    #[validate(custom(function = "validate_non_blank"))]
    pub name: String,
    pub workspace_template: Option<String>,
    pub default_shell: Option<String>,
    #[serde(default)]
    pub default_scopes: Option<Vec<String>>,
    #[serde(default)]
    pub max_timeout_ms: Option<i64>,
    #[serde(default)]
    pub max_output_bytes: Option<i64>,
    #[serde(default)]
    pub max_file_bytes: Option<i64>,
    #[serde(default)]
    pub max_sessions: Option<i64>,
    #[serde(default)]
    pub network_enabled: Option<bool>,
    #[serde(default)]
    pub approval_mode: Option<String>,
    #[serde(default)]
    pub approval_endpoint: Option<String>,
    #[serde(default)]
    pub approval_model: Option<String>,
    #[serde(default)]
    pub approval_timeout_ms: Option<i64>,
    #[serde(default)]
    pub approval_max_input_bytes: Option<i64>,
    #[serde(default)]
    pub approval_max_concurrent: Option<i64>,
    #[serde(default)]
    pub approval_max_output_tokens: Option<i64>,
    #[serde(default)]
    pub approval_api_key: Option<String>,
    #[serde(default)]
    pub clear_approval_api_key: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct UpdateApplicationRequest {
    #[validate(custom(function = "validate_non_blank"))]
    pub name: String,
    pub is_active: bool,
    pub workspace_template: Option<String>,
    pub default_shell: Option<String>,
    #[serde(default)]
    pub default_scopes: Option<Vec<String>>,
    #[serde(default)]
    pub max_timeout_ms: Option<i64>,
    #[serde(default)]
    pub max_output_bytes: Option<i64>,
    #[serde(default)]
    pub max_file_bytes: Option<i64>,
    #[serde(default)]
    pub max_sessions: Option<i64>,
    #[serde(default)]
    pub network_enabled: Option<bool>,
    #[serde(default)]
    pub approval_mode: Option<String>,
    #[serde(default)]
    pub approval_endpoint: Option<String>,
    #[serde(default)]
    pub approval_model: Option<String>,
    #[serde(default)]
    pub approval_timeout_ms: Option<i64>,
    #[serde(default)]
    pub approval_max_input_bytes: Option<i64>,
    #[serde(default)]
    pub approval_max_concurrent: Option<i64>,
    #[serde(default)]
    pub approval_max_output_tokens: Option<i64>,
    #[serde(default)]
    pub approval_api_key: Option<String>,
    #[serde(default)]
    pub clear_approval_api_key: bool,
}

#[derive(Clone, Debug, FromRow)]
pub struct ApprovalSettingsRecord {
    pub settings_id: i16,
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub timeout_ms: i64,
    pub max_input_bytes: i64,
    pub max_concurrent: i64,
    pub max_output_tokens: i64,
    pub api_key_ciphertext: Option<Vec<u8>>,
    pub api_key_nonce: Option<Vec<u8>>,
    pub api_key_key_version: Option<i16>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, FromRow)]
pub struct ApplicationApprovalSecretRecord {
    pub api_key_ciphertext: Option<Vec<u8>>,
    pub api_key_nonce: Option<Vec<u8>>,
    pub api_key_key_version: Option<i16>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ApprovalSettingsResponse {
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub timeout_ms: i64,
    pub max_input_bytes: i64,
    pub max_concurrent: i64,
    pub max_output_tokens: i64,
    pub configured: bool,
    pub api_key_configured: bool,
    pub api_key_source: String,
    pub secret_storage_ready: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct UpdateApprovalSettingsRequest {
    #[serde(default = "default_approval_enabled")]
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    #[validate(range(min = 100, max = 30_000))]
    pub timeout_ms: i64,
    #[validate(range(min = 1, max = 524_288))]
    pub max_input_bytes: i64,
    #[validate(range(min = 1, max = 64))]
    pub max_concurrent: i64,
    #[serde(default = "default_approval_output_tokens")]
    #[validate(range(min = 256, max = 8_192))]
    pub max_output_tokens: i64,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
}

fn default_approval_enabled() -> bool {
    true
}

fn default_approval_output_tokens() -> i64 {
    1024
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ApprovalTestResponse {
    pub ok: bool,
    pub stage: String,
    pub message: String,
    pub latency_ms: u64,
    pub model: Option<String>,
    pub decision: Option<String>,
    pub risk: Option<String>,
    pub reason_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct ApplicationTokenResponse {
    pub token_id: i64,
    pub application_id: i64,
    pub token_name: String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
    pub max_timeout_ms: Option<i64>,
    pub max_output_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
    pub max_sessions: Option<i64>,
    pub network_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct CreateApplicationTokenRequest {
    #[validate(range(min = 1))]
    pub application_id: i64,
    #[validate(custom(function = "validate_non_blank"))]
    pub token_name: String,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    #[serde(default)]
    pub max_timeout_ms: Option<i64>,
    #[serde(default)]
    pub max_output_bytes: Option<i64>,
    #[serde(default)]
    pub max_file_bytes: Option<i64>,
    #[serde(default)]
    pub max_sessions: Option<i64>,
    #[serde(default)]
    pub network_enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct CreateApplicationTokenResponse {
    pub token: String,
    pub metadata: ApplicationTokenResponse,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct UpdateApplicationTokenRequest {
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Option<Vec<String>>,
    pub max_timeout_ms: Option<i64>,
    pub max_output_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
    pub max_sessions: Option<i64>,
    pub network_enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct WorkspaceBindingResponse {
    pub workspace_binding_id: i64,
    pub application_id: i64,
    pub external_user_id: String,
    pub workspace_key: String,
    pub external_user_hash: String,
    pub workspace_root: String,
    pub is_active: bool,
    pub last_used_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub lifecycle_state: String,
    pub archived_at: Option<DateTime<Utc>>,
    pub resource_kind: Option<String>,
    pub resource_id: Option<String>,
    pub write_lease_owner: Option<String>,
    pub write_lease_acquired_at: Option<DateTime<Utc>>,
    pub write_lease_expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct WorkspaceLeaseRequest {
    #[validate(custom(function = "validate_non_blank"))]
    pub owner: String,
    #[serde(default = "default_lease_ttl_seconds")]
    #[validate(range(min = 60, max = 86_400))]
    pub ttl_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct WorkspaceLeaseReleaseRequest {
    #[validate(custom(function = "validate_non_blank"))]
    pub owner: String,
}

/// Request body for the atomic, stale-guarded write-lease takeover endpoint.
///
/// `expected_owner` is the lease owner the caller believes currently holds
/// the binding's write lease; the server uses it as the optimistic
/// compare-and-swap guard. `new_owner` is the value that will be assigned if
/// the takeover (or same-owner idempotent renew) succeeds. The granted TTL
/// is a server constant and is not client-controlled.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct WorkspaceLeaseTakeoverRequest {
    #[validate(custom(function = "validate_non_blank"))]
    pub expected_owner: String,
    #[validate(custom(function = "validate_non_blank"))]
    pub new_owner: String,
}

/// Row produced by `SELECT ... FOR UPDATE` on the workspace binding during
/// the takeover transaction. Captures the pre-update lease state plus the
/// database clock at lock time (`db_now`) so the caller can classify the
/// request without any application-clock dependency.
#[derive(Clone, Debug, FromRow)]
pub struct WorkspaceLeaseTakeoverLockRow {
    pub workspace_binding_id: i64,
    pub application_id: i64,
    pub workspace_key: String,
    pub is_active: bool,
    pub lifecycle_state: String,
    pub resource_kind: Option<String>,
    pub resource_id: Option<String>,
    pub write_lease_owner: Option<String>,
    pub write_lease_acquired_at: Option<DateTime<Utc>>,
    pub write_lease_expires_at: Option<DateTime<Utc>>,
    /// Database NOW() captured at lock time, in the database's UTC clock.
    pub db_now: DateTime<Utc>,
}

/// Machine-readable conflict reasons for the takeover endpoint.
///
/// `SameOwner` is intentionally unreachable: when `new_owner` already holds
/// the lease the SQL classifies the request as an idempotent same-owner
/// renew and returns success with `took_over_foreign = false`. Only the
/// remaining four reasons can surface through the 409 conflict path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TakeoverConflictReason {
    /// No lease is currently held on the binding; the caller should use
    /// the ordinary acquire endpoint instead.
    NoLease,
    /// A foreign lease matches `expected_owner` but its last refresh is
    /// inside the stale window; the caller should wait or retry.
    LiveLease,
    /// The current lease owner does not match the caller-supplied
    /// `expected_owner`; another writer displaced or refreshed it.
    ExpectedOwnerMismatch,
    /// The binding is not active (archived, resetting, or deactivated).
    NotActive,
}

impl TakeoverConflictReason {
    /// Stable wire-format identifier used in the `reason` field of the
    /// 409 conflict body. Callers dispatch on this value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoLease => "no_lease",
            Self::LiveLease => "live_lease",
            Self::ExpectedOwnerMismatch => "expected_owner_mismatch",
            Self::NotActive => "not_active",
        }
    }
}

/// Successful takeover response.
///
/// `previous_owner`/`previous_acquired_at`/`previous_expires_at` describe the
/// lease immediately before the takeover. `took_over_foreign` is true when the
/// previous owner differs from `new_owner` (so the caller displaced a stale
/// foreign lease); false means the call was an idempotent same-owner renew.
/// `cancellation` reports the best-effort outcome of cancelling every runner
/// session that was scoped to the binding (not just to the displaced owner:
/// the cancel helper cancels by `RunnerOwner::WorkspaceBinding`).
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WorkspaceLeaseTakeoverResponse {
    #[serde(flatten)]
    pub binding: WorkspaceBindingResponse,
    pub previous_owner: Option<String>,
    pub previous_acquired_at: Option<DateTime<Utc>>,
    pub previous_expires_at: Option<DateTime<Utc>>,
    pub took_over_foreign: bool,
    pub granted_ttl_seconds: u64,
    pub stale_threshold_seconds: u64,
    pub cancellation: WorkspaceLeaseCancellationOutcome,
}

/// Best-effort outcome of cancelling runner sessions scoped to the binding.
/// Cancellation errors never roll back the lease transfer; they are recorded
/// here and in the audit log.
#[derive(Clone, Debug, Serialize, ToSchema, Default)]
pub struct WorkspaceLeaseCancellationOutcome {
    pub attempted: bool,
    pub succeeded: bool,
    pub sessions_cancelled: u64,
    pub error: Option<String>,
}

/// Structured conflict body for takeover failures.
///
/// Stock callers (or other applications) can inspect `current.lease_owner`
/// and `current.lease_acquired_at` to determine whether to wait or to use
/// the value as the next `expected_owner`, and use `reason` for dispatch.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WorkspaceLeaseTakeoverConflict {
    pub error: String,
    /// Machine-readable reason. See [`TakeoverConflictReason::as_str`].
    pub reason: String,
    pub current: WorkspaceLeaseStatusResponse,
    pub stale_threshold_seconds: u64,
}

/// SQL row shape for the resource workspace lease-status read. Kept separate
/// from the API response so the `stale_threshold_seconds` constant (which
/// lives in the API layer) is not entangled with database decoding.
#[derive(Clone, Debug, FromRow)]
pub struct WorkspaceLeaseStatusRow {
    pub workspace_binding_id: i64,
    pub application_id: i64,
    pub workspace_key: String,
    pub is_active: bool,
    pub lifecycle_state: String,
    pub resource_kind: Option<String>,
    pub resource_id: Option<String>,
    pub write_lease_owner: Option<String>,
    pub write_lease_acquired_at: Option<DateTime<Utc>>,
    pub write_lease_expires_at: Option<DateTime<Utc>>,
}

/// Authenticated lease-status response exposed via the lease-status read
/// endpoint. Scoped to the caller's application and resource binding so no
/// unrelated bindings are leaked. The `stale_threshold_seconds` field is
/// filled in by the handler after the SQL fetch because the constant lives
/// in the API layer and is not stored in the database row.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WorkspaceLeaseStatusResponse {
    pub workspace_binding_id: i64,
    pub application_id: i64,
    pub workspace_key: String,
    pub is_active: bool,
    pub lifecycle_state: String,
    pub resource_kind: Option<String>,
    pub resource_id: Option<String>,
    pub write_lease_owner: Option<String>,
    pub write_lease_acquired_at: Option<DateTime<Utc>>,
    pub write_lease_expires_at: Option<DateTime<Utc>>,
    pub stale_threshold_seconds: u64,
}

impl From<WorkspaceLeaseStatusRow> for WorkspaceLeaseStatusResponse {
    fn from(row: WorkspaceLeaseStatusRow) -> Self {
        Self {
            workspace_binding_id: row.workspace_binding_id,
            application_id: row.application_id,
            workspace_key: row.workspace_key,
            is_active: row.is_active,
            lifecycle_state: row.lifecycle_state,
            resource_kind: row.resource_kind,
            resource_id: row.resource_id,
            write_lease_owner: row.write_lease_owner,
            write_lease_acquired_at: row.write_lease_acquired_at,
            write_lease_expires_at: row.write_lease_expires_at,
            stale_threshold_seconds: 0,
        }
    }
}

fn default_lease_ttl_seconds() -> u64 {
    3_600
}

#[derive(Clone, Debug, Deserialize, Serialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct ListWorkspaceBindingsParams {
    #[param(minimum = 1, maximum = 200)]
    #[validate(range(min = 1, max = 200))]
    pub limit: Option<i64>,
    #[param(minimum = 0)]
    #[validate(range(min = 0))]
    pub offset: Option<i64>,
    #[validate(range(min = 1))]
    pub application_id: Option<i64>,
    pub external_user_id: Option<String>,
    pub workspace_key: Option<String>,
    pub is_active: Option<bool>,
    #[validate(custom(function = "validate_lifecycle_state"))]
    pub lifecycle_state: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct ListApplicationsParams {
    #[param(minimum = 1, maximum = 200)]
    #[validate(range(min = 1, max = 200))]
    pub limit: Option<i64>,
    #[param(minimum = 0)]
    #[validate(range(min = 0))]
    pub offset: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_trimmed_nonempty")]
    pub search: Option<String>,
    pub is_active: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct ListApplicationTokensParams {
    #[param(minimum = 1, maximum = 200)]
    #[validate(range(min = 1, max = 200))]
    pub limit: Option<i64>,
    #[param(minimum = 0)]
    #[validate(range(min = 0))]
    pub offset: Option<i64>,
    #[validate(range(min = 1))]
    pub application_id: Option<i64>,
    pub is_active: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct ListMcpTokensParams {
    #[param(minimum = 1, maximum = 200)]
    #[validate(range(min = 1, max = 200))]
    pub limit: Option<i64>,
    #[param(minimum = 0)]
    #[validate(range(min = 0))]
    pub offset: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_trimmed_nonempty")]
    pub search: Option<String>,
    pub user_id: Option<i64>,
    pub is_active: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct ListRunnerManagersParams {
    #[param(minimum = 1, maximum = 200)]
    #[validate(range(min = 1, max = 200))]
    pub limit: Option<i64>,
    #[param(minimum = 0)]
    #[validate(range(min = 0))]
    pub offset: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_trimmed_nonempty")]
    pub search: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct ListWorkspaceRunnersParams {
    #[param(minimum = 1, maximum = 200)]
    #[validate(range(min = 1, max = 200))]
    pub limit: Option<i64>,
    #[param(minimum = 0)]
    #[validate(range(min = 0))]
    pub offset: Option<i64>,
    pub status: Option<String>,
    pub owner_kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct ListRunnerSessionsParams {
    #[param(minimum = 1, maximum = 200)]
    #[validate(range(min = 1, max = 200))]
    pub limit: Option<i64>,
    #[param(minimum = 0)]
    #[validate(range(min = 0))]
    pub offset: Option<i64>,
    pub owner_kind: Option<String>,
    pub state: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct ListAuditLogsParams {
    #[param(minimum = 1, maximum = 200)]
    #[validate(range(min = 1, max = 200))]
    pub limit: Option<i64>,
    #[param(minimum = 0)]
    #[validate(range(min = 0))]
    pub offset: Option<i64>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub action: Option<String>,
    pub search: Option<String>,
    #[validate(custom(function = "validate_audit_status"))]
    pub status: Option<String>,
    pub actor_user_id: Option<i64>,
    pub actor_application_id: Option<i64>,
    pub workspace_binding_id: Option<i64>,
    pub session_id: Option<i64>,
    pub request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct AuditLogResponse {
    pub audit_id: i64,
    pub actor_user_id: Option<i64>,
    pub actor_application_id: Option<i64>,
    pub actor_type: String,
    pub action: String,
    pub target_type: String,
    pub target_id: String,
    pub workspace_binding_id: Option<i64>,
    pub external_user_id: Option<String>,
    pub payload: serde_json::Value,
    pub request_id: Option<String>,
    pub session_id: Option<i64>,
    pub duration_ms: Option<i64>,
    pub status: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Page<T: ToSchema> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct ApplicationTokenRecord {
    pub token_id: i64,
    pub application_id: i64,
    pub name: String,
    pub is_active: bool,
    pub workspace_template: Option<String>,
    pub default_shell: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub default_scopes: Vec<String>,
    pub app_max_timeout_ms: Option<i64>,
    pub app_max_output_bytes: Option<i64>,
    pub app_max_file_bytes: Option<i64>,
    pub app_max_sessions: Option<i64>,
    pub app_network_enabled: bool,
    pub app_approval_mode: String,
    pub app_approval_endpoint: Option<String>,
    pub app_approval_model: Option<String>,
    pub app_approval_timeout_ms: Option<i64>,
    pub app_approval_max_input_bytes: Option<i64>,
    pub app_approval_max_concurrent: Option<i64>,
    pub app_approval_max_output_tokens: Option<i64>,
    pub app_approval_api_key_configured: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
    pub max_timeout_ms: Option<i64>,
    pub max_output_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
    pub max_sessions: Option<i64>,
    pub network_enabled: bool,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ApplicationCapabilitiesResponse {
    pub application_id: i64,
    pub application_name: String,
    pub scopes: Vec<String>,
    pub max_timeout_ms: Option<u64>,
    pub max_output_bytes: Option<usize>,
    pub max_file_bytes: Option<usize>,
    pub max_sessions: Option<usize>,
    pub network_enabled: bool,
    pub runner_available: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct GitWorkspaceSyncRequest {
    #[validate(custom(function = "validate_non_blank"))]
    pub remote_url: String,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct GitWorkspaceSyncResponse {
    pub status: String,
    pub cloned: bool,
    pub dirty: bool,
    pub branch: Option<String>,
    pub head_commit: Option<String>,
}

#[derive(Clone, Debug)]
pub struct McpTokenPolicy {
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
    pub max_timeout_ms: Option<i64>,
    pub max_output_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
    pub max_sessions: Option<i64>,
    pub network_enabled: bool,
}

impl ApplicationResponse {
    pub fn resource_limits(&self) -> ResourceLimits {
        ResourceLimits {
            max_timeout_ms: to_u64(self.max_timeout_ms),
            max_output_bytes: to_usize(self.max_output_bytes),
            max_file_bytes: to_usize(self.max_file_bytes),
            max_sessions: to_usize(self.max_sessions),
            network_enabled: self.network_enabled,
        }
    }
}

impl ApplicationTokenRecord {
    pub fn application_limits(&self) -> ResourceLimits {
        ResourceLimits {
            max_timeout_ms: to_u64(self.app_max_timeout_ms),
            max_output_bytes: to_usize(self.app_max_output_bytes),
            max_file_bytes: to_usize(self.app_max_file_bytes),
            max_sessions: to_usize(self.app_max_sessions),
            network_enabled: self.app_network_enabled,
        }
    }

    pub fn token_limits(&self) -> ResourceLimits {
        ResourceLimits {
            max_timeout_ms: to_u64(self.max_timeout_ms),
            max_output_bytes: to_usize(self.max_output_bytes),
            max_file_bytes: to_usize(self.max_file_bytes),
            max_sessions: to_usize(self.max_sessions),
            network_enabled: self.network_enabled,
        }
    }
}

fn to_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn to_usize(value: Option<i64>) -> Option<usize> {
    value.and_then(|value| usize::try_from(value).ok())
}

impl From<UserRecord> for UserResponse {
    fn from(value: UserRecord) -> Self {
        Self {
            user_id: value.user_id,
            login_name: value.login_name,
            display_name: value.display_name,
            email: value.email,
            timezone: value.timezone,
            workspace_root: value.workspace_root.unwrap_or_default(),
            is_admin: value.is_admin,
            is_active: value.is_active,
            must_change_password: value.must_change_password,
            last_login_at: value.last_login_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct WorkspaceRunnerResponse {
    pub runner_id: i64,
    pub runner_manager_id: Option<i64>,
    pub owner_kind: String,
    pub owner_user_id: Option<i64>,
    pub owner_workspace_binding_id: Option<i64>,
    pub container_name: String,
    pub container_id: Option<String>,
    pub runtime: String,
    pub runtime_class: Option<String>,
    pub image_name: String,
    pub status: String,
    pub network_enabled: bool,
    pub workspace_root: String,
    pub last_active_at: DateTime<Utc>,
    pub last_observed_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct RunnerSessionResponse {
    pub session_id: u64,
    pub owner_kind: String,
    pub owner_id: i64,
    pub session_key: Option<String>,
    pub state: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub wall_time_seconds: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct RunnerManagerResponse {
    pub runner_manager_id: i64,
    pub name: String,
    pub endpoint: String,
    pub enabled: bool,
    pub image: String,
    pub network_enabled: bool,
    pub max_output_bytes: i64,
    pub max_timeout_ms: i64,
    pub max_sessions: i64,
    pub pids_limit: i64,
    pub memory_limit: String,
    pub cpu_limit: String,
    pub host_workspace_root: Option<String>,
    pub status: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct CreateRunnerManagerResponse {
    pub manager: RunnerManagerResponse,
    pub token: Option<String>,
}

#[derive(Clone, Debug, Deserialize, FromRow)]
pub struct RunnerManagerRecord {
    pub runner_manager_id: i64,
    pub name: String,
    pub endpoint: String,
    pub access_token_hash: String,
    pub enabled: bool,
    pub image: String,
    pub network_enabled: bool,
    pub max_output_bytes: i64,
    pub max_timeout_ms: i64,
    pub max_sessions: i64,
    pub pids_limit: i64,
    pub memory_limit: String,
    pub cpu_limit: String,
    pub host_workspace_root: Option<String>,
    pub status: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct CreateRunnerManagerRequest {
    #[validate(custom(function = "validate_non_blank"))]
    pub name: String,
    #[validate(url)]
    pub endpoint: String,
    #[serde(default)]
    pub access_token: Option<String>,
    #[validate(custom(function = "validate_non_blank"))]
    pub image: String,
    pub network_enabled: bool,
    #[validate(range(min = 1, max = 16_777_216))]
    pub max_output_bytes: i64,
    #[validate(range(min = 1, max = 3_600_000))]
    pub max_timeout_ms: i64,
    #[validate(range(min = 1, max = 1024))]
    pub max_sessions: i64,
    #[validate(range(min = 1, max = 65_536))]
    pub pids_limit: i64,
    #[validate(custom(function = "validate_non_blank"))]
    pub memory_limit: String,
    #[validate(custom(function = "validate_non_blank"))]
    pub cpu_limit: String,
    #[serde(default)]
    pub host_workspace_root: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct UpdateRunnerManagerRequest {
    #[validate(url)]
    pub endpoint: String,
    pub enabled: bool,
    #[validate(custom(function = "validate_non_blank"))]
    pub image: String,
    pub network_enabled: bool,
    #[validate(range(min = 1, max = 16_777_216))]
    pub max_output_bytes: i64,
    #[validate(range(min = 1, max = 3_600_000))]
    pub max_timeout_ms: i64,
    #[validate(range(min = 1, max = 1024))]
    pub max_sessions: i64,
    #[validate(range(min = 1, max = 65_536))]
    pub pids_limit: i64,
    #[validate(custom(function = "validate_non_blank"))]
    pub memory_limit: String,
    #[validate(custom(function = "validate_non_blank"))]
    pub cpu_limit: String,
    #[serde(default)]
    pub host_workspace_root: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct OperationsSummary {
    pub active_runners: i64,
    pub active_sessions: i64,
    pub failed_operations: i64,
    pub archived_workspaces: i64,
    pub runner_managers_total: i64,
    pub runner_managers_online: i64,
    pub runner_managers_offline: i64,
    pub runner_managers_disabled: i64,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::{
        ListApplicationsParams, ListMcpTokensParams, ListRunnerManagersParams,
        WorkspaceBindingResponse, deserialize_optional_trimmed_nonempty,
    };

    #[test]
    fn omitted_search_defaults_to_none_for_list_query_params() {
        let applications: ListApplicationsParams =
            serde_json::from_value(json!({})).expect("parse");
        assert!(applications.search.is_none());

        let mcp: ListMcpTokensParams = serde_json::from_value(json!({})).expect("parse");
        assert!(mcp.search.is_none());

        let managers: ListRunnerManagersParams = serde_json::from_value(json!({})).expect("parse");
        assert!(managers.search.is_none());
    }

    #[test]
    fn blank_search_collapses_to_none_for_list_query_params() {
        let applications: ListApplicationsParams =
            serde_json::from_value(json!({ "search": "   " })).expect("parse");
        assert!(applications.search.is_none());

        let mcp: ListMcpTokensParams =
            serde_json::from_value(json!({ "search": "" })).expect("parse");
        assert!(mcp.search.is_none());

        let managers: ListRunnerManagersParams =
            serde_json::from_value(json!({ "search": "\t\n" })).expect("parse");
        assert!(managers.search.is_none());
    }

    #[test]
    fn non_blank_search_is_trimmed_for_list_query_params() {
        let applications: ListApplicationsParams =
            serde_json::from_value(json!({ "search": "  alpha  " })).expect("parse");
        assert_eq!(applications.search.as_deref(), Some("alpha"));

        let mcp: ListMcpTokensParams =
            serde_json::from_value(json!({ "search": "beta" })).expect("parse");
        assert_eq!(mcp.search.as_deref(), Some("beta"));

        let managers: ListRunnerManagersParams =
            serde_json::from_value(json!({ "search": " gamma " })).expect("parse");
        assert_eq!(managers.search.as_deref(), Some("gamma"));
    }

    #[test]
    fn empty_input_object_deserializes_optional_search_with_default() {
        // Mirrors `#[serde(default, deserialize_with = ...)]` from a request without the key.
        #[derive(serde::Deserialize)]
        struct Input {
            #[serde(default, deserialize_with = "deserialize_optional_trimmed_nonempty")]
            value: Option<String>,
        }

        let input: Input = serde_json::from_value(json!({})).expect("parse");
        assert!(input.value.is_none());
    }

    #[test]
    fn takeover_request_rejects_blank_owners() {
        use validator::Validate;

        let valid: super::WorkspaceLeaseTakeoverRequest =
            serde_json::from_value(json!({ "expected_owner": "a", "new_owner": "b" }))
                .expect("parse");
        assert!(valid.validate().is_ok());

        let blank_expected: super::WorkspaceLeaseTakeoverRequest =
            serde_json::from_value(json!({ "expected_owner": "  ", "new_owner": "b" }))
                .expect("parse");
        assert!(
            blank_expected.validate().is_err(),
            "blank expected_owner should fail validation"
        );

        let blank_new: super::WorkspaceLeaseTakeoverRequest =
            serde_json::from_value(json!({ "expected_owner": "a", "new_owner": "" }))
                .expect("parse");
        assert!(
            blank_new.validate().is_err(),
            "blank new_owner should fail validation"
        );
    }

    #[test]
    fn takeover_response_includes_previous_owner_and_cancellation_outcome() {
        let response = super::WorkspaceLeaseTakeoverResponse {
            binding: WorkspaceBindingResponse {
                workspace_binding_id: 7,
                application_id: 1,
                external_user_id: "__resource__".to_string(),
                workspace_key: "code_project:abc".to_string(),
                external_user_hash: "hash".to_string(),
                workspace_root: "/tmp/ws".to_string(),
                is_active: true,
                last_used_at: Utc::now(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                lifecycle_state: "active".to_string(),
                archived_at: None,
                resource_kind: Some("code_project".to_string()),
                resource_id: Some("abc".to_string()),
                write_lease_owner: Some("conversation:b".to_string()),
                write_lease_acquired_at: Some(Utc::now()),
                write_lease_expires_at: Some(Utc::now()),
            },
            previous_owner: Some("conversation:a".to_string()),
            previous_acquired_at: Some(Utc::now()),
            previous_expires_at: Some(Utc::now()),
            took_over_foreign: true,
            granted_ttl_seconds: 600,
            stale_threshold_seconds: 180,
            cancellation: super::WorkspaceLeaseCancellationOutcome {
                attempted: true,
                succeeded: true,
                sessions_cancelled: 1,
                error: None,
            },
        };
        let value = serde_json::to_value(&response).expect("serialize");
        assert_eq!(value["took_over_foreign"], json!(true));
        assert_eq!(value["granted_ttl_seconds"], json!(600));
        assert_eq!(value["stale_threshold_seconds"], json!(180));
        assert_eq!(value["cancellation"]["sessions_cancelled"], json!(1));
        assert_eq!(value["write_lease_owner"], json!("conversation:b"));
        assert_eq!(value["previous_owner"], json!("conversation:a"));
    }

    #[test]
    fn takeover_conflict_reports_machine_readable_reason_and_current_state() {
        let conflict = super::WorkspaceLeaseTakeoverConflict {
            error: "lease is still within the stale window".to_string(),
            reason: "live_lease".to_string(),
            stale_threshold_seconds: 180,
            current: super::WorkspaceLeaseStatusResponse {
                workspace_binding_id: 9,
                application_id: 1,
                workspace_key: "code_project:xyz".to_string(),
                is_active: true,
                lifecycle_state: "active".to_string(),
                resource_kind: Some("code_project".to_string()),
                resource_id: Some("xyz".to_string()),
                write_lease_owner: Some("conversation:a".to_string()),
                write_lease_acquired_at: Some(Utc::now()),
                write_lease_expires_at: Some(Utc::now()),
                stale_threshold_seconds: 180,
            },
        };
        let value = serde_json::to_value(&conflict).expect("serialize");
        assert_eq!(value["reason"], json!("live_lease"));
        assert_eq!(value["stale_threshold_seconds"], json!(180));
        assert_eq!(
            value["current"]["write_lease_owner"],
            json!("conversation:a")
        );
    }

    #[test]
    fn lease_status_row_converts_to_response_with_zero_threshold() {
        let row = super::WorkspaceLeaseStatusRow {
            workspace_binding_id: 1,
            application_id: 1,
            workspace_key: "code_project:abc".to_string(),
            is_active: true,
            lifecycle_state: "active".to_string(),
            resource_kind: Some("code_project".to_string()),
            resource_id: Some("abc".to_string()),
            write_lease_owner: None,
            write_lease_acquired_at: None,
            write_lease_expires_at: None,
        };
        let response: super::WorkspaceLeaseStatusResponse = row.into();
        // The handler always overwrites stale_threshold_seconds with the
        // server constant; the conversion leaves it at zero so the omission
        // is visible.
        assert_eq!(response.stale_threshold_seconds, 0);
        assert_eq!(response.write_lease_owner, None);
        assert_eq!(response.lifecycle_state, "active");
    }
}
