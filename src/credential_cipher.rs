//! App-owned protection for persisted Git upstream credentials.
//!
//! The shared `cloudiful-secret-crypto` primitive owns the frame format, the
//! nonce and the AEAD; this module owns the desk binding rules: which purpose
//! desk Git credentials belong to, which row a stored frame may be opened for,
//! and how a failure is reported. A frame is bound to the workspace binding
//! and the upstream row that holds it, so a frame copied from one row never
//! opens under another row's identity.

use std::sync::OnceLock;

use cloudiful_secret_crypto::{SecretCipher, SecretCipherError, SecretPurpose};

pub use cloudiful_secret_crypto::{MASTER_KEY_LEN, MasterKey};

/// Purpose every desk Git upstream credential is sealed under.
pub(crate) const GIT_CREDENTIAL_PURPOSE: &str = "desk.git_upstream_credential";

/// Master key version this build writes and expects to read.
pub(crate) const CREDENTIAL_KEY_VERSION: u32 = 1;

/// Why a credential could not be protected or recovered.
///
/// Both variants are metadata-only: no key material, no plaintext and no
/// frame bytes reach an error, a log line or an API response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialCipherError {
    #[error("credential encryption key is not configured")]
    KeyUnavailable,
    #[error("stored credential could not be decrypted")]
    Unreadable,
}

/// Protects one deployment's Git upstream credentials.
pub struct CredentialProtector {
    cipher: SecretCipher,
}

impl Clone for CredentialProtector {
    fn clone(&self) -> Self {
        Self {
            cipher: self.cipher.clone(),
        }
    }
}

impl std::fmt::Debug for CredentialProtector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CredentialProtector(<redacted>)")
    }
}

impl CredentialProtector {
    pub fn new(master_key: MasterKey) -> Self {
        Self {
            cipher: SecretCipher::new(master_key, CREDENTIAL_KEY_VERSION),
        }
    }

    /// Seals a plaintext credential for one upstream row.
    pub fn seal(
        &self,
        workspace_binding_id: i64,
        upstream_id: i64,
        plaintext: &str,
    ) -> Result<Vec<u8>, CredentialCipherError> {
        self.cipher
            .seal(
                &purpose()?,
                &key_name(workspace_binding_id, upstream_id),
                plaintext.as_bytes(),
            )
            .map_err(|_| CredentialCipherError::Unreadable)
    }

    /// Opens a stored frame for the row that is expected to own it.
    ///
    /// The frame is read under the expected row identity, never under the
    /// identity carried in the frame header, so a frame that was copied from
    /// another row fails instead of decrypting.
    pub fn open(
        &self,
        workspace_binding_id: i64,
        upstream_id: i64,
        frame: &[u8],
    ) -> Result<String, CredentialCipherError> {
        let opened = self
            .cipher
            .open(
                &purpose()?,
                &key_name(workspace_binding_id, upstream_id),
                frame,
            )
            .map_err(|_| CredentialCipherError::Unreadable)?;
        String::from_utf8(opened.into_inner()).map_err(|_| CredentialCipherError::Unreadable)
    }
}

/// Key name a credential frame is bound to.
///
/// The name is derived from the row identity only. It never contains a
/// username, a token or a URL.
pub fn key_name(workspace_binding_id: i64, upstream_id: i64) -> String {
    format!("workspace_binding.{workspace_binding_id}.upstream.{upstream_id}")
}

fn purpose() -> Result<SecretPurpose, CredentialCipherError> {
    static PURPOSE: OnceLock<Result<SecretPurpose, SecretCipherError>> = OnceLock::new();
    PURPOSE
        .get_or_init(|| SecretPurpose::new(GIT_CREDENTIAL_PURPOSE))
        .clone()
        .map_err(|_| CredentialCipherError::Unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protector() -> CredentialProtector {
        CredentialProtector::new(MasterKey::from_bytes(&[7u8; 32]).expect("master key"))
    }

    #[test]
    fn sealed_credential_opens_for_its_own_row() {
        let protector = protector();
        let frame = protector
            .seal(918001, 42, "super-secret")
            .expect("seal credential");

        assert!(!frame.windows(11).any(|window| window == b"super-secret"));
        assert_eq!(
            protector.open(918001, 42, &frame).expect("open credential"),
            "super-secret"
        );
    }

    #[test]
    fn sealed_credential_is_bound_to_its_row() {
        let protector = protector();
        let frame = protector
            .seal(918001, 42, "super-secret")
            .expect("seal credential");

        assert_eq!(
            protector.open(918002, 42, &frame).unwrap_err(),
            CredentialCipherError::Unreadable
        );
        assert_eq!(
            protector.open(918001, 43, &frame).unwrap_err(),
            CredentialCipherError::Unreadable
        );
    }

    #[test]
    fn another_master_key_cannot_open_the_frame() {
        let frame = protector()
            .seal(918001, 42, "super-secret")
            .expect("seal credential");
        let other =
            CredentialProtector::new(MasterKey::from_bytes(&[8u8; 32]).expect("master key"));

        assert_eq!(
            other.open(918001, 42, &frame).unwrap_err(),
            CredentialCipherError::Unreadable
        );
    }

    #[test]
    fn key_name_is_a_bounded_identifier() {
        let name = key_name(918001, 42);
        assert!(cloudiful_secret_crypto::is_valid_key_name(&name));
        assert!(name.contains("918001"));
        assert!(name.contains("42"));
    }
}
