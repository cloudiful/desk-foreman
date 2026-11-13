use chrono::Utc;

use super::resource_workspace_actor;
use crate::git_gateway::capability::{CapabilityAlias, CapabilityClaims};

const LEASE_OWNER: &str = "conversation:1";

fn aliases() -> Vec<CapabilityAlias> {
    vec![CapabilityAlias {
        remote_alias: "origin".to_string(),
        upstream_id: 22,
        generation: 3,
    }]
}

fn claims_for(actor: &crate::actor::ActorContext) -> CapabilityClaims {
    let binding = actor
        .workspace_binding
        .as_ref()
        .expect("resource binding")
        .clone();
    let now = Utc::now();
    let expiry = crate::git_gateway::capability_expiry(&binding, now);
    crate::tools::shared::capability_claims(actor, &binding, &aliases(), now, expiry)
}

#[test]
fn resource_capability_carries_the_binding_subject_and_the_real_caller() {
    let temp = tempfile::tempdir().expect("tempdir");
    let actor = resource_workspace_actor(temp.path(), None);

    let claims = claims_for(&actor);

    assert_eq!(
        claims.external_user_id, "__resource__",
        "the capability subject must be the binding subject the gateway compares"
    );
    assert_ne!(
        claims.external_user_id,
        actor.external_user_id.clone().unwrap_or_default(),
        "the caller subject must not replace the binding subject"
    );
    assert_eq!(
        claims.principal_id, actor.principal_id,
        "the real caller principal must be preserved"
    );
    assert_eq!(claims.binding_id, 1);
    assert_eq!(claims.app_id, 1);
    assert!(claims.read, "reads stay available");
    assert!(
        !claims.write,
        "a lease-less resource caller must not receive write admission"
    );
    assert!(claims.lease_owner.is_empty());
    assert_eq!(claims.aliases, aliases());
}

#[test]
fn resource_capability_write_follows_the_lease() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut actor = resource_workspace_actor(temp.path(), Some(LEASE_OWNER));
    let binding = actor.workspace_binding.as_mut().expect("resource binding");
    binding.write_lease_owner = Some(LEASE_OWNER.to_string());
    binding.write_lease_acquired_at = Some(Utc::now());
    binding.write_lease_expires_at = Some(Utc::now() + chrono::Duration::minutes(10));

    let claims = claims_for(&actor);

    assert_eq!(claims.lease_owner, LEASE_OWNER);
    assert!(claims.write, "the lease holder keeps write admission");
    assert_eq!(claims.external_user_id, "__resource__");
}

#[test]
fn personal_capability_subject_stays_the_caller_subject() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut actor = resource_workspace_actor(temp.path(), None);
    let binding = actor.workspace_binding.as_mut().expect("binding");
    binding.external_user_id = "user-42".to_string();
    binding.resource_kind = None;
    binding.resource_id = None;
    actor.external_user_id = Some("user-42".to_string());

    let claims = claims_for(&actor);

    assert_eq!(claims.external_user_id, "user-42");
    assert!(claims.write, "a personal binding needs no lease for writes");
}
