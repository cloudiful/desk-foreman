use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const WORKSPACE_READ: &str = "workspace.read";
pub const WORKSPACE_SEARCH: &str = "workspace.search";
pub const WORKSPACE_SHELL: &str = "workspace.shell";
pub const WORKSPACE_PATCH: &str = "workspace.patch";
pub const ALL_SCOPES: [&str; 4] = [
    WORKSPACE_READ,
    WORKSPACE_SEARCH,
    WORKSPACE_SHELL,
    WORKSPACE_PATCH,
];

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
pub struct ResourceLimits {
    pub max_timeout_ms: Option<u64>,
    pub max_output_bytes: Option<usize>,
    pub max_file_bytes: Option<usize>,
    pub max_sessions: Option<usize>,
    pub network_enabled: bool,
}

impl ResourceLimits {
    pub fn unrestricted(network_enabled: bool) -> Self {
        Self {
            network_enabled,
            ..Self::default()
        }
    }

    pub fn minimum(self, other: Self) -> Self {
        Self {
            max_timeout_ms: min_optional(self.max_timeout_ms, other.max_timeout_ms),
            max_output_bytes: min_optional(self.max_output_bytes, other.max_output_bytes),
            max_file_bytes: min_optional(self.max_file_bytes, other.max_file_bytes),
            max_sessions: min_optional(self.max_sessions, other.max_sessions),
            network_enabled: self.network_enabled && other.network_enabled,
        }
    }
}

fn min_optional<T: Ord>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AccessPolicy {
    pub scopes: Vec<String>,
    pub limits: ResourceLimits,
}

impl AccessPolicy {
    pub fn new(scopes: impl IntoIterator<Item = String>, limits: ResourceLimits) -> Self {
        let allowed = ALL_SCOPES.into_iter().collect::<HashSet<_>>();
        let mut scopes = scopes
            .into_iter()
            .filter(|scope| allowed.contains(scope.as_str()))
            .collect::<Vec<_>>();
        scopes.sort();
        scopes.dedup();
        Self { scopes, limits }
    }

    pub fn allows(&self, scope: &str) -> bool {
        self.scopes.iter().any(|candidate| candidate == scope)
    }

    pub fn intersect_scopes(
        application_scopes: &[String],
        token_scopes: &[String],
        server_scopes: &[String],
    ) -> Vec<String> {
        let token = token_scopes.iter().collect::<HashSet<_>>();
        let server = server_scopes.iter().collect::<HashSet<_>>();
        application_scopes
            .iter()
            .filter(|scope| token.contains(scope) && server.contains(scope))
            .cloned()
            .collect()
    }

    pub fn from_layers(
        application_scopes: &[String],
        token_scopes: &[String],
        server_scopes: &[String],
        application_limits: ResourceLimits,
        token_limits: ResourceLimits,
        server_limits: ResourceLimits,
    ) -> Self {
        Self::new(
            Self::intersect_scopes(application_scopes, token_scopes, server_scopes),
            application_limits
                .minimum(token_limits)
                .minimum(server_limits),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{AccessPolicy, ResourceLimits};

    #[test]
    fn scopes_are_intersected_across_all_layers() {
        let policy = AccessPolicy::from_layers(
            &["workspace.read".into(), "workspace.shell".into()],
            &["workspace.shell".into(), "workspace.patch".into()],
            &["workspace.shell".into(), "workspace.read".into()],
            ResourceLimits::unrestricted(false),
            ResourceLimits::unrestricted(true),
            ResourceLimits::unrestricted(true),
        );
        assert_eq!(policy.scopes, vec!["workspace.shell"]);
        assert!(!policy.limits.network_enabled);
    }

    #[test]
    fn resource_limits_use_the_smallest_value() {
        let result = ResourceLimits {
            max_timeout_ms: Some(100),
            max_output_bytes: None,
            max_file_bytes: Some(20),
            max_sessions: Some(4),
            network_enabled: true,
        }
        .minimum(ResourceLimits {
            max_timeout_ms: Some(200),
            max_output_bytes: Some(10),
            max_file_bytes: Some(10),
            max_sessions: Some(2),
            network_enabled: false,
        });
        assert_eq!(result.max_timeout_ms, Some(100));
        assert_eq!(result.max_output_bytes, Some(10));
        assert_eq!(result.max_file_bytes, Some(10));
        assert_eq!(result.max_sessions, Some(2));
        assert!(!result.network_enabled);
    }
}
