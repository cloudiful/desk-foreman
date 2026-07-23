use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use desk_foreman::shell::ShellToolOutput;
use runner_protocol::RunnerSessionStatus;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::Child,
    sync::Mutex,
    time::sleep,
};

pub const DEFAULT_YIELD_TIME_MS: u64 = 1000;

pub(super) struct SessionState {
    pub(super) started_at: Instant,
    pub(super) last_activity: Instant,
    pub(super) buffer: Vec<u8>,
    pub(super) delivered_offset: usize,
    pub(super) stdout_buffer: Vec<u8>,
    pub(super) stderr_buffer: Vec<u8>,
    pub(super) stdout_delivered_offset: usize,
    pub(super) stderr_delivered_offset: usize,
    pub(super) chunk_counter: u64,
    pub(super) exit_code: Option<i32>,
    pub(super) stdout_bytes: usize,
    pub(super) stderr_bytes: usize,
    pub(super) output_dropped: bool,
    pub(super) timed_out: bool,
    pub(super) cancelled: bool,
}

pub(super) struct ShellSession {
    pub(super) id: u64,
    pub(super) writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    pub(super) child: Mutex<Child>,
    pub(super) state: Mutex<SessionState>,
    pub(super) max_output_bytes: usize,
    pub(super) output_is_combined: bool,
}

impl ShellSession {
    pub(super) fn new(
        id: u64,
        writer: Box<dyn AsyncWrite + Send + Unpin>,
        child: Child,
        max_output_bytes: usize,
        output_is_combined: bool,
    ) -> Self {
        Self {
            id,
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            state: Mutex::new(SessionState {
                started_at: Instant::now(),
                last_activity: Instant::now(),
                buffer: Vec::new(),
                delivered_offset: 0,
                stdout_buffer: Vec::new(),
                stderr_buffer: Vec::new(),
                stdout_delivered_offset: 0,
                stderr_delivered_offset: 0,
                chunk_counter: 0,
                exit_code: None,
                stdout_bytes: 0,
                stderr_bytes: 0,
                output_dropped: false,
                timed_out: false,
                cancelled: false,
            }),
            max_output_bytes,
            output_is_combined,
        }
    }

    pub(super) fn spawn_reader<R>(session: Arc<Self>, mut reader: R, stderr: bool)
    where
        R: AsyncRead + Send + Unpin + 'static,
    {
        let output_session = Arc::clone(&session);
        tokio::spawn(async move {
            let task = async move {
                let mut buffer = vec![0u8; 8192];
                loop {
                    let read = reader.read(&mut buffer).await?;
                    if read == 0 {
                        break;
                    }
                    output_session.push_output(&buffer[..read], stderr).await;
                }
                Ok::<(), std::io::Error>(())
            };
            if let Err(error) = task.await {
                session
                    .push_output(
                        format!("\n[desk-foreman read error: {error}]\n").as_bytes(),
                        true,
                    )
                    .await;
            }
        });
    }

    pub(super) fn spawn_timeout_watchdog(session: Arc<Self>, timeout: Duration) {
        tokio::spawn(async move {
            sleep(timeout).await;
            let _ = session.kill_timed_out().await;
        });
    }

    pub(super) async fn interact(
        &self,
        chars: &str,
        yield_time_ms: Option<u64>,
        max_output_tokens: Option<usize>,
        max_output_bytes: Option<usize>,
    ) -> anyhow::Result<ShellToolOutput> {
        {
            let mut state = self.state.lock().await;
            state.last_activity = Instant::now();
        }
        if !chars.is_empty() {
            let mut writer = self.writer.lock().await;
            writer
                .write_all(chars.as_bytes())
                .await
                .context("failed to write to session")?;
            writer
                .flush()
                .await
                .context("failed to flush session input")?;
        }

        let wait_ms = yield_time_ms.unwrap_or(DEFAULT_YIELD_TIME_MS);
        if wait_ms > 0 {
            sleep(Duration::from_millis(wait_ms)).await;
        }

        self.snapshot(max_output_tokens, max_output_bytes).await
    }

    pub(super) async fn push_output(&self, bytes: &[u8], stderr: bool) {
        let mut state = self.state.lock().await;
        if stderr {
            state.stderr_bytes = state.stderr_bytes.saturating_add(bytes.len());
            state.stderr_buffer.extend_from_slice(bytes);
            let max_buffer = self.max_output_bytes.saturating_mul(8).max(1);
            if state.stderr_buffer.len() > max_buffer {
                let drop = state.stderr_buffer.len() - max_buffer;
                state.stderr_buffer.drain(0..drop);
                state.stderr_delivered_offset = state.stderr_delivered_offset.saturating_sub(drop);
                state.output_dropped = true;
            }
        } else {
            state.stdout_bytes = state.stdout_bytes.saturating_add(bytes.len());
            state.stdout_buffer.extend_from_slice(bytes);
            let max_buffer = self.max_output_bytes.saturating_mul(8).max(1);
            if state.stdout_buffer.len() > max_buffer {
                let drop = state.stdout_buffer.len() - max_buffer;
                state.stdout_buffer.drain(0..drop);
                state.stdout_delivered_offset = state.stdout_delivered_offset.saturating_sub(drop);
                state.output_dropped = true;
            }
        }
        state.buffer.extend_from_slice(bytes);
        let max_buffer = self.max_output_bytes.saturating_mul(8).max(1);
        if state.buffer.len() > max_buffer {
            let drop = state.buffer.len() - max_buffer;
            state.buffer.drain(0..drop);
            state.delivered_offset = state.delivered_offset.saturating_sub(drop);
            state.output_dropped = true;
        }
        state.last_activity = Instant::now();
    }

    pub(super) async fn last_activity(&self) -> Instant {
        self.state.lock().await.last_activity
    }

    pub(super) async fn kill(&self) -> anyhow::Result<()> {
        let mut child = self.child.lock().await;
        child.kill().await.context("failed to kill expired session")
    }

    pub(super) async fn kill_timed_out(&self) -> anyhow::Result<()> {
        let mut child = self.child.lock().await;
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        {
            let mut state = self.state.lock().await;
            state.timed_out = true;
        }
        child
            .kill()
            .await
            .context("failed to kill timed out session")
    }

    pub(super) async fn cancel(&self) -> anyhow::Result<()> {
        self.state.lock().await.cancelled = true;
        let mut child = self.child.lock().await;
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        child.kill().await.context("failed to cancel session")
    }

    pub(super) async fn snapshot(
        &self,
        max_output_tokens: Option<usize>,
        max_output_bytes: Option<usize>,
    ) -> anyhow::Result<ShellToolOutput> {
        let exit_code = {
            let mut child = self.child.lock().await;
            child
                .try_wait()
                .context("failed to check child status")?
                .and_then(|status| status.code())
        };
        let mut state = self.state.lock().await;
        state.last_activity = Instant::now();
        if exit_code.is_some() {
            state.exit_code = exit_code;
        }

        let unread = &state.buffer[state.delivered_offset..];
        let original_token_count = (!unread.is_empty()).then(|| count_tokens_lossy(unread));
        let byte_limit = byte_limit(
            self.max_output_bytes
                .min(max_output_bytes.unwrap_or(self.max_output_bytes)),
            max_output_tokens,
        );
        let returned = utf8_prefix_len(unread, byte_limit);
        let output = String::from_utf8_lossy(&unread[..returned]).to_string();
        let channel_limit = byte_limit / 2;
        let stdout_unread = &state.stdout_buffer[state.stdout_delivered_offset..];
        let stderr_unread = &state.stderr_buffer[state.stderr_delivered_offset..];
        let stdout_returned = utf8_prefix_len(stdout_unread, channel_limit);
        let stderr_returned =
            utf8_prefix_len(stderr_unread, byte_limit.saturating_sub(stdout_returned));
        let stdout = String::from_utf8_lossy(&stdout_unread[..stdout_returned]).to_string();
        let stderr = String::from_utf8_lossy(&stderr_unread[..stderr_returned]).to_string();
        let truncated = returned < unread.len() || state.output_dropped;
        let has_more = returned < unread.len();
        state.output_dropped = false;
        state.delivered_offset += returned;
        state.stdout_delivered_offset += stdout_returned;
        state.stderr_delivered_offset += stderr_returned;
        if state.delivered_offset > 65_536 && state.delivered_offset >= state.buffer.len() / 2 {
            let delivered_offset = state.delivered_offset;
            state.buffer.drain(0..delivered_offset);
            state.delivered_offset = 0;
        }
        if state.stdout_delivered_offset > 65_536
            && state.stdout_delivered_offset >= state.stdout_buffer.len() / 2
        {
            let delivered_offset = state.stdout_delivered_offset;
            state.stdout_buffer.drain(0..delivered_offset);
            state.stdout_delivered_offset = 0;
        }
        if state.stderr_delivered_offset > 65_536
            && state.stderr_delivered_offset >= state.stderr_buffer.len() / 2
        {
            let delivered_offset = state.stderr_delivered_offset;
            state.stderr_buffer.drain(0..delivered_offset);
            state.stderr_delivered_offset = 0;
        }
        let chunk_id = if has_more {
            state.chunk_counter += 1;
            Some(format!("session-{}-chunk-{}", self.id, state.chunk_counter))
        } else {
            None
        };
        let next_cursor =
            has_more.then(|| format!("session-{}-offset-{}", self.id, state.delivered_offset));
        let wall_time_seconds = state.started_at.elapsed().as_secs_f64();

        Ok(ShellToolOutput {
            wall_time_seconds,
            output,
            stdout,
            stderr,
            output_is_combined: self.output_is_combined,
            chunk_id,
            exit_code: state.exit_code,
            session_id: state.exit_code.is_none().then_some(self.id),
            original_token_count,
            truncated,
            has_more,
            next_cursor,
            stdout_bytes: state.stdout_bytes,
            stderr_bytes: state.stderr_bytes,
            timed_out: state.timed_out,
        })
    }

    pub(super) async fn status(&self) -> anyhow::Result<RunnerSessionStatus> {
        let mut child = self.child.lock().await;
        let exit_code = child.try_wait()?.and_then(|status| status.code());
        let state = self.state.lock().await;
        Ok(RunnerSessionStatus {
            session_id: self.id,
            owner: runner_protocol::RunnerOwner::InternalUser { user_id: 0 },
            session_key: None,
            state: if state.cancelled {
                "cancelled".to_string()
            } else if state.timed_out {
                "timed_out".to_string()
            } else if exit_code.is_some() {
                "exited".to_string()
            } else {
                "running".to_string()
            },
            exit_code,
            timed_out: state.timed_out,
            wall_time_seconds: state.started_at.elapsed().as_secs_f64(),
        })
    }
}

fn byte_limit(max_output_bytes: usize, max_output_tokens: Option<usize>) -> usize {
    match max_output_tokens {
        Some(tokens) => max_output_bytes.min(tokens.saturating_mul(16).max(1024)),
        None => max_output_bytes,
    }
}

fn count_tokens_lossy(bytes: &[u8]) -> usize {
    String::from_utf8_lossy(bytes).split_whitespace().count()
}

fn utf8_prefix_len(bytes: &[u8], limit: usize) -> usize {
    let mut end = bytes.len().min(limit);
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
    }
    end
}
