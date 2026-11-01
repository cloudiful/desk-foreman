use std::time::Duration;

use runner_protocol::{PrincipalAdmissionRequest, RunnerOwner};

use super::{docker::DockerRunnerBackend, docker_lifecycle::ContainerMetadata};

#[cfg(test)]
#[path = "principal_admission_tests.rs"]
mod tests;

pub(super) fn verified_id<'a>(
    backend: &DockerRunnerBackend,
    owner: &RunnerOwner,
    metadata: &'a ContainerMetadata,
) -> anyhow::Result<&'a str> {
    if !metadata.matches_principal(owner, &owner.container_name(), &backend.manager_id) {
        anyhow::bail!("principal container identity could not be verified");
    }
    let id = metadata
        .container_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("missing container identity"))?;
    let request = PrincipalAdmissionRequest {
        owner: owner.clone(),
        container_id: id.to_owned(),
    };
    if !request.is_valid() {
        anyhow::bail!("invalid principal container identity");
    }
    Ok(id)
}

pub(super) async fn admit(
    backend: &DockerRunnerBackend,
    owner: &RunnerOwner,
    id: &str,
) -> anyhow::Result<()> {
    let config = backend.config.read().await;
    let url = config
        .control_plane_url
        .clone()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow::anyhow!("principal admission unavailable"))?;
    let token = config.auth_token.clone();
    drop(config);
    if token.is_empty() {
        anyhow::bail!("principal admission unavailable");
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| anyhow::anyhow!("principal admission client unavailable"))?;
    send(&client, &url, &token, owner, id).await
}

async fn send(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    owner: &RunnerOwner,
    id: &str,
) -> anyhow::Result<()> {
    let response = client
        .post(format!(
            "{}/api/internal/runner-manager/principal-admission",
            base.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .json(&PrincipalAdmissionRequest {
            owner: owner.clone(),
            container_id: id.to_owned(),
        })
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("principal admission request failed"))?;
    if response.status() != reqwest::StatusCode::NO_CONTENT {
        anyhow::bail!("principal admission rejected");
    }
    Ok(())
}
