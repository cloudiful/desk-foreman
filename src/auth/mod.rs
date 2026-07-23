pub(crate) mod identity;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use rand::{TryRng, rngs::SysRng};
use sha2::{Digest, Sha256};

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let mut salt_bytes = [0u8; 16];
    SysRng
        .try_fill_bytes(&mut salt_bytes)
        .map_err(|error| anyhow::anyhow!("failed to generate password salt: {error}"))?;
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|error| anyhow::anyhow!("failed to encode password salt: {error}"))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|value| value.to_string())
        .map_err(|error| anyhow::anyhow!("failed to hash password: {error}"))
}

pub fn verify_password(password: &str, password_hash: &str) -> anyhow::Result<bool> {
    let parsed = PasswordHash::new(password_hash)
        .map_err(|error| anyhow::anyhow!("invalid password hash: {error}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub fn hash_bearer_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::{hash_bearer_token, hash_password, verify_password};

    #[test]
    fn hashes_and_verifies_passwords() {
        let password_hash = hash_password("secret123").expect("password hash");
        assert!(verify_password("secret123", &password_hash).expect("password verify"));
        assert!(!verify_password("wrong", &password_hash).expect("password verify"));
    }

    #[test]
    fn token_hash_is_stable() {
        assert_eq!(hash_bearer_token("abc"), hash_bearer_token("abc"));
        assert_ne!(hash_bearer_token("abc"), hash_bearer_token("def"));
    }
}
