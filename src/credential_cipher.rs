//! Encryption for workspace Git upstream upstream credentials (issue 1180 P27).
//!
//! Only the ciphertext envelope is ever persisted. The key comes from
//! `DF_UPSTREAM_CREDENTIAL_KEY`; a missing or empty value keeps public upstream
//! metadata working but makes credential writes fail closed. A configured key
//! that is not 64 hex characters fails startup instead of falling back to an
//! ephemeral key.

use anyhow::{Context, bail};
use ring::{
    aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey},
    hkdf::{HKDF_SHA256, KeyType, Salt},
    hmac,
    rand::{SecureRandom, SystemRandom},
};

pub const CREDENTIAL_KEY_ENV: &str = "DF_UPSTREAM_CREDENTIAL_KEY";

const ENVELOPE_VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const AAD_DOMAIN: &[u8] = b"desk-foreman.workspace-git-upstream.v1";

/// 32-byte upstream credential key. Deliberately not `Debug`: the bytes must
/// never reach logs, audit payloads or error messages.
#[derive(Clone)]
pub struct UpstreamCredentialKey {
    bytes: [u8; 32],
}

impl UpstreamCredentialKey {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        match std::env::var(CREDENTIAL_KEY_ENV) {
            Ok(value) => Self::parse(&value),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => {
                bail!("{CREDENTIAL_KEY_ENV} must be valid UTF-8")
            }
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Option<Self>> {
        let value = value.trim();
        if value.is_empty() {
            return Ok(None);
        }
        if value.len() != 64 {
            bail!("{CREDENTIAL_KEY_ENV} must be exactly 64 hex characters");
        }
        let mut bytes = [0u8; 32];
        for (index, slot) in bytes.iter_mut().enumerate() {
            let hi = hex_digit(value.as_bytes()[index * 2])
                .with_context(|| format!("{CREDENTIAL_KEY_ENV} must contain only hex digits"))?;
            let lo = hex_digit(value.as_bytes()[index * 2 + 1])
                .with_context(|| format!("{CREDENTIAL_KEY_ENV} must contain only hex digits"))?;
            *slot = (hi << 4) | lo;
        }
        Ok(Some(Self { bytes }))
    }

    pub fn encrypt(
        &self,
        workspace_binding_id: i64,
        upstream_id: i64,
        plaintext: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        let key = self.less_safe_key()?;
        let mut nonce_bytes = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| anyhow::anyhow!("failed to generate upstream credential nonce"))?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut ciphertext = plaintext.to_vec();
        key.seal_in_place_append_tag(
            nonce,
            Aad::from(aad(workspace_binding_id, upstream_id)),
            &mut ciphertext,
        )
        .map_err(|_| anyhow::anyhow!("failed to encrypt upstream credential"))?;

        let mut envelope = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
        envelope.push(ENVELOPE_VERSION);
        envelope.extend_from_slice(&nonce_bytes);
        envelope.extend_from_slice(&ciphertext);
        Ok(envelope)
    }

    pub fn decrypt(
        &self,
        workspace_binding_id: i64,
        upstream_id: i64,
        envelope: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        if envelope.len() < 1 + NONCE_LEN + TAG_LEN {
            bail!("upstream credential envelope is truncated");
        }
        if envelope[0] != ENVELOPE_VERSION {
            bail!("unsupported upstream credential envelope version");
        }
        let mut nonce_bytes = [0u8; NONCE_LEN];
        nonce_bytes.copy_from_slice(&envelope[1..1 + NONCE_LEN]);
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let key = self.less_safe_key()?;
        let mut ciphertext = envelope[1 + NONCE_LEN..].to_vec();
        let plaintext = key
            .open_in_place(
                nonce,
                Aad::from(aad(workspace_binding_id, upstream_id)),
                &mut ciphertext,
            )
            .map_err(|_| anyhow::anyhow!("upstream credential authentication failed"))?;
        Ok(plaintext.to_vec())
    }

    /// Derive the gateway capability signing key. The root credential key is
    /// never exposed; HKDF domain separation keeps this signing key distinct
    /// from the upstream credential encryption key.
    pub fn derive_git_capability_key(&self) -> anyhow::Result<hmac::Key> {
        let salt = Salt::new(HKDF_SHA256, b"df-git-gateway-capability-v1");
        let prk = salt.extract(&self.bytes);
        let okm = prk
            .expand(&[b"capability-signing"], CapabilityKeyType)
            .map_err(|_| anyhow::anyhow!("failed to derive gateway capability key"))?;
        let mut material = [0u8; 32];
        okm.fill(&mut material)
            .map_err(|_| anyhow::anyhow!("failed to derive gateway capability key"))?;
        Ok(hmac::Key::new(hmac::HMAC_SHA256, &material))
    }

    fn less_safe_key(&self) -> anyhow::Result<LessSafeKey> {
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &self.bytes)
            .map_err(|_| anyhow::anyhow!("invalid upstream credential key"))?;
        Ok(LessSafeKey::new(unbound))
    }
}

struct CapabilityKeyType;

impl KeyType for CapabilityKeyType {
    fn len(&self) -> usize {
        32
    }
}

fn aad(workspace_binding_id: i64, upstream_id: i64) -> Vec<u8> {
    let mut aad = Vec::with_capacity(AAD_DOMAIN.len() + 17);
    aad.extend_from_slice(AAD_DOMAIN);
    aad.push(b':');
    aad.extend_from_slice(&workspace_binding_id.to_be_bytes());
    aad.push(b':');
    aad.extend_from_slice(&upstream_id.to_be_bytes());
    aad
}

fn hex_digit(byte: u8) -> anyhow::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(anyhow::anyhow!("not a hex digit")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> UpstreamCredentialKey {
        UpstreamCredentialKey { bytes: [seed; 32] }
    }

    #[test]
    fn parses_only_64_hex_characters() {
        let parsed = UpstreamCredentialKey::parse(&"ab".repeat(32))
            .unwrap()
            .unwrap();
        assert_eq!(parsed.bytes, [0xab; 32]);
        assert!(UpstreamCredentialKey::parse("").unwrap().is_none());
        assert!(UpstreamCredentialKey::parse("   ").unwrap().is_none());
        assert!(UpstreamCredentialKey::parse("abcd").is_err());
        assert!(UpstreamCredentialKey::parse(&"zz".repeat(32)).is_err());
        assert!(UpstreamCredentialKey::parse(&"a".repeat(65)).is_err());
    }

    #[test]
    fn round_trips_with_binding_and_upstream_aad() {
        let key = key(7);
        let envelope = key.encrypt(11, 22, b"token-value").unwrap();
        assert_eq!(envelope[0], ENVELOPE_VERSION);
        assert_ne!(&envelope[1 + NONCE_LEN..], b"token-value");
        assert_eq!(key.decrypt(11, 22, &envelope).unwrap(), b"token-value");
    }

    #[test]
    fn rejects_wrong_aad_key_version_and_tampering() {
        let primary = key(7);
        let envelope = primary.encrypt(11, 22, b"token-value").unwrap();

        assert!(primary.decrypt(12, 22, &envelope).is_err());
        assert!(primary.decrypt(11, 23, &envelope).is_err());
        assert!(key(8).decrypt(11, 22, &envelope).is_err());

        let mut tampered = envelope.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert!(primary.decrypt(11, 22, &tampered).is_err());

        let mut wrong_version = envelope.clone();
        wrong_version[0] = 2;
        assert!(primary.decrypt(11, 22, &wrong_version).is_err());

        assert!(primary.decrypt(11, 22, &envelope[..NONCE_LEN + 4]).is_err());
    }
}
