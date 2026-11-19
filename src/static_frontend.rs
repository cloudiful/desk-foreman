use std::path::Path;

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use tower_http::services::{ServeDir, ServeFile};

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

pub(crate) fn router<S>(dist: &Path) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .nest_service("/assets", ServeDir::new(dist.join("assets")))
        .fallback_service(ServeDir::new(dist).fallback(ServeFile::new(dist.join("index.html"))))
        .layer(middleware::from_fn(static_policy))
}

async fn static_policy(request: Request, next: Next) -> Response {
    let raw = request.uri().path();
    let Some(path) = safe_path(raw) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if reserved_path(&path) || (is_assets(&path) && !is_assets(raw)) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut response = next.run(request).await;
    let cache = cache_control(&path, response.status());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

fn is_assets(path: &str) -> bool {
    path == "/assets" || path.starts_with("/assets/")
}

fn safe_path(path: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let hi = char::from(bytes.next()?).to_digit(16)?;
            let lo = char::from(bytes.next()?).to_digit(16)?;
            let value = (hi * 16 + lo) as u8;
            if value == b'/' || value == b'\\' {
                return None;
            }
            decoded.push(value);
        } else {
            decoded.push(byte);
        }
    }
    let decoded = String::from_utf8(decoded).ok()?;
    if decoded.contains(['\\', '\0'])
        || decoded
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    {
        return None;
    }
    Some(decoded)
}

fn reserved_path(path: &str) -> bool {
    ["/api", "/git", "/mcp", "/healthz", "/readyz"]
        .iter()
        .any(|prefix| {
            path == *prefix
                || path
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

fn cache_control(path: &str, status: StatusCode) -> &'static str {
    if (status.is_success() || status == StatusCode::NOT_MODIFIED) && path.starts_with("/assets/") {
        IMMUTABLE
    } else {
        "no-cache"
    }
}

#[cfg(test)]
mod tests {
    use super::{IMMUTABLE, cache_control, is_assets, reserved_path, safe_path};
    use axum::http::StatusCode;

    #[test]
    fn reserved_prefixes_do_not_fall_back_to_spa() {
        for path in [
            "/api",
            "/api/unknown",
            "/git/x",
            "/mcp",
            "/mcp/unknown",
            "/healthz",
            "/readyz/extra",
        ] {
            assert!(reserved_path(path), "{path}");
        }
        for path in ["/apix", "/github", "/mcp-other", "/settings"] {
            assert!(!reserved_path(path), "{path}");
        }
        assert!(reserved_path(&safe_path("/%61pi/unknown").unwrap()));
        assert!(is_assets(&safe_path("/%61ssets/a.js").unwrap()));
        assert!(!is_assets("/%61ssets/a.js"));
    }

    #[test]
    fn reject_traversal_and_invalid_escapes() {
        for path in [
            "/../index.html",
            "/%2e%2e/secret",
            "/assets/%2fsecret",
            "/assets/%5csecret",
            "/assets/%00",
            "/assets/%xx",
        ] {
            assert!(safe_path(path).is_none(), "{path}");
        }
        assert_eq!(
            safe_path("/assets/app.js").as_deref(),
            Some("/assets/app.js")
        );
    }

    #[test]
    fn cache_only_successful_assets_immutably() {
        assert_eq!(cache_control("/assets/app.js", StatusCode::OK), IMMUTABLE);
        assert_eq!(
            cache_control("/assets/app.js", StatusCode::NOT_MODIFIED),
            IMMUTABLE
        );
        assert_eq!(
            cache_control("/assets/missing.js", StatusCode::NOT_FOUND),
            "no-cache"
        );
        assert_eq!(cache_control("/index.html", StatusCode::OK), "no-cache");
        assert_eq!(cache_control("/assets-other", StatusCode::OK), "no-cache");
    }
}
