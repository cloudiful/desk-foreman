//! Authenticated reverse-relay tunnel ingress (issue 749 P2b).
//!
//! Thin module entry. A manager opens this endpoint once and keeps it for the
//! lifetime of its connectivity; the control plane then relays a tenant's
//! private agent HTTP through it. The endpoint carries no tenant data of its
//! own: it authenticates the manager, binds the connection to that manager
//! alone, and hands the socket to the connection task.

mod connection;

use axum::{
    extract::{State, WebSocketUpgrade, ws::WebSocket},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use runner_protocol::service_relay::{MAX_WS_MESSAGE_BYTES, RELAY_PROTOCOL_VERSION};

use crate::{AppState, error::AppError};

use super::runner_manager::runner_manager_from_token;

/// Relay protocol negotiated in the manager handshake.
#[utoipa::path(
    get,
    path = "/api/internal/runner-manager/service-relay",
    tag = "internal",
    description = "Authenticated reverse relay tunnel for one runner manager. \
        Negotiates `df-env-http.v2`; a manager may hold only one live tunnel.",
    responses(
        (status = 101),
        (status = 400, body = crate::error::ErrorResponse, description = "Relay subprotocol not offered"),
        (status = 401, body = crate::error::ErrorResponse, description = "Runner manager bearer required"),
        (status = 403, body = crate::error::ErrorResponse, description = "Runner manager is disabled"),
        (status = 409, body = crate::error::ErrorResponse, description = "A live tunnel already exists for this manager")
    )
)]
pub async fn runner_manager_service_relay(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, AppError> {
    // Authenticate before anything else: an unauthenticated peer must not be
    // able to occupy the single tunnel slot of a real manager.
    let manager = runner_manager_from_token(&state, &headers).await?;
    if !offers_relay_protocol(&headers) {
        return Err(AppError::bad_request(format!(
            "relay subprotocol {RELAY_PROTOCOL_VERSION} is required"
        )));
    }
    let registry = state.runner_broker.service_relay().clone();
    let admission = connection::admit(registry.clone(), manager.runner_manager_id)?;
    let manager_id = manager.runner_manager_id;
    let socket = upgrade
        // Bound both the whole message and the single frame at the upgrade,
        // before anything is buffered: the relay's own 64 KiB transport
        // bound is enforced by the codec rather than after a read that
        // already paid for the bytes.
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .max_frame_size(MAX_WS_MESSAGE_BYTES)
        .protocols([RELAY_PROTOCOL_VERSION.to_string()])
        .on_upgrade(move |socket: WebSocket| async move {
            connection::run(registry, manager_id, admission, socket).await;
        });
    Ok(socket.into_response())
}

/// True when the peer offered the relay subprotocol in its handshake.
///
/// Checked rather than assumed: without it the connection would be upgraded
/// and then carry frames no peer in this protocol could interpret.
fn offers_relay_protocol(headers: &HeaderMap) -> bool {
    let Some(offered) = headers.get("sec-websocket-protocol") else {
        return false;
    };
    let Ok(offered) = offered.to_str() else {
        return false;
    };
    offered
        .split(',')
        .any(|value| value.trim().eq_ignore_ascii_case(RELAY_PROTOCOL_VERSION))
}
