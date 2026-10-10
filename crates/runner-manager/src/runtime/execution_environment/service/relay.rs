//! Manager-side service admission (issue 749 P2c).
//!
//! Runs when one relayed stream's body is complete: under the environment
//! mutex and file reservation it re-checks the durable barrier, the owner
//! labels, and the running state, derives the private target and the
//! credential, and registers a sticky cancel receiver. Both locks are
//! released before anything touches the network, so a slow agent can never
//! wedge ensure or stop. There is no auto ensure or start here: an absent
//! or stopped container is refused, never created.

use std::net::IpAddr;

use runner_protocol::RunnerOwner;
use runner_protocol::execution_environment as protocol;
use tokio::sync::watch;

use super::super::file_lock::{EnvFileLock, lock_holder_token, release_and_finish};
use super::super::inspect::inspect_environment_container;
use super::super::tombstone::{TombstoneStore, ensure_generation_admissible};
use super::super::{ExecutionEnvironmentProfile, inspect::ensure_owner_matches};
use super::credential::ServiceCredentialStore;
use super::target::private_agent_ip;
use crate::runtime::DockerRunnerBackend;

/// Everything one relayed stream needs after admission.
///
/// `Debug` is redacted on purpose: the token must never reach a log line
/// through a derived format.
pub(crate) struct AdmittedService {
    pub(crate) environment_id: String,
    pub(crate) generation: i64,
    pub(crate) target: IpAddr,
    pub(crate) port: u16,
    pub(crate) username: String,
    pub(crate) token: String,
    pub(crate) cancel: watch::Receiver<bool>,
}

impl std::fmt::Debug for AdmittedService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmittedService")
            .field("environment_id", &self.environment_id)
            .field("generation", &self.generation)
            .field("target", &self.target)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Admit one relayed stream against the durable barrier and the live
/// container, then release every lock before returning.
///
/// Fails closed when the profile is disabled or misconfigured, when the
/// barrier already closed the generation, when the container is absent,
/// foreign, or not running, when no private address exists on the trusted
/// network, or when the credential is unavailable. A refused stream never
/// dials out.
pub(crate) async fn admit_service_stream(
    backend: &DockerRunnerBackend,
    profile: &ExecutionEnvironmentProfile,
    environment_id: &str,
    generation: i64,
) -> anyhow::Result<AdmittedService> {
    profile.ensure_enabled()?;
    let service = profile.ensure_service_profile()?.ok_or_else(|| {
        anyhow::anyhow!("execution environment service is not configured on this manager")
    })?;
    let network = service.network.clone();
    let port = service.port;
    let username = service.auth_username.clone();
    let env_name = service.auth_env_name.clone();
    let deadline = profile.operation_deadline()?;
    let state_dir = profile.ensure_state_dir()?.to_path_buf();
    let store = TombstoneStore::open(&state_dir, environment_id)?;

    let lock = backend.environment_runner_lock(environment_id);
    let lock_budget = deadline.remaining()?;
    let _guard = tokio::time::timeout(lock_budget, lock.lock())
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out acquiring execution environment {environment_id} lock within the operation deadline"
            )
        })?;
    deadline.remaining_ms()?;
    let manager_id = backend.manager_id.clone();
    let file_lock = EnvFileLock::acquire(&store.lock_path(), lock_holder_token(&manager_id))?;

    let outcome: anyhow::Result<AdmittedService> = async move {
        deadline.remaining_ms()?;
        let barrier = store.load()?;
        // A stopped or superseded generation is closed: no outbound dial.
        ensure_generation_admissible(&barrier, generation)?;

        let owner = RunnerOwner::ExecutionEnvironment {
            environment_id: environment_id.to_string(),
            generation,
        };
        let container_name = owner.container_name();
        let info = inspect_environment_container(
            backend,
            &container_name,
            deadline.remaining_ms()?,
        )
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "execution environment {environment_id} generation {generation} has no container; refusing to start one for a relayed request"
            )
        })?;
        ensure_owner_matches(&info, &owner)?;
        if info.status != protocol::EXECUTION_ENVIRONMENT_RUNNING_STATE && info.status != "running" {
            anyhow::bail!(
                "execution environment {environment_id} generation {generation} is {}; refusing to relay",
                info.status
            );
        }

        // Second bounded inspect for the network document. The fake-CLI
        // harness answers every inspect identically, so tests stay honest
        // about the two-call shape.
        let document = inspect_network_document(backend, &container_name, deadline.remaining_ms()?)
            .await?;
        let target = private_agent_ip(&document, &network)?;

        let credentials = ServiceCredentialStore::new(&state_dir);
        let token = credentials.read_token(environment_id, generation, &env_name)?;

        let cancel = backend.service_cancel().register(environment_id);
        Ok(AdmittedService {
            environment_id: environment_id.to_string(),
            generation,
            target,
            port,
            username,
            token,
            cancel,
        })
    }
    .await;

    release_and_finish(file_lock, outcome)
}

/// Fetch the full `docker inspect` document for network derivation.
async fn inspect_network_document(
    backend: &DockerRunnerBackend,
    container_name: &str,
    timeout_ms: u64,
) -> anyhow::Result<serde_json::Value> {
    let output = backend
        .docker_output_bounded(
            vec![
                "inspect".to_string(),
                container_name.to_string(),
                "--format".to_string(),
                "{{json .}}".to_string(),
            ],
            timeout_ms,
        )
        .await?;
    if output.timed_out {
        anyhow::bail!("docker inspect timed out for {container_name}");
    }
    if output.exit_code != Some(0) {
        anyhow::bail!("docker inspect failed for {container_name}");
    }
    serde_json::from_str(output.stdout.trim())
        .map_err(|error| anyhow::anyhow!("failed to parse docker inspect metadata: {error}"))
}
