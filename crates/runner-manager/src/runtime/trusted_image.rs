//! Neutral digest-pinned trusted image validation.
//!
//! This grammar is shared by every trusted manager-owned container image. It
//! accepts only `name@sha256:<64 lowercase hex>` and never echoes the image
//! value in an error.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrustedImageError {
    Unpinned,
    Invalid,
}

pub(crate) fn validate_pinned_image(image: &str) -> Result<(), TrustedImageError> {
    let Some((name, digest)) = image.split_once('@') else {
        return Err(TrustedImageError::Unpinned);
    };
    if name.is_empty()
        || name.len() > 255
        || name.contains('@')
        || name
            .chars()
            .any(|character| matches!(character, ',' | '\n' | '\r' | '\0' | ' '))
        || name.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(TrustedImageError::Invalid);
    }
    let Some(hash) = digest.strip_prefix("sha256:") else {
        return Err(TrustedImageError::Invalid);
    };
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(TrustedImageError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{TrustedImageError, validate_pinned_image};

    fn digest(hex: &str) -> String {
        format!("registry.example.com/team/service@sha256:{hex}")
    }

    #[test]
    fn accepts_only_a_digest_pinned_image() {
        assert_eq!(validate_pinned_image(&digest(&"a".repeat(64))), Ok(()));

        for unpinned in [
            "registry.example.com/team/service:latest",
            "registry.example.com/team/service",
            "service",
            "",
        ] {
            assert_eq!(
                validate_pinned_image(unpinned),
                Err(TrustedImageError::Unpinned)
            );
        }

        let valid = digest(&"a".repeat(64));
        for invalid in [
            format!("{}@sha256:{}", "a".repeat(256), "a".repeat(64)),
            format!("{}@sha256:{}", "bad name", "a".repeat(64)),
            format!("{}@sha256:{}", "bad,name", "a".repeat(64)),
            format!("{}@sha256:{}", "bad\nname", "a".repeat(64)),
            format!("{}@sha256:{}", "bad@name", "a".repeat(64)),
            valid.replace("sha256:", "sha512:"),
            valid.replace(&"a".repeat(64), &"a".repeat(63)),
            valid.replace(&"a".repeat(64), &"A".repeat(64)),
            valid.replace(&"a".repeat(64), &"g".repeat(64)),
        ] {
            assert_eq!(
                validate_pinned_image(&invalid),
                Err(TrustedImageError::Invalid),
                "invalid image must be refused"
            );
        }
    }
}
