//! Public native Git Smart HTTP routes (issue 1180 P28).
//!
//! Exactly three route shapes are registered. Path, method, the single
//! `service` discriminator, and Basic capability auth are validated before any
//! database read or upstream connection.

use axum::{
    Router,
    body::Body,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode, Uri, header},
    response::Response,
    routing::get,
};
use chrono::Utc;

use crate::{
    AppState,
    actor::check_write_lease,
    db::queries,
    git_gateway::{
        capability::{CapabilityError, alias_claim, decode_basic_password, verify},
        gateway_enabled,
        proxy::{GatewayRoute, UpstreamTarget, error_response, forward, parse_target},
        signing_key,
    },
};

const GIT_USERNAME: &str = "df-git";

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/git/{binding_id}/{remote_alias}/info/refs", get(info_refs))
        .route(
            "/git/{binding_id}/{remote_alias}/git-upload-pack",
            axum::routing::post(upload_pack),
        )
        .route(
            "/git/{binding_id}/{remote_alias}/git-receive-pack",
            axum::routing::post(receive_pack),
        )
}

async fn info_refs(
    State(state): State<AppState>,
    Path((binding_id, remote_alias)): Path<(i64, String)>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    let Some(service) = info_refs_service(&uri) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid Git service");
    };
    let route = match service {
        "git-upload-pack" => GatewayRoute::InfoRefs("git-upload-pack"),
        "git-receive-pack" => GatewayRoute::InfoRefs("git-receive-pack"),
        _ => return error_response(StatusCode::BAD_REQUEST, "invalid Git service"),
    };
    handle(&state, binding_id, &remote_alias, route, &headers, None).await
}

async fn upload_pack(
    State(state): State<AppState>,
    Path((binding_id, remote_alias)): Path<(i64, String)>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if uri.query().is_some() {
        return error_response(StatusCode::BAD_REQUEST, "unexpected query");
    }
    handle(
        &state,
        binding_id,
        &remote_alias,
        GatewayRoute::UploadPack,
        &headers,
        Some(body),
    )
    .await
}

async fn receive_pack(
    State(state): State<AppState>,
    Path((binding_id, remote_alias)): Path<(i64, String)>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if uri.query().is_some() {
        return error_response(StatusCode::BAD_REQUEST, "unexpected query");
    }
    handle(
        &state,
        binding_id,
        &remote_alias,
        GatewayRoute::ReceivePack,
        &headers,
        Some(body),
    )
    .await
}

async fn handle(
    state: &AppState,
    binding_id: i64,
    remote_alias: &str,
    route: GatewayRoute,
    headers: &HeaderMap,
    body: Option<Body>,
) -> Response {
    if !gateway_enabled(&state.config) {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "git gateway disabled");
    }
    if binding_id <= 0 || !valid_alias(remote_alias) {
        return error_response(StatusCode::BAD_REQUEST, "invalid Git route");
    }
    let Some((username, token)) = decode_basic_password(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    ) else {
        return error_response(StatusCode::UNAUTHORIZED, "authentication required");
    };
    if username != GIT_USERNAME {
        return error_response(StatusCode::UNAUTHORIZED, "authentication required");
    }
    let Some(key) = signing_key(&state.config) else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "git gateway disabled");
    };
    let claims = match verify(&key, &token, Utc::now().timestamp()) {
        Ok(claims) => claims,
        Err(CapabilityError::Expired) => {
            return error_response(StatusCode::UNAUTHORIZED, "git capability expired");
        }
        Err(error) => return error_response(StatusCode::UNAUTHORIZED, error.message()),
    };

    let binding = match queries::find_workspace_binding_by_id(&state.db, binding_id).await {
        Ok(Some(binding)) => binding,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "workspace not found"),
        Err(_) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "git gateway unavailable");
        }
    };
    let upstream =
        match queries::resolve_gateway_upstream(&state.db, binding_id, remote_alias).await {
            Ok(Some(upstream)) => upstream,
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "upstream not found"),
            Err(_) => {
                return error_response(StatusCode::SERVICE_UNAVAILABLE, "git gateway unavailable");
            }
        };

    if upstream.application_id != binding.application_id
        || claims.app_id != upstream.application_id
        || claims.binding_id != binding.workspace_binding_id
        || claims.external_user_id != binding.external_user_id
        || claims.principal_id.is_empty()
        || !upstream.application_active
        || !binding.is_active
        || binding.lifecycle_state != "active"
        || binding.archived_at.is_some()
        || !upstream.enabled
    {
        return error_response(StatusCode::FORBIDDEN, "git access denied");
    }
    if alias_claim(
        &claims,
        remote_alias,
        upstream.upstream_id,
        upstream.capability_generation,
    )
    .is_none()
    {
        return error_response(StatusCode::FORBIDDEN, "git capability rejected");
    }

    let needs_write = matches!(
        route,
        GatewayRoute::ReceivePack | GatewayRoute::InfoRefs("git-receive-pack")
    );
    if needs_write {
        if !claims.write || check_write_lease(&binding, Some(&claims.lease_owner)).is_err() {
            return error_response(StatusCode::FORBIDDEN, "git write access denied");
        }
    } else if !claims.read {
        return error_response(StatusCode::FORBIDDEN, "git read access denied");
    }

    let target = match parse_target(&upstream.target_url, route) {
        Ok(url) => UpstreamTarget {
            url,
            username: upstream.credential_username,
            ciphertext: upstream.credential_ciphertext,
            workspace_binding_id: upstream.workspace_binding_id,
            upstream_id: upstream.upstream_id,
        },
        Err(_) => return error_response(StatusCode::BAD_GATEWAY, "upstream target invalid"),
    };

    forward(state, route, headers, body, target).await
}

fn info_refs_service(uri: &Uri) -> Option<&'static str> {
    let query = uri.query()?;
    let mut parts = query.split('&');
    let first = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (key, value) = first.split_once('=')?;
    if key != "service" {
        return None;
    }
    match value {
        "git-upload-pack" => Some("git-upload-pack"),
        "git-receive-pack" => Some("git-receive-pack"),
        _ => None,
    }
}

fn valid_alias(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_refs_requires_exactly_one_known_service() {
        let uri = |value: &str| value.parse::<Uri>().unwrap();
        assert_eq!(
            info_refs_service(&uri("/git/1/origin/info/refs?service=git-upload-pack")),
            Some("git-upload-pack")
        );
        assert_eq!(
            info_refs_service(&uri("/git/1/origin/info/refs?service=git-receive-pack")),
            Some("git-receive-pack")
        );
        assert_eq!(
            info_refs_service(&uri("/git/1/origin/info/refs?service=git-invalid")),
            None
        );
        assert_eq!(
            info_refs_service(&uri("/git/1/origin/info/refs?service=git-upload-pack&x=1")),
            None
        );
        assert_eq!(
            info_refs_service(&uri(
                "/git/1/origin/info/refs?service=git-upload-pack&service=git-receive-pack"
            )),
            None
        );
        assert_eq!(info_refs_service(&uri("/git/1/origin/info/refs")), None);
    }
}
