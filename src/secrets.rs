use anyhow::{Context, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::Aead};
use rand::Rng;

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const KEY_VERSION_V1: i16 = 1;
pub const MASTER_KEY_ENV: &str = "DESK_FOREMAN_SECRET_MASTER_KEY";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedSecret {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub key_version: i16,
}

#[derive(Clone)]
pub struct SecretManager {
    key: [u8; KEY_BYTES],
}

impl SecretManager {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(value) = std::env::var(MASTER_KEY_ENV).ok() else {
            return Ok(None);
        };
        let value = value.trim();
        if value.is_empty() {
            return Ok(None);
        }
        let bytes = STANDARD
            .decode(value.as_bytes())
            .context("DESK_FOREMAN_SECRET_MASTER_KEY must be valid base64")?;
        let key = bytes.try_into().map_err(|bytes: Vec<u8>| {
            anyhow!(
                "DESK_FOREMAN_SECRET_MASTER_KEY must decode to {KEY_BYTES} bytes, got {}",
                bytes.len()
            )
        })?;
        Ok(Some(Self { key }))
    }

    pub fn encrypt(&self, plaintext: &str) -> anyhow::Result<EncryptedSecret> {
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let mut nonce = [0_u8; NONCE_BYTES];
        rand::rng().fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(&Nonce::from(nonce), plaintext.as_bytes())
            .map_err(|_| anyhow!("failed to encrypt secret"))?;
        Ok(EncryptedSecret {
            ciphertext,
            nonce: nonce.to_vec(),
            key_version: KEY_VERSION_V1,
        })
    }

    pub fn decrypt(&self, encrypted: &EncryptedSecret) -> anyhow::Result<String> {
        if encrypted.key_version != KEY_VERSION_V1 {
            return Err(anyhow!(
                "unsupported secret key version {}",
                encrypted.key_version
            ));
        }
        let nonce: &[u8; NONCE_BYTES] = encrypted
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("secret nonce must be {NONCE_BYTES} bytes"))?;
        let plaintext = ChaCha20Poly1305::new((&self.key).into())
            .decrypt(&Nonce::from(*nonce), encrypted.ciphertext.as_ref())
            .map_err(|_| anyhow!("failed to decrypt secret"))?;
        String::from_utf8(plaintext).context("secret plaintext is not valid utf-8")
    }
}

#[cfg(test)]
mod tests {
    use super::{EncryptedSecret, SecretManager};

    fn manager() -> SecretManager {
        SecretManager { key: [7_u8; 32] }
    }

    #[test]
    fn secret_round_trip() {
        let manager = manager();
        let encrypted = manager.encrypt("secret").expect("encrypt");
        assert_eq!(manager.decrypt(&encrypted).expect("decrypt"), "secret");
    }

    #[test]
    fn wrong_key_and_tampering_are_rejected() {
        let manager = manager();
        let encrypted = manager.encrypt("secret").expect("encrypt");
        let wrong = SecretManager { key: [9_u8; 32] };
        assert!(wrong.decrypt(&encrypted).is_err());

        let mut tampered = encrypted.clone();
        tampered.ciphertext[0] ^= 1;
        assert!(manager.decrypt(&tampered).is_err());

        let invalid = EncryptedSecret {
            ciphertext: encrypted.ciphertext,
            nonce: encrypted.nonce,
            key_version: 2,
        };
        assert!(manager.decrypt(&invalid).is_err());
    }
}
