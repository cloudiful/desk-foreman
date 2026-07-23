use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use validator::Validate;

use crate::api::validation::{
    deserialize_optional_trimmed_nonempty, validate_non_blank, validate_sort_dir,
    validate_user_sort_by,
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
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct AuthMeResponse {
    pub user: UserResponse,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct UserPageResponse {
    pub items: Vec<UserResponse>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, FromRow)]
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
    #[param(minimum = 1, maximum = 100)]
    #[validate(range(min = 1, max = 100))]
    pub limit: Option<i64>,
    #[param(minimum = 0)]
    #[validate(range(min = 0))]
    pub offset: Option<i64>,
    #[validate(custom(function = "validate_user_sort_by"))]
    pub sort_by: Option<String>,
    #[validate(custom(function = "validate_sort_dir"))]
    pub sort_dir: Option<String>,
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
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, FromRow)]
pub struct ApprovalSettingsRecord {
    pub settings_id: i16,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub timeout_ms: i64,
    pub max_input_bytes: i64,
    pub max_concurrent: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ApprovalSettingsResponse {
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub timeout_ms: i64,
    pub max_input_bytes: i64,
    pub max_concurrent: i64,
    pub configured: bool,
    pub api_key_configured: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
pub struct UpdateApprovalSettingsRequest {
    pub endpoint: Option<String>,
    pub model: Option<String>,
    #[validate(range(min = 100, max = 30_000))]
    pub timeout_ms: i64,
    #[validate(range(min = 1, max = 524_288))]
    pub max_input_bytes: i64,
    #[validate(range(min = 1, max = 64))]
    pub max_concurrent: i64,
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
}

#[derive(Clone, Debug, Deserialize, Serialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct ListWorkspaceBindingsParams {
    #[param(minimum = 1, maximum = 100)]
    #[validate(range(min = 1, max = 100))]
    pub limit: Option<i64>,
    #[param(minimum = 0)]
    #[validate(range(min = 0))]
    pub offset: Option<i64>,
    #[validate(range(min = 1))]
    pub application_id: Option<i64>,
    pub external_user_id: Option<String>,
    pub workspace_key: Option<String>,
    pub is_active: Option<bool>,
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
pub struct AuditLogPageResponse {
    pub items: Vec<AuditLogResponse>,
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
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
    pub max_timeout_ms: Option<i64>,
    pub max_output_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
    pub max_sessions: Option<i64>,
    pub network_enabled: bool,
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
            last_login_at: value.last_login_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceRunnerSummary {
    pub runner_id: i64,
    pub owner_kind: String,
    pub container_name: String,
    pub container_id: Option<String>,
    pub runtime: String,
    pub runtime_class: Option<String>,
    pub image_name: String,
    pub status: String,
    pub network_enabled: bool,
    pub workspace_root: String,
    pub last_error: Option<String>,
    pub last_active_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct WorkspaceRunnerResponse {
    pub runner_id: i64,
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
pub struct OperationsSummary {
    pub active_runners: i64,
    pub active_sessions: i64,
    pub failed_operations: i64,
    pub archived_workspaces: i64,
}
