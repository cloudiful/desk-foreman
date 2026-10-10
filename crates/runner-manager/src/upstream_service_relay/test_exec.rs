//! Test-only relay execution (issue 749 P2c, `#[cfg(test)]`).
//!
//! The scoped injection the completion decision authorizes: tests build an
//! admitted loopback target with a real stored credential and a real sticky
//! cancel, while production derives its target only from inspected private
//! addresses. No runtime flag, no production bypass.

use super::respond::{answer, send_error};
use super::streams::{ExecStream, OutMsg};
use crate::runtime::execution_environment::service::{cancel, credential, relay};
use runner_protocol::service_relay::{OpenFrame, RelayErrorCode};

/// Test execution: the admitted loopback target is constructed by the test.
///
/// `#[cfg(test)]` on purpose: production must derive its target only from
/// inspected private addresses, which never include loopback.
#[cfg(test)]
pub(crate) struct TestExec {
    agent: std::net::SocketAddr,
    username: String,
    store: credential::ServiceCredentialStore,
    env_name: String,
    cancel: cancel::ServiceCancelRegistry,
}

#[cfg(test)]
impl TestExec {
    pub(crate) fn new(
        agent: std::net::SocketAddr,
        username: String,
        store: credential::ServiceCredentialStore,
        env_name: String,
        cancel: cancel::ServiceCancelRegistry,
    ) -> Self {
        Self {
            agent,
            username,
            store,
            env_name,
            cancel,
        }
    }
}

#[cfg(test)]
impl ExecStream for TestExec {
    async fn exec(&self, open: OpenFrame, body: Vec<u8>, out: tokio::sync::mpsc::Sender<OutMsg>) {
        // Fixed test environment carrying the real stored credential; the
        // wire's environment id is irrelevant to the loopback hop, exactly
        // as the decision scopes it (transport proof, not Docker admission).
        let environment_id = "123e4567-e89b-12d3-a456-426614174000";
        let token = match self.store.read_token(environment_id, 4, &self.env_name) {
            Ok(token) => token,
            Err(_) => {
                send_error(&out, open.stream_id, RelayErrorCode::UpstreamUnavailable).await;
                return;
            }
        };
        let admitted = relay::AdmittedService {
            environment_id: environment_id.to_string(),
            generation: 4,
            target: self.agent.ip(),
            port: self.agent.port(),
            username: self.username.clone(),
            token,
            cancel: self.cancel.register(environment_id),
        };
        answer(&open, body, &admitted, &out).await;
    }
}
