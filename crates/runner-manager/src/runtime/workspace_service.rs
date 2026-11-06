//! Operator-only selection of the trusted workspace service image.

use super::trusted_image::{TrustedImageError, validate_pinned_image};

pub(crate) const WORKSPACE_SERVICE_IMAGE_ENV: &str = "RUNNER_WORKSPACE_SERVICE_IMAGE";

/// Read the operator-provided workspace service image.
///
/// A missing or blank variable disables the slot. A malformed value is a typed
/// error; a valid digest-pinned value is returned unchanged. There is no
/// fallback to the ordinary runner image, a default image, or request input.
pub(crate) fn workspace_service_image() -> Result<Option<String>, TrustedImageError> {
    let Some(value) = std::env::var_os(WORKSPACE_SERVICE_IMAGE_ENV) else {
        return Ok(None);
    };
    let image = value
        .into_string()
        .map_err(|_| TrustedImageError::Invalid)?;
    image_from_raw(Some(image))
}

/// Require the trusted pinned image consumed once per scoped operation.
///
/// A missing or invalid operator setting refuses; there is no fallback to
/// the ordinary runner image or to caller input.
pub(crate) fn require_workspace_service_image() -> anyhow::Result<String> {
    match workspace_service_image() {
        Ok(Some(image)) => Ok(image),
        Ok(None) => anyhow::bail!("workspace service image is not configured"),
        Err(_) => anyhow::bail!("workspace service image is invalid"),
    }
}

fn image_from_raw(raw: Option<String>) -> Result<Option<String>, TrustedImageError> {
    match raw {
        None => Ok(None),
        Some(image) if image.trim().is_empty() => Ok(None),
        Some(image) => {
            validate_pinned_image(&image)?;
            Ok(Some(image))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TrustedImageError, image_from_raw};

    fn digest() -> String {
        format!(
            "registry.example.com/team/service@sha256:{}",
            "a".repeat(64)
        )
    }

    #[test]
    fn slot_is_disabled_when_missing_or_blank() {
        assert_eq!(image_from_raw(None), Ok(None));
        assert_eq!(image_from_raw(Some(String::new())), Ok(None));
        assert_eq!(image_from_raw(Some("   ".to_string())), Ok(None));
    }

    #[test]
    fn a_valid_pin_is_returned_unchanged() {
        let image = digest();
        assert_eq!(image_from_raw(Some(image.clone())), Ok(Some(image)));
    }

    #[test]
    fn invalid_or_unpinned_values_are_refused() {
        assert_eq!(
            image_from_raw(Some("service:latest".to_string())),
            Err(TrustedImageError::Unpinned)
        );
        assert_eq!(
            image_from_raw(Some("service@sha256:not-hex".to_string())),
            Err(TrustedImageError::Invalid)
        );
    }
}
