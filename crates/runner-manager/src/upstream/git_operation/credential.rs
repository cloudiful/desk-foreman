//! Control-plane credential client for one claimed Git job.
//!
//! Uses only the configured manager origin and manager bearer. The broker URL
//! is never used to reach the control plane, and no secret is logged.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::pin::Pin;

use futures_util::StreamExt;
use runner_protocol::{GitJobCredential, GitJobSpec};
use tokio::time::Instant;

use super::{GitCloneFailure, GitCloneFailureCode};

const MAX_CREDENTIAL_BODY_BYTES: usize = 8192;

pub(crate) type GitCredentialFuture<'a> = Pin<
    Box<dyn std::future::Future<Output = Result<GitJobCredential, GitCloneFailure>> + Send + 'a>,
>;

/// Injected credential source. Production uses the control plane; tests use a
/// fake or a loopback configured-manager fixture.
pub(crate) trait GitCredentialSource: Send + Sync {
    fn fetch<'a>(&'a self, spec: &'a GitJobSpec, deadline: Instant) -> GitCredentialFuture<'a>;
}

pub(crate) fn build_askpass_script() -> &'static str {
    "#!/bin/sh\ncase \"$1\" in\nUsername) printf '%s\\n' broker ;;\nPassword) /bin/cat /git-cred/password ;;\n*) exit 1 ;;\nesac\n"
}

pub(crate) fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), GitCloneFailure> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::CredentialUnavailable))?;
    file.write_all(bytes)
        .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::CredentialUnavailable))?;
    file.sync_all()
        .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::CredentialUnavailable))
}

pub(crate) struct ControlPlaneCredentialSource {
    client: reqwest::Client,
    base_url: String,
    manager_id: i64,
    token: String,
    broker_host: String,
}

impl ControlPlaneCredentialSource {
    pub(crate) fn new(
        base_url: String,
        manager_id: i64,
        token: String,
        broker_host: String,
    ) -> Result<Self, GitCloneFailure> {
        let parsed = reqwest::Url::parse(&base_url)
            .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::CredentialUnavailable))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.username() != ""
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
            || manager_id <= 0
            || token.is_empty()
            || broker_host.is_empty()
        {
            return Err(GitCloneFailure::clean(
                GitCloneFailureCode::CredentialUnavailable,
            ));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::CredentialUnavailable))?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            manager_id,
            token,
            broker_host,
        })
    }

    fn credential_url(&self, job_id: uuid::Uuid) -> String {
        format!(
            "{}/api/internal/runner-managers/{}/git-jobs/{}/credential",
            self.base_url, self.manager_id, job_id
        )
    }

    async fn fetch_inner(
        &self,
        spec: &GitJobSpec,
        deadline: Instant,
    ) -> Result<GitJobCredential, GitCloneFailure> {
        let url = self.credential_url(spec.job_id);
        let mut authorization =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", self.token))
                .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::CredentialUnavailable))?;
        authorization.set_sensitive(true);
        let response = tokio::time::timeout_at(
            deadline,
            self.client
                .get(url)
                .header(reqwest::header::AUTHORIZATION, authorization)
                .header("x-df-git-operation-id", spec.operation_id.to_string())
                .send(),
        )
        .await
        .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::Timeout))?
        .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::CredentialUnavailable))?;
        if !response.status().is_success() {
            return Err(GitCloneFailure::clean(
                GitCloneFailureCode::CredentialUnavailable,
            ));
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::CredentialUnavailable))?;
            if body.len().saturating_add(chunk.len()) > MAX_CREDENTIAL_BODY_BYTES {
                return Err(GitCloneFailure::clean(
                    GitCloneFailureCode::CredentialUnavailable,
                ));
            }
            body.extend_from_slice(&chunk);
        }
        let credential: GitJobCredential = serde_json::from_slice(&body)
            .map_err(|_| GitCloneFailure::clean(GitCloneFailureCode::CredentialUnavailable))?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        if credential.deadline_unix_ms <= now_ms
            || credential.deadline_unix_ms > spec.deadline_unix_ms
            || credential.username != runner_protocol::GIT_JOB_CREDENTIAL_USERNAME
            || !desk_foreman::runner::git_job_credentials::validate_git_job_credential(
                &credential,
                spec.code_project_id,
            )
            || !authority_matches(&credential.remote_url, &self.broker_host)
        {
            return Err(GitCloneFailure::clean(
                GitCloneFailureCode::CredentialUnavailable,
            ));
        }
        Ok(credential)
    }
}

impl GitCredentialSource for ControlPlaneCredentialSource {
    fn fetch<'a>(&'a self, spec: &'a GitJobSpec, deadline: Instant) -> GitCredentialFuture<'a> {
        Box::pin(async move { self.fetch_inner(spec, deadline).await })
    }
}

pub(crate) fn authority_matches(url: &str, broker_host: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" || parsed.username() != "" || parsed.password().is_some() {
        return false;
    }
    let (expected_host, expected_port) = match broker_host.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => (host, port),
            Err(_) => (broker_host, 443),
        },
        None => (broker_host, 443),
    };
    parsed.host_str() == Some(expected_host)
        && parsed.port_or_known_default() == Some(expected_port)
        && parsed.query().is_none()
        && parsed.fragment().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use uuid::Uuid;

    fn spec() -> GitJobSpec {
        GitJobSpec {
            job_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4(),
            owner: runner_protocol::RunnerOwner::ApplicationPrincipal {
                application_id: 1,
                workspace_binding_id: 2,
                principal_digest: "a".repeat(64),
            },
            application_id: 1,
            workspace_binding_id: 2,
            principal_digest: "a".repeat(64),
            code_project_id: Uuid::new_v4(),
            authorization_generation: Uuid::new_v4(),
            canonical_branch: "main".to_owned(),
            expected_base: None,
            reserved_bytes: 1024,
            deadline_unix_ms: i64::MAX,
            workspace_root: "/tmp/private".to_owned(),
        }
    }

    #[tokio::test]
    async fn configured_loopback_manager_delivers_a_validated_credential() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let project = Uuid::new_v4();
        let deadline = i64::MAX;
        let body = serde_json::json!({
            "username": "broker",
            "password": format!("smcpw1.{}.{}", Uuid::new_v4(), "a".repeat(64)),
            "remote_url": format!("https://broker.example.invalid/api/internal/code-projects/{project}/git"),
            "deadline_unix_ms": deadline,
        })
        .to_string();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = vec![0_u8; 4096];
            let read = stream.read(&mut request).await.expect("read");
            let request = String::from_utf8_lossy(&request[..read]).to_string();
            assert!(request.contains("authorization: Bearer token"));
            assert!(request.contains("x-df-git-operation-id:"));
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.expect("write");
        });
        let source = ControlPlaneCredentialSource::new(
            format!("http://{addr}/"),
            7,
            "token".to_owned(),
            "broker.example.invalid".to_owned(),
        )
        .expect("source");
        let mut request = spec();
        request.code_project_id = project;
        let credential = source
            .fetch(&request, Instant::now() + Duration::from_secs(5))
            .await
            .expect("credential");
        assert_eq!(credential.username, "broker");
        server.await.expect("server");
    }

    #[test]
    fn origin_validation_rejects_other_schemes_and_escapes() {
        assert!(
            ControlPlaneCredentialSource::new(
                "file:///tmp/x".to_owned(),
                1,
                "token".to_owned(),
                "broker.example.invalid".to_owned()
            )
            .is_err()
        );
        assert!(
            ControlPlaneCredentialSource::new(
                "http://user:pass@127.0.0.1:1/".to_owned(),
                1,
                "token".to_owned(),
                "broker.example.invalid".to_owned()
            )
            .is_err()
        );
        assert!(
            ControlPlaneCredentialSource::new(
                "http://127.0.0.1:1/base".to_owned(),
                1,
                "token".to_owned(),
                "broker.example.invalid".to_owned()
            )
            .is_err()
        );
    }

    async fn serve_once(response: String) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = vec![0_u8; 4096];
            let _ = stream.read(&mut request).await.expect("read");
            stream.write_all(response.as_bytes()).await.expect("write");
        });
        (format!("http://{addr}/"), server)
    }

    #[tokio::test]
    async fn oversized_or_redirected_credential_responses_are_rejected() {
        let oversized = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 9000\r\nconnection: close\r\n\r\n{}",
            "a".repeat(9000)
        );
        let (base, server) = serve_once(oversized).await;
        let source = ControlPlaneCredentialSource::new(
            base,
            7,
            "token".to_owned(),
            "broker.example.invalid".to_owned(),
        )
        .expect("source");
        assert_eq!(
            source
                .fetch(&spec(), Instant::now() + Duration::from_secs(5))
                .await
                .expect_err("oversized")
                .code,
            GitCloneFailureCode::CredentialUnavailable
        );
        server.await.expect("server");

        let redirect = "HTTP/1.1 302 Found\r\nlocation: https://elsewhere.invalid/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned();
        let (base, server) = serve_once(redirect).await;
        let source = ControlPlaneCredentialSource::new(
            base,
            7,
            "token".to_owned(),
            "broker.example.invalid".to_owned(),
        )
        .expect("source");
        assert_eq!(
            source
                .fetch(&spec(), Instant::now() + Duration::from_secs(5))
                .await
                .expect_err("redirect")
                .code,
            GitCloneFailureCode::CredentialUnavailable
        );
        server.await.expect("server");
    }

    #[tokio::test]
    async fn expired_credential_is_rejected_without_echoing_it() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let project = Uuid::new_v4();
        let body = serde_json::json!({
            "username": "broker",
            "password": format!("smcpw1.{}.{}", Uuid::new_v4(), "a".repeat(64)),
            "remote_url": format!("https://broker.example.invalid/api/internal/code-projects/{project}/git"),
            "deadline_unix_ms": now_ms - 1,
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let (base, server) = serve_once(response).await;
        let source = ControlPlaneCredentialSource::new(
            base,
            7,
            "token".to_owned(),
            "broker.example.invalid".to_owned(),
        )
        .expect("source");
        let mut request = spec();
        request.code_project_id = project;
        let failure = source
            .fetch(&request, Instant::now() + Duration::from_secs(5))
            .await
            .expect_err("expired");
        assert_eq!(failure.code, GitCloneFailureCode::CredentialUnavailable);
        assert!(!format!("{failure:?}").contains("smcpw1"));
        server.await.expect("server");
    }
}
