use super::*;
use runner_protocol::workspace_principal::application_principal_owner;
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn admission_requires_exact_204_and_times_out_boundedly() {
    let owner = application_principal_owner(1, 2, "u", "w").unwrap();
    for status in [204, 200, 401, 409, 500] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = vec![0; 8192];
            let n = socket.read(&mut data).await.unwrap();
            let request = String::from_utf8_lossy(&data[..n]);
            assert!(request.starts_with("POST /api/internal/runner-manager/principal-admission "));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-token")
            );
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 {status} result\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        assert_eq!(
            send(
                &client,
                &format!("http://{address}"),
                "test-token",
                &owner,
                "immutable-id"
            )
            .await
            .is_ok(),
            status == 204
        );
        server.await.unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap();
    assert!(
        send(
            &client,
            &format!("http://{address}"),
            "test-token",
            &owner,
            "id"
        )
        .await
        .is_err()
    );
    server.await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn docker_execution_requires_ack_and_targets_inspected_id() {
    use super::super::RunnerBackend;
    use runner_protocol::{RunnerCommandRequest, RunnerShellRequest};
    use std::os::unix::fs::PermissionsExt;
    let owner = application_principal_owner(1, 2, "u", "w").unwrap();
    for status in [204, 200, 401, 409, 500] {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("docker.log");
        let fake = temp.path().join("docker");
        let metadata = serde_json::json!({"Name":format!("/{}", owner.container_name()), "Id":"immutable-id", "State":{"Status":"running"}, "Config":{"Labels":{"desk-foreman.managed":"true", "desk-foreman.owner":owner.stable_key(), "desk-foreman.manager":"test-manager"}}}).to_string();
        std::fs::write(&fake, format!("#!/bin/sh\nif [ \"$1\" = __df_fixture_ready ]; then exit 0; fi\necho \"$@\" >> '{}'\nif [ \"$1\" = inspect ]; then printf '%s\\n' '{}'; fi\n", log.display(), metadata)).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        super::super::docker_tests::probe_fixture_readiness(&fake, |path, arg| {
            std::process::Command::new(path).arg(arg).output()
        })
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut b = [0; 8192];
                socket.read(&mut b).await.unwrap();
                socket.write_all(format!("HTTP/1.1 {status} result\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            }
        });
        let mut config = super::super::docker_tests::test_config();
        config.control_plane_url = Some(format!("http://{addr}"));
        config.docker_cli = fake.to_string_lossy().into();
        let backend = DockerRunnerBackend::new(
            Arc::new(tokio::sync::RwLock::new(config)),
            "test-manager".into(),
            super::super::RunnerLifecycleReporter::noop(),
            super::super::NativeTaskStore::with_uid_source(
                None,
                super::super::execution_environment::service::host_identity::UidSource::Fixed(1000),
            ),
        );
        let shell = RunnerShellRequest {
            owner: owner.clone(),
            workspace_root: "/tmp".into(),
            working_dir: "/tmp".into(),
            shell: "sh".into(),
            login: false,
            tty: false,
            command: "true".into(),
            network_enabled: false,
        };
        let target = backend.prepare_shell_spawn(shell).await;
        assert_eq!(target.is_ok(), status == 204);
        if let Ok(target) = target {
            assert!(target.args.iter().any(|arg| arg == "immutable-id"));
            assert!(!target.args.contains(&owner.container_name()));
        }
        let command = RunnerCommandRequest {
            owner: owner.clone(),
            workspace_root: "/tmp".into(),
            working_dir: "/tmp".into(),
            program: "true".into(),
            args: vec![],
            timeout_ms: None,
            max_output_bytes: None,
            network_enabled: false,
        };
        assert_eq!(backend.run_command(command).await.is_ok(), status == 204);
        server.await.unwrap();
        let calls = std::fs::read_to_string(log).unwrap();
        assert_eq!(
            calls.lines().filter(|s| s.starts_with("exec ")).count(),
            usize::from(status == 204)
        );
        if status == 204 {
            assert!(calls.contains("immutable-id true"));
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn unverified_container_and_missing_url_never_execute() {
    use super::super::RunnerBackend;
    use runner_protocol::RunnerCommandRequest;
    use std::os::unix::fs::PermissionsExt;
    let owner = application_principal_owner(1, 2, "u", "w").unwrap();
    for managed in ["false", "true"] {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("log");
        let fake = temp.path().join("docker");
        let metadata = serde_json::json!({"Name":format!("/{}", owner.container_name()), "Id":"immutable-id", "State":{"Status":"running"}, "Config":{"Labels":{"desk-foreman.managed":managed, "desk-foreman.owner":owner.stable_key(), "desk-foreman.manager":"test-manager"}}}).to_string();
        std::fs::write(&fake, format!("#!/bin/sh\nif [ \"$1\" = __df_fixture_ready ]; then exit 0; fi\necho \"$@\" >> '{}'\nif [ \"$1\" = inspect ]; then printf '%s\\n' '{}'; fi\n", log.display(), metadata)).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        super::super::docker_tests::probe_fixture_readiness(&fake, |path, arg| {
            std::process::Command::new(path).arg(arg).output()
        })
        .unwrap();
        let mut config = super::super::docker_tests::test_config();
        config.docker_cli = fake.to_string_lossy().into();
        let backend = DockerRunnerBackend::new(
            Arc::new(tokio::sync::RwLock::new(config)),
            "test-manager".into(),
            super::super::RunnerLifecycleReporter::noop(),
            super::super::NativeTaskStore::with_uid_source(
                None,
                super::super::execution_environment::service::host_identity::UidSource::Fixed(1000),
            ),
        );
        let command = RunnerCommandRequest {
            owner: owner.clone(),
            workspace_root: "/tmp".into(),
            working_dir: "/tmp".into(),
            program: "true".into(),
            args: vec![],
            timeout_ms: None,
            max_output_bytes: None,
            network_enabled: false,
        };
        assert!(backend.run_command(command).await.is_err());
        assert!(
            !std::fs::read_to_string(log)
                .unwrap()
                .lines()
                .any(|s| s.starts_with("exec "))
        );
    }
}

#[tokio::test]
async fn missing_url_rejects_principal_admission() {
    let config = Arc::new(tokio::sync::RwLock::new(
        super::super::docker_tests::test_config(),
    ));
    let backend = DockerRunnerBackend::new(
        config,
        "test-manager".into(),
        super::super::RunnerLifecycleReporter::noop(),
        super::super::NativeTaskStore::with_uid_source(
            None,
            super::super::execution_environment::service::host_identity::UidSource::Fixed(1000),
        ),
    );
    let owner = application_principal_owner(1, 2, "u", "w").unwrap();
    assert!(admit(&backend, &owner, "immutable-id").await.is_err());
}
