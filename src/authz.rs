use std::collections::BTreeSet;

use cloudiful_authz::{
    Action, Attenuation, Authority, Authorizer, AuthorizerConfig, AuthzRequest, Capability,
    CredentialContext, Decision, DomainId, FactsRevision, Grant, PrincipalRef, ResolvedRoles,
    ResourceKind, ResourceRef, RoleId, RoleSpec, ServerContext, ServiceId,
};

#[cfg(test)]
use cloudiful_authz::PrincipalKind;

use crate::{
    actor::ActorContext,
    db::types::UserRecord,
    policy::{ALL_SCOPES, WORKSPACE_PATCH, WORKSPACE_READ, WORKSPACE_SEARCH, WORKSPACE_SHELL},
};

pub const DOMAIN: &str = "desk-foreman";
pub const AUDIENCE: &str = "desk-foreman";
pub const WORKSPACE_RESOURCE: &str = "workspace";
pub const CONTROL_RESOURCE: &str = "control";
pub const ADMIN_ROLE: &str = "admin";

/// Enumerated workspace actions. The scope strings remain wire/compatibility
/// inputs and map to these actions at the existing boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceAction {
    Read,
    Search,
    Shell,
    Patch,
}

impl WorkspaceAction {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 4] = [Self::Read, Self::Search, Self::Shell, Self::Patch];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Search => "search",
            Self::Shell => "shell",
            Self::Patch => "patch",
        }
    }

    pub(crate) fn from_scope(scope: &str) -> Option<Self> {
        match scope {
            WORKSPACE_READ => Some(Self::Read),
            WORKSPACE_SEARCH => Some(Self::Search),
            WORKSPACE_SHELL => Some(Self::Shell),
            WORKSPACE_PATCH => Some(Self::Patch),
            _ => None,
        }
    }
}

/// Enumerated administrator actions. There is no wildcard and no workspace
/// authority: the workspace scope decisions stay on the workspace resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlAction {
    User,
    Token,
    Application,
    Binding,
    Lease,
    GitUpstream,
    Audit,
    Runtime,
    RunnerManager,
    Workspace,
}

impl ControlAction {
    pub const ALL: [Self; 10] = [
        Self::User,
        Self::Token,
        Self::Application,
        Self::Binding,
        Self::Lease,
        Self::GitUpstream,
        Self::Audit,
        Self::Runtime,
        Self::RunnerManager,
        Self::Workspace,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Token => "token",
            Self::Application => "application",
            Self::Binding => "binding",
            Self::Lease => "lease",
            Self::GitUpstream => "git_upstream",
            Self::Audit => "audit",
            Self::Runtime => "runtime",
            Self::RunnerManager => "runner_manager",
            Self::Workspace => "workspace",
        }
    }
}

/// Trusted authorization facts for one actor or one application request.
///
/// The facts come from the already-verified authentication path: the
/// principal is the actor identity constructed there, roles come from the
/// application subject scopes or the internal-human role set, and credential
/// and server attenuations are explicit grant sets. They are never derived
/// from the flat compatibility [`crate::policy::AccessPolicy`] view.
#[derive(Clone, Debug)]
pub(crate) struct WorkspaceFacts {
    pub principal: PrincipalRef,
    pub roles: BTreeSet<RoleId>,
    pub credential: Option<Attenuation>,
    pub server: Attenuation,
}

/// One typed authorization operation. The variants carry the verified facts
/// each decision actually has: workspace requests carry an actor's fact set,
/// while control requests carry the current user record because admin gates
/// run before any workspace actor exists.
pub(crate) enum Operation<'a> {
    Workspace {
        facts: &'a WorkspaceFacts,
        action: WorkspaceAction,
    },
    Control {
        user: &'a UserRecord,
        action: ControlAction,
    },
}

/// The single local authorization decision entry. It maps the typed operation
/// to the core's operation-only request and registered live authority.
pub(crate) fn check(authorizer: &Authorizer, operation: Operation<'_>) -> Decision {
    match operation {
        Operation::Workspace { facts, action } => {
            let request = workspace_request(facts, action);
            authorizer.authorize(&request, Authority::Registered(&resolved(facts)))
        }
        Operation::Control { user, action } => {
            let request = control_request(action);
            authorizer.authorize(&request, Authority::Registered(&control_resolved(user)))
        }
    }
}

pub async fn build() -> Result<Authorizer, cloudiful_authz::InitError> {
    let mut roles = workspace_roles();
    roles.push(RoleSpec {
        id: RoleId::new(ADMIN_ROLE),
        capabilities: ControlAction::ALL
            .into_iter()
            .map(|action| Capability::new(CONTROL_RESOURCE, action.as_str()))
            .collect(),
    });
    Authorizer::new(AuthorizerConfig::new(FactsRevision(1), [DOMAIN], roles)).await
}

pub(crate) fn internal_user_facts(
    principal_id: &str,
    token_scopes: Option<&[String]>,
    server_scopes: &[String],
) -> WorkspaceFacts {
    WorkspaceFacts {
        principal: PrincipalRef::human(principal_id),
        roles: ALL_SCOPES.iter().map(|scope| RoleId::new(*scope)).collect(),
        credential: token_scopes.map(grants_for_scopes),
        server: grants_for_scopes(server_scopes),
    }
}

pub(crate) fn application_facts(
    principal_id: &str,
    application_scopes: &[String],
    token_scopes: &[String],
    server_scopes: &[String],
) -> WorkspaceFacts {
    WorkspaceFacts {
        principal: PrincipalRef::application(principal_id),
        roles: roles_for_scopes(application_scopes),
        credential: Some(grants_for_scopes(token_scopes)),
        server: grants_for_scopes(server_scopes),
    }
}

#[cfg(test)]
pub(crate) fn fixture_facts(principal_id: &str, kind: PrincipalKind) -> WorkspaceFacts {
    WorkspaceFacts {
        principal: PrincipalRef::new(kind, principal_id),
        roles: ALL_SCOPES.iter().map(|scope| RoleId::new(*scope)).collect(),
        credential: None,
        server: grants_for_scopes(&all_scope_strings()),
    }
}

/// Existing aggregate capability-view query. Every item is derived through
/// [`check`]; this is not a second permission engine.
pub(crate) fn scope_decisions(authorizer: &Authorizer, facts: &WorkspaceFacts) -> Vec<String> {
    let mut scopes = ALL_SCOPES
        .into_iter()
        .filter(|scope| {
            let action = WorkspaceAction::from_scope(scope).expect("registered workspace scope");
            check(authorizer, Operation::Workspace { facts, action }).is_allowed()
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    scopes.sort();
    scopes
}

pub(crate) fn actor_facts(actor: &ActorContext) -> &WorkspaceFacts {
    &actor.workspace_facts
}

fn workspace_roles() -> Vec<RoleSpec> {
    ALL_SCOPES
        .into_iter()
        .map(|scope| RoleSpec {
            id: RoleId::new(scope),
            capabilities: BTreeSet::from([Capability::new(
                WORKSPACE_RESOURCE,
                WorkspaceAction::from_scope(scope)
                    .expect("known workspace scope")
                    .as_str(),
            )]),
        })
        .collect()
}

fn roles_for_scopes(scopes: &[String]) -> BTreeSet<RoleId> {
    scopes
        .iter()
        .filter(|scope| WorkspaceAction::from_scope(scope).is_some())
        .map(RoleId::new)
        .collect()
}

fn grants_for_scopes(scopes: &[String]) -> Attenuation {
    Attenuation::grants(scopes.iter().filter_map(|scope| {
        let action = WorkspaceAction::from_scope(scope)?;
        Some(Grant::new(DOMAIN, WORKSPACE_RESOURCE, action.as_str()))
    }))
}

fn workspace_request(facts: &WorkspaceFacts, action: WorkspaceAction) -> AuthzRequest {
    AuthzRequest {
        resource: workspace_resource(),
        action: Action::new(action.as_str()),
        audience: ServiceId::new(AUDIENCE),
        credential: facts.credential.clone().map(|grants| CredentialContext {
            principal: facts.principal.clone(),
            audience: ServiceId::new(AUDIENCE),
            grants,
        }),
        server: Some(ServerContext {
            grants: facts.server.clone(),
        }),
    }
}

fn control_request(action: ControlAction) -> AuthzRequest {
    AuthzRequest {
        resource: ResourceRef {
            kind: ResourceKind::new(CONTROL_RESOURCE),
            id: None,
            domain: DomainId::new(DOMAIN),
        },
        action: Action::new(action.as_str()),
        audience: ServiceId::new(AUDIENCE),
        credential: None,
        server: None,
    }
}

fn workspace_resource() -> ResourceRef {
    ResourceRef {
        kind: ResourceKind::new(WORKSPACE_RESOURCE),
        id: None,
        domain: DomainId::new(DOMAIN),
    }
}

fn resolved(facts: &WorkspaceFacts) -> ResolvedRoles {
    ResolvedRoles::new(facts.principal.clone(), DOMAIN, facts.roles.iter().cloned())
}

fn control_resolved(user: &UserRecord) -> ResolvedRoles {
    let principal = PrincipalRef::human(format!("user:{}", user.user_id));
    let roles = if user.is_admin {
        BTreeSet::from([RoleId::new(ADMIN_ROLE)])
    } else {
        BTreeSet::new()
    };
    ResolvedRoles::new(principal, DOMAIN, roles)
}

#[cfg(test)]
fn all_scope_strings() -> Vec<String> {
    ALL_SCOPES
        .iter()
        .map(|scope| (*scope).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::policy::{ALL_SCOPES, AccessPolicy, ResourceLimits};

    fn user(is_admin: bool) -> UserRecord {
        UserRecord {
            user_id: 7,
            login_name: "u".to_string(),
            password_hash: "h".to_string(),
            display_name: "U".to_string(),
            email: "u@example.invalid".to_string(),
            timezone: "UTC".to_string(),
            workspace_root: None,
            is_admin,
            is_active: true,
            must_change_password: false,
            deleted_at: None,
            last_login_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn scopes(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn allowed(authorizer: &Authorizer, facts: &WorkspaceFacts, action: WorkspaceAction) -> bool {
        check(authorizer, Operation::Workspace { facts, action }).is_allowed()
    }

    fn allowed_control(authorizer: &Authorizer, user: &UserRecord, action: ControlAction) -> bool {
        check(authorizer, Operation::Control { user, action }).is_allowed()
    }

    #[tokio::test]
    async fn workspace_roles_cover_every_existing_scope() {
        let roles = workspace_roles();
        assert_eq!(roles.len(), ALL_SCOPES.len());
        for scope in ALL_SCOPES {
            assert!(roles.iter().any(|role| role.id == RoleId::new(scope)));
        }
    }

    #[tokio::test]
    async fn four_scopes_are_intersected_across_all_layers() {
        let authorizer = build().await.expect("authorizer");
        let facts = application_facts(
            "application:1:user:default",
            &scopes(&[WORKSPACE_READ, WORKSPACE_SHELL]),
            &scopes(&[WORKSPACE_SHELL, WORKSPACE_PATCH]),
            &scopes(&[WORKSPACE_SHELL, WORKSPACE_READ]),
        );

        assert!(allowed(&authorizer, &facts, WorkspaceAction::Shell));
        assert!(!allowed(&authorizer, &facts, WorkspaceAction::Read));
        assert!(!allowed(&authorizer, &facts, WorkspaceAction::Patch));
        assert!(!allowed(&authorizer, &facts, WorkspaceAction::Search));
    }

    #[tokio::test]
    async fn unknown_scope_strings_map_to_no_workspace_action() {
        let authorizer = build().await.expect("authorizer");
        let facts = application_facts(
            "application:1:user:default",
            &scopes(&[WORKSPACE_READ, "workspace.legacy"]),
            &scopes(&[WORKSPACE_READ, "workspace.unknown"]),
            &scopes(&[WORKSPACE_READ, "workspace.other"]),
        );

        assert_eq!(WorkspaceAction::from_scope("workspace.legacy"), None);
        assert!(allowed(&authorizer, &facts, WorkspaceAction::Read));
        assert_eq!(scope_decisions(&authorizer, &facts), vec![WORKSPACE_READ]);
    }

    #[tokio::test]
    async fn every_empty_explicit_layer_denies() {
        let authorizer = build().await.expect("authorizer");
        let read = scopes(&[WORKSPACE_READ]);

        let empty_roles = application_facts("application:1:user:default", &[], &read, &read);
        assert!(!allowed(&authorizer, &empty_roles, WorkspaceAction::Read));

        let empty_credential = application_facts("application:1:user:default", &read, &[], &read);
        assert!(!allowed(
            &authorizer,
            &empty_credential,
            WorkspaceAction::Read
        ));

        let empty_server = application_facts("application:1:user:default", &read, &read, &[]);
        assert!(!allowed(&authorizer, &empty_server, WorkspaceAction::Read));

        let token_cannot_grant_role = application_facts(
            "application:1:user:default",
            &[],
            &scopes(&[WORKSPACE_SHELL, WORKSPACE_PATCH]),
            &scopes(&[WORKSPACE_SHELL, WORKSPACE_PATCH]),
        );
        assert!(!allowed(
            &authorizer,
            &token_cannot_grant_role,
            WorkspaceAction::Shell
        ));
        assert!(!allowed(
            &authorizer,
            &token_cannot_grant_role,
            WorkspaceAction::Patch
        ));
    }

    #[tokio::test]
    async fn absent_session_credential_differs_from_an_empty_token() {
        let authorizer = build().await.expect("authorizer");
        let session = internal_user_facts("user:7", None, &scopes(&[WORKSPACE_READ]));
        assert!(allowed(&authorizer, &session, WorkspaceAction::Read));

        let empty_token = internal_user_facts("user:7", Some(&[]), &scopes(&[WORKSPACE_READ]));
        assert!(!allowed(&authorizer, &empty_token, WorkspaceAction::Read));
    }

    #[tokio::test]
    async fn admin_role_allows_only_the_enumerated_control_actions() {
        let authorizer = build().await.expect("authorizer");
        let admin = user(true);
        for action in ControlAction::ALL {
            assert!(
                allowed_control(&authorizer, &admin, action),
                "admin must allow {}",
                action.as_str()
            );
        }

        let non_admin = user(false);
        for action in ControlAction::ALL {
            assert!(
                !allowed_control(&authorizer, &non_admin, action),
                "non-admin must deny {}",
                action.as_str()
            );
        }
    }

    #[tokio::test]
    async fn admin_role_grants_no_workspace_permission() {
        let authorizer = build().await.expect("authorizer");
        let facts = WorkspaceFacts {
            principal: PrincipalRef::human("user:7"),
            roles: BTreeSet::from([RoleId::new(ADMIN_ROLE)]),
            credential: None,
            server: grants_for_scopes(&scopes(&[
                WORKSPACE_READ,
                WORKSPACE_SEARCH,
                WORKSPACE_SHELL,
                WORKSPACE_PATCH,
            ])),
        };
        for action in WorkspaceAction::ALL {
            assert!(
                !allowed(&authorizer, &facts, action),
                "admin must not imply {}",
                action.as_str()
            );
        }
    }

    #[tokio::test]
    async fn flat_compatibility_view_matches_engine_decisions() {
        let authorizer = build().await.expect("authorizer");
        let applications = [
            scopes(&[
                WORKSPACE_READ,
                WORKSPACE_SEARCH,
                WORKSPACE_SHELL,
                WORKSPACE_PATCH,
            ]),
            scopes(&[WORKSPACE_READ, WORKSPACE_SHELL]),
            scopes(&[]),
        ];
        let tokens = [
            scopes(&[
                WORKSPACE_READ,
                WORKSPACE_SEARCH,
                WORKSPACE_SHELL,
                WORKSPACE_PATCH,
            ]),
            scopes(&[WORKSPACE_SEARCH, WORKSPACE_PATCH]),
            scopes(&[]),
        ];
        let servers = [
            scopes(&[
                WORKSPACE_READ,
                WORKSPACE_SEARCH,
                WORKSPACE_SHELL,
                WORKSPACE_PATCH,
            ]),
            scopes(&[WORKSPACE_READ, WORKSPACE_PATCH]),
            scopes(&[]),
        ];

        for application in &applications {
            for token in &tokens {
                for server in &servers {
                    let policy = AccessPolicy::from_layers(
                        application,
                        token,
                        server,
                        ResourceLimits::unrestricted(false),
                        ResourceLimits::unrestricted(true),
                        ResourceLimits::unrestricted(true),
                    );
                    let facts =
                        application_facts("application:1:user:default", application, token, server);
                    let engine = scope_decisions(&authorizer, &facts);
                    assert_eq!(policy.scopes, engine);
                    for scope in ALL_SCOPES {
                        let action = WorkspaceAction::from_scope(scope).expect("known scope");
                        assert_eq!(policy.allows(scope), allowed(&authorizer, &facts, action));
                    }
                }
            }
        }
    }
}
