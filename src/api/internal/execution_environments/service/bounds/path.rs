//! Relative-path bound for the private-service gateway (issue 749 P2b).
//!
//! Split from [`super::bounds`] so the whole rule set stays readable. The
//! path is treated as an opaque relative reference, never as a URL: a
//! gateway that could be talked into building an absolute URL would let a
//! caller choose the target, which is exactly what this relay prevents.

use super::BoundsRefusal;

/// Largest relative path accepted for one service request.
pub const MAX_SERVICE_PATH_BYTES: usize = 1024;

/// Accept only a plain relative path.
///
/// Percent escapes are refused rather than decoded: a gateway that decoded
/// them would be guessing at what the caller meant, and a caller that needs
/// an escape can send the literal character the agent expects.
pub fn validated_path(raw: &str) -> Result<String, BoundsRefusal> {
    if raw.is_empty() || raw.len() > MAX_SERVICE_PATH_BYTES {
        return Err(BoundsRefusal::InvalidPath);
    }
    if raw.starts_with('/')
        || raw.contains('\\')
        || raw.contains('%')
        || raw.contains('?')
        || raw.contains('#')
    {
        return Err(BoundsRefusal::InvalidPath);
    }
    if raw.chars().any(|character| character.is_control()) {
        return Err(BoundsRefusal::InvalidPath);
    }
    // An authority-shaped path would smuggle a target past the agent hop.
    if raw.contains("://") {
        return Err(BoundsRefusal::InvalidPath);
    }
    for segment in raw.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(BoundsRefusal::InvalidPath);
        }
    }
    Ok(raw.to_string())
}
