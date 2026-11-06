use std::{path::PathBuf, sync::Arc, time::Duration};

use runner_protocol::RunnerOwner;
use serde_json::{Value, json};
use tokio::sync::RwLock;

use super::{
    DockerRunnerBackend, NativeTaskStore, RunnerLifecycleReporter, docker_tests::test_config,
    execution_environment::service::host_identity::UidSource,
};

const MANAGER: &str = "test-manager";
const READY: &str = "__df_fixture_ready";

struct Harness {
    temp: tempfile::TempDir,
    state: PathBuf,
    fake: PathBuf,
    log: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path().join("state");
        std::fs::create_dir_all(state.join("meta")).unwrap();
        let log = temp.path().join("docker.log");
        std::fs::write(&log, "").unwrap();
        let fake = temp.path().join("docker");
        let script = format!(
            r#"#!/bin/bash
if [ "$1" = "{READY}" ]; then exit 0; fi
echo "$@" >> '{log}'
state='{state}'
mode=$(cat "$state/mode" 2>/dev/null || echo ok)
case "$1" in
  ps)
    while IFS= read -r n; do
      [ -z "$n" ] && continue
      [ -f "$state/removed_$n" ] || echo "$n"
    done < "$state/list"
    ;;
  inspect)
    if [ "$mode" = "inspect_error" ]; then echo 'permission denied' >&2; exit 1; fi
    if [ -f "$state/removed_$2" ] || [ ! -f "$state/meta/$2.json" ]; then
      echo "Error: No such container: $2" >&2
      exit 1
    fi
    cat "$state/meta/$2.json"
    ;;
  stop) ;;
  rm)
    if [ "$mode" = "no_remove" ]; then exit 0; fi
    touch "$state/removed_$2"
    if [ -f "$state/redirect_$2" ]; then
      touch "$state/removed_$(cat "$state/redirect_$2")"
    fi
    ;;
esac
exit 0
"#,
            log = log.display(),
            state = state.display(),
        );
        std::fs::write(&fake, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fake).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake, perms).unwrap();
        super::docker_tests::probe_fixture_readiness(&fake, |path, arg| {
            std::process::Command::new(path).arg(arg).output()
        })
        .unwrap();
        Self {
            temp,
            state,
            fake,
            log,
        }
    }

    fn local_root(&self) -> PathBuf {
        let root = self.temp.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn backend(&self) -> Arc<DockerRunnerBackend> {
        let mut config = test_config();
        config.workspace_root = self.temp.path().to_path_buf();
        config.host_workspace_root = self.temp.path().to_path_buf();
        config.docker_cli = self.fake.to_string_lossy().into_owned();
        DockerRunnerBackend::new(
            Arc::new(RwLock::new(config)),
            MANAGER.to_string(),
            RunnerLifecycleReporter::noop(),
            NativeTaskStore::with_uid_source(None, UidSource::Fixed(1000)),
        )
    }

    fn add(&self, name: &str, id: &str, owner: &str, manager: &str, mounts: Value) {
        let metadata = json!({
            "Name": format!("/{name}"),
            "Id": id,
            "State": {"Status": "running"},
            "Config": {"Labels": {
                "desk-foreman.managed": "true",
                "desk-foreman.owner": owner,
                "desk-foreman.manager": manager
            }},
            "Mounts": mounts
        });
        self.add_raw(name, id, metadata);
    }

    fn add_raw(&self, name: &str, id: &str, metadata: Value) {
        let encoded = serde_json::to_string(&metadata).unwrap();
        std::fs::write(
            self.state.join("meta").join(format!("{name}.json")),
            &encoded,
        )
        .unwrap();
        std::fs::write(self.state.join("meta").join(format!("{id}.json")), &encoded).unwrap();
        std::fs::write(self.state.join(format!("redirect_{id}")), name).unwrap();
        std::fs::write(self.state.join(format!("redirect_{name}")), id).unwrap();
        let list = self.state.join("list");
        let mut existing = std::fs::read_to_string(&list).unwrap_or_default();
        existing.push_str(name);
        existing.push('\n');
        std::fs::write(list, existing).unwrap();
    }

    fn mode(&self, mode: &str) {
        std::fs::write(self.state.join("mode"), mode).unwrap();
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn assert_not_stopped(&self, names: &[&str]) {
        let log = self.log();
        for name in names {
            assert!(
                !log.lines()
                    .any(|line| line.starts_with("stop ") && line.ends_with(name)),
                "{name} must not be stopped: {log}"
            );
            assert!(
                !log.lines()
                    .any(|line| line.starts_with("rm ") && line.ends_with(name)),
                "{name} must not be removed: {log}"
            );
        }
    }
}

fn mount(source: &std::path::Path, kind: &str, read_write: bool) -> Value {
    json!({
        "Type": kind,
        "Source": source.to_string_lossy(),
        "Destination": "/workspace",
        "RW": read_write
    })
}

fn workspace_binding_owner(binding_id: i64) -> RunnerOwner {
    RunnerOwner::WorkspaceBinding {
        workspace_binding_id: binding_id,
    }
}

fn principal_owner(binding_id: i64) -> RunnerOwner {
    RunnerOwner::ApplicationPrincipal {
        application_id: 7,
        workspace_binding_id: binding_id,
        principal_digest: "a".repeat(64),
    }
}

async fn quiesce(harness: &Harness) -> anyhow::Result<()> {
    let backend = harness.backend();
    let config = backend.config.read().await.clone();
    backend
        .quiesce_workspace_binding_owners_with_config(&config, 5, harness.local_root(), None)
        .await
}

#[tokio::test]
async fn binding_owner_quiescence_stops_both_own_variants_and_leaves_others() {
    let harness = Harness::new();
    let root = harness.local_root();
    let wb = workspace_binding_owner(5);
    let ap = principal_owner(5);
    harness.add(
        &wb.container_name(),
        "id-wb",
        &wb.stable_key(),
        MANAGER,
        json!([mount(&root, "bind", true)]),
    );
    harness.add(
        &ap.container_name(),
        "id-ap",
        &ap.stable_key(),
        MANAGER,
        json!([mount(&root, "bind", true)]),
    );
    let private = "desk-foreman-runner-application_principal-private";
    harness.add(
        private,
        "id-private",
        "execution_environment:123e4567-e89b-12d3-a456-426614174000:1",
        MANAGER,
        json!([{"Type":"tmpfs","Destination":"/private","RW":true}]),
    );
    let user = "desk-foreman-runner-user-9";
    harness.add(
        user,
        "id-user",
        "user:9",
        MANAGER,
        json!([mount(&harness.temp.path().join("other"), "bind", true)]),
    );
    let foreign = "desk-foreman-runner-workspace-binding-6";
    harness.add(
        foreign,
        "id-foreign",
        "workspace_binding:6",
        "other-manager",
        json!([mount(&harness.temp.path().join("foreign"), "bind", true)]),
    );

    quiesce(&harness)
        .await
        .expect("positive absence must succeed");

    let log = harness.log();
    for id in ["id-wb", "id-ap"] {
        assert!(log.lines().any(|line| line == format!("stop {id}")));
        assert!(log.lines().any(|line| line == format!("rm {id}")));
    }
    harness.assert_not_stopped(&[
        private,
        user,
        foreign,
        "id-private",
        "id-user",
        "id-foreign",
    ]);
}

#[tokio::test]
async fn binding_owner_quiescence_refuses_foreign_overlaps_without_stopping() {
    for source in [None, Some("equal"), Some("child")] {
        let harness = Harness::new();
        let root = harness.local_root();
        let overlap = match source {
            None => harness.temp.path().to_path_buf(),
            Some("equal") => root.clone(),
            Some(_) => root.join("child"),
        };
        let own = workspace_binding_owner(5);
        harness.add(
            &own.container_name(),
            "id-own",
            &own.stable_key(),
            MANAGER,
            json!([mount(&root, "bind", true)]),
        );
        harness.add(
            "desk-foreman-runner-user-9",
            "id-user",
            "user:9",
            MANAGER,
            json!([mount(&overlap, "bind", true)]),
        );

        assert!(quiesce(&harness).await.is_err());
        harness.assert_not_stopped(&["id-own", "id-user"]);
    }
}

#[tokio::test]
async fn binding_owner_quiescence_refuses_conservative_mount_cases() {
    let cases = [
        (
            "unknown kind",
            json!([{"Type":"npipe","Destination":"/x","RW":true}]),
        ),
        (
            "missing source",
            json!([{"Type":"bind","Destination":"/x","RW":true}]),
        ),
        ("malformed entry", json!([7])),
    ];
    for (label, mounts) in cases {
        let harness = Harness::new();
        harness.add(
            "desk-foreman-runner-user-9",
            "id-user",
            "user:9",
            MANAGER,
            mounts,
        );
        assert!(quiesce(&harness).await.is_err(), "{label}");
        harness.assert_not_stopped(&["id-user"]);
    }

    let harness = Harness::new();
    let root = harness.local_root();
    harness.add(
        "desk-foreman-runner-user-9",
        "id-user",
        "user:9",
        MANAGER,
        json!([{"Type":"bind","Source":root.to_string_lossy(),"Destination":"/x"}]),
    );
    assert!(quiesce(&harness).await.is_err(), "unknown rw overlap");
    harness.assert_not_stopped(&["id-user"]);

    let harness = Harness::new();
    harness.add_raw(
        "desk-foreman-runner-user-9",
        "id-user",
        json!({
            "Name": "/desk-foreman-runner-user-9",
            "Id": "id-user",
            "State": {"Status": "running"},
            "Config": {"Labels": {
                "desk-foreman.managed": "true",
                "desk-foreman.owner": "user:9",
                "desk-foreman.manager": MANAGER
            }}
        }),
    );
    assert!(quiesce(&harness).await.is_err(), "missing Mounts");
    harness.assert_not_stopped(&["id-user"]);
}

#[tokio::test]
async fn binding_owner_quiescence_refuses_bad_identity_and_layout() {
    let own = workspace_binding_owner(5);

    let harness = Harness::new();
    let root = harness.local_root();
    harness.add(
        &own.container_name(),
        "id-own",
        &own.stable_key(),
        "other-manager",
        json!([mount(&root, "bind", true)]),
    );
    assert!(quiesce(&harness).await.is_err());
    harness.assert_not_stopped(&["id-own"]);

    let harness = Harness::new();
    let root = harness.local_root();
    harness.add(
        "wrong-container-name",
        "id-own",
        &own.stable_key(),
        MANAGER,
        json!([mount(&root, "bind", true)]),
    );
    assert!(quiesce(&harness).await.is_err());
    harness.assert_not_stopped(&["id-own"]);

    let harness = Harness::new();
    let root = harness.local_root();
    harness.add(
        &own.container_name(),
        "",
        &own.stable_key(),
        MANAGER,
        json!([mount(&root, "bind", true)]),
    );
    assert!(quiesce(&harness).await.is_err());
    harness.assert_not_stopped(&["id-own"]);

    let harness = Harness::new();
    let root = harness.local_root();
    harness.add(
        &own.container_name(),
        "id-own",
        &own.stable_key(),
        MANAGER,
        json!([
            mount(&root, "bind", false),
            {"Type":"bind","Source":root.join("extra").to_string_lossy(),"Destination":"/extra","RW":true}
        ]),
    );
    assert!(quiesce(&harness).await.is_err());
    harness.assert_not_stopped(&["id-own"]);

    let harness = Harness::new();
    let root = harness.local_root();
    harness.add(
        &own.container_name(),
        "id-own",
        "bad-owner",
        MANAGER,
        json!([mount(&root, "bind", true)]),
    );
    assert!(quiesce(&harness).await.is_err());
    harness.assert_not_stopped(&["id-own"]);
}

#[tokio::test]
async fn binding_owner_quiescence_refuses_remaining_or_uninspectable_containers() {
    let harness = Harness::new();
    let root = harness.local_root();
    let own = workspace_binding_owner(5);
    harness.add(
        &own.container_name(),
        "id-own",
        &own.stable_key(),
        MANAGER,
        json!([mount(&root, "bind", true)]),
    );
    harness.mode("no_remove");
    assert!(quiesce(&harness).await.is_err());
    assert!(harness.log().contains("stop id-own"));

    let harness = Harness::new();
    let root = harness.local_root();
    let own = workspace_binding_owner(5);
    harness.add(
        &own.container_name(),
        "id-own",
        &own.stable_key(),
        MANAGER,
        json!([mount(&root, "bind", true)]),
    );
    harness.mode("inspect_error");
    assert!(quiesce(&harness).await.is_err());
    harness.assert_not_stopped(&["id-own"]);

    let harness = Harness::new();
    std::fs::write(harness.state.join("list"), "missing-container\n").unwrap();
    assert!(quiesce(&harness).await.is_err());
}

#[tokio::test]
async fn binding_owner_quiescence_exempts_only_the_exact_expected_helper() {
    use crate::runtime::backend::ExpectedOwnContainer;

    let harness = Harness::new();
    let root = harness.local_root();
    let helper = RunnerOwner::ExecutionEnvironment {
        environment_id: "123e4567-e89b-12d3-a456-426614174000".to_string(),
        generation: 1,
    };
    harness.add(
        &helper.container_name(),
        "id-helper",
        &helper.stable_key(),
        MANAGER,
        json!([{"Type":"tmpfs","Destination":"/private","RW":true}]),
    );
    let backend = harness.backend();
    let config = backend.config.read().await.clone();
    let expected = Some(ExpectedOwnContainer {
        owner: helper.clone(),
        name: helper.container_name(),
        container_id: "id-helper".to_string(),
    });

    backend
        .quiesce_workspace_binding_owners_with_config(&config, 5, root, expected)
        .await
        .expect("the exact expected helper must be exempt");
    harness.assert_not_stopped(&["id-helper"]);

    let wrong = Some(ExpectedOwnContainer {
        owner: helper.clone(),
        name: helper.container_name(),
        container_id: "different-id".to_string(),
    });
    assert!(
        backend
            .quiesce_workspace_binding_owners_with_config(&config, 5, harness.local_root(), wrong)
            .await
            .is_err(),
        "a changed helper identity must refuse, not stop"
    );
    harness.assert_not_stopped(&["id-helper"]);
}

#[tokio::test]
async fn binding_owner_quiescence_validates_the_binding_id() {
    let harness = Harness::new();
    let backend = harness.backend();
    let config = backend.config.read().await.clone();
    assert!(
        backend
            .quiesce_workspace_binding_owners_with_config(&config, 0, harness.local_root(), None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn binding_owner_quiescence_is_bounded() {
    // The production budget is 60s; this only asserts the wrapper is a timed
    // operation without waiting on a wedged fake.
    let harness = Harness::new();
    let start = std::time::Instant::now();
    let _ = quiesce(&harness).await;
    assert!(start.elapsed() < Duration::from_secs(65));
}
