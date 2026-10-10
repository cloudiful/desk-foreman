use utoipa::{
    Modify, OpenApi,
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
};

use crate::{
    db::types::*,
    error::ErrorResponse,
    shell::ShellToolOutput,
    tools::{
        params::*,
        readonly::types::*,
        shared::{ApplyPatchOutput, CancelSessionOutput},
    },
};

/// Name of the security scheme the internal application-authenticated routes
/// advertise. It mirrors the `application_actor` bearer guard, so generated
/// clients send the same `Authorization: Bearer <application token>` header the
/// runtime already requires.
pub(crate) const APPLICATION_BEARER_SCHEME: &str = "application_bearer";

#[derive(OpenApi)]
#[openapi(
    paths(
        super::authn::login,
        super::authn::logout,
        super::authn::me,
        super::authn::change_password,
        super::admin::users::list_users,
        super::admin::users::create_user,
        super::admin::users::update_user,
        super::admin::users::reset_user_password,
        super::admin::users::delete_user,
        super::admin::tokens::list_mcp_tokens,
        super::admin::tokens::create_mcp_token,
        super::admin::tokens::delete_mcp_token,
        super::admin::tokens::update_mcp_token,
        super::admin::applications::list_applications,
        super::admin::applications::create_application,
        super::admin::applications::update_application,
        super::admin::applications::list_application_tokens,
        super::admin::applications::create_application_token,
        super::admin::applications::delete_application_token,
        super::admin::applications::update_application_token,
        super::admin::workspace_bindings::list_workspace_bindings,
        super::admin::workspace_bindings::get_workspace_binding,
        super::admin::workspace_bindings::archive_workspace_binding,
        super::admin::workspace_bindings::restore_workspace_binding,
        super::admin::workspace_bindings::reset_workspace_binding,
        super::admin::workspace_bindings::acquire_workspace_binding_lease,
        super::admin::workspace_bindings::release_workspace_binding_lease,
        super::internal::lease::acquire_workspace_binding_lease,
        super::internal::lease::release_workspace_binding_lease,
        super::internal::lease::acquire_resource_workspace_lease,
        super::internal::lease::release_resource_workspace_lease,
        super::internal::lease::takeover_resource_workspace_lease,
        super::internal::lease::resource_workspace_lease_status,
        super::internal::application::application_capabilities,
        super::internal::application::sync_resource_workspace_git,
        super::internal::execution_environments::handlers::ensure_execution_environment,
        super::internal::execution_environments::handlers::invalidate_execution_environment,
        super::internal::execution_environments::lifecycle::provision_execution_environment,
        super::internal::execution_environments::lifecycle::stop_execution_environment,
        super::internal::execution_environments::lifecycle::replace_execution_environment,
        super::internal::execution_environments::lifecycle::execution_environment_status,
        super::internal::execution_environments::tasks::admit_native_task_handler,
        super::internal::execution_environments::tasks::find_native_task_handler,
        super::internal::execution_environments::tasks::cancel_native_task_handler,
        super::internal::execution_environments::tasks::release_native_task_handler,
        super::internal::execution_environments::tasks::seeds::seed_native_task_handler,
        super::internal::execution_environments::tasks::seeds::find_native_task_seed_handler,
        super::internal::execution_environments::tasks::prepare::prepare_native_task_handler,
        super::internal::execution_environments::tasks::prepare::find_native_task_preparation_handler,
        super::internal::execution_environments::service::handler::execution_environment_service_get,
        super::internal::execution_environments::service::handler::execution_environment_service_post,
        super::internal::execution_environments::service::handler::execution_environment_service_put,
        super::internal::execution_environments::service::handler::execution_environment_service_patch,
        super::internal::execution_environments::service::handler::execution_environment_service_delete,
        super::internal::execution_environments::service::handler::execution_environment_service_head,
        super::internal::execution_environments::service::handler::execution_environment_service_options,
        super::internal::runner_manager::runner_manager_config,
        super::internal::runner_manager::next_runner_job,
        super::internal::runner_manager::complete_runner_job,
        super::internal::runner_manager::report_workspace_runner,
        super::internal::runner_manager_environment_jobs::task_seed_for_preparation_job,
        super::internal::runner_manager_service_relay::runner_manager_service_relay,
        super::admin::operations::list_audit_logs,
        super::admin::operations::list_workspace_runners,
        super::admin::operations::list_runner_sessions,
        super::admin::operations::operations_summary,
        super::admin::runner_managers::list_runner_managers,
        super::admin::runner_managers::create_runner_manager,
        super::admin::runner_managers::update_runner_manager,
        super::tools::session::shell,
        super::tools::session::shell_as_admin,
        super::tools::session::write_stdin,
        super::tools::session::write_stdin_as_admin,
        super::tools::session::cancel_session,
        super::tools::session::cancel_session_as_admin,
        super::tools::session::apply_patch,
        super::tools::session::apply_patch_as_admin,
        super::tools::readonly::read,
        super::tools::readonly::read_as_admin,
        super::tools::readonly::glob,
        super::tools::readonly::glob_as_admin,
        super::tools::readonly::grep,
        super::tools::readonly::grep_as_admin,
        super::tools::readonly::stat,
        super::tools::readonly::stat_as_admin,
    ),
    components(
        schemas(
            AuthLoginRequest,
             AuthMeResponse,
             ChangePasswordRequest,
            CreateUserRequest,
            CreateMcpTokenRequest,
            CreateMcpTokenResponse,
            UpdateMcpTokenRequest,
            CreateApplicationRequest,
            UpdateApplicationRequest,
            ApplicationResponse,
            CreateApplicationTokenRequest,
            CreateApplicationTokenResponse,
            UpdateApplicationTokenRequest,
            ApplicationTokenResponse,
             WorkspaceBindingResponse,
             ApplicationCapabilitiesResponse,
             RunnerManagerResponse,
             CreateRunnerManagerRequest,
             CreateRunnerManagerResponse,
             UpdateRunnerManagerRequest,
             GitWorkspaceSyncRequest,
             GitWorkspaceSyncResponse,
            crate::api::internal::execution_environments::EnsureExecutionEnvironmentRequest,
            crate::api::internal::execution_environments::InvalidateExecutionEnvironmentRequest,
            crate::api::internal::execution_environments::ProvisionExecutionEnvironmentRequest,
            crate::api::internal::execution_environments::StopExecutionEnvironmentRequest,
            crate::api::internal::execution_environments::ReplaceExecutionEnvironmentRequest,
            crate::api::internal::execution_environments::tasks::types::AdmitNativeTaskRequest,
            crate::api::internal::execution_environments::tasks::types::CancelNativeTaskRequest,
            crate::api::internal::execution_environments::tasks::types::ReleaseNativeTaskRequest,
            crate::api::internal::execution_environments::tasks::types::NativeTaskDescriptor,
            crate::api::internal::execution_environments::tasks::seeds::types::SeedNativeTaskRequest,
            crate::api::internal::execution_environments::tasks::seeds::types::SeedFileInput,
            crate::api::internal::execution_environments::tasks::seeds::types::NativeTaskSeedDescriptor,
            crate::api::internal::execution_environments::tasks::seeds::types::NativeTaskSeedContent,
            crate::api::internal::execution_environments::tasks::prepare::types::PrepareNativeTaskRequestApi,
            crate::api::internal::execution_environments::tasks::prepare::types::NativeTaskPreparationDescriptor,
            crate::db::execution_environments::EnvironmentOperationDescriptor,
            crate::db::execution_environments::OperationDescriptor,
            ExecutionEnvironment,
            WorkspaceLeaseRequest,
            WorkspaceLeaseReleaseRequest,
            WorkspaceLeaseTakeoverRequest,
            WorkspaceLeaseTakeoverResponse,
            WorkspaceLeaseTakeoverConflict,
            WorkspaceLeaseStatusResponse,
            WorkspaceLeaseCancellationOutcome,
            UpdateUserRequest,
            McpTokenResponse,
            ResetPasswordRequest,
            AuditLogResponse,
            WorkspaceRunnerResponse,
             RunnerSessionResponse,
             runner_protocol::RunnerLifecycleEvent,
            OperationsSummary,
             UserResponse,
            Page<UserResponse>,
            Page<AuditLogResponse>,
            Page<ApplicationResponse>,
            Page<ApplicationTokenResponse>,
            Page<McpTokenResponse>,
            Page<RunnerManagerResponse>,
            Page<WorkspaceBindingResponse>,
            Page<WorkspaceRunnerResponse>,
            Page<RunnerSessionResponse>,
            EmptyParams,
            ShellParams,
            WriteStdinParams,
            CancelSessionParams,
            ApplyPatchParams,
            ReadParams,
            GrepParams,
            GlobParams,
            StatPathParams,
            ShellToolOutput,
            ApplyPatchOutput,
            CancelSessionOutput,
            ReadOutput,
            DirectoryEntry,
            SearchMatch,
            GrepOutput,
            StatPathOutput,
            ErrorResponse
        )
    ),
    tags(
        (name = "auth", description = "Web authentication"),
        (name = "admin-users", description = "Admin user management"),
        (name = "internal", description = "Internal application and runner-manager API"),
        (name = "tools", description = "Workspace tool API for the authenticated user"),
        (name = "admin-tools", description = "Admin-targeted workspace tool API")
    ),
    modifiers(&ApplicationBearer)
)]
struct ApiDoc;

/// Declares the application bearer security scheme in `components`. Operations
/// opt into it one by one so public routes (login, health) stay unauthenticated.
struct ApplicationBearer;

impl Modify for ApplicationBearer {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi
            .components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                APPLICATION_BEARER_SCHEME,
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .description(Some(
                            "Bearer token of the calling application, checked by the internal API guard.",
                        ))
                        .build(),
                ),
            );
    }
}

pub fn openapi_document() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

#[cfg(test)]
#[path = "openapi_security_tests.rs"]
mod security_tests;

#[cfg(test)]
#[path = "openapi_security_expectations.rs"]
mod security_expectations;
