use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use super::clone_plan::{
    GitCloneCommand, GitCloneExecutor, GitCloneLimits, GitClonePaths, GitPlan,
};
use super::docker_runner::{
    CLEANUP_OUTPUT_LIMIT, DockerCliExecutor, DockerCommandExecutor, DockerCommandOutcome,
    DockerCommandSpec, DockerExecutorError, DockerFuture, DockerRunnerError, DockerStreamLimits,
    DockerTrustedGitRunner, GitPlanRunner, KILL_REAP_TIMEOUT,
};
use super::helper::{GitVerifyLimits, HelperPaths, HelperPlan, plan_command};
use super::verify::{
    TrustedGitCommand, TrustedGitInvocation, TrustedGitRunner, TrustedGitRunnerError,
};

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
        image: format!("runner@sha256:{}", "a".repeat(64)),
    }
}

fn plan() -> HelperPlan {
    plan_command(
        &paths(),
        &TrustedGitCommand::Fsck,
        &limits(),
        Duration::from_secs(600),
    )
    .expect("plan")
}

fn invocation() -> TrustedGitInvocation {
    TrustedGitInvocation {
        command: TrustedGitCommand::Fsck,
        paths: paths(),
        plan: plan(),
    }
}

fn outcome(exit_code: i32, stdout: &[u8], stderr: &[u8]) -> DockerCommandOutcome {
    DockerCommandOutcome {
        exit_code,
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
    }
}

#[derive(Clone)]
struct RecordingExecutor {
    inner: Arc<Mutex<RecordingState>>,
}

struct RecordingState {
    results: VecDeque<Result<DockerCommandOutcome, DockerExecutorError>>,
    calls: Vec<Vec<String>>,
}

impl RecordingExecutor {
    fn new(
        results: impl IntoIterator<Item = Result<DockerCommandOutcome, DockerExecutorError>>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RecordingState {
                results: results.into_iter().collect(),
                calls: Vec::new(),
            })),
        }
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.inner.lock().expect("calls").calls.clone()
    }
}

impl DockerCommandExecutor for RecordingExecutor {
    fn run<'a>(&'a self, spec: DockerCommandSpec) -> DockerFuture<'a> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let mut state = inner.lock().expect("executor");
            state.calls.push(spec.args.clone());
            state
                .results
                .pop_front()
                .unwrap_or_else(|| Ok(outcome(0, b"", b"")))
        })
    }
}

#[tokio::test]
async fn executes_one_closed_command_then_exact_name_cleanup() {
    let executor =
        RecordingExecutor::new([Ok(outcome(0, b"sha1\n", b"")), Ok(outcome(0, b"", b""))]);
    let mut runner = DockerTrustedGitRunner::new(
        executor.clone(),
        "docker".to_owned(),
        Some("unix:///tmp/test.sock".to_owned()),
        None,
    )
    .expect("runner");
    let invocation = invocation();
    let name = invocation.plan.container_name().to_owned();
    let output = runner.run(invocation).await.expect("run");
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, b"sha1\n");

    let calls = executor.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0][0], "docker");
    assert!(calls[0].contains(&"--pull=never".to_owned()));
    assert!(calls[0].contains(&"--network".to_owned()));
    assert!(calls[0].contains(&"none".to_owned()));
    assert_eq!(
        calls[1],
        vec!["docker".to_owned(), "rm".to_owned(), "-f".to_owned(), name]
    );
}

#[tokio::test]
async fn cleanup_runs_after_every_run_outcome() {
    for (run_result, expected) in [
        (
            Err(DockerExecutorError::TimedOut),
            TrustedGitRunnerError::TimedOut,
        ),
        (
            Err(DockerExecutorError::Cancelled),
            TrustedGitRunnerError::Cancelled,
        ),
        (
            Err(DockerExecutorError::OutputLimit),
            TrustedGitRunnerError::OutputLimit,
        ),
        (
            Err(DockerExecutorError::ReapTimeout),
            TrustedGitRunnerError::ReapTimeout,
        ),
        (
            Err(DockerExecutorError::SpawnFailed),
            TrustedGitRunnerError::Failed,
        ),
        (
            Err(DockerExecutorError::Failed),
            TrustedGitRunnerError::Failed,
        ),
    ] {
        let executor = RecordingExecutor::new([run_result, Ok(outcome(0, b"", b""))]);
        let mut runner =
            DockerTrustedGitRunner::new(executor.clone(), "docker".to_owned(), None, None)
                .expect("runner");
        assert_eq!(runner.run(invocation()).await, Err(expected));
        assert_eq!(executor.calls().len(), 2);
    }
}

#[tokio::test]
async fn already_absent_cleanup_is_success_and_other_cleanup_failure_is_fixed() {
    let executor = RecordingExecutor::new([
        Ok(outcome(0, b"", b"")),
        Ok(outcome(1, b"", b"Error: No such container: x")),
    ]);
    let mut runner =
        DockerTrustedGitRunner::new(executor, "docker".to_owned(), None, None).expect("runner");
    runner.run(invocation()).await.expect("already absent");

    let executor = RecordingExecutor::new([
        Ok(outcome(0, b"", b"")),
        Ok(outcome(1, b"", b"permission denied")),
    ]);
    let mut runner =
        DockerTrustedGitRunner::new(executor, "docker".to_owned(), None, None).expect("runner");
    assert_eq!(
        runner.run(invocation()).await,
        Err(TrustedGitRunnerError::Cleanup)
    );

    let executor = RecordingExecutor::new([
        Ok(outcome(0, b"", b"")),
        Err(DockerExecutorError::ReapTimeout),
    ]);
    let mut runner =
        DockerTrustedGitRunner::new(executor, "docker".to_owned(), None, None).expect("runner");
    assert_eq!(
        runner.run(invocation()).await,
        Err(TrustedGitRunnerError::ReapTimeout)
    );

    let executor =
        RecordingExecutor::new([Ok(outcome(0, b"", b"")), Err(DockerExecutorError::Failed)]);
    let mut runner =
        DockerTrustedGitRunner::new(executor, "docker".to_owned(), None, None).expect("runner");
    assert_eq!(
        runner.run(invocation()).await,
        Err(TrustedGitRunnerError::Cleanup)
    );
}

#[tokio::test]
async fn malformed_plan_is_rejected_before_execution() {
    let executor = RecordingExecutor::new([]);
    let mut runner = DockerTrustedGitRunner::new(executor.clone(), "docker".to_owned(), None, None)
        .expect("runner");
    let mut request = invocation();
    let mut args = request.plan.docker_args().to_vec();
    args.retain(|argument| argument != "--pull=never");
    request.plan = request.plan.test_with_docker_args(args);
    assert_eq!(
        runner.run(request).await,
        Err(TrustedGitRunnerError::Failed)
    );
    assert!(executor.calls().is_empty());

    for mutate in [
        |args: &mut Vec<String>| args.push("--privileged".to_owned()),
        |args: &mut Vec<String>| {
            args[0] = "sh".to_owned();
        },
        |args: &mut Vec<String>| {
            if let Some(position) = args.iter().position(|arg| arg == "--network") {
                args[position + 1] = "host".to_owned();
            }
        },
        |args: &mut Vec<String>| {
            if let Some(position) = args.iter().position(|arg| arg == "--mount") {
                args[position + 1] = "type=bind,source=/untrusted,target=/git-verify,rw".to_owned();
            }
        },
        |args: &mut Vec<String>| {
            if let Some(position) = args.iter().position(|arg| arg == "--env") {
                args[position + 1] = "HOME=/untrusted".to_owned();
            }
        },
        |args: &mut Vec<String>| {
            if let Some(position) = args.iter().position(|arg| arg == "--user") {
                args[position + 1] = "0:0".to_owned();
            }
        },
        |args: &mut Vec<String>| {
            if let Some(position) = args.iter().position(|arg| arg == "git") {
                args[position - 1] = "runner:latest".to_owned();
            }
        },
    ] {
        let executor = RecordingExecutor::new([]);
        let mut runner =
            DockerTrustedGitRunner::new(executor.clone(), "docker".to_owned(), None, None)
                .expect("runner");
        let mut request = invocation();
        let mut args = request.plan.docker_args().to_vec();
        mutate(&mut args);
        request.plan = request.plan.test_with_docker_args(args);
        assert_eq!(
            runner.run(request).await,
            Err(TrustedGitRunnerError::Failed)
        );
        assert!(executor.calls().is_empty());
    }
}

#[tokio::test]
async fn executor_rejects_forged_mount_mode_before_cli() {
    let executor = RecordingExecutor::new([]);
    let mut runner = DockerTrustedGitRunner::new(executor.clone(), "docker".to_owned(), None, None)
        .expect("runner");
    let mut fsck = invocation();
    let mut args = fsck.plan.docker_args().to_vec();
    let position = args
        .iter()
        .position(|argument| argument.ends_with(",target=/git-verify,ro"))
        .expect("ro mount");
    args[position] = args[position].replace(",ro", ",rw");
    fsck.plan = fsck.plan.test_with_docker_args(args);
    assert_eq!(runner.run(fsck).await, Err(TrustedGitRunnerError::Failed));
    assert!(executor.calls().is_empty());

    let init_command = TrustedGitCommand::Init {
        object_format: runner_protocol::GitObjectFormat::Sha1,
        initial_branch: "main".to_owned(),
    };
    let mut init = TrustedGitInvocation {
        command: init_command.clone(),
        paths: paths(),
        plan: plan_command(&paths(), &init_command, &limits(), Duration::from_secs(600))
            .expect("init plan"),
    };
    let mut args = init.plan.docker_args().to_vec();
    let position = args
        .iter()
        .position(|argument| argument.ends_with(",target=/git-verify,rw"))
        .expect("rw mount");
    args[position] = args[position].replace(",rw", ",ro");
    init.plan = init.plan.test_with_docker_args(args);
    assert_eq!(runner.run(init).await, Err(TrustedGitRunnerError::Failed));
    assert!(executor.calls().is_empty());
}

#[test]
fn invalid_cli_and_host_are_rejected() {
    assert!(matches!(
        DockerTrustedGitRunner::new(RecordingExecutor::new([]), String::new(), None, None),
        Err(DockerRunnerError::InvalidDockerCli)
    ));
    assert!(matches!(
        DockerTrustedGitRunner::new(
            RecordingExecutor::new([]),
            "docker".to_owned(),
            Some("bad\nhost".to_owned()),
            None
        ),
        Err(DockerRunnerError::InvalidDockerHost)
    ));
    assert!(matches!(
        DockerCliExecutor::new("docker\n".to_owned(), None),
        Err(DockerRunnerError::InvalidDockerCli)
    ));
}

fn script(directory: &Path, name: &str, body: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, body).expect("script");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("script mode");
    path
}

fn spec(args: Vec<String>, deadline: Duration, output_limit: usize) -> DockerCommandSpec {
    DockerCommandSpec {
        args,
        deadline,
        output_limit,
        stream_limits: None,
        cancel: None,
        kill_reap_limit: KILL_REAP_TIMEOUT,
    }
}

fn spec_with_streams(
    args: Vec<String>,
    deadline: Duration,
    output_limit: usize,
    stdout: usize,
    stderr: usize,
) -> DockerCommandSpec {
    DockerCommandSpec {
        args,
        deadline,
        output_limit,
        stream_limits: Some(DockerStreamLimits { stdout, stderr }),
        cancel: None,
        kill_reap_limit: KILL_REAP_TIMEOUT,
    }
}

fn pid_from(path: &Path) -> u32 {
    fs::read_to_string(path)
        .expect("pid file")
        .trim()
        .parse()
        .expect("pid")
}

fn process_gone(pid: u32) -> bool {
    !Path::new("/proc").join(pid.to_string()).exists()
}

#[tokio::test]
async fn custom_cli_and_environment_are_sanitized() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output = temp.path().join("env.txt");
    let docker = script(
        temp.path(),
        "docker-env",
        "#!/bin/sh\nprintf '%s\\n' \"$PATH\" > \"$1\"\nprintf '%s\\n' \"${DOCKER_HOST-unset}\" >> \"$1\"\nprintf '%s\\n' \"${HOME-unset}\" >> \"$1\"\n",
    );
    let executor = DockerCliExecutor::new(
        docker.to_string_lossy().into_owned(),
        Some("unix:///tmp/test.sock".to_owned()),
    )
    .expect("executor");
    let outcome = executor
        .run(spec(
            vec![output.to_string_lossy().into_owned()],
            Duration::from_secs(2),
            1024,
        ))
        .await
        .expect("run");
    assert_eq!(outcome.exit_code, 0);
    assert_eq!(
        fs::read_to_string(output).expect("env output"),
        "/usr/bin:/bin\nunix:///tmp/test.sock\nunset\n"
    );
}

#[tokio::test]
async fn timeout_kills_and_reaps_the_child() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pid_file = temp.path().join("pid");
    let docker = script(
        temp.path(),
        "docker-timeout",
        "#!/bin/sh\necho $$ > \"$1\"\nsleep 10\n",
    );
    let executor =
        DockerCliExecutor::new(docker.to_string_lossy().into_owned(), None).expect("executor");
    assert_eq!(
        executor
            .run(spec(
                vec![pid_file.to_string_lossy().into_owned()],
                Duration::from_millis(100),
                1024,
            ))
            .await,
        Err(DockerExecutorError::TimedOut)
    );
    let pid = pid_from(&pid_file);
    assert!(process_gone(pid), "timed-out child {pid} still present");
}

#[tokio::test]
async fn watch_cancel_kills_and_reaps_the_child() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pid_file = temp.path().join("pid");
    let docker = script(
        temp.path(),
        "docker-cancel",
        "#!/bin/sh\necho $$ > \"$1\"\nsleep 10\n",
    );
    let executor =
        DockerCliExecutor::new(docker.to_string_lossy().into_owned(), None).expect("executor");
    let (sender, receiver) = watch::channel(false);
    let mut cancel_spec = spec(
        vec![pid_file.to_string_lossy().into_owned()],
        Duration::from_secs(5),
        1024,
    );
    cancel_spec.cancel = Some(receiver);
    let mut run = executor.run(cancel_spec);
    tokio::select! {
        result = &mut run => panic!("run completed before cancel: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(150)) => {}
    }
    sender.send(true).expect("cancel");
    assert_eq!(run.await, Err(DockerExecutorError::Cancelled));
    let pid = pid_from(&pid_file);
    assert!(process_gone(pid), "cancelled child {pid} still present");
}

#[tokio::test]
async fn continuous_output_stops_at_the_shared_quota() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pid_file = temp.path().join("pid");
    let docker = script(
        temp.path(),
        "docker-flood",
        "#!/bin/sh\necho $$ > \"$1\"\nwhile :; do printf 'xxxxxxxxxxxxxxxx'; done\n",
    );
    let executor =
        DockerCliExecutor::new(docker.to_string_lossy().into_owned(), None).expect("executor");
    assert_eq!(
        executor
            .run(spec(
                vec![pid_file.to_string_lossy().into_owned()],
                Duration::from_secs(5),
                512,
            ))
            .await,
        Err(DockerExecutorError::OutputLimit)
    );
    let pid = pid_from(&pid_file);
    assert!(process_gone(pid), "overflow child {pid} still present");
}

#[tokio::test]
async fn independent_stream_limits_stop_the_offending_stream() {
    let temp = tempfile::tempdir().expect("tempdir");
    for (name, body, stdout_limit, stderr_limit) in [
        (
            "docker-stream-stdout",
            "#!/bin/sh\necho $$ > \"$1\"\nprintf '....................'\nprintf '....................' >&2\n",
            15,
            1024,
        ),
        (
            "docker-stream-stderr",
            "#!/bin/sh\necho $$ > \"$1\"\nprintf '....................'\nprintf '....................' >&2\n",
            1024,
            15,
        ),
    ] {
        let pid_file = temp.path().join(format!("{name}.pid"));
        let docker = script(temp.path(), name, body);
        let executor =
            DockerCliExecutor::new(docker.to_string_lossy().into_owned(), None).expect("executor");
        assert_eq!(
            executor
                .run(spec_with_streams(
                    vec![pid_file.to_string_lossy().into_owned()],
                    Duration::from_secs(5),
                    4096,
                    stdout_limit,
                    stderr_limit,
                ))
                .await,
            Err(DockerExecutorError::OutputLimit)
        );
        let pid = pid_from(&pid_file);
        assert!(process_gone(pid), "overflow child {pid} still present");
    }
}

#[tokio::test]
async fn normal_exit_is_reaped_without_leaking_output_or_processes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pid_file = temp.path().join("pid");
    let docker = script(
        temp.path(),
        "docker-ok",
        "#!/bin/sh\necho $$ > \"$1\"\necho hello\nexit 7\n",
    );
    let executor =
        DockerCliExecutor::new(docker.to_string_lossy().into_owned(), None).expect("executor");
    let outcome = executor
        .run(spec(
            vec![pid_file.to_string_lossy().into_owned()],
            Duration::from_secs(2),
            CLEANUP_OUTPUT_LIMIT,
        ))
        .await
        .expect("run");
    assert_eq!(outcome.exit_code, 7);
    assert_eq!(outcome.stdout, b"hello\n");
    let pid = pid_from(&pid_file);
    assert!(process_gone(pid), "exited child {pid} still present");
}

#[tokio::test]
async fn typed_clone_runner_executes_one_plan_then_exact_name_cleanup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let calls = temp.path().join("calls");
    let docker = script(
        temp.path(),
        "docker-clone",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nexit 0\n",
            calls.display()
        ),
    );
    let executor =
        DockerCliExecutor::new(docker.to_string_lossy().into_owned(), None).expect("executor");
    let runner = GitPlanRunner::new(executor);
    let limits = GitCloneLimits {
        image: format!("helper@sha256:{}", "a".repeat(64)),
        network: "df-git-net".to_owned(),
        broker_host: "broker.example.invalid".to_owned(),
        uid: 1000,
        gid: 1000,
        fetch_volume_bytes: 4096,
        bundle_bytes: 2048,
        logical_object_bytes: 2048,
        checkout_bytes: 2048,
        tree_entries: 8,
        deadline: Duration::from_secs(5),
        memory_limit: "128m".to_owned(),
        cpu_limit: "1".to_owned(),
        pids_limit: 16,
    };
    let paths = GitClonePaths {
        control_dir: temp.path().join("control"),
        credential_dir: temp.path().join("credential"),
        volume_name: "dfgit-clone".to_owned(),
        uid: 1000,
        gid: 1000,
    };
    let plan = GitPlan::new(
        GitCloneCommand::Proof {
            proof_name: "mount-proof-abc".to_owned(),
        },
        &paths,
        &limits,
        "dfgit-proof-clone".to_owned(),
        Duration::from_secs(5),
        128,
        128,
    )
    .expect("plan");
    runner.run_plan(&plan, None).await.expect("run");
    let recorded = std::fs::read_to_string(calls).expect("calls");
    let lines: Vec<&str> = recorded.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("run --rm --pull=never --name dfgit-proof-clone"));
    assert_eq!(lines[1], "rm -f dfgit-proof-clone");
}
