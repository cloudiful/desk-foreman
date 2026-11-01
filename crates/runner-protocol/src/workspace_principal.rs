use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::RunnerOwner;

pub fn application_principal_owner(
    application_id: i64,
    workspace_binding_id: i64,
    external_user_id: &str,
    workspace_key: &str,
) -> Result<RunnerOwner, &'static str> {
    if application_id <= 0 || workspace_binding_id <= 0 {
        return Err("application and workspace binding IDs must be positive");
    }
    if external_user_id.is_empty()
        || external_user_id.len() > 1024
        || workspace_key.is_empty()
        || workspace_key.len() > 1024
    {
        return Err("principal fields must be nonempty and at most 1024 bytes");
    }
    let mut hash = Sha256::new();
    hash.update(b"desk-foreman:application-principal:v1\0");
    for part in [
        application_id.to_string().as_bytes(),
        external_user_id.as_bytes(),
        workspace_key.as_bytes(),
    ] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    Ok(RunnerOwner::ApplicationPrincipal {
        application_id,
        workspace_binding_id,
        principal_digest: hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}
use utoipa::ToSchema;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PrincipalAdmissionRequest {
    #[serde(deserialize_with = "strict_admission_owner")]
    pub owner: RunnerOwner,
    pub container_id: String,
}

fn strict_admission_owner<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<RunnerOwner, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let owner: RunnerOwner =
        serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)?;
    if serde_json::to_value(&owner).map_err(serde::de::Error::custom)? != value {
        return Err(serde::de::Error::custom("invalid owner fields"));
    }
    Ok(owner)
}

impl PrincipalAdmissionRequest {
    pub fn is_valid(&self) -> bool {
        (StopWorkspacePrincipalRequest {
            owner: self.owner.clone(),
            principal_id: "identity".into(),
        })
        .is_valid()
            && !self.container_id.is_empty()
            && self.container_id.len() <= 256
            && !self
                .container_id
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StopWorkspacePrincipalRequest {
    pub owner: RunnerOwner,
    pub principal_id: String,
}

pub const MAX_CANONICAL_PRINCIPAL_KEY_BYTES: usize = 4096;

impl StopWorkspacePrincipalRequest {
    pub fn is_valid(&self) -> bool {
        matches!(&self.owner, RunnerOwner::ApplicationPrincipal { application_id, workspace_binding_id, principal_digest }
            if *application_id > 0 && *workspace_binding_id > 0 && principal_digest.len() == 64
            && principal_digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
            && !self.principal_id.is_empty()
            && self.principal_id.len() <= MAX_CANONICAL_PRINCIPAL_KEY_BYTES
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StopWorkspacePrincipalAck {
    pub state: StopWorkspacePrincipalState,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopWorkspacePrincipalState {
    Stopped,
    Conflict,
    Busy,
    Unsupported,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CleanupWorkspaceBindingPrincipalsRequest {
    pub workspace_binding_id: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePrincipalCleanupResult {
    pub owner: RunnerOwner,
    pub state: StopWorkspacePrincipalState,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CleanupWorkspaceBindingPrincipalsAck {
    pub state: StopWorkspacePrincipalState,
    pub owners: Vec<WorkspacePrincipalCleanupResult>,
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_cleanup_wire_roundtrips_and_rejects_unknown_fields() {
        let request = CleanupWorkspaceBindingPrincipalsRequest {
            workspace_binding_id: 7,
        };
        assert_eq!(
            serde_json::from_value::<CleanupWorkspaceBindingPrincipalsRequest>(
                serde_json::to_value(&request).unwrap()
            )
            .unwrap(),
            request
        );
        assert!(
            serde_json::from_value::<CleanupWorkspaceBindingPrincipalsRequest>(
                serde_json::json!({"workspace_binding_id":7,"extra":true})
            )
            .is_err()
        );
        for state in [
            StopWorkspacePrincipalState::Stopped,
            StopWorkspacePrincipalState::Conflict,
            StopWorkspacePrincipalState::Busy,
            StopWorkspacePrincipalState::Unsupported,
            StopWorkspacePrincipalState::Failed,
        ] {
            let ack = CleanupWorkspaceBindingPrincipalsAck {
                state: state.clone(),
                owners: vec![WorkspacePrincipalCleanupResult {
                    owner: application_principal_owner(1, 7, "u", "w").unwrap(),
                    state,
                    detail: Some("detail".into()),
                }],
                detail: None,
            };
            assert_eq!(
                serde_json::from_value::<CleanupWorkspaceBindingPrincipalsAck>(
                    serde_json::to_value(&ack).unwrap()
                )
                .unwrap(),
                ack
            );
        }
    }

    #[test]
    fn principal_identity_is_bounded_unambiguous_and_private() {
        let owner = application_principal_owner(1, 2, "alice:private", "workspace").unwrap();
        assert_eq!(
            owner,
            application_principal_owner(1, 2, "alice:private", "workspace").unwrap()
        );
        for other in [
            application_principal_owner(3, 2, "alice:private", "workspace"),
            application_principal_owner(1, 3, "alice:private", "workspace"),
            application_principal_owner(1, 2, "bob", "workspace"),
            application_principal_owner(1, 2, "alice:private", "other"),
        ] {
            assert_ne!(owner, other.unwrap());
        }
        assert_ne!(
            application_principal_owner(1, 2, "a:b", "c").unwrap(),
            application_principal_owner(1, 2, "a", "b:c").unwrap()
        );
        let key = owner.stable_key();
        assert!(!key.contains("alice"));
        let digest = key.rsplit(':').next().unwrap();
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_eq!(
            owner.container_name(),
            format!("desk-foreman-runner-{}", key.replace([':', '/'], "-"))
        );
        assert_eq!(
            serde_json::from_value::<RunnerOwner>(serde_json::to_value(&owner).unwrap()).unwrap(),
            owner
        );
        for result in [
            application_principal_owner(0, 2, "a", "b"),
            application_principal_owner(1, 0, "a", "b"),
            application_principal_owner(1, 2, "", "b"),
            application_principal_owner(1, 2, "a", ""),
            application_principal_owner(1, 2, &"é".repeat(513), "b"),
            application_principal_owner(1, 2, "a", &"x".repeat(1025)),
        ] {
            assert!(result.is_err());
        }
    }

    #[test]
    fn existing_owner_keys_and_serde_are_unchanged() {
        for (owner, key, json) in [
            (
                RunnerOwner::InternalUser { user_id: 7 },
                "user:7",
                serde_json::json!({"InternalUser":{"user_id":7}}),
            ),
            (
                RunnerOwner::WorkspaceBinding {
                    workspace_binding_id: 9,
                },
                "workspace_binding:9",
                serde_json::json!({"WorkspaceBinding":{"workspace_binding_id":9}}),
            ),
            (
                RunnerOwner::ExecutionEnvironment {
                    environment_id: "env".into(),
                    generation: 3,
                },
                "execution_environment:env:3",
                serde_json::json!({"ExecutionEnvironment":{"environment_id":"env","generation":3}}),
            ),
        ] {
            assert_eq!(owner.stable_key(), key);
            assert_eq!(serde_json::to_value(owner).unwrap(), json);
        }
    }

    #[test]
    fn stop_request_requires_principal_owner_and_bounded_key() {
        let owner = application_principal_owner(1, 2, "user", "workspace").unwrap();
        let valid = StopWorkspacePrincipalRequest {
            owner: owner.clone(),
            principal_id: "p".into(),
        };
        assert!(valid.is_valid());
        for bad in [
            StopWorkspacePrincipalRequest {
                owner: RunnerOwner::InternalUser { user_id: 1 },
                principal_id: "p".into(),
            },
            StopWorkspacePrincipalRequest {
                owner: RunnerOwner::ApplicationPrincipal {
                    application_id: 0,
                    workspace_binding_id: 2,
                    principal_digest: "a".repeat(64),
                },
                principal_id: "p".into(),
            },
            StopWorkspacePrincipalRequest {
                owner: RunnerOwner::ApplicationPrincipal {
                    application_id: 1,
                    workspace_binding_id: 0,
                    principal_digest: "a".repeat(64),
                },
                principal_id: "p".into(),
            },
            StopWorkspacePrincipalRequest {
                owner: RunnerOwner::ApplicationPrincipal {
                    application_id: 1,
                    workspace_binding_id: 2,
                    principal_digest: "A".repeat(64),
                },
                principal_id: "p".into(),
            },
            StopWorkspacePrincipalRequest {
                owner: RunnerOwner::ApplicationPrincipal {
                    application_id: 1,
                    workspace_binding_id: 2,
                    principal_digest: "a".repeat(63),
                },
                principal_id: "p".into(),
            },
            StopWorkspacePrincipalRequest {
                owner: owner.clone(),
                principal_id: String::new(),
            },
            StopWorkspacePrincipalRequest {
                owner,
                principal_id: "é".repeat(MAX_CANONICAL_PRINCIPAL_KEY_BYTES / 2 + 1),
            },
        ] {
            assert!(!bad.is_valid());
        }
    }

    #[test]
    fn maximum_component_canonical_key_fits_stop_request() {
        let external = "e".repeat(1024);
        let workspace = "w".repeat(1024);
        let owner = application_principal_owner(i64::MAX, 1, &external, &workspace).unwrap();
        let key = format!("application:{}:{}:{}", i64::MAX, external, workspace);
        assert!(key.len() > 1024 && key.len() < MAX_CANONICAL_PRINCIPAL_KEY_BYTES);
        let request = StopWorkspacePrincipalRequest {
            owner,
            principal_id: key,
        };
        assert!(request.is_valid());
        assert_eq!(
            serde_json::from_str::<StopWorkspacePrincipalRequest>(
                &serde_json::to_string(&request).unwrap()
            )
            .unwrap(),
            request
        );
        let at_limit = StopWorkspacePrincipalRequest {
            principal_id: "x".repeat(MAX_CANONICAL_PRINCIPAL_KEY_BYTES),
            ..request.clone()
        };
        assert!(at_limit.is_valid());
        let over_limit = StopWorkspacePrincipalRequest {
            principal_id: "x".repeat(MAX_CANONICAL_PRINCIPAL_KEY_BYTES + 1),
            ..request
        };
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn protocol_roundtrips_and_rejects_unknown_fields() {
        let request = StopWorkspacePrincipalRequest {
            owner: application_principal_owner(1, 1, "user", "workspace").unwrap(),
            principal_id: "principal".into(),
        };
        assert_eq!(
            serde_json::from_value::<StopWorkspacePrincipalRequest>(
                serde_json::to_value(&request).unwrap()
            )
            .unwrap(),
            request
        );
        assert!(
            serde_json::from_value::<StopWorkspacePrincipalRequest>(
                serde_json::json!({"owner":request.owner,"principal_id":"p","extra":true})
            )
            .is_err()
        );
        for (state, name) in [
            (StopWorkspacePrincipalState::Stopped, "stopped"),
            (StopWorkspacePrincipalState::Conflict, "conflict"),
            (StopWorkspacePrincipalState::Busy, "busy"),
            (StopWorkspacePrincipalState::Unsupported, "unsupported"),
            (StopWorkspacePrincipalState::Failed, "failed"),
        ] {
            let ack = StopWorkspacePrincipalAck {
                state,
                detail: Some("detail".into()),
            };
            let value = serde_json::to_value(&ack).unwrap();
            assert_eq!(value["state"], name);
            assert_eq!(
                serde_json::from_value::<StopWorkspacePrincipalAck>(value).unwrap(),
                ack
            );
        }
    }
}
