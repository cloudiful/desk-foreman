use std::collections::HashMap;

use runner_protocol::{NativeTaskRuntimeIdentity, RunnerOwner, native_task_workspace_directory};
use uuid::Uuid;

use super::{
    LABEL_ENVIRONMENT, LABEL_GENERATION, LABEL_MANAGED, LABEL_MANAGER, LABEL_NATIVE_BINDING_ID,
    LABEL_NATIVE_PREPARATION_GENERATION, LABEL_NATIVE_SEED_SHA256, LABEL_NATIVE_TASK_ID,
    LABEL_OWNER, NATIVE_TASK_WORKSPACE_ROOT,
    argv::{EnvironmentRunSpec, ServiceRunSpec, build_native_task_run_argv},
    inspect::{
        ContainerMount, EnvContainerInfo, ensure_native_task_container_matches,
        ensure_native_task_stop_mounts_match,
    },
    test_support,
};

fn owner() -> RunnerOwner {
    RunnerOwner::ExecutionEnvironment {
        environment_id: "123e4567-e89b-12d3-a456-426614174000".to_string(),
        generation: 4,
    }
}

fn identity() -> NativeTaskRuntimeIdentity {
    NativeTaskRuntimeIdentity {
        task_id: Uuid::from_u128(7),
        workspace_binding_id: 42,
        manifest_sha256: "a".repeat(64),
        preparation_generation: 4,
    }
}

fn info(task_source: &str) -> EnvContainerInfo {
    info_for(42, task_source)
}

fn info_for(binding_id: i64, task_source: &str) -> EnvContainerInfo {
    let owner = owner();
    let identity = NativeTaskRuntimeIdentity {
        workspace_binding_id: binding_id,
        ..identity()
    };
    let mut labels = HashMap::new();
    for (key, value) in [
        (LABEL_OWNER, owner.stable_key()),
        (LABEL_MANAGED, "true".to_string()),
        (LABEL_MANAGER, "manager-7".to_string()),
        (
            LABEL_ENVIRONMENT,
            "123e4567-e89b-12d3-a456-426614174000".to_string(),
        ),
        (LABEL_GENERATION, "4".to_string()),
        (LABEL_NATIVE_TASK_ID, identity.task_id.to_string()),
        (LABEL_NATIVE_BINDING_ID, binding_id.to_string()),
        (LABEL_NATIVE_SEED_SHA256, identity.manifest_sha256.clone()),
        (LABEL_NATIVE_PREPARATION_GENERATION, "4".to_string()),
    ] {
        labels.insert(key.to_string(), value);
    }
    let workspace_directory = native_task_workspace_directory(binding_id);
    EnvContainerInfo {
        status: "running".to_string(),
        owner_key: Some(owner.stable_key()),
        environment_id: Some("123e4567-e89b-12d3-a456-426614174000".to_string()),
        generation: Some(4),
        image: Some("test-image".to_string()),
        labels,
        user: Some("1000:1000".to_string()),
        working_dir: Some(workspace_directory.clone()),
        privileged: Some(false),
        publish_all_ports: Some(false),
        port_bindings_empty: true,
        mounts: Some(vec![
            ContainerMount {
                kind: Some("volume".to_string()),
                name: Some("df-env-123e4567e89b12d3a456426614174000".to_string()),
                source: Some("/var/lib/docker/volumes/state/_data".to_string()),
                destination: Some("/state".to_string()),
                read_write: Some(true),
            },
            ContainerMount {
                kind: Some("bind".to_string()),
                name: None,
                source: Some(task_source.to_string()),
                destination: Some(workspace_directory),
                read_write: Some(true),
            },
        ]),
    }
}

#[test]
fn native_task_run_argv_contains_exactly_the_state_and_task_mounts() {
    let config = test_support::test_manager_config("docker");
    let command = vec!["/opt/agent".to_string(), "serve --unchanged".to_string()];
    let owner_stable_key = owner().stable_key();
    let service = ServiceRunSpec {
        network: "trusted-service-network",
        credential_env_file: "/private/service.env",
    };
    let spec = EnvironmentRunSpec {
        container_name: "df-env-4",
        owner_stable_key: &owner_stable_key,
        environment_id: "123e4567-e89b-12d3-a456-426614174000",
        generation: 4,
        manager_id: "manager-7",
        volume_name: "df-env-123e4567e89b12d3a456426614174000",
        state_mount_target: "/state",
        state_workdir: "/state/work",
        command: &command,
        service: Some(service),
        volume_uid: 1000,
        volume_gid: 1000,
        config: &config,
    };
    let task = identity();
    let task_source = "/daemon/native/123e4567-e89b-12d3-a456-426614174000/00000000-0000-0000-0000-000000000007/tree";
    let workspace_directory = native_task_workspace_directory(task.workspace_binding_id);
    let argv = build_native_task_run_argv(&spec, &task, task_source);

    assert_eq!(argv.iter().filter(|arg| *arg == "-v").count(), 1);
    assert_eq!(argv.iter().filter(|arg| *arg == "--mount").count(), 1);
    assert!(argv.contains(&"df-env-123e4567e89b12d3a456426614174000:/state".to_string()));
    assert!(argv.contains(&format!(
        "type=bind,source={task_source},target={workspace_directory}"
    )));
    assert!(
        !argv
            .iter()
            .any(|arg| arg == "-p" || arg == "--publish" || arg == "--privileged")
    );
    let workdir = argv
        .iter()
        .position(|arg| arg == "--workdir")
        .expect("workdir");
    assert_eq!(argv[workdir + 1], workspace_directory);
    assert!(argv.contains(&"1000:1000".to_string()));
    assert!(argv.contains(&format!("DF_EXECUTION_ENVIRONMENT={}", spec.environment_id)));
    assert!(argv.contains(&format!("DF_GENERATION={}", spec.generation)));
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["--network", "trusted-service-network"])
    );
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["--env-file", "/private/service.env"])
    );
    assert!(argv.contains(&format!("{LABEL_NATIVE_TASK_ID}={}", task.task_id)));
    assert!(argv.contains(&format!("{LABEL_NATIVE_BINDING_ID}=42")));
    assert!(argv.contains(&format!(
        "{LABEL_NATIVE_SEED_SHA256}={}",
        task.manifest_sha256
    )));
    assert!(argv.contains(&format!("{LABEL_NATIVE_PREPARATION_GENERATION}=4")));
    assert_eq!(&argv[argv.len() - 2..], command.as_slice());
}

#[test]
fn two_bindings_launch_into_distinct_checkout_directories() {
    let config = test_support::test_manager_config("docker");
    let command = vec!["/opt/agent".to_string()];
    let owner_stable_key = owner().stable_key();
    let spec = EnvironmentRunSpec {
        container_name: "df-env-4",
        owner_stable_key: &owner_stable_key,
        environment_id: "123e4567-e89b-12d3-a456-426614174000",
        generation: 4,
        manager_id: "manager-7",
        volume_name: "df-env-123e4567e89b12d3a456426614174000",
        state_mount_target: "/state",
        state_workdir: "/state/work",
        command: &command,
        service: None,
        volume_uid: 1000,
        volume_gid: 1000,
        config: &config,
    };
    let source = "/daemon/native/environment/task/tree";
    let directory_for = |binding_id: i64| {
        let argv = build_native_task_run_argv(
            &spec,
            &NativeTaskRuntimeIdentity {
                workspace_binding_id: binding_id,
                ..identity()
            },
            source,
        );
        let mount = argv
            .iter()
            .find(|arg| arg.starts_with("type=bind,"))
            .expect("task mount")
            .clone();
        let workdir = argv[argv
            .iter()
            .position(|arg| arg == "--workdir")
            .expect("workdir")
            + 1]
        .clone();
        assert_eq!(
            mount,
            format!("type=bind,source={source},target={workdir}"),
            "the mount destination and the container working directory are one directory"
        );
        assert!(mount.starts_with(&format!(
            "type=bind,source={source},target={NATIVE_TASK_WORKSPACE_ROOT}/"
        )));
        workdir
    };

    let first = directory_for(42);
    let second = directory_for(7);
    assert_eq!(first, "/workspace/bindings/42");
    assert_eq!(second, "/workspace/bindings/7");
    assert_ne!(first, second, "two bindings never share one checkout");
    assert_ne!(first, NATIVE_TASK_WORKSPACE_ROOT);
    assert_ne!(second, NATIVE_TASK_WORKSPACE_ROOT);
}

#[test]
fn a_checkout_created_for_another_binding_is_not_reused() {
    let owner = owner();
    let task = identity();
    let source = "/daemon/native/environment/task/tree";
    let expected_volume = "df-env-123e4567e89b12d3a456426614174000";
    let validate = |info: &EnvContainerInfo| {
        ensure_native_task_container_matches(
            info,
            &owner,
            "manager-7",
            "test-image",
            &task,
            expected_volume,
            "/state",
            std::path::Path::new(source),
            1000,
            1000,
        )
    };

    assert!(validate(&info_for(7, source)).is_err());

    let mut shared_root_workdir = info(source);
    shared_root_workdir.working_dir = Some(NATIVE_TASK_WORKSPACE_ROOT.to_string());
    assert!(validate(&shared_root_workdir).is_err());

    let mut shared_root_mount = info(source);
    shared_root_mount.mounts.as_mut().expect("mounts")[1].destination =
        Some(NATIVE_TASK_WORKSPACE_ROOT.to_string());
    assert!(validate(&shared_root_mount).is_err());

    let mut parent_of_bind = info(source);
    parent_of_bind.mounts.as_mut().expect("mounts")[1].destination =
        Some(NATIVE_TASK_WORKSPACE_ROOT.to_string() + "/bindings");
    assert!(validate(&parent_of_bind).is_err());
}

#[test]
fn the_stop_candidate_mount_must_match_its_own_binding_directory() {
    let source = "/daemon/native/environment/task/tree";
    let state_volume = "df-env-123e4567e89b12d3a456426614174000";
    let matches = |info: &EnvContainerInfo| {
        ensure_native_task_stop_mounts_match(info, state_volume, std::path::Path::new(source))
            .is_ok()
    };

    assert!(matches(&info_for(42, source)));
    assert!(matches(&info_for(7, source)));

    let mut other_binding_directory = info_for(42, source);
    other_binding_directory.mounts.as_mut().expect("mounts")[1].destination =
        Some(native_task_workspace_directory(7));
    assert!(!matches(&other_binding_directory));

    let mut shared_root = info_for(42, source);
    shared_root.mounts.as_mut().expect("mounts")[1].destination =
        Some(NATIVE_TASK_WORKSPACE_ROOT.to_string());
    assert!(!matches(&shared_root));

    let mut missing_binding_label = info_for(42, source);
    missing_binding_label.labels.remove(LABEL_NATIVE_BINDING_ID);
    assert!(!matches(&missing_binding_label));

    let mut malformed_binding_label = info_for(42, source);
    malformed_binding_label
        .labels
        .insert(LABEL_NATIVE_BINDING_ID.to_string(), "../../etc".to_string());
    assert!(!matches(&malformed_binding_label));
}

#[test]
fn native_inspect_requires_exact_mounts_identity_user_and_no_published_ports() {
    let owner = owner();
    let task = identity();
    let source = "/daemon/native/environment/task/tree";
    let expected_volume = "df-env-123e4567e89b12d3a456426614174000";
    let validate = |info: &EnvContainerInfo| {
        ensure_native_task_container_matches(
            info,
            &owner,
            "manager-7",
            "test-image",
            &task,
            expected_volume,
            "/state",
            std::path::Path::new(source),
            1000,
            1000,
        )
    };

    assert!(validate(&info(source)).is_ok());

    let mut wrong_volume = info(source);
    wrong_volume.mounts.as_mut().expect("mounts")[0].name = Some("other-volume".to_string());
    assert!(validate(&wrong_volume).is_err());

    let mut wrong_source = info(source);
    wrong_source.mounts.as_mut().expect("mounts")[1].source = Some("/untrusted".to_string());
    assert!(validate(&wrong_source).is_err());

    let mut read_only = info(source);
    read_only.mounts.as_mut().expect("mounts")[1].read_write = Some(false);
    assert!(validate(&read_only).is_err());

    let mut extra_mount = info(source);
    extra_mount
        .mounts
        .as_mut()
        .expect("mounts")
        .push(ContainerMount {
            kind: Some("bind".to_string()),
            name: None,
            source: Some("/other".to_string()),
            destination: Some("/other".to_string()),
            read_write: Some(true),
        });
    assert!(validate(&extra_mount).is_err());

    let mut changed_image = info(source);
    changed_image.image = Some("untrusted-image:latest".to_string());
    assert!(validate(&changed_image).is_err());

    let mut changed_identity = info(source);
    changed_identity
        .labels
        .insert(LABEL_NATIVE_TASK_ID.to_string(), Uuid::nil().to_string());
    assert!(validate(&changed_identity).is_err());

    let mut changed_seed = info(source);
    changed_seed
        .labels
        .insert(LABEL_NATIVE_SEED_SHA256.to_string(), "b".repeat(64));
    assert!(validate(&changed_seed).is_err());

    let mut changed_user = info(source);
    changed_user.user = Some("0:0".to_string());
    assert!(validate(&changed_user).is_err());

    let mut changed_workdir = info(source);
    changed_workdir.working_dir = Some("/state/work".to_string());
    assert!(validate(&changed_workdir).is_err());

    let mut published_ports = info(source);
    published_ports.port_bindings_empty = false;
    assert!(validate(&published_ports).is_err());

    let mut all_ports_published = info(source);
    all_ports_published.publish_all_ports = Some(true);
    assert!(validate(&all_ports_published).is_err());

    let mut privileged = info(source);
    privileged.privileged = Some(true);
    assert!(validate(&privileged).is_err());
}
