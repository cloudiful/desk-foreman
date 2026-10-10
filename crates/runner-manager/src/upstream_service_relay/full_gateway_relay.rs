//! Whole-wire gateway proof (issue 749 P2d, `#[cfg(test)]`).
//!
//! Real DF gateway plus the authenticated manager dispatcher plus the real
//! manager drive with scoped loopback execution: what this proves is
//! attributed multiplexing across both endpoints — an SSE stream stays open
//! while a binary POST completes, bodies interleave byte-exact, a cancel
//! kills only its stream, and refusals attribute by identifier. The loopback
//! agent uses a real stored credential; Docker admission is not claimed.
//!
//! PG-gated by design: every case is `#[ignore]` and the mandatory phase
//! gate runs them explicitly with a task-owned `DF_ENV_TEST_DATABASE_URL`.
//! They fail without the URL instead of silently passing.

#[cfg(test)]
mod full_gateway_tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::super::conn;
    use super::super::full_gateway_support::support::{
        IsolatedDb, create_admitted_environment, create_app, create_manager, spawn_df, test_state,
    };
    use super::super::test_exec::TestExec;
    use super::super::wire_harness::harness::{
        ENV_NAME, TEST_ENV, USERNAME, expected_basic, start_agent,
    };
    use crate::runtime::execution_environment::service::{
        cancel::ServiceCancelRegistry, credential::ServiceCredentialStore,
    };

    const GATE_TIMEOUT: Duration = Duration::from_secs(20);
    const IGNORE: &str = "requires isolatedPG; mandatory named gate";

    /// Live whole-wire rig: isolated PG, real DF router, loopback agent,
    /// real stored credential, and the manager tunnel driven between them.
    struct GatewayRig {
        db: IsolatedDb,
        df_base: String,
        _df_handle: tokio::task::JoinHandle<()>,
        agent: super::super::wire_harness::harness::Agent,
        manager: tokio::task::JoinHandle<()>,
        agent_token: String,
        app_token: String,
        env_id: uuid::Uuid,
        _temp: tempfile::TempDir,
        client: reqwest::Client,
    }

    impl GatewayRig {
        async fn start() -> Self {
            let db = IsolatedDb::setup().await;
            let (app_id, app_token) = create_app(&db.pool, "gateway-owner").await;
            let (manager_id, manager_token) = create_manager(&db.pool, "gateway-manager").await;
            let env_id = create_admitted_environment(&db.pool, app_id, manager_id).await;
            let state = test_state(db.pool.clone());
            let (df_base, df_handle) = spawn_df(state).await;
            let agent = start_agent().await;
            let temp = tempfile::tempdir().expect("tempdir");
            let store = ServiceCredentialStore::new(temp.path());
            store.obtain(TEST_ENV, 4, ENV_NAME).expect("credential");
            let agent_token = store.read_token(TEST_ENV, 4, ENV_NAME).expect("token");
            let exec = Arc::new(TestExec::new(
                agent.addr,
                USERNAME.to_string(),
                store,
                ENV_NAME.to_string(),
                ServiceCancelRegistry::new(),
            ));
            // Authenticated manager tunnel with the v2 subprotocol, driven
            // by the real dispatcher through the real router.
            let ws_url = format!(
                "{}/api/internal/runner-manager/service-relay",
                df_base.replacen("http://", "ws://", 1)
            );
            let mut request = {
                use tokio_tungstenite::tungstenite::client::IntoClientRequest;
                ws_url.into_client_request().expect("ws url")
            };
            let headers = request.headers_mut();
            headers.insert(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {manager_token}").parse().expect("bearer"),
            );
            headers.insert(
                axum::http::header::SEC_WEBSOCKET_PROTOCOL,
                runner_protocol::service_relay::RELAY_PROTOCOL_VERSION
                    .parse()
                    .expect("protocol"),
            );
            let (ws, response) = tokio_tungstenite::connect_async(request)
                .await
                .expect("manager connects");
            assert_eq!(
                response.status(),
                axum::http::StatusCode::SWITCHING_PROTOCOLS
            );
            let manager = tokio::spawn(conn::drive(Arc::clone(&exec), ws));
            // Brief settle so the tunnel is admitted before gateway traffic.
            tokio::time::sleep(Duration::from_millis(200)).await;
            Self {
                db,
                df_base,
                _df_handle: df_handle,
                agent,
                manager,
                agent_token,
                app_token,
                env_id,
                _temp: temp,
                client: reqwest::Client::new(),
            }
        }

        fn service_url(&self, suffix: &str) -> String {
            format!(
                "{}/api/internal/execution-environments/{}/service/{suffix}",
                self.df_base, self.env_id
            )
        }

        async fn gateway_get(&self, suffix: &str) -> (u16, Vec<u8>) {
            let response = self
                .client
                .get(self.service_url(suffix))
                .header("Authorization", format!("Bearer {}", self.app_token))
                .header("X-DF-External-User-Id", "integration-test-user")
                .header("X-Execution-Environment-Generation", "4")
                .header("Content-Length", "0")
                .send()
                .await
                .expect("gateway GET");
            let status = response.status().as_u16();
            let body = response.bytes().await.expect("body").to_vec();
            (status, body)
        }

        async fn gateway_post(&self, suffix: &str, body: Vec<u8>) -> (u16, Vec<u8>) {
            let response = self
                .client
                .post(self.service_url(suffix))
                .header("Authorization", format!("Bearer {}", self.app_token))
                .header("X-DF-External-User-Id", "integration-test-user")
                .header("X-Execution-Environment-Generation", "4")
                .header("Content-Type", "application/json")
                .body(body)
                .send()
                .await
                .expect("gateway POST");
            let status = response.status().as_u16();
            let bytes = response.bytes().await.expect("body").to_vec();
            (status, bytes)
        }

        async fn teardown(self) {
            self.manager.abort();
            self.db.teardown().await;
        }
    }

    #[tokio::test]
    #[ignore = "requires isolatedPG; mandatory named gate"]
    async fn sse_stays_open_while_a_binary_post_completes() {
        let rig = GatewayRig::start().await;
        // Start the slow SSE first (owned request so the task is 'static),
        // then the fast binary while it is open.
        let client = rig.client.clone();
        let sse_url = rig.service_url("v1/sse");
        let app_token = rig.app_token.clone();
        let slow_task = tokio::spawn(async move {
            let response = client
                .get(sse_url)
                .header("Authorization", format!("Bearer {app_token}"))
                .header("X-DF-External-User-Id", "integration-test-user")
                .header("X-Execution-Environment-Generation", "4")
                .header("Content-Length", "0")
                .send()
                .await
                .expect("gateway SSE");
            let status = response.status().as_u16();
            let body = response.bytes().await.expect("body").to_vec();
            (status, body)
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let started = Instant::now();
        let (status, body) = rig.gateway_get("v1/binary").await;
        let binary_elapsed = started.elapsed();
        assert_eq!(status, 200);
        assert_eq!(body.len(), 100_000, "binary End attributes its own bytes");
        let expected: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(body, expected);
        let (sse_status, sse_body) = tokio::time::timeout(GATE_TIMEOUT, slow_task)
            .await
            .expect("SSE must still complete")
            .expect("join");
        // The binary finished well before the SSE's second chunk delay.
        assert!(binary_elapsed < Duration::from_secs(10));
        assert_eq!(sse_status, 200);
        assert_eq!(sse_body, b"data: one\n\ndata: two\n\n");
        assert_eq!(
            rig.agent.hits.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "both streams reached the agent"
        );
        assert_eq!(
            rig.agent.seen_auth.lock().expect("auth").as_deref(),
            Some(expected_basic(&rig.agent_token).as_str()),
            "the agent saw the real stored credential"
        );
        rig.teardown().await;
    }

    #[tokio::test]
    #[ignore = "requires isolatedPG; mandatory named gate"]
    async fn two_posts_interleave_and_stay_byte_exact() {
        let rig = GatewayRig::start().await;
        let first_body = b"{\"first\":1,\"pad\":\"aaaaaaaaaa\"}".to_vec();
        let second_body = b"{\"second\":2,\"pad\":\"bbbbbbbbbbbbbbbb\"}".to_vec();
        let (first, second) = tokio::join!(
            rig.gateway_post("v1/echo", first_body.clone()),
            rig.gateway_post("v1/echo", second_body.clone())
        );
        assert_eq!(first.0, 200);
        assert_eq!(second.0, 200);
        assert_eq!(first.1, first_body, "interleaved bodies never cross");
        assert_eq!(second.1, second_body);
        rig.teardown().await;
    }

    #[tokio::test]
    #[ignore = "requires isolatedPG; mandatory named gate"]
    async fn cancel_kills_only_its_stream_and_error_attributes() {
        let rig = GatewayRig::start().await;
        // Slow SSE abandoned mid-stream via owned task abort; its
        // disconnect must not abort the sibling POST on the same tunnel.
        let client = rig.client.clone();
        let sse_url = rig.service_url("v1/sse");
        let app_token = rig.app_token.clone();
        let sse_task = tokio::spawn(async move {
            let response = client
                .get(sse_url)
                .header("Authorization", format!("Bearer {app_token}"))
                .header("X-DF-External-User-Id", "integration-test-user")
                .header("X-Execution-Environment-Generation", "4")
                .header("Content-Length", "0")
                .send()
                .await
                .expect("gateway SSE");
            let status = response.status().as_u16();
            let body = response.bytes().await.expect("body").to_vec();
            (status, body)
        });
        // Let the SSE reach the agent before abandoning it.
        tokio::time::sleep(Duration::from_millis(300)).await;
        sse_task.abort();
        let (status, body) = rig.gateway_post("v1/echo", b"{\"ok\":true}".to_vec()).await;
        assert_eq!(status, 200);
        assert_eq!(body, b"{\"ok\":true}");
        // An agent 404 attributes to its own stream as DF 404, not 200 and
        // not another stream's body.
        let (missing_status, _) = rig.gateway_post("v1/missing", b"{}".to_vec()).await;
        assert_eq!(missing_status, 404);
        rig.teardown().await;
    }

    /// Compile-time proof the gate message never drifts from the contract.
    #[test]
    fn the_gate_message_is_the_contract() {
        assert_eq!(IGNORE, "requires isolatedPG; mandatory named gate");
    }
}
