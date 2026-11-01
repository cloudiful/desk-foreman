use crate::runtime::{docker::DockerRunnerBackend, docker_lifecycle::owner_from_stable_key};
use runner_protocol::{
    CleanupWorkspaceBindingPrincipalsAck, CleanupWorkspaceBindingPrincipalsRequest, RunnerOwner,
    StopWorkspacePrincipalAck, StopWorkspacePrincipalState as State,
    WorkspacePrincipalCleanupResult,
};

impl DockerRunnerBackend {
    pub(super) async fn stop_isolated_principal(
        &self,
        owner: RunnerOwner,
    ) -> anyhow::Result<StopWorkspacePrincipalAck> {
        let ack = |state, detail: Option<String>| StopWorkspacePrincipalAck { state, detail };
        if !matches!(&owner, RunnerOwner::ApplicationPrincipal { .. }) {
            return Ok(ack(State::Conflict, Some("not a principal owner".into())));
        }
        let lock = self.runner_lock(&owner);
        let _guard = lock.lock().await;
        if self.has_active_operation(&owner.stable_key()) {
            return Ok(ack(State::Busy, None));
        }
        let name = owner.container_name();
        let metadata = match self.inspect_container_metadata(&name).await {
            Ok(None) => return Ok(ack(State::Stopped, None)),
            Ok(Some(metadata)) => metadata,
            Err(error) => return Ok(ack(State::Failed, Some(error.to_string()))),
        };
        if !metadata.matches_principal(&owner, &name, &self.manager_id) {
            return Ok(ack(
                State::Conflict,
                Some("container ownership mismatch".into()),
            ));
        }
        let id = metadata
            .container_id
            .as_deref()
            .expect("validated container id");
        if let Err(error) = self.terminate_container_for_takeover(id).await {
            self.report_cleanup_failed(&name, &error);
            return Ok(ack(State::Failed, Some(error.to_string())));
        }
        match (
            self.inspect_container_metadata(id).await,
            self.inspect_container_metadata(&name).await,
        ) {
            (Ok(None), Ok(None)) => {
                self.report_removed(&name);
                Ok(ack(State::Stopped, None))
            }
            _ => Ok(ack(
                State::Failed,
                Some("container absence not confirmed".into()),
            )),
        }
    }

    async fn binding_principal_scan(
        &self,
        binding_id: i64,
    ) -> anyhow::Result<Vec<(RunnerOwner, bool)>> {
        let mut candidates = std::collections::BTreeSet::new();
        for filter in [
            "label=desk-foreman.managed",
            "label=desk-foreman.owner",
            "name=desk-foreman-runner-application_principal-",
        ] {
            let list = self
                .docker_output(["ps", "-a", "--filter", filter, "--format", "{{.Names}}"])
                .await?;
            candidates.extend(
                list.lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            );
        }
        let mut owners = Vec::new();
        for name in candidates.iter().map(String::as_str) {
            let metadata = self
                .inspect_container_metadata(name)
                .await?
                .ok_or_else(|| anyhow::anyhow!("listed container disappeared during inspection"))?;
            let key = metadata
                .owner_key
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("managed container has no owner label"))?;
            let named_owner = name
                .strip_prefix("desk-foreman-runner-application_principal-")
                .and_then(|rest| {
                    let mut parts = rest.split('-');
                    let (app, binding, digest) = (parts.next()?, parts.next()?, parts.next()?);
                    if parts.next().is_some() {
                        return None;
                    }
                    owner_from_stable_key(&format!(
                        "application_principal:{app}:{binding}:{digest}"
                    ))
                });
            if name.starts_with("desk-foreman-runner-application_principal-")
                && named_owner.is_none()
            {
                anyhow::bail!("principal container name is malformed");
            }
            if matches!(&named_owner, Some(RunnerOwner::ApplicationPrincipal { workspace_binding_id, .. }) if *workspace_binding_id == binding_id)
                && named_owner.as_ref().map(RunnerOwner::stable_key).as_deref() != Some(key)
            {
                anyhow::bail!("principal container name and owner label disagree");
            }
            let owner = match owner_from_stable_key(key) {
                Some(owner) => owner,
                None if key.starts_with("execution_environment:") => continue,
                None => anyhow::bail!("managed container has unverifiable owner label"),
            };
            if let RunnerOwner::ApplicationPrincipal {
                workspace_binding_id,
                ..
            } = &owner
            {
                if *workspace_binding_id != binding_id {
                    continue;
                }
                let verified = metadata.matches_principal(&owner, name, &self.manager_id);
                owners.push((owner, verified));
            }
        }
        Ok(owners)
    }

    pub(super) async fn cleanup_binding_principals(
        &self,
        request: CleanupWorkspaceBindingPrincipalsRequest,
    ) -> anyhow::Result<CleanupWorkspaceBindingPrincipalsAck> {
        let mut result = CleanupWorkspaceBindingPrincipalsAck {
            state: State::Failed,
            owners: Vec::new(),
            detail: None,
        };
        if request.workspace_binding_id <= 0 {
            result.detail = Some("invalid binding".into());
            return Ok(result);
        }
        let owners = match self
            .binding_principal_scan(request.workspace_binding_id)
            .await
        {
            Ok(owners) => owners,
            Err(error) => {
                result.detail = Some(error.to_string());
                return Ok(result);
            }
        };
        for (owner, verified) in owners {
            let ack = if verified {
                self.stop_isolated_principal(owner.clone()).await?
            } else {
                StopWorkspacePrincipalAck {
                    state: State::Conflict,
                    detail: Some("container ownership mismatch".into()),
                }
            };
            result.owners.push(WorkspacePrincipalCleanupResult {
                owner,
                state: ack.state,
                detail: ack.detail,
            });
        }
        if result.owners.iter().any(|r| r.state != State::Stopped) {
            result.detail = Some("one or more owners did not stop".into());
            return Ok(result);
        }
        match self
            .binding_principal_scan(request.workspace_binding_id)
            .await
        {
            Ok(remaining) if remaining.is_empty() => result.state = State::Stopped,
            Ok(_) => result.detail = Some("principal containers remain".into()),
            Err(error) => result.detail = Some(error.to_string()),
        }
        Ok(result)
    }
}
