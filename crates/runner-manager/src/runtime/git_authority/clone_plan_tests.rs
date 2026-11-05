use super::clone_plan::{GitCloneCommand, GitCloneProfile, GitPlan, GitPlanError};
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn clone_profiles_reject_forged_mount_and_network_combinations() {
    let paths = super::clone_plan::GitClonePaths {
        control_dir: PathBuf::from("/tmp/control"),
        credential_dir: PathBuf::from("/tmp/cred"),
        volume_name: "dfgit-test".to_owned(),
        uid: 1000,
        gid: 1000,
    };
    let limits = super::clone_plan::GitCloneLimits {
        image: format!("helper@sha256:{}", "a".repeat(64)),
        network: "df-git-net".to_owned(),
        broker_host: "broker.example.invalid".to_owned(),
        uid: 1000,
        gid: 1000,
        fetch_volume_bytes: 1024,
        bundle_bytes: 1024,
        logical_object_bytes: 1024,
        checkout_bytes: 1024,
        tree_entries: 8,
        deadline: Duration::from_secs(10),
        memory_limit: "256m".to_owned(),
        cpu_limit: "1".to_owned(),
        pids_limit: 16,
    };
    let proof = GitPlan::new(
        GitCloneCommand::Proof {
            proof_name: "mount-proof-abc".to_owned(),
        },
        &paths,
        &limits,
        "dfgit-proof".to_owned(),
        Duration::from_secs(5),
        128,
        128,
    )
    .expect("proof plan");
    assert_eq!(proof.profile, GitCloneProfile::Proof);
    assert!(proof.execution_args()[0] != "--mount");
    assert!(matches!(
        GitPlan::new(
            GitCloneCommand::Proof {
                proof_name: "../escape".to_owned(),
            },
            &paths,
            &limits,
            "dfgit-proof".to_owned(),
            Duration::from_secs(5),
            128,
            128,
        ),
        Err(GitPlanError::Command)
    ));
}

fn git_command() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    command
}

fn execute_plan(
    plan: &GitPlan,
    image: &str,
    repo: &std::path::Path,
    control: &std::path::Path,
) -> (i32, Vec<u8>, Vec<u8>) {
    let args = plan.docker_args();
    let image_index = args.iter().position(|arg| arg == image).expect("image");
    let mut argv: Vec<String> = args[image_index + 1..].to_vec();
    for argument in &mut argv {
        if argument == "/git-fetch/repo" {
            *argument = repo.to_string_lossy().into_owned();
        } else if let Some(rest) = argument.strip_prefix("/git-control/") {
            *argument = control.join(rest).to_string_lossy().into_owned();
        }
    }
    let output = git_command().args(&argv[1..]).output().expect("host git");
    (
        output.status.code().unwrap_or(-1),
        output.stdout,
        output.stderr,
    )
}

fn plan(
    command: GitCloneCommand,
    _image: &str,
    paths: &super::clone_plan::GitClonePaths,
    limits: &super::clone_plan::GitCloneLimits,
    index: u32,
    stdout: usize,
) -> GitPlan {
    GitPlan::new(
        command,
        paths,
        limits,
        format!("dfgit-real-{index}"),
        Duration::from_secs(10),
        stdout,
        65536,
    )
    .expect("plan")
}

#[test]
fn benign_real_git_executes_the_closed_sequence_and_one_ref_bundle() {
    let temp = tempfile::tempdir().expect("temp");
    let source = temp.path().join("source");
    let repo = temp.path().join("repo");
    let control = temp.path().join("control");
    std::fs::create_dir(&control).expect("control");
    std::fs::write(
        control.join("mount-proof-abc"),
        b"desk-foreman git mount proof v1\n",
    )
    .expect("proof");
    std::fs::create_dir(&source).expect("source");
    let output = git_command()
        .arg("init")
        .arg("-b")
        .arg("main")
        .arg(&source)
        .output()
        .expect("init");
    assert!(output.status.success());
    std::fs::write(source.join("file.txt"), b"hello").expect("file");
    std::fs::write(source.join("run.sh"), b"hello").expect("script");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        source.join("run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("mode");
    std::os::unix::fs::symlink("file.txt", source.join("link")).expect("link");
    let output = git_command()
        .args(["-C", source.to_str().expect("source"), "add", "."])
        .output()
        .expect("add");
    assert!(output.status.success());
    let output = git_command()
        .args([
            "-C",
            source.to_str().expect("source"),
            "commit",
            "-m",
            "one",
        ])
        .output()
        .expect("commit");
    assert!(output.status.success());

    let image = format!("helper@sha256:{}", "a".repeat(64));
    let limits = super::clone_plan::GitCloneLimits {
        image: image.clone(),
        network: "df-git-net".to_owned(),
        broker_host: "broker.example.invalid".to_owned(),
        uid: 1000,
        gid: 1000,
        fetch_volume_bytes: 1024 * 1024,
        bundle_bytes: 1024 * 1024,
        logical_object_bytes: 1024 * 1024,
        checkout_bytes: 1024 * 1024,
        tree_entries: 64,
        deadline: Duration::from_secs(10),
        memory_limit: "256m".to_owned(),
        cpu_limit: "1".to_owned(),
        pids_limit: 16,
    };
    let paths = super::clone_plan::GitClonePaths {
        control_dir: control.clone(),
        credential_dir: temp.path().join("credential"),
        volume_name: "dfgit-real".to_owned(),
        uid: 1000,
        gid: 1000,
    };

    let proof = plan(
        GitCloneCommand::Proof {
            proof_name: "mount-proof-abc".to_owned(),
        },
        &image,
        &paths,
        &limits,
        0,
        256,
    );
    let (code, stdout, _) = execute_plan(&proof, &image, &repo, &control);
    assert_eq!(code, 0);
    assert_eq!(
        String::from_utf8_lossy(&stdout).trim(),
        "62eee4ed5398a1a8f6555a6ede7e20d7634a571e"
    );

    let remote = format!("file://{}", source.display());
    let ls = plan(
        GitCloneCommand::NetLsRemote {
            remote_url: remote.clone(),
            branch: "main".to_owned(),
        },
        &image,
        &paths,
        &limits,
        1,
        65536,
    );
    let (code, stdout, _) = execute_plan(&ls, &image, &repo, &control);
    assert_eq!(code, 0);
    let (head, format) = super::clone_fetch::parse_ls_remote(&stdout, "main").expect("ls-remote");
    assert_eq!(format, runner_protocol::GitObjectFormat::Sha1);

    let init = plan(
        GitCloneCommand::FetchInit {
            object_format: format,
            branch: "main".to_owned(),
        },
        &image,
        &paths,
        &limits,
        2,
        65536,
    );
    assert_eq!(execute_plan(&init, &image, &repo, &control).0, 0);

    let fetch = plan(
        GitCloneCommand::Fetch {
            remote_url: remote,
            branch: "main".to_owned(),
        },
        &image,
        &paths,
        &limits,
        3,
        65536,
    );
    assert_eq!(execute_plan(&fetch, &image, &repo, &control).0, 0);

    let verify = plan(
        GitCloneCommand::Verify {
            head: head.clone(),
            expected_base: None,
        },
        &image,
        &paths,
        &limits,
        4,
        65536,
    );
    let (code, stdout, _) = execute_plan(&verify, &image, &repo, &control);
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), head);

    let local_ref = plan(
        GitCloneCommand::ReadLocalRef {
            branch: "main".to_owned(),
        },
        &image,
        &paths,
        &limits,
        5,
        65536,
    );
    let (code, stdout, _) = execute_plan(&local_ref, &image, &repo, &control);
    assert_eq!(code, 0);
    super::verify_fetched_head(&stdout, "main", &head).expect("local ref");

    let cat_file = plan(
        GitCloneCommand::CatFileCommit { head: head.clone() },
        &image,
        &paths,
        &limits,
        6,
        65536,
    );
    assert_eq!(execute_plan(&cat_file, &image, &repo, &control).0, 0);

    let tree = plan(
        GitCloneCommand::ListTreeSizes { head: head.clone() },
        &image,
        &paths,
        &limits,
        7,
        65536,
    );
    let (code, stdout, _) = execute_plan(&tree, &image, &repo, &control);
    assert_eq!(code, 0);
    let stats = super::clone_fetch::parse_tree_records(&stdout, 64, 1024).expect("tree");
    assert_eq!(stats.entries, 3);
    assert_eq!(stats.files, 2);
    assert_eq!(stats.symlinks, 1);
    assert_eq!(stats.total_bytes, 10);

    let objects = plan(
        GitCloneCommand::ListObjectSizes,
        &image,
        &paths,
        &limits,
        8,
        65536,
    );
    let (code, stdout, _) = execute_plan(&objects, &image, &repo, &control);
    assert_eq!(code, 0);
    let object_stats =
        super::clone_fetch::parse_object_batch(&stdout, 64, 1024 * 1024).expect("objects");
    assert!(object_stats.objects >= 3);

    let ancestry = plan(
        GitCloneCommand::MergeBaseIsAncestor {
            base: head.clone(),
            head: head.clone(),
        },
        &image,
        &paths,
        &limits,
        9,
        65536,
    );
    assert_eq!(execute_plan(&ancestry, &image, &repo, &control).0, 0);

    let bundle = plan(
        GitCloneCommand::BundleCreate {
            branch: "main".to_owned(),
        },
        &image,
        &paths,
        &limits,
        10,
        1024 * 1024,
    );
    let (code, stdout, stderr) = execute_plan(&bundle, &image, &repo, &control);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
    assert!(!stdout.is_empty());
    let bundle_path = temp.path().join("bundle.git");
    std::fs::write(&bundle_path, &stdout).expect("bundle");
    let imported = temp.path().join("imported");
    let output = git_command()
        .arg("clone")
        .arg("--branch")
        .arg("main")
        .arg(&bundle_path)
        .arg(&imported)
        .output()
        .expect("clone bundle");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = git_command()
        .args([
            "-C",
            imported.to_str().expect("imported"),
            "rev-parse",
            "HEAD",
        ])
        .output()
        .expect("rev-parse");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), head);
}

#[test]
fn install_profiles_use_only_bundle_and_install_volume_mounts() {
    use super::clone_plan::GitPlan;
    use super::clone_plan::{
        BUNDLE_TARGET, GitCloneLimits, GitInstallPaths, INSTALL_VOLUME_TARGET,
    };
    let paths = GitInstallPaths {
        bundle_path: PathBuf::from("/private/op/bundle.git"),
        volume_name: "dfgit-install".to_owned(),
        uid: 1000,
        gid: 1000,
    };
    let limits = GitCloneLimits {
        image: format!("helper@sha256:{}", "a".repeat(64)),
        network: "df-git-net".to_owned(),
        broker_host: "broker.example.invalid".to_owned(),
        uid: 1000,
        gid: 1000,
        fetch_volume_bytes: 4096,
        bundle_bytes: 4096,
        logical_object_bytes: 4096,
        checkout_bytes: 4096,
        tree_entries: 8,
        deadline: Duration::from_secs(10),
        memory_limit: "256m".to_owned(),
        cpu_limit: "1".to_owned(),
        pids_limit: 16,
    };
    for (index, command) in [
        GitCloneCommand::InstallClone {
            branch: "main".to_owned(),
        },
        GitCloneCommand::InstallShowObjectFormat,
        GitCloneCommand::InstallExport,
    ]
    .into_iter()
    .enumerate()
    {
        let plan = GitPlan::new_install(
            command,
            &paths,
            &limits,
            format!("dfgit-install-{index}"),
            Duration::from_secs(5),
            4096,
            1024,
        )
        .expect("install plan");
        let args = plan.docker_args();
        assert!(args.iter().any(|arg| arg == "--network=none"));
        assert!(!args.iter().any(|arg| arg == "--network"));
        assert!(
            args.iter()
                .any(|arg| arg.contains(&format!("target={INSTALL_VOLUME_TARGET}")))
        );
        assert_eq!(
            args.iter()
                .any(|arg| arg.contains(&format!("target={BUNDLE_TARGET}"))),
            matches!(plan.command, GitCloneCommand::InstallClone { .. })
        );
        assert!(!args.iter().any(|arg| arg.contains("target=/git-cred")));
        assert!(!args.iter().any(|arg| arg.contains("target=/git-control")));
    }
    let clone = GitPlan::new_install(
        GitCloneCommand::InstallClone {
            branch: "main".to_owned(),
        },
        &paths,
        &limits,
        "dfgit-clone".to_owned(),
        Duration::from_secs(5),
        4096,
        1024,
    )
    .expect("clone");
    assert!(
        clone
            .docker_args()
            .iter()
            .any(|arg| arg == "/git-bundle/bundle.git")
    );
    assert!(
        clone
            .docker_args()
            .iter()
            .any(|arg| arg == "--no-hardlinks")
    );
    assert!(
        clone
            .docker_args()
            .iter()
            .any(|arg| arg == "--single-branch")
    );
    let export = GitPlan::new_install(
        GitCloneCommand::InstallExport,
        &paths,
        &limits,
        "dfgit-export".to_owned(),
        Duration::from_secs(5),
        4096,
        1024,
    )
    .expect("export");
    assert!(export.docker_args().iter().any(|arg| arg == "tar"));
    assert!(matches!(
        GitPlan::new_install(
            GitCloneCommand::InstallClone {
                branch: "main; rm -rf /".to_owned(),
            },
            &paths,
            &limits,
            "dfgit-bad".to_owned(),
            Duration::from_secs(5),
            4096,
            1024,
        ),
        Err(GitPlanError::Command)
    ));
}
