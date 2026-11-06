use super::StopWorkspacePrincipalRequest;

use crate::lease_test_support as support;

#[test]
fn stop_request_rejects_unknown_fields() {
    assert!(
        serde_json::from_str::<StopWorkspacePrincipalRequest>(
            r#"{"expected_lease_owner":"owner","owner_id":"forged"}"#
        )
        .is_err()
    );
    let request: StopWorkspacePrincipalRequest =
        serde_json::from_str(r#"{"expected_lease_owner":"owner"}"#).unwrap();
    assert_eq!(request.expected_lease_owner, "owner");
}

async fn http(
    address: std::net::SocketAddr,
    method: &str,
    path: &str,
    token: &str,
    claims: &str,
    body: &str,
) -> (u16, serde_json::Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\n{claims}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let status = text
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let body = text.split_once("\r\n\r\n").unwrap().1;
    (
        status,
        serde_json::from_str(body).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
#[ignore = "requires newly initialized disposable P4m PostgreSQL"]
async fn router_stop_is_fenced_and_lease_scoped() -> anyhow::Result<()> {
    use crate::{
        db::{
            principal_admission::{self, AdmissionOutcome},
            queries,
            types::{CreateApplicationTokenRequest, CreateMcpTokenRequest},
        },
        runner::{PullRunnerService, RunnerBroker},
    };
    use runner_protocol::{RunnerJobResult, workspace_principal::application_principal_owner};
    use std::sync::Arc;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&std::env::var("P4M_TEST_DATABASE_URL")?)
        .await?;
    sqlx::raw_sql(include_str!(
        "../../sql/principal_admission/test_lifecycle_seed.sql"
    ))
    .execute(&pool)
    .await?;
    sqlx::raw_sql(include_str!(
        "../../sql/principal_admission/test_coverage_seed.sql"
    ))
    .execute(&pool)
    .await?;
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_coverage_delete.sql"
    ))
    .execute(&pool)
    .await?;
    let token_request = CreateApplicationTokenRequest {
        application_id: 817301,
        token_name: "p4m".into(),
        expires_at: None,
        scopes: None,
        max_timeout_ms: None,
        max_output_bytes: None,
        max_file_bytes: None,
        max_sessions: None,
        network_enabled: None,
    };
    let (token, _) = queries::create_application_token(&pool, &token_request).await?;
    let (user_token, _) = queries::create_mcp_token(
        &pool,
        &CreateMcpTokenRequest {
            user_id: 817301,
            token_name: "p4m-user".into(),
            expires_at: None,
            scopes: None,
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: None,
        },
    )
    .await?;
    let mut restricted = token_request.clone();
    restricted.token_name = "p4m-restricted".into();
    restricted.scopes = Some(vec![]);
    let (restricted_token, _) = queries::create_application_token(&pool, &restricted).await?;
    let temp = tempfile::tempdir()?;
    let mut state = support::fake_state(support::FakeRunnerService::new());
    state.db = pool.clone();
    let mut config = (*state.config).clone();
    config.workspace_root = temp.path().into();
    state.config = Arc::new(config);
    let broker = RunnerBroker::new(pool.clone());
    state.runner_broker = broker.clone();
    state.runner = PullRunnerService::new(broker.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let application = queries::find_application_by_id(&pool, 817301)
        .await?
        .unwrap();
    let server_state = state.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, super::super::router().with_state(server_state))
            .await
            .unwrap()
    });
    let capabilities = "/api/internal/principal-workspace-capabilities";
    assert_eq!(
        http(address, "GET", capabilities, &token, "", "").await,
        (200, serde_json::json!({"protocol_version":1}))
    );
    assert_eq!(
        http(address, "GET", capabilities, "invalid", "", "")
            .await
            .0,
        401
    );
    assert_eq!(
        http(address, "GET", capabilities, &user_token, "", "")
            .await
            .0,
        401
    );
    assert_eq!(std::fs::read_dir(temp.path())?.count(), 0);
    assert_eq!(
        http(address, "GET", capabilities, "p4k5-live", "", "")
            .await
            .0,
        401
    );
    assert!(
        queries::find_workspace_binding_by_resource(&pool, 817301, "artifact", "one")
            .await?
            .is_none()
    );
    let path = "/api/internal/resource-workspaces/artifact/one/workspace-sessions/stop";
    let claims = |external: &str, lease: &str| {
        format!(
            "X-DF-Runner-Isolation: principal\r\nX-DF-External-User-Id: {external}\r\nX-DF-Workspace-Key: artifact:one\r\nX-DF-Lease-Owner: {lease}\r\n"
        )
    };
    let body = r#"{"expected_lease_owner":"person-a"}"#;
    let duplicate = format!(
        "{}X-DF-External-User-Id: person-a\r\n",
        claims("person-a", "person-a")
    );
    assert_eq!(
        http(address, "POST", path, &token, &duplicate, body)
            .await
            .0,
        400
    );
    assert_eq!(
        http(
            address,
            "POST",
            path,
            &token,
            &claims("person-a", "person-b"),
            body
        )
        .await
        .0,
        400
    );
    assert_eq!(
        http(
            address,
            "POST",
            path,
            &token,
            &claims("person-a", "person-a"),
            r#"{"expected_lease_owner":"person-a","owner":"forged"}"#
        )
        .await
        .0,
        400
    );
    assert_eq!(
        http(
            address,
            "POST",
            path,
            &restricted_token,
            &claims("person-a", "person-a"),
            body
        )
        .await
        .0,
        403
    );
    assert_eq!(
        http(
            address,
            "POST",
            path,
            &token,
            &claims("person-a", "person-a"),
            body
        )
        .await
        .0,
        404
    );
    assert!(
        queries::find_workspace_binding_by_resource(&pool, 817301, "artifact", "one")
            .await?
            .is_none()
    );
    let binding = crate::auth::identity::resolve_or_create_resource_binding(
        &state,
        &application,
        "artifact",
        "one",
        "artifact:one",
    )
    .await
    .unwrap();
    let id = binding.workspace_binding_id;
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_p4m_foreign_app.sql"
    ))
    .execute(&pool)
    .await?;
    let mut foreign_request = token_request.clone();
    foreign_request.application_id = 817302;
    let (foreign_token, _) = queries::create_application_token(&pool, &foreign_request).await?;
    assert_eq!(
        http(
            address,
            "POST",
            path,
            &foreign_token,
            &claims("person-a", "person-a"),
            body
        )
        .await
        .0,
        404
    );
    assert!(
        queries::find_workspace_binding_by_resource(&pool, 817302, "artifact", "one")
            .await?
            .is_none()
    );
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_p4m_environment.sql"
    ))
    .execute(&pool)
    .await?;
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_p4m_native.sql"
    ))
    .bind(id)
    .execute(&pool)
    .await?;
    assert_eq!(
        http(
            address,
            "POST",
            path,
            &token,
            &claims("person-a", "person-a"),
            body
        )
        .await
        .0,
        409
    );
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_p4m_native_delete.sql"
    ))
    .execute(&pool)
    .await?;
    queries::acquire_workspace_write_lease(&pool, id, "person-a", 300)
        .await?
        .unwrap();
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_p4m_expire.sql"
    ))
    .bind(id)
    .execute(&pool)
    .await?;
    let owner = application_principal_owner(817301, id, "person-a", "artifact:one").unwrap();
    assert!(matches!(
        principal_admission::admit(&pool, 817601, &owner, "test-container").await?,
        AdmissionOutcome::Admitted(_)
    ));
    let worker = tokio::spawn({
        let broker = broker.clone();
        async move {
            for index in 0..4 {
                let job = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    broker.next_job(817601),
                )
                .await
                .unwrap()
                .unwrap();
                let expected_external = if index == 3 { "person-b" } else { "person-a" };
                let stop: runner_protocol::StopWorkspacePrincipalRequest =
                    serde_json::from_value(job.payload.clone()).unwrap();
                assert_eq!(
                    stop.owner,
                    application_principal_owner(817301, id, expected_external, "artifact:one")
                        .unwrap()
                );
                assert_eq!(
                    stop.principal_id,
                    format!("application:817301:{expected_external}:artifact:one")
                );
                broker
                    .complete_job(
                        817601,
                        RunnerJobResult {
                            job_id: job.job_id,
                            ok: true,
                            result: Some(serde_json::json!({"state":"stopped","detail":null})),
                            error: None,
                        },
                    )
                    .await
                    .unwrap();
            }
        }
    });
    let response = http(
        address,
        "POST",
        path,
        &token,
        &claims("person-a", "person-a"),
        body,
    )
    .await;
    assert_eq!(response.0, 200);
    assert_eq!(
        response.1,
        serde_json::json!({"principal_stopped":true,"lease_released":true,"workspace_binding_id":id})
    );
    assert!(matches!(
        principal_admission::admit(&pool, 817601, &owner, "late").await?,
        AdmissionOutcome::Fenced
    ));
    assert_eq!(
        http(
            address,
            "POST",
            path,
            &token,
            &claims("person-a", "person-a"),
            body
        )
        .await
        .1["lease_released"],
        false
    );
    queries::acquire_workspace_write_lease(&pool, id, "person-b", 300)
        .await?
        .unwrap();
    assert_eq!(
        http(
            address,
            "POST",
            path,
            &token,
            &claims("person-a", "person-a"),
            body
        )
        .await
        .1["lease_released"],
        false
    );
    assert_eq!(
        queries::find_workspace_binding_by_id(&pool, id)
            .await?
            .unwrap()
            .write_lease_owner
            .as_deref(),
        Some("person-b")
    );
    let response = http(
        address,
        "POST",
        path,
        &token,
        &claims("person-b", "person-b"),
        r#"{"expected_lease_owner":"person-b"}"#,
    )
    .await;
    assert_eq!(response.0, 200);
    assert_eq!(response.1["lease_released"], true);
    worker.await?;
    queries::acquire_workspace_write_lease(&pool, id, "person-c", 300)
        .await?
        .unwrap();
    let mut stopping = tokio::spawn({
        let token = token.clone();
        async move {
            let headers = "X-DF-Runner-Isolation: principal\r\nX-DF-External-User-Id: person-c\r\nX-DF-Workspace-Key: artifact:one\r\nX-DF-Lease-Owner: person-c\r\n";
            http(
                address,
                "POST",
                path,
                &token,
                headers,
                r#"{"expected_lease_owner":"person-c"}"#,
            )
            .await
        }
    });
    let job = tokio::time::timeout(std::time::Duration::from_secs(3), broker.next_job(817601))
        .await?
        .unwrap();
    let mut acquiring = tokio::spawn({
        let pool = pool.clone();
        async move { queries::acquire_workspace_write_lease(&pool, id, "person-d", 300).await }
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut acquiring)
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut stopping)
            .await
            .is_err()
    );
    broker
        .complete_job(
            817601,
            RunnerJobResult {
                job_id: job.job_id,
                ok: true,
                result: Some(serde_json::json!({"state":"stopped","detail":null})),
                error: None,
            },
        )
        .await?;
    let stopped = stopping.await?;
    assert_eq!(stopped.0, 200);
    assert_eq!(stopped.1["lease_released"], true);
    assert_eq!(
        acquiring.await??.unwrap().write_lease_owner.as_deref(),
        Some("person-d")
    );
    let timed_out = http(
        address,
        "POST",
        path,
        &token,
        &claims("person-d", "person-d"),
        r#"{"expected_lease_owner":"person-d"}"#,
    )
    .await;
    assert_eq!(timed_out.0, 503);
    assert_eq!(
        queries::find_workspace_binding_by_id(&pool, id)
            .await?
            .unwrap()
            .write_lease_owner
            .as_deref(),
        Some("person-d")
    );
    let d_owner = application_principal_owner(817301, id, "person-d", "artifact:one").unwrap();
    assert!(matches!(
        principal_admission::admit(&pool, 817601, &d_owner, "after-timeout").await?,
        AdmissionOutcome::Fenced
    ));
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_p4m_disable_manager.sql"
    ))
    .execute(&pool)
    .await?;
    let failed = http(
        address,
        "POST",
        path,
        &token,
        &claims("person-d", "person-d"),
        r#"{"expected_lease_owner":"person-d"}"#,
    )
    .await;
    assert_eq!(failed.0, 503);
    assert_eq!(
        queries::find_workspace_binding_by_id(&pool, id)
            .await?
            .unwrap()
            .write_lease_owner
            .as_deref(),
        Some("person-d")
    );
    let fenced = application_principal_owner(817301, id, "person-d", "artifact:one").unwrap();
    assert!(matches!(
        principal_admission::admit(&pool, 817601, &fenced, "after-failure").await?,
        AdmissionOutcome::Fenced
    ));
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_endpoint_manager.sql"
    ))
    .bind(817103_i64)
    .bind("p4m-manager")
    .bind(crate::auth::hash_bearer_token("p4m-manager"))
    .bind(true)
    .execute(&pool)
    .await?;
    assert_eq!(
        http(address, "GET", capabilities, "p4m-manager", "", "")
            .await
            .0,
        401
    );
    server.abort();
    pool.close().await;
    Ok(())
}
