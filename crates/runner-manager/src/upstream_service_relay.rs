//! Reverse relay tunnel to the control plane (issue 749 P2c).
//!
//! Thin entry: one manager holds one authenticated WebSocket to the control
//! plane for the lifetime of its connectivity, beside the existing upstream
//! pull worker. The connection itself lives in [`conn`]; streams live in
//! [`streams`]. Without a configured control-plane URL the relay stays
//! disabled and startup is unaffected.
//!
//! Reconnects use bounded backoff and redacted logs: a failure names the
//! reason, never the bearer and never the full URL query (there is none).

pub(crate) mod conn;
pub(crate) mod conn_state;
pub(crate) mod dispatch;
#[cfg(test)]
mod full_gateway_db;
#[cfg(test)]
mod full_gateway_relay;
#[cfg(test)]
mod full_gateway_support;
pub(crate) mod respond;
pub(crate) mod streams;
#[cfg(test)]
mod test_exec;
#[cfg(test)]
mod wire_cancel_tests;
#[cfg(test)]
mod wire_harness;
#[cfg(test)]
mod wire_plane;
#[cfg(test)]
mod wire_quota_tests;
#[cfg(test)]
mod wire_tests;

use std::sync::Arc;
use std::time::Duration;

use runner_protocol::service_relay::{MAX_WS_MESSAGE_BYTES, RELAY_PROTOCOL_VERSION};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::config::SharedRunnerManagerConfig;
use crate::runtime::DockerRunnerBackend;
use crate::runtime::execution_environment::ExecutionEnvironmentProfile;

/// Base backoff between reconnect attempts; doubles to [`MAX_BACKOFF`].
const BASE_BACKOFF: Duration = Duration::from_secs(1);
/// Longest wait between reconnect attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Hold the reverse tunnel open, reconnecting across failures.
///
/// Returns only when the control-plane URL is removed from the config.
/// A missing URL disables the relay without failing startup.
pub async fn run(
    config: SharedRunnerManagerConfig,
    docker: Option<Arc<DockerRunnerBackend>>,
) -> anyhow::Result<()> {
    let Some(docker) = docker else {
        tracing::info!("service relay is disabled: no docker backend on this manager");
        return Ok(());
    };
    let mut backoff = BASE_BACKOFF;
    loop {
        let (url, token) = {
            let config = config.read().await;
            (config.control_plane_url.clone(), config.auth_token.clone())
        };
        let Some(url) = url.filter(|url| !url.trim().is_empty()) else {
            tracing::info!("service relay is disabled: no control-plane URL configured");
            return Ok(());
        };
        match drive_once(&url, &token, Arc::clone(&docker)).await {
            Ok(()) => {
                tracing::info!("service relay tunnel closed; reconnecting");
            }
            Err(error) => {
                tracing::warn!(error = %error, "service relay tunnel ended; reconnecting");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Connect once and drive the tunnel until it ends.
async fn drive_once(
    control_plane_url: &str,
    token: &str,
    docker: Arc<DockerRunnerBackend>,
) -> anyhow::Result<()> {
    let url = relay_ws_url(control_plane_url)?;
    let mut request = url
        .into_client_request()
        .map_err(|error| anyhow::anyhow!("invalid relay URL: {error}"))?;
    let headers = request.headers_mut();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {token}")
            .parse()
            .map_err(|_| anyhow::anyhow!("relay bearer does not encode"))?,
    );
    headers.insert(
        axum::http::header::SEC_WEBSOCKET_PROTOCOL,
        RELAY_PROTOCOL_VERSION
            .parse()
            .map_err(|_| anyhow::anyhow!("relay protocol does not encode"))?,
    );
    // Bound message and frame size in the client config, before a read can
    // buffer them: the relay's own 64 KiB transport bound stays enforced by
    // the codec as well, so neither end is the only line of defence.
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_WS_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_WS_MESSAGE_BYTES));
    let (ws, response) = tokio_tungstenite::connect_async_with_config(request, Some(config), false)
        .await
        .map_err(|error| anyhow::anyhow!("relay tunnel connect failed: {error}"))?;
    if response.status() != axum::http::StatusCode::SWITCHING_PROTOCOLS {
        anyhow::bail!("relay tunnel handshake was refused");
    }
    // Fresh trusted configuration per connection: an operator change
    // applies on the next reconnect without a restart.
    let profile = Arc::new(ExecutionEnvironmentProfile::from_env_or_default());
    conn::drive(Arc::new(streams::ProdExec::new(docker, profile)), ws).await;
    Ok(())
}

/// Map the configured control-plane URL onto the relay tunnel URL.
///
/// `http` becomes `ws` and `https` becomes `wss`; anything else is refused
/// rather than guessed, because dialing the wrong scheme would send the
/// manager bearer somewhere unexpected.
fn relay_ws_url(control_plane_url: &str) -> anyhow::Result<String> {
    let trimmed = control_plane_url.trim_end_matches('/');
    if let Some(rest) = trimmed.strip_prefix("https://") {
        Ok(format!(
            "wss://{rest}/api/internal/runner-manager/service-relay"
        ))
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        Ok(format!(
            "ws://{rest}/api/internal/runner-manager/service-relay"
        ))
    } else {
        anyhow::bail!("control-plane URL must start with http:// or https://")
    }
}

#[cfg(test)]
mod tests {
    use super::relay_ws_url;

    #[test]
    fn http_becomes_ws_and_https_becomes_wss() {
        assert_eq!(
            relay_ws_url("http://cp.internal:8080").expect("http"),
            "ws://cp.internal:8080/api/internal/runner-manager/service-relay"
        );
        assert_eq!(
            relay_ws_url("https://cp.internal/").expect("https"),
            "wss://cp.internal/api/internal/runner-manager/service-relay"
        );
    }

    #[test]
    fn a_scheme_that_is_not_http_is_refused_rather_than_guessed() {
        for url in [
            "cp.internal:8080",
            "ws://cp.internal/relay",
            "ftp://cp.internal",
        ] {
            assert!(
                relay_ws_url(url).is_err(),
                "{url} must be refused, not reinterpreted"
            );
        }
    }

    /// End-to-end refusal over a real loopback WebSocket (no Docker, no
    /// agent): a fake control plane opens a stream for a foreign container
    /// and the manager answers one fixed-code `Error`, never a success and
    /// never a diagnostic.
    #[tokio::test]
    async fn a_foreign_stream_is_refused_over_the_wire() {
        use futures_util::{SinkExt, StreamExt};
        use runner_protocol::service_relay::{
            OpenFrame, RelayControl, RelayErrorCode, RelayId, StreamFrame, decode_control,
            encode_control,
        };
        use uuid::Uuid;

        use crate::runtime::execution_environment::service::profile::ServiceProfileConfig;
        use crate::runtime::execution_environment::test_support;

        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("bin");
        std::fs::create_dir_all(&dir).expect("bin");
        let log = temp.path().join("docker.log");
        let json_file = dir.join("foreign.json");
        std::fs::write(
            &json_file,
            test_support::inspect_json("running", "someone-else", test_support::ENV_ID, "4"),
        )
        .expect("inspect json");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
            log.display(),
            json_file.display()
        );
        let cli = test_support::write_executable(&dir, "docker", &script);
        let docker = test_support::backend_with_cli(cli.to_str().expect("cli"));
        let state_root = test_support::state_dir(temp.path());
        std::fs::create_dir_all(&state_root).expect("state");
        let mut profile = test_support::enabled_profile(&state_root);
        profile.service =
            crate::runtime::execution_environment::service::profile::ServiceProfile::Configured(
                ServiceProfileConfig {
                    network: "df-private".to_string(),
                    port: 8443,
                    auth_username: "agent".to_string(),
                    auth_env_name: "DF_AGENT_TOKEN".to_string(),
                },
            );
        let profile = std::sync::Arc::new(profile);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback control plane");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(socket)
                .await
                .expect("handshake");
            let open = OpenFrame {
                stream_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
                environment_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
                generation: 4,
                method: "GET".to_string(),
                path: "v1/models".to_string(),
                query: None,
                allowed_headers: Vec::new(),
                body_length: 0,
            };
            let stream_id = open.stream_id;
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                encode_control(&RelayControl::Open(open))
                    .expect("open")
                    .into(),
            ))
            .await
            .expect("send open");
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                encode_control(&RelayControl::RequestEnd(StreamFrame { stream_id }))
                    .expect("end")
                    .into(),
            ))
            .await
            .expect("send end");
            let reply = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
                .await
                .expect("manager answers")
                .expect("frame")
                .expect("message");
            match reply {
                tokio_tungstenite::tungstenite::Message::Text(text) => text.to_string(),
                other => panic!("expected a text error frame, got {other:?}"),
            }
        });

        let url = format!("ws://{addr}/api/internal/runner-manager/service-relay");
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("manager connects");
        super::conn::drive(
            std::sync::Arc::new(super::streams::ProdExec::new(docker, profile)),
            ws,
        )
        .await;
        let reply = server.await.expect("server finishes");
        match decode_control(&reply).expect("decodable") {
            RelayControl::Error(frame) => {
                assert_eq!(frame.code, RelayErrorCode::UpstreamUnavailable)
            }
            other => panic!("a refused stream must end in Error, got {other:?}"),
        }
        assert!(
            !reply.contains("someone-else"),
            "no label detail may reach the wire"
        );
    }
}
