use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

use runner_protocol::{GitRuntimeAvailability, GitRuntimeReason, GitRuntimeStatus};
use tempfile::{Builder, TempDir};

use super::runtime_config::{check_git_runtime, classify_directory_identity};
use crate::config::{RunnerBackendKind, RunnerManagerConfig};
use crate::runtime::execution_environment::service::host_identity::UidSource;

fn fixture_base() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME is required for private fixture directories");
    let ai = home.join(".cache/ai");
    fs::create_dir_all(&ai).expect("create private fixture parent");
    fs::set_permissions(&ai, fs::Permissions::from_mode(0o700))
        .expect("private fixture parent mode");
    let base = ai.join("1180-dfi2b1");
    fs::create_dir_all(&base).expect("create fixture base");
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).expect("fixture base mode");
    base
}

fn fixture() -> TempDir {
    let fixture = Builder::new()
        .prefix("runtime-config-")
        .tempdir_in(fixture_base())
        .expect("fixture");
    mode(fixture.path(), 0o700);
    fixture
}

fn mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode");
}

fn private_dir(parent: &Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    fs::create_dir(&path).expect("private fixture dir");
    mode(&path, 0o700);
    path
}

fn workspace_dir(parent: &Path) -> PathBuf {
    let path = parent.join("workspace");
    fs::create_dir(&path).expect("workspace fixture dir");
    mode(&path, 0o700);
    path
}

fn config(workspace: &Path, private: Option<PathBuf>) -> RunnerManagerConfig {
    RunnerManagerConfig {
        control_plane_url: None,
        manager_id: "test-manager".to_owned(),
        bind_addr: "127.0.0.1:0".to_owned(),
        auth_token: "test-token".to_owned(),
        backend: RunnerBackendKind::Docker,
        workspace_root: workspace.to_path_buf(),
        host_workspace_root: workspace.to_path_buf(),
        image: "general-runner:latest".to_owned(),
        git_image: Some(format!("helper@sha256:{}", "a".repeat(64))),
        git_private_root: private,
        git_host_private_root: None,
        git_network: None,
        git_broker_host: None,
        git_max_fetch_volume_bytes: None,
        git_max_bundle_bytes: None,
        git_max_logical_object_bytes: None,
        git_max_checkout_bytes: None,
        git_max_tree_entries: None,
        workdir: "/workspace".to_owned(),
        network_enabled: false,
        max_output_bytes: 262_144,
        max_timeout_ms: 600_000,
        max_sessions: 1,
        pids_limit: 256,
        memory_limit: "1g".to_owned(),
        cpu_limit: "2".to_owned(),
        idle_ttl: Duration::from_secs(60),
        docker_cli: "docker".to_owned(),
        docker_host: None,
        runtime_class: None,
        native_task_root: None,
        native_task_host_root: None,
    }
}

fn real_uid() -> u32 {
    UidSource::Proc.effective_uid().expect("effective uid")
}

fn status(config: &RunnerManagerConfig, uid: u32) -> (GitRuntimeStatus, Option<GitRuntimeReason>) {
    let availability = check_git_runtime(config, Some(uid));
    (availability.status, availability.reason)
}

#[test]
fn structural_status_matrix_is_fixed_and_ordered() {
    let root = fixture();
    let workspace = workspace_dir(root.path());
    let private = private_dir(root.path(), "private");
    let mut base = config(&workspace, Some(private.clone()));

    assert_eq!(
        check_git_runtime(&base, None).reason,
        Some(GitRuntimeReason::ManagerUidUnavailable)
    );
    assert_eq!(
        status(&base, 0),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::ManagerRunningAsRoot)
        )
    );

    base.backend = RunnerBackendKind::Direct;
    assert_eq!(
        status(&base, real_uid()),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::BackendUnsupported)
        )
    );
    base.backend = RunnerBackendKind::Docker;

    base.git_image = None;
    assert_eq!(
        status(&base, real_uid()),
        (
            GitRuntimeStatus::Unconfigured,
            Some(GitRuntimeReason::ImageMissing)
        )
    );
    base.git_image = Some("helper:latest".to_owned());
    assert_eq!(
        status(&base, real_uid()),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::ImageNotDigestPinned)
        )
    );
    base.git_image = Some(format!("helper@sha256:{}", "A".repeat(64)));
    assert_eq!(
        status(&base, real_uid()),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::ImageInvalid)
        )
    );
    base.git_image = Some(format!("helper@sha256:{}", "a".repeat(64)));
    base.max_timeout_ms = 0;
    assert_eq!(
        status(&base, real_uid()),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::LimitsInvalid)
        )
    );
    base.max_timeout_ms = 600_000;

    base.git_private_root = None;
    assert_eq!(
        status(&base, real_uid()),
        (
            GitRuntimeStatus::Unconfigured,
            Some(GitRuntimeReason::PrivateRootMissing)
        )
    );
    base.git_private_root = Some(private.clone());
    assert_eq!(
        status(&base, real_uid()),
        (GitRuntimeStatus::ConfiguredUnverified, None)
    );
}

#[test]
fn private_root_spelling_is_normalized_without_touching_the_path() {
    let root = fixture();
    let workspace = workspace_dir(root.path());
    let private = private_dir(root.path(), "private");
    let file = root.path().join("file");
    fs::write(&file, b"not a directory").expect("fixture file");
    let non_private = private_dir(root.path(), "non-private");
    mode(&non_private, 0o755);
    let open_ancestor = root.path().join("open");
    fs::create_dir(&open_ancestor).expect("open ancestor");
    mode(&open_ancestor, 0o777);
    let under_open = private_dir(&open_ancestor, "private");

    for (candidate, reason) in [
        (
            PathBuf::from("relative/private"),
            GitRuntimeReason::PrivateRootNotAbsolute,
        ),
        (
            root.path().join("..").join("private"),
            GitRuntimeReason::PrivateRootNotNormalized,
        ),
        (
            PathBuf::from(format!("{},private", private.display())),
            GitRuntimeReason::PrivateRootNotNormalized,
        ),
        (
            PathBuf::from(format!("{}\\private", private.display())),
            GitRuntimeReason::PrivateRootNotNormalized,
        ),
        (
            PathBuf::from(format!("{}/private/", private.display())),
            GitRuntimeReason::PrivateRootNotNormalized,
        ),
        (
            PathBuf::from(format!("{}/pri\nvate", private.display())),
            GitRuntimeReason::PrivateRootNotNormalized,
        ),
        (
            root.path().join("missing"),
            GitRuntimeReason::PrivateRootUnavailable,
        ),
        (file, GitRuntimeReason::PrivateRootNotDirectory),
        (non_private, GitRuntimeReason::PrivateRootNotPrivate),
        (under_open, GitRuntimeReason::PrivateRootAncestorUnsafe),
    ] {
        let check = config(&workspace, Some(candidate.clone()));
        assert_eq!(
            status(&check, real_uid()),
            (GitRuntimeStatus::Invalid, Some(reason)),
            "candidate {candidate:?}"
        );
    }
}

#[test]
fn symlinked_ancestor_and_final_root_are_rejected() {
    let root = fixture();
    let workspace = workspace_dir(root.path());
    let real = private_dir(root.path(), "real");
    let ancestor_link = root.path().join("ancestor-link");
    symlink(&real, &ancestor_link).expect("ancestor link");
    let final_link = root.path().join("final-link");
    symlink(&real, &final_link).expect("final link");

    for candidate in [ancestor_link.join("child"), final_link.clone()] {
        let check = config(&workspace, Some(candidate.clone()));
        assert_eq!(
            status(&check, real_uid()),
            (
                GitRuntimeStatus::Invalid,
                Some(GitRuntimeReason::PrivateRootSymlink)
            ),
            "candidate {candidate:?}"
        );
    }
}

#[test]
fn root_uid_and_gid_zero_policy_is_statistical_not_privileged() {
    assert_eq!(classify_directory_identity(1000, 0o700, 1000, true), Ok(()));
    assert_eq!(
        classify_directory_identity(1001, 0o700, 1000, true),
        Err(GitRuntimeReason::PrivateRootNotOwned)
    );
    assert_eq!(
        classify_directory_identity(1000, 0o750, 1000, true),
        Err(GitRuntimeReason::PrivateRootNotPrivate)
    );
    assert_eq!(classify_directory_identity(0, 0o755, 1000, false), Ok(()));
    assert_eq!(
        classify_directory_identity(1000, 0o755, 1000, false),
        Ok(())
    );
    assert_eq!(
        classify_directory_identity(1000, 0o770, 1000, false),
        Err(GitRuntimeReason::PrivateRootAncestorUnsafe)
    );
    assert_eq!(
        classify_directory_identity(1001, 0o755, 1000, false),
        Err(GitRuntimeReason::PrivateRootNotOwned)
    );
}

#[test]
fn overlap_is_rejected_in_both_directions_and_by_identity() {
    let root = fixture();
    let workspace = workspace_dir(root.path());
    let private = private_dir(root.path(), "private");

    let equal = config(&workspace, Some(workspace.clone()));
    assert_eq!(
        status(&equal, real_uid()),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::PrivateRootOverlapsWorkspace)
        )
    );

    let under = private_dir(&workspace, "private");
    let under_config = config(&workspace, Some(under));
    assert_eq!(
        status(&under_config, real_uid()),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::PrivateRootOverlapsWorkspace)
        )
    );

    let contains_workspace = private_dir(&private, "workspace");
    let contains_config = config(&contains_workspace, Some(private));
    assert_eq!(
        status(&contains_config, real_uid()),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::PrivateRootOverlapsWorkspace)
        )
    );
}

#[test]
fn forbidden_root_identity_that_cannot_be_proven_is_invalid() {
    let root = fixture();
    let workspace = workspace_dir(root.path());
    let private = private_dir(root.path(), "private");

    let mut malformed = config(Path::new("relative"), Some(private.clone()));
    malformed.host_workspace_root = workspace.clone();
    assert_eq!(
        status(&malformed, real_uid()),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::WorkspaceRootUnproven)
        )
    );

    let loop_path = root.path().join("loop");
    symlink(&loop_path, &loop_path).expect("symlink loop");
    let mut unresolved = config(&workspace, Some(private));
    unresolved.native_task_root = Some(loop_path);
    assert_eq!(
        status(&unresolved, real_uid()),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::WorkspaceRootUnproven)
        )
    );
}

#[test]
fn docker_host_classification_is_structural_only() {
    let root = fixture();
    let workspace = workspace_dir(root.path());
    let private = private_dir(root.path(), "private");
    let mut base = config(&workspace, Some(private));
    let uid = real_uid();

    for local in [None, Some(""), Some("unix:///var/run/docker.sock")] {
        base.docker_host = local.map(str::to_owned);
        assert_eq!(
            status(&base, uid),
            (GitRuntimeStatus::ConfiguredUnverified, None)
        );
    }

    for remote in [
        "tcp://daemon:2375",
        "http://daemon:2375",
        "https://daemon:2375",
        "ssh://daemon",
        "daemon:2375",
    ] {
        base.docker_host = Some(remote.to_owned());
        base.git_host_private_root = None;
        assert_eq!(
            status(&base, uid),
            (
                GitRuntimeStatus::Invalid,
                Some(GitRuntimeReason::HostRootRequiredForRemoteDaemon)
            ),
            "remote {remote}"
        );
        base.git_host_private_root = Some(PathBuf::from("/var/lib/desk-foreman/git"));
        assert_eq!(
            status(&base, uid),
            (GitRuntimeStatus::ConfiguredUnverified, None),
            "remote mapped {remote}"
        );
        base.git_host_private_root = Some(PathBuf::from("relative"));
        assert_eq!(
            status(&base, uid),
            (
                GitRuntimeStatus::Invalid,
                Some(GitRuntimeReason::HostRootNotAbsolute)
            )
        );
        base.git_host_private_root = Some(PathBuf::from("/var/lib/../git"));
        assert_eq!(
            status(&base, uid),
            (
                GitRuntimeStatus::Invalid,
                Some(GitRuntimeReason::HostRootNotNormalized)
            )
        );
    }

    for unsupported in ["fd://1", "/var/run/docker.sock", "http://", "tcp://"] {
        base.docker_host = Some(unsupported.to_owned());
        base.git_host_private_root = Some(PathBuf::from("/var/lib/desk-foreman/git"));
        assert_eq!(
            status(&base, uid),
            (
                GitRuntimeStatus::Invalid,
                Some(GitRuntimeReason::DockerHostUnsupported)
            ),
            "unsupported {unsupported}"
        );
    }
}

#[test]
fn availability_wire_shape_contains_no_configuration_values() {
    let root = fixture();
    let workspace = workspace_dir(root.path());
    let private = private_dir(root.path(), "private");
    let mut base = config(&workspace, Some(private.clone()));
    base.docker_host = Some("tcp://daemon:2375".to_owned());
    base.git_host_private_root = Some(PathBuf::from("/var/lib/desk-foreman/git"));

    for (config, expected_status) in [
        (
            {
                let mut value = base.clone();
                value.git_image = None;
                value
            },
            GitRuntimeStatus::Unconfigured,
        ),
        (
            {
                let mut value = base.clone();
                value.git_image = Some("helper:latest".to_owned());
                value
            },
            GitRuntimeStatus::Invalid,
        ),
        (base.clone(), GitRuntimeStatus::ConfiguredUnverified),
    ] {
        let availability = check_git_runtime(&config, Some(real_uid()));
        assert_eq!(availability.status, expected_status);
        let value = serde_json::to_value(availability).expect("serialize");
        let object = value.as_object().expect("object");
        assert_eq!(object.len(), 2);
        assert!(object.contains_key("status"));
        assert!(object.contains_key("reason"));
        let text = value.to_string();
        assert!(!text.contains(private.to_string_lossy().as_ref()));
        assert!(!text.contains(workspace.to_string_lossy().as_ref()));
        assert!(!text.contains("helper"));
        assert!(!text.contains("daemon"));
        assert!(!text.contains("sha256"));
    }

    let configured = GitRuntimeAvailability {
        status: GitRuntimeStatus::ConfiguredUnverified,
        reason: None,
    };
    assert_eq!(
        serde_json::to_string(&configured).expect("serialize"),
        r#"{"status":"configured_unverified","reason":null}"#
    );
}

#[test]
fn checks_never_create_or_change_the_private_root() {
    let root = fixture();
    let workspace = workspace_dir(root.path());
    let missing = root.path().join("missing");
    let missing_config = config(&workspace, Some(missing.clone()));
    assert_eq!(
        status(&missing_config, real_uid()),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::PrivateRootUnavailable)
        )
    );
    assert!(!missing.exists());

    let existing = private_dir(root.path(), "existing");
    mode(&existing, 0o755);
    let existing_config = config(&workspace, Some(existing.clone()));
    assert_eq!(
        status(&existing_config, real_uid()),
        (
            GitRuntimeStatus::Invalid,
            Some(GitRuntimeReason::PrivateRootNotPrivate)
        )
    );
    assert_eq!(
        fs::metadata(&existing)
            .expect("existing root")
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}
