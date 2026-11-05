//! Narrow Docker execution for the trusted Git verifier.
//!
//! The helper plan is validated before any subprocess. One closed Docker run
//! is followed by an exact-name cleanup, and the collector owns kill/reap and
//! the shared output/deadline budget.

use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};

use super::super::docker_command::is_missing_container_error;
use super::helper::{HelperPlan, HelperPlanError, MountMode};
use super::verify::{
    TrustedGitFuture, TrustedGitInvocation, TrustedGitOutput, TrustedGitRunner,
    TrustedGitRunnerError,
};

pub(crate) const KILL_REAP_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const CLEANUP_TOTAL_TIMEOUT: Duration = Duration::from_secs(5);
const CLEANUP_COMMAND_TIMEOUT: Duration = Duration::from_secs(4);
pub(crate) const CLEANUP_OUTPUT_LIMIT: usize = 4096;
const CLEANUP_REAP_TIMEOUT: Duration =
    CLEANUP_TOTAL_TIMEOUT.saturating_sub(CLEANUP_COMMAND_TIMEOUT);
const READ_BUFFER_BYTES: usize = 8192;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DockerExecutorError {
    SpawnFailed,
    TimedOut,
    Cancelled,
    OutputLimit,
    ReapTimeout,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DockerCommandOutcome {
    pub(crate) exit_code: i32,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DockerStreamLimits {
    pub(crate) stdout: usize,
    pub(crate) stderr: usize,
}

#[derive(Debug)]
pub(crate) struct DockerCommandSpec {
    pub(crate) args: Vec<String>,
    pub(crate) deadline: Duration,
    pub(crate) output_limit: usize,
    pub(crate) stream_limits: Option<DockerStreamLimits>,
    pub(crate) cancel: Option<watch::Receiver<bool>>,
    pub(crate) kill_reap_limit: Duration,
}

pub(crate) type DockerFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DockerCommandOutcome, DockerExecutorError>> + Send + 'a>>;

pub(crate) trait DockerCommandExecutor: Send + Sync {
    fn run<'a>(&'a self, spec: DockerCommandSpec) -> DockerFuture<'a>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DockerRunnerError {
    InvalidDockerCli,
    InvalidDockerHost,
}

impl std::fmt::Display for DockerRunnerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidDockerCli => "docker CLI is invalid",
            Self::InvalidDockerHost => "docker host is invalid",
        })
    }
}

impl std::error::Error for DockerRunnerError {}

fn validate_executable(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !value
            .chars()
            .any(|character| matches!(character, '\0' | '\n' | '\r'))
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

pub(crate) struct DockerCliExecutor {
    docker_cli: String,
    docker_host: Option<String>,
}

impl DockerCliExecutor {
    pub(crate) fn new(
        docker_cli: String,
        docker_host: Option<String>,
    ) -> Result<Self, DockerRunnerError> {
        if !validate_executable(&docker_cli) {
            return Err(DockerRunnerError::InvalidDockerCli);
        }
        if let Some(host) = &docker_host
            && !validate_executable(host)
        {
            return Err(DockerRunnerError::InvalidDockerHost);
        }
        Ok(Self {
            docker_cli,
            docker_host,
        })
    }

    async fn run_inner(
        &self,
        spec: DockerCommandSpec,
    ) -> Result<DockerCommandOutcome, DockerExecutorError> {
        let mut command = Command::new(&self.docker_cli);
        command.args(&spec.args);
        command.env_clear();
        command.env("PATH", "/usr/bin:/bin");
        if let Some(host) = &self.docker_host {
            command.env("DOCKER_HOST", host);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = command
            .spawn()
            .map_err(|_| DockerExecutorError::SpawnFailed)?;
        GitDockerChildCollector::collect(
            child,
            spec.deadline,
            spec.output_limit,
            spec.stream_limits,
            spec.cancel,
            spec.kill_reap_limit,
        )
        .await
    }
}

impl DockerCommandExecutor for DockerCliExecutor {
    fn run<'a>(&'a self, spec: DockerCommandSpec) -> DockerFuture<'a> {
        Box::pin(async move { self.run_inner(spec).await })
    }
}

pub(crate) struct DockerTrustedGitRunner<E> {
    executor: E,
    docker_cli: String,
    cancel: Option<watch::Receiver<bool>>,
}

impl<E: DockerCommandExecutor> DockerTrustedGitRunner<E> {
    pub(crate) fn new(
        executor: E,
        docker_cli: String,
        docker_host: Option<String>,
        cancel: Option<watch::Receiver<bool>>,
    ) -> Result<Self, DockerRunnerError> {
        if !validate_executable(&docker_cli) {
            return Err(DockerRunnerError::InvalidDockerCli);
        }
        if let Some(host) = &docker_host
            && !validate_executable(host)
        {
            return Err(DockerRunnerError::InvalidDockerHost);
        }
        Ok(Self {
            executor,
            docker_cli,
            cancel,
        })
    }

    async fn cleanup(&self, plan: &HelperPlan) -> Result<(), TrustedGitRunnerError> {
        let mut args = plan.cleanup_args().to_vec();
        args[0] = self.docker_cli.clone();
        let outcome = self
            .executor
            .run(DockerCommandSpec {
                args,
                deadline: CLEANUP_COMMAND_TIMEOUT,
                output_limit: CLEANUP_OUTPUT_LIMIT,
                stream_limits: None,
                cancel: None,
                kill_reap_limit: CLEANUP_REAP_TIMEOUT,
            })
            .await;
        match outcome {
            Ok(outcome) => {
                let message = format!(
                    "{}{}",
                    String::from_utf8_lossy(&outcome.stdout),
                    String::from_utf8_lossy(&outcome.stderr)
                );
                if outcome.exit_code == 0 || is_missing_container_error(&message) {
                    Ok(())
                } else {
                    Err(TrustedGitRunnerError::Cleanup)
                }
            }
            Err(DockerExecutorError::ReapTimeout) => Err(TrustedGitRunnerError::ReapTimeout),
            Err(_) => Err(TrustedGitRunnerError::Cleanup),
        }
    }

    async fn run_inner(
        &mut self,
        invocation: TrustedGitInvocation,
    ) -> Result<TrustedGitOutput, TrustedGitRunnerError> {
        invocation
            .plan
            .validate_for_invocation(&invocation.command)
            .map_err(|_: HelperPlanError| TrustedGitRunnerError::Failed)?;
        let expected_mount = format!(
            "type=bind,source={},target=/git-verify,{}",
            invocation.paths.operation_root.display(),
            MountMode::for_command(&invocation.command).suffix()
        );
        let expected_user = format!("{}:{}", invocation.paths.uid, invocation.paths.gid);
        if !invocation
            .plan
            .docker_args()
            .iter()
            .any(|argument| argument == &expected_mount)
            || !invocation
                .plan
                .docker_args()
                .iter()
                .any(|argument| argument == &expected_user)
        {
            return Err(TrustedGitRunnerError::Failed);
        }
        let mut args = invocation.plan.docker_args().to_vec();
        args[0] = self.docker_cli.clone();
        let run = self
            .executor
            .run(DockerCommandSpec {
                args,
                deadline: invocation.plan.deadline(),
                output_limit: invocation.plan.max_output_bytes(),
                stream_limits: None,
                cancel: self.cancel.clone(),
                kill_reap_limit: KILL_REAP_TIMEOUT,
            })
            .await;
        self.cleanup(&invocation.plan).await?;
        match run {
            Ok(outcome) => Ok(TrustedGitOutput {
                exit_code: outcome.exit_code,
                stdout: outcome.stdout,
                stderr: outcome.stderr,
            }),
            Err(DockerExecutorError::TimedOut) => Err(TrustedGitRunnerError::TimedOut),
            Err(DockerExecutorError::Cancelled) => Err(TrustedGitRunnerError::Cancelled),
            Err(DockerExecutorError::OutputLimit) => Err(TrustedGitRunnerError::OutputLimit),
            Err(DockerExecutorError::ReapTimeout) => Err(TrustedGitRunnerError::ReapTimeout),
            Err(DockerExecutorError::SpawnFailed | DockerExecutorError::Failed) => {
                Err(TrustedGitRunnerError::Failed)
            }
        }
    }
}

impl<E: DockerCommandExecutor> TrustedGitRunner for DockerTrustedGitRunner<E> {
    fn run<'a>(&'a mut self, invocation: TrustedGitInvocation) -> TrustedGitFuture<'a> {
        Box::pin(async move { self.run_inner(invocation).await })
    }
}

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

enum Chunk {
    Data(Stream, Vec<u8>),
    End,
    Error,
}

struct GitDockerChildCollector;

impl GitDockerChildCollector {
    async fn collect(
        mut child: Child,
        deadline: Duration,
        output_limit: usize,
        stream_limits: Option<DockerStreamLimits>,
        cancel: Option<watch::Receiver<bool>>,
        kill_reap_limit: Duration,
    ) -> Result<DockerCommandOutcome, DockerExecutorError> {
        if output_limit == 0 {
            terminate(child, kill_reap_limit).await?;
            return Err(DockerExecutorError::OutputLimit);
        }
        if cancellation_requested(&cancel) {
            terminate(child, kill_reap_limit).await?;
            return Err(DockerExecutorError::Cancelled);
        }
        let Some(stdout) = child.stdout.take() else {
            terminate(child, kill_reap_limit).await?;
            return Err(DockerExecutorError::Failed);
        };
        let Some(stderr) = child.stderr.take() else {
            terminate(child, kill_reap_limit).await?;
            return Err(DockerExecutorError::Failed);
        };

        let (sender, mut receiver) = mpsc::channel(8);
        let stdout_task = spawn_reader(stdout, sender.clone(), Stream::Stdout);
        let stderr_task = spawn_reader(stderr, sender, Stream::Stderr);
        let deadline_at = Instant::now() + deadline;
        let cancel_enabled = cancel.is_some();
        let mut cancel = cancel;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut total = 0_usize;
        let mut ended = 0_u8;

        loop {
            if ended == 2 {
                break;
            }
            tokio::select! {
                biased;
                cancelled = wait_cancelled(&mut cancel), if cancel_enabled => {
                    let error = if cancelled { DockerExecutorError::Cancelled } else { continue };
                    return Err(fail_after_terminate(&mut child, stdout_task, stderr_task, kill_reap_limit, error).await);
                }
                _ = timeout_at(deadline_at, std::future::pending::<()>()) => {
                    return Err(fail_after_terminate(&mut child, stdout_task, stderr_task, kill_reap_limit, DockerExecutorError::TimedOut).await);
                }
                message = receiver.recv() => {
                    match message {
                        Some(Chunk::Data(stream, bytes)) => {
                            total = match total.checked_add(bytes.len()) {
                                Some(total) => total,
                                None => {
                                    return Err(fail_after_terminate(&mut child, stdout_task, stderr_task, kill_reap_limit, DockerExecutorError::OutputLimit).await);
                                }
                            };
                            if total > output_limit {
                                return Err(fail_after_terminate(&mut child, stdout_task, stderr_task, kill_reap_limit, DockerExecutorError::OutputLimit).await);
                            }
                            if let Some(limits) = stream_limits {
                                let (target, limit) = match stream {
                                    Stream::Stdout => (&stdout, limits.stdout),
                                    Stream::Stderr => (&stderr, limits.stderr),
                                };
                                if target.len().saturating_add(bytes.len()) > limit {
                                    return Err(fail_after_terminate(&mut child, stdout_task, stderr_task, kill_reap_limit, DockerExecutorError::OutputLimit).await);
                                }
                            }
                            match stream {
                                Stream::Stdout => stdout.extend_from_slice(&bytes),
                                Stream::Stderr => stderr.extend_from_slice(&bytes),
                            }
                        }
                        Some(Chunk::End) => ended += 1,
                        Some(Chunk::Error) | None => {
                            if ended < 2 {
                                return Err(fail_after_terminate(&mut child, stdout_task, stderr_task, kill_reap_limit, DockerExecutorError::Failed).await);
                            }
                            break;
                        }
                    }
                }
            }
        }

        let _ = stdout_task.await;
        let _ = stderr_task.await;
        match timeout(kill_reap_limit, child.wait()).await {
            Ok(Ok(status)) => Ok(DockerCommandOutcome {
                exit_code: status.code().unwrap_or(-1),
                stdout,
                stderr,
            }),
            Ok(Err(_)) => {
                let _ = child.start_kill();
                let _ = timeout(kill_reap_limit, child.wait()).await;
                Err(DockerExecutorError::Failed)
            }
            Err(_) => {
                let _ = child.start_kill();
                match timeout(kill_reap_limit, child.wait()).await {
                    Ok(Ok(_)) => Err(DockerExecutorError::TimedOut),
                    _ => Err(DockerExecutorError::ReapTimeout),
                }
            }
        }
    }
}

fn spawn_reader<R>(mut reader: R, sender: mpsc::Sender<Chunk>, stream: Stream) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = [0_u8; READ_BUFFER_BYTES];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    if sender
                        .send(Chunk::Data(stream, buffer[..read].to_vec()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(_) => {
                    let _ = sender.send(Chunk::Error).await;
                    return;
                }
            }
        }
        let _ = sender.send(Chunk::End).await;
    })
}

fn cancellation_requested(cancel: &Option<watch::Receiver<bool>>) -> bool {
    let Some(receiver) = cancel else {
        return false;
    };
    match receiver.has_changed() {
        Ok(_) => *receiver.borrow(),
        Err(_) => true,
    }
}

async fn wait_cancelled(cancel: &mut Option<watch::Receiver<bool>>) -> bool {
    let Some(receiver) = cancel else {
        return std::future::pending().await;
    };
    match receiver.changed().await {
        Ok(()) => *receiver.borrow(),
        Err(_) => true,
    }
}

async fn fail_after_terminate(
    child: &mut Child,
    stdout_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
    kill_reap_limit: Duration,
    error: DockerExecutorError,
) -> DockerExecutorError {
    stdout_task.abort();
    stderr_task.abort();
    let _ = stdout_task.await;
    let _ = stderr_task.await;
    if let Err(reap_error) = terminate_at(child, kill_reap_limit).await {
        return reap_error;
    }
    error
}

async fn terminate(mut child: Child, kill_reap_limit: Duration) -> Result<(), DockerExecutorError> {
    terminate_at(&mut child, kill_reap_limit).await
}

async fn terminate_at(
    child: &mut Child,
    kill_reap_limit: Duration,
) -> Result<(), DockerExecutorError> {
    if kill_reap_limit.is_zero() {
        return Err(DockerExecutorError::ReapTimeout);
    }
    let _ = child.start_kill();
    match timeout(kill_reap_limit, child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => Err(DockerExecutorError::Failed),
        Err(_) => Err(DockerExecutorError::ReapTimeout),
    }
}

impl DockerCliExecutor {
    /// Execute one validated closed clone plan with independent stream caps.
    pub(crate) async fn run_clone_plan(
        &self,
        plan: &super::clone_plan::GitPlan,
        cancel: Option<watch::Receiver<bool>>,
    ) -> Result<DockerCommandOutcome, DockerExecutorError> {
        let run = self
            .run_inner(DockerCommandSpec {
                args: plan.execution_args().to_vec(),
                deadline: plan.deadline(),
                output_limit: plan.combined_limit(),
                stream_limits: Some(DockerStreamLimits {
                    stdout: plan.stdout_limit(),
                    stderr: plan.stderr_limit(),
                }),
                cancel,
                kill_reap_limit: KILL_REAP_TIMEOUT,
            })
            .await;
        self.cleanup_clone_plan(plan).await?;
        run
    }

    async fn cleanup_clone_plan(
        &self,
        plan: &super::clone_plan::GitPlan,
    ) -> Result<(), DockerExecutorError> {
        let args = plan.cleanup_args()[1..].to_vec();
        let outcome = self
            .run_inner(DockerCommandSpec {
                args,
                deadline: CLEANUP_COMMAND_TIMEOUT,
                output_limit: CLEANUP_OUTPUT_LIMIT,
                stream_limits: Some(DockerStreamLimits {
                    stdout: CLEANUP_OUTPUT_LIMIT,
                    stderr: CLEANUP_OUTPUT_LIMIT,
                }),
                cancel: None,
                kill_reap_limit: CLEANUP_REAP_TIMEOUT,
            })
            .await;
        match outcome {
            Ok(outcome) => {
                let message = format!(
                    "{}{}",
                    String::from_utf8_lossy(&outcome.stdout),
                    String::from_utf8_lossy(&outcome.stderr)
                );
                if outcome.exit_code == 0 || is_missing_container_error(&message) {
                    Ok(())
                } else {
                    Err(DockerExecutorError::Failed)
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Exact-name create for one validated volume plan.
    pub(crate) async fn run_clone_volume_create(
        &self,
        plan: &super::clone_volume::GitVolumePlan,
    ) -> Result<DockerCommandOutcome, DockerExecutorError> {
        let args = plan
            .create_args(&self.docker_cli)
            .map_err(|_| DockerExecutorError::Failed)?;
        self.run_inner(DockerCommandSpec {
            args: args[1..].to_vec(),
            deadline: plan.deadline,
            output_limit: CLEANUP_OUTPUT_LIMIT,
            stream_limits: Some(DockerStreamLimits {
                stdout: CLEANUP_OUTPUT_LIMIT,
                stderr: CLEANUP_OUTPUT_LIMIT,
            }),
            cancel: None,
            kill_reap_limit: KILL_REAP_TIMEOUT,
        })
        .await
    }

    /// Exact-name removal for the same validated volume plan.
    pub(crate) async fn run_clone_volume_remove(
        &self,
        plan: &super::clone_volume::GitVolumePlan,
    ) -> Result<DockerCommandOutcome, DockerExecutorError> {
        let args = plan
            .remove_args(&self.docker_cli)
            .map_err(|_| DockerExecutorError::Failed)?;
        self.run_inner(DockerCommandSpec {
            args: args[1..].to_vec(),
            deadline: plan.deadline.min(CLEANUP_COMMAND_TIMEOUT),
            output_limit: CLEANUP_OUTPUT_LIMIT,
            stream_limits: Some(DockerStreamLimits {
                stdout: CLEANUP_OUTPUT_LIMIT,
                stderr: CLEANUP_OUTPUT_LIMIT,
            }),
            cancel: None,
            kill_reap_limit: CLEANUP_REAP_TIMEOUT,
        })
        .await
    }
}

/// Production typed executor for closed clone and volume plans.
pub(crate) struct GitPlanRunner {
    executor: DockerCliExecutor,
}

impl GitPlanRunner {
    pub(crate) fn new(executor: DockerCliExecutor) -> Self {
        Self { executor }
    }
}

impl super::clone_plan::GitCloneExecutor for GitPlanRunner {
    fn run_plan<'a>(
        &'a self,
        plan: &'a super::clone_plan::GitPlan,
        cancel: Option<watch::Receiver<bool>>,
    ) -> super::clone_plan::GitPlanFuture<'a> {
        Box::pin(async move {
            self.executor
                .run_clone_plan(plan, cancel)
                .await
                .map(|outcome| super::clone_plan::GitPlanOutcome {
                    exit_code: outcome.exit_code,
                    stdout: outcome.stdout,
                    stderr: outcome.stderr,
                })
                .map_err(map_git_clone_error)
        })
    }

    fn create_volume<'a>(
        &'a self,
        plan: &'a super::clone_volume::GitVolumePlan,
    ) -> super::clone_plan::GitPlanFuture<'a> {
        Box::pin(async move {
            self.executor
                .run_clone_volume_create(plan)
                .await
                .map(|outcome| super::clone_plan::GitPlanOutcome {
                    exit_code: outcome.exit_code,
                    stdout: outcome.stdout,
                    stderr: outcome.stderr,
                })
                .map_err(map_git_clone_error)
        })
    }

    fn remove_volume<'a>(
        &'a self,
        plan: &'a super::clone_volume::GitVolumePlan,
    ) -> super::clone_plan::GitPlanFuture<'a> {
        Box::pin(async move {
            self.executor
                .run_clone_volume_remove(plan)
                .await
                .map(|outcome| super::clone_plan::GitPlanOutcome {
                    exit_code: outcome.exit_code,
                    stdout: outcome.stdout,
                    stderr: outcome.stderr,
                })
                .map_err(map_git_clone_error)
        })
    }
}

fn map_git_clone_error(error: DockerExecutorError) -> super::clone_plan::GitCloneExecutorError {
    match error {
        DockerExecutorError::TimedOut => super::clone_plan::GitCloneExecutorError::TimedOut,
        DockerExecutorError::Cancelled => super::clone_plan::GitCloneExecutorError::Cancelled,
        DockerExecutorError::OutputLimit => super::clone_plan::GitCloneExecutorError::OutputLimit,
        DockerExecutorError::ReapTimeout => super::clone_plan::GitCloneExecutorError::ReapTimeout,
        DockerExecutorError::SpawnFailed | DockerExecutorError::Failed => {
            super::clone_plan::GitCloneExecutorError::Failed
        }
    }
}
