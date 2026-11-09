//! Short-lived, workspace-scoped native Git capability (issue 1180 P28).
//!
//! The token is a signed claims envelope, not a stored random secret. It is
//! derived from the upstream credential key through a domain-separated HKDF
//! key and is never the upstream credential, application bearer or admin token.

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use ring::hmac;
use serde::{Deserialize, Serialize};

const TOKEN_PREFIX: &str = "dfgit1.";
const TOKEN_VERSION: u8 = 1;
const MAX_TOKEN_BYTES: usize = 16 * 1024;
pub(crate) const CAPABILITY_TTL_SECONDS: i64 = 10 * 60;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CapabilityAlias {
    pub remote_alias: String,
    pub upstream_id: i64,
    pub generation: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CapabilityClaims {
    pub v: u8,
    pub app_id: i64,
    pub binding_id: i64,
    pub external_user_id: String,
    pub principal_id: String,
    pub lease_owner: String,
    pub read: bool,
    pub write: bool,
    pub aliases: Vec<CapabilityAlias>,
    pub iat: i64,
    pub exp: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityError {
    Malformed,
    UnsupportedVersion,
    InvalidSignature,
    Expired,
}

impl CapabilityError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Malformed | Self::UnsupportedVersion | Self::InvalidSignature => {
                "git capability rejected"
            }
            Self::Expired => "git capability expired",
        }
    }
}

pub fn mint(key: &hmac::Key, claims: &CapabilityClaims) -> anyhow::Result<String> {
    if claims.aliases.len() > 200 {
        anyhow::bail!("too many Git capability aliases");
    }
    let payload = serde_json::to_vec(claims)?;
    if payload.len() > MAX_TOKEN_BYTES / 2 {
        anyhow::bail!("Git capability payload is too large");
    }
    let payload = URL_SAFE_NO_PAD.encode(payload);
    let tag = hmac::sign(key, payload.as_bytes());
    let token = format!(
        "{TOKEN_PREFIX}{payload}.{}",
        URL_SAFE_NO_PAD.encode(tag.as_ref())
    );
    if token.len() > MAX_TOKEN_BYTES {
        anyhow::bail!("Git capability token is too large");
    }
    Ok(token)
}

pub fn verify(key: &hmac::Key, token: &str, now: i64) -> Result<CapabilityClaims, CapabilityError> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(CapabilityError::Malformed);
    }
    let token = token
        .strip_prefix(TOKEN_PREFIX)
        .ok_or(CapabilityError::Malformed)?;
    let (payload, tag) = token.split_once('.').ok_or(CapabilityError::Malformed)?;
    if payload.is_empty() || tag.is_empty() {
        return Err(CapabilityError::Malformed);
    }
    let tag = URL_SAFE_NO_PAD
        .decode(tag)
        .map_err(|_| CapabilityError::Malformed)?;
    hmac::verify(key, payload.as_bytes(), &tag).map_err(|_| CapabilityError::InvalidSignature)?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| CapabilityError::Malformed)?;
    let claims: CapabilityClaims =
        serde_json::from_slice(&payload).map_err(|_| CapabilityError::Malformed)?;
    if claims.v != TOKEN_VERSION {
        return Err(CapabilityError::UnsupportedVersion);
    }
    if claims.exp <= now {
        return Err(CapabilityError::Expired);
    }
    if claims.iat > now.saturating_add(60) {
        return Err(CapabilityError::Malformed);
    }
    Ok(claims)
}

pub fn decode_basic_password(header: Option<&str>) -> Option<(String, String)> {
    let value = header?.strip_prefix("Basic ")?;
    let decoded = STANDARD.decode(value).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (username, password) = decoded.split_once(':')?;
    Some((username.to_string(), password.to_string()))
}

pub fn alias_claim<'a>(
    claims: &'a CapabilityClaims,
    remote_alias: &str,
    upstream_id: i64,
    generation: i64,
) -> Option<&'a CapabilityAlias> {
    claims.aliases.iter().find(|alias| {
        alias.remote_alias == remote_alias
            && alias.upstream_id == upstream_id
            && alias.generation == generation
    })
}

pub fn principal_id(application_id: i64, external_user_id: &str, workspace_key: &str) -> String {
    format!("application:{application_id}:{external_user_id}:{workspace_key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signing_key(seed: u8) -> hmac::Key {
        hmac::Key::new(hmac::HMAC_SHA256, &[seed; 32])
    }

    fn claims() -> CapabilityClaims {
        CapabilityClaims {
            v: TOKEN_VERSION,
            app_id: 7,
            binding_id: 11,
            external_user_id: "user-1".into(),
            principal_id: "application:7:user-1:default".into(),
            lease_owner: "session-1".into(),
            read: true,
            write: true,
            aliases: vec![CapabilityAlias {
                remote_alias: "origin".into(),
                upstream_id: 22,
                generation: 3,
            }],
            iat: 1_000,
            exp: 1_600,
        }
    }

    #[test]
    fn round_trips_and_binds_alias_generation() {
        let key = signing_key(1);
        let token = mint(&key, &claims()).unwrap();
        let verified = verify(&key, &token, 1_200).unwrap();
        assert_eq!(verified, claims());
        assert!(
            alias_claim(&verified, "origin", 22, 3).is_some(),
            "exact alias row must match"
        );
        assert!(alias_claim(&verified, "origin", 22, 4).is_none());
        assert!(alias_claim(&verified, "upstream", 22, 3).is_none());
        assert!(alias_claim(&verified, "origin", 23, 3).is_none());
    }

    #[test]
    fn rejects_tamper_expiry_wrong_key_and_version() {
        let key = signing_key(1);
        let token = mint(&key, &claims()).unwrap();

        assert_eq!(verify(&key, &token, 1_600), Err(CapabilityError::Expired));
        assert_eq!(
            verify(&key, &token.replace("dfgit1.", "dfgit2."), 1_200),
            Err(CapabilityError::Malformed)
        );
        assert_eq!(
            verify(&key, "dfgit1.not-a-token", 1_200),
            Err(CapabilityError::Malformed)
        );
        assert_eq!(
            verify(&signing_key(2), &token, 1_200),
            Err(CapabilityError::InvalidSignature)
        );

        let mut tampered = token.into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        assert_eq!(
            verify(&key, &String::from_utf8(tampered).unwrap(), 1_200),
            Err(CapabilityError::InvalidSignature)
        );

        let unsupported = CapabilityClaims { v: 2, ..claims() };
        let token = mint(&key, &unsupported).unwrap();
        assert_eq!(
            verify(&key, &token, 1_200),
            Err(CapabilityError::UnsupportedVersion)
        );
    }

    #[test]
    fn decodes_basic_credentials_and_rejects_other_schemes() {
        let header = format!("Basic {}", STANDARD.encode("df-git:dfgit1.payload.tag"));
        assert_eq!(
            decode_basic_password(Some(&header)),
            Some(("df-git".into(), "dfgit1.payload.tag".into()))
        );
        assert!(decode_basic_password(Some("Bearer token")).is_none());
        assert!(decode_basic_password(Some("Basic !!!")).is_none());
        assert!(decode_basic_password(None).is_none());
    }
}
