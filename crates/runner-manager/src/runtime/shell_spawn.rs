use std::{path::PathBuf, process::Stdio};

use anyhow::Context;
use pty_process::Command as PtyCommand;
use tokio::process::Command;

use super::ProcessSpawnTarget;

pub fn append_shell_args(login: bool, raw: &str) -> Vec<String> {
    if login {
        vec!["-lc".to_string(), raw.to_string()]
    } else {
        vec!["-c".to_string(), raw.to_string()]
    }
}

pub fn build_command(target: &ProcessSpawnTarget) -> Command {
    let mut cmd = Command::new(&target.program);
    cmd.args(&target.args);
    if let Some(cwd) = &target.cwd {
        cmd.current_dir(cwd);
    }
    for (key, value) in &target.env {
        cmd.env(key, value);
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd
}

pub fn build_pty_command(target: &ProcessSpawnTarget) -> PtyCommand {
    let mut cmd = PtyCommand::new(&target.program).args(&target.args);
    if let Some(cwd) = &target.cwd {
        cmd = cmd.current_dir(PathBuf::from(cwd));
    }
    for (key, value) in &target.env {
        cmd = cmd.env(key, value);
    }
    cmd
}

pub fn open_pty() -> anyhow::Result<(pty_process::Pty, pty_process::Pts)> {
    let (pty, pts) = pty_process::open().context("failed to open PTY")?;
    pty.resize(pty_process::Size::new(24, 80))
        .context("failed to resize PTY")?;
    Ok((pty, pts))
}
