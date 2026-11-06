use std::path::PathBuf;
use std::time::Duration;

use crate::config::{RunnerBackendKind, RunnerManagerConfig};
use runner_protocol::GitObjectFormat;

use super::helper::{
    GitVerifyConfigError, GitVerifyLimits, HelperPaths, HelperPlanError, fixed_environment,
    plan_command, plan_command_with_output_limit,
};
use super::verify::TrustedGitCommand;

fn paths() -> HelperPaths {
    let root = PathBuf::from("/var/lib/desk-foreman/verify/op-1");
    HelperPaths {
        repo_dir: root.join("repo"),
        home_dir: root.join("home"),
        template_dir: root.join("template"),
        operation_root: root,
        uid: 1000,
        gid: 1000,
    }
}

fn limits() -> GitVerifyLimits {
    GitVerifyLimits {
        timeout: Duration::from_secs(600),
        max_output_bytes: 262_144,
        memory_limit: "1g".to_owned(),
        cpu_limit: "2".to_owned(),
        pids_limit: 256,
        image: format!("registry.example/runner@sha256:{}", "a".repeat(64)),
    }
}

fn config() -> RunnerManagerConfig {
    RunnerManagerConfig {
        control_plane_url: None,
        manager_id: "test".to_owned(),
        bind_addr: "127.0.0.1:0".to_owned(),
        auth_token: "token".to_owned(),
        backend: RunnerBackendKind::Direct,
        workspace_root: PathBuf::from("/workspace"),
        host_workspace_root: PathBuf::from("/workspace"),
        image: format!("runner@sha256:{}", "a".repeat(64)),
        git_image: Some(format!("runner@sha256:{}", "a".repeat(64))),
        git_private_root: None,
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

#[test]
fn exact_args_use_only_the_private_root_and_closed_environment() {
    let plan = plan_command(
        &paths(),
        &TrustedGitCommand::Fsck,
        &limits(),
        Duration::from_secs(600),
    )
    .expect("plan");

    assert_eq!(&plan.docker_args()[..2], ["docker", "run"]);
    assert!(plan.docker_args().contains(&"--read-only".to_owned()));
    assert!(plan.docker_args().contains(&"--pull=never".to_owned()));
    plan.validate_for_execution().expect("valid plan");
    assert!(plan.docker_args().contains(&"--network".to_owned()));
    assert!(plan.docker_args().contains(&"none".to_owned()));
    assert!(plan.docker_args().contains(&"--cap-drop".to_owned()));
    assert!(plan.docker_args().contains(&"ALL".to_owned()));
    assert!(
        plan.docker_args()
            .contains(&"no-new-privileges:true".to_owned())
    );
    assert_eq!(
        plan.docker_args()
            .iter()
            .filter(|arg| *arg == "--mount")
            .count(),
        1
    );
    let mount = format!(
        "type=bind,source={},target=/git-verify,ro",
        paths().operation_root.display()
    );
    assert!(plan.docker_args().contains(&mount));
    assert!(
        plan.docker_args()
            .contains(&"/tmp:rw,nosuid,nodev,noexec,size=262144".to_owned())
    );
    assert!(plan.docker_args().contains(&"--shm-size".to_owned()));
    assert!(plan.docker_args().contains(&"262144".to_owned()));
    assert!(plan.docker_args().contains(&"fsize=262144".to_owned()));
    assert!(!plan.docker_args().iter().any(|argument| {
        argument.contains("/workspace")
            || argument.contains("docker.sock")
            || argument == "--privileged"
            || argument == "--pid"
            || argument == "--ipc"
            || argument == "--device"
            || argument == "--env-file"
            || argument == "-v"
            || argument == "--volume"
    }));

    let user = plan
        .docker_args()
        .iter()
        .position(|argument| argument == "--user")
        .expect("user flag");
    assert_eq!(plan.docker_args()[user + 1], "1000:1000");

    let git = plan
        .docker_args()
        .iter()
        .position(|argument| argument == "git")
        .expect("git program");
    assert_eq!(plan.docker_args()[git - 1], limits().image);

    for (name, value) in fixed_environment() {
        assert!(
            plan.docker_args().contains(&format!("{name}={value}")),
            "missing fixed env {name}"
        );
    }
    assert_eq!(
        plan.docker_args()
            .iter()
            .filter(|arg| *arg == "--env")
            .count(),
        fixed_environment().len()
    );
    for forbidden in [
        "SSH_AUTH_SOCK",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "SSH_PRIVATE_KEY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "LD_PRELOAD",
    ] {
        assert!(
            !plan
                .docker_args()
                .iter()
                .any(|argument| argument.contains(forbidden)),
            "inherited {forbidden}"
        );
    }
    assert_eq!(plan.deadline(), Duration::from_secs(600));
    assert_eq!(plan.max_output_bytes(), 262_144);
}

#[test]
fn mount_mode_and_scratch_caps_are_derived_and_not_forgeable() {
    let fsck = plan_command(
        &paths(),
        &TrustedGitCommand::Fsck,
        &limits(),
        Duration::from_secs(600),
    )
    .expect("fsck plan");
    fsck.validate_for_invocation(&TrustedGitCommand::Fsck)
        .expect("ro fsck");
    let mut forged = fsck.docker_args().to_vec();
    let position = forged
        .iter()
        .position(|argument| argument.ends_with(",target=/git-verify,ro"))
        .expect("mount");
    forged[position] = forged[position].replace(",ro", ",rw");
    let forged = fsck.clone().test_with_docker_args(forged);
    assert_eq!(
        forged.validate_for_invocation(&TrustedGitCommand::Fsck),
        Err(HelperPlanError::UnsafePath)
    );

    let init_command = TrustedGitCommand::Init {
        object_format: GitObjectFormat::Sha1,
        initial_branch: "main".to_owned(),
    };
    let init = plan_command(&paths(), &init_command, &limits(), Duration::from_secs(600))
        .expect("init plan");
    init.validate_for_invocation(&init_command)
        .expect("rw init");
    let mut forged = init.docker_args().to_vec();
    let position = forged
        .iter()
        .position(|argument| argument.ends_with(",target=/git-verify,rw"))
        .expect("mount");
    forged[position] = forged[position].replace(",rw", ",ro");
    let forged = init.clone().test_with_docker_args(forged);
    assert_eq!(
        forged.validate_for_invocation(&init_command),
        Err(HelperPlanError::UnsafePath)
    );

    let mut duplicated = fsck.docker_args().to_vec();
    duplicated.extend(["--shm-size".to_owned(), "262144".to_owned()]);
    assert_eq!(
        fsck.clone()
            .test_with_docker_args(duplicated)
            .validate_for_invocation(&TrustedGitCommand::Fsck),
        Err(HelperPlanError::UnsafePath)
    );

    let mut wrong_scratch = fsck.docker_args().to_vec();
    let position = wrong_scratch
        .iter()
        .position(|argument| argument == "fsize=262144")
        .expect("ulimit");
    wrong_scratch[position] = "fsize=1024".to_owned();
    assert_eq!(
        fsck.clone()
            .test_with_docker_args(wrong_scratch)
            .validate_for_invocation(&TrustedGitCommand::Fsck),
        Err(HelperPlanError::UnsafePath)
    );
}

#[test]
fn plans_are_fresh_per_step_and_share_the_private_mount() {
    let first = plan_command(
        &paths(),
        &TrustedGitCommand::Fsck,
        &limits(),
        Duration::from_secs(600),
    )
    .expect("first plan");
    let second = plan_command(
        &paths(),
        &TrustedGitCommand::Fsck,
        &limits(),
        Duration::from_secs(599),
    )
    .expect("second plan");
    assert_ne!(first.container_name(), second.container_name());
    let mount = format!(
        "type=bind,source={},target=/git-verify,ro",
        paths().operation_root.display()
    );
    assert!(first.docker_args().contains(&mount));
    assert!(second.docker_args().contains(&mount));
}

#[test]
fn cleanup_is_the_exact_operation_container() {
    let plan = plan_command(
        &paths(),
        &TrustedGitCommand::ShowObjectFormat,
        &limits(),
        Duration::from_secs(600),
    )
    .expect("plan");
    assert_eq!(
        plan.cleanup_args(),
        vec![
            "docker".to_owned(),
            "rm".to_owned(),
            "-f".to_owned(),
            plan.container_name().to_owned()
        ]
    );
    assert!(
        !plan
            .cleanup_args()
            .iter()
            .any(|argument| argument.contains('*') || argument == "--all")
    );
}

#[test]
fn bad_images_limits_users_and_paths_are_rejected() {
    let mut bad = limits();
    bad.image = "runner:latest".to_owned();
    assert_eq!(
        plan_command(
            &paths(),
            &TrustedGitCommand::Fsck,
            &bad,
            Duration::from_secs(600)
        ),
        Err(HelperPlanError::UnpinnedImage)
    );

    let mut bad = limits();
    bad.image = format!("runner@sha256:{}", "A".repeat(64));
    assert_eq!(
        plan_command(
            &paths(),
            &TrustedGitCommand::Fsck,
            &bad,
            Duration::from_secs(600)
        ),
        Err(HelperPlanError::InvalidImage)
    );

    let mut bad = limits();
    bad.image = format!("runner@sha256:{}", "a".repeat(63));
    assert_eq!(
        plan_command(
            &paths(),
            &TrustedGitCommand::Fsck,
            &bad,
            Duration::from_secs(600)
        ),
        Err(HelperPlanError::InvalidImage)
    );

    let mut bad = limits();
    bad.memory_limit = "0g".to_owned();
    assert_eq!(
        plan_command(
            &paths(),
            &TrustedGitCommand::Fsck,
            &bad,
            Duration::from_secs(600)
        ),
        Err(HelperPlanError::InvalidLimits)
    );

    let mut bad = limits();
    bad.memory_limit = "unlimited".to_owned();
    assert_eq!(
        plan_command(
            &paths(),
            &TrustedGitCommand::Fsck,
            &bad,
            Duration::from_secs(600)
        ),
        Err(HelperPlanError::InvalidLimits)
    );

    let mut bad = limits();
    bad.cpu_limit = "0".to_owned();
    assert_eq!(
        plan_command(
            &paths(),
            &TrustedGitCommand::Fsck,
            &bad,
            Duration::from_secs(600)
        ),
        Err(HelperPlanError::InvalidLimits)
    );

    let mut bad = limits();
    bad.timeout = Duration::ZERO;
    assert_eq!(
        plan_command(
            &paths(),
            &TrustedGitCommand::Fsck,
            &bad,
            Duration::from_secs(600)
        ),
        Err(HelperPlanError::InvalidLimits)
    );

    let mut bad = paths();
    bad.uid = 0;
    assert_eq!(
        plan_command(
            &bad,
            &TrustedGitCommand::Fsck,
            &limits(),
            Duration::from_secs(600)
        ),
        Err(HelperPlanError::RootUser)
    );

    for root in [
        PathBuf::from("/var/lib/desk-foreman/verify/op,1"),
        PathBuf::from("/var/lib/desk-foreman/verify/op\n1"),
        PathBuf::from("/var/lib/desk-foreman/verify/../op"),
        PathBuf::from("relative/op"),
    ] {
        let mut bad = paths();
        bad.operation_root = root;
        bad.repo_dir = bad.operation_root.join("repo");
        bad.home_dir = bad.operation_root.join("home");
        bad.template_dir = bad.operation_root.join("template");
        assert_eq!(
            plan_command(
                &bad,
                &TrustedGitCommand::Fsck,
                &limits(),
                Duration::from_secs(600)
            ),
            Err(HelperPlanError::UnsafePath)
        );
    }
}

#[test]
fn closed_command_arguments_reject_non_hex_values() {
    assert_eq!(
        plan_command(
            &paths(),
            &TrustedGitCommand::CatFileCommit {
                head: "not-hex".to_owned()
            },
            &limits(),
            Duration::from_secs(600)
        ),
        Err(HelperPlanError::InvalidCommandArgument)
    );
    assert_eq!(
        plan_command(
            &paths(),
            &TrustedGitCommand::MergeBaseIsAncestor {
                base: "a".repeat(40),
                head: "b".repeat(39)
            },
            &limits(),
            Duration::from_secs(600)
        ),
        Err(HelperPlanError::InvalidCommandArgument)
    );
    assert_eq!(
        plan_command(
            &paths(),
            &TrustedGitCommand::RevParseCommit {
                head: "g".repeat(40)
            },
            &limits(),
            Duration::from_secs(600)
        ),
        Err(HelperPlanError::InvalidCommandArgument)
    );
}

#[test]
fn export_commands_use_only_the_closed_ref() {
    let branch = "feature/export";
    let show = plan_command(
        &paths(),
        &TrustedGitCommand::ShowRef {
            branch: branch.to_owned(),
        },
        &limits(),
        Duration::from_secs(600),
    )
    .expect("show-ref plan");
    let args = show.docker_args();
    let position = args
        .iter()
        .position(|argument| argument == "show-ref")
        .expect("show-ref");
    assert_eq!(
        &args[position + 1..position + 4],
        ["--verify", "--hash", "refs/heads/feature/export"]
    );
    assert!(!args.iter().any(|argument| argument == "--heads"));

    let bundle = plan_command(
        &paths(),
        &TrustedGitCommand::BundleCreate {
            branch: branch.to_owned(),
        },
        &limits(),
        Duration::from_secs(600),
    )
    .expect("bundle plan");
    let args = bundle.docker_args();
    let position = args
        .iter()
        .position(|argument| argument == "bundle")
        .expect("bundle");
    assert_eq!(
        &args[position + 1..position + 5],
        ["create", "-q", "-", "refs/heads/feature/export"]
    );
}

#[test]
fn export_commands_reject_malformed_branch() {
    for branch in [
        "",
        "-main",
        "/main",
        "main/",
        ".main",
        "main.",
        "main.lock",
        "../main",
        "main/../x",
        "main/./x",
        "main/.hidden",
        "main/x.lock",
        "main//x",
        "main@{x}",
        "main x",
    ] {
        assert_eq!(
            plan_command(
                &paths(),
                &TrustedGitCommand::Init {
                    object_format: GitObjectFormat::Sha1,
                    initial_branch: branch.to_owned(),
                },
                &limits(),
                Duration::from_secs(600)
            ),
            Err(HelperPlanError::InvalidCommandArgument),
            "branch {branch:?}"
        );
        assert_eq!(
            plan_command(
                &paths(),
                &TrustedGitCommand::ShowRef {
                    branch: branch.to_owned()
                },
                &limits(),
                Duration::from_secs(600)
            ),
            Err(HelperPlanError::InvalidCommandArgument),
            "branch {branch:?}"
        );
    }
}

#[test]
fn export_planner_uses_an_explicit_output_limit() {
    let plan = plan_command_with_output_limit(
        &paths(),
        &TrustedGitCommand::BundleCreate {
            branch: "main".to_owned(),
        },
        &limits(),
        Duration::from_secs(600),
        4096,
    )
    .expect("limited plan");
    assert_eq!(plan.max_output_bytes(), 4096);
    let default = plan_command(
        &paths(),
        &TrustedGitCommand::Fsck,
        &limits(),
        Duration::from_secs(600),
    )
    .expect("default plan");
    assert_eq!(default.max_output_bytes(), limits().max_output_bytes);
    assert_eq!(
        plan_command_with_output_limit(
            &paths(),
            &TrustedGitCommand::BundleCreate {
                branch: "main".to_owned()
            },
            &limits(),
            Duration::from_secs(600),
            0
        ),
        Err(HelperPlanError::InvalidLimits)
    );
}

#[test]
fn init_and_object_format_arguments_are_closed() {
    let init = plan_command(
        &paths(),
        &TrustedGitCommand::Init {
            object_format: GitObjectFormat::Sha256,
            initial_branch: "feature/init".to_owned(),
        },
        &limits(),
        Duration::from_secs(600),
    )
    .expect("init plan");
    assert!(
        init.docker_args()
            .contains(&"--object-format=sha256".to_owned())
    );
    assert!(
        init.docker_args()
            .contains(&"--initial-branch=feature/init".to_owned())
    );
    assert!(
        init.docker_args()
            .contains(&"--ref-format=files".to_owned())
    );
    assert!(
        init.docker_args()
            .iter()
            .any(|argument| argument == "/git-verify/repo")
    );
    assert!(
        init.docker_args()
            .iter()
            .any(|argument| argument == "--template=/git-verify/template")
    );
    let mount = format!(
        "type=bind,source={},target=/git-verify,rw",
        paths().operation_root.display()
    );
    assert!(init.docker_args().contains(&mount));

    let show = plan_command(
        &paths(),
        &TrustedGitCommand::ShowObjectFormat,
        &limits(),
        Duration::from_secs(600),
    )
    .expect("show plan");
    assert!(
        show.docker_args()
            .windows(2)
            .any(|pair| pair[0] == "rev-parse" && pair[1] == "--show-object-format")
    );
}

#[test]
fn config_policy_requires_positive_limits_and_digest_image() {
    let mut config = config();
    let limits = GitVerifyLimits::from_config(&config).expect("valid config");
    assert_eq!(limits.timeout, Duration::from_secs(600));
    assert_eq!(limits.max_output_bytes, 262_144);
    assert_eq!(limits.memory_limit, "1g");
    assert_eq!(limits.cpu_limit, "2");
    assert_eq!(limits.pids_limit, 256);

    let dedicated = format!("registry.example/git-helper@sha256:{}", "b".repeat(64));
    config.git_image = Some(dedicated.clone());
    config.image = "runner:latest".to_owned();
    let limits = GitVerifyLimits::from_config(&config).expect("general tag stays independent");
    assert_eq!(limits.image, dedicated);
    assert_eq!(config.image, "runner:latest");

    config.git_image = None;
    assert_eq!(
        GitVerifyLimits::from_config(&config),
        Err(GitVerifyConfigError::UnpinnedImage)
    );

    config.git_image = Some("runner:latest".to_owned());
    assert_eq!(
        GitVerifyLimits::from_config(&config),
        Err(GitVerifyConfigError::UnpinnedImage)
    );

    config.git_image = Some(format!("runner@sha256:{}", "a".repeat(64)));
    config.max_timeout_ms = 0;
    assert_eq!(
        GitVerifyLimits::from_config(&config),
        Err(GitVerifyConfigError::InvalidLimits)
    );

    config.max_timeout_ms = 600_000;
    config.max_output_bytes = 0;
    assert_eq!(
        GitVerifyLimits::from_config(&config),
        Err(GitVerifyConfigError::InvalidLimits)
    );

    config.max_output_bytes = 262_144;
    config.pids_limit = 0;
    assert_eq!(
        GitVerifyLimits::from_config(&config),
        Err(GitVerifyConfigError::InvalidLimits)
    );

    config.pids_limit = 256;
    config.memory_limit = "unlimited".to_owned();
    assert_eq!(
        GitVerifyLimits::from_config(&config),
        Err(GitVerifyConfigError::InvalidMemoryLimit)
    );

    config.memory_limit = "1g".to_owned();
    config.cpu_limit = "nan".to_owned();
    assert_eq!(
        GitVerifyLimits::from_config(&config),
        Err(GitVerifyConfigError::InvalidCpuLimit)
    );

    config.cpu_limit = "2".to_owned();
    config.git_image = Some(format!("runner@sha256:{}", "A".repeat(64)));
    assert_eq!(
        GitVerifyLimits::from_config(&config),
        Err(GitVerifyConfigError::InvalidImage)
    );
}
