use anyhow::{Context, ensure};
use runner_protocol::{RetainedSessionState, RetainedSessionStatus, SessionOutputChunk};

#[derive(Default)]
struct Stream {
    bytes: Vec<u8>,
    floor: u64,
    end: u64,
    complete: bool,
}

impl Stream {
    fn push(&mut self, bytes: &[u8], limit: usize) -> anyhow::Result<()> {
        let end = self
            .end
            .checked_add(u64::try_from(bytes.len())?)
            .context("output offset overflow")?;
        if bytes.len() >= limit {
            self.bytes.clear();
            self.bytes.extend_from_slice(&bytes[bytes.len() - limit..]);
        } else {
            let drop = self.bytes.len().saturating_sub(limit - bytes.len());
            self.bytes.drain(..drop);
            self.bytes.extend_from_slice(bytes);
        }
        self.floor = end - u64::try_from(self.bytes.len())?;
        self.end = end;
        Ok(())
    }
}

pub(super) struct RetainedOutput {
    session_id: u64,
    limit: usize,
    stdout: Stream,
    stderr: Stream,
    state: RetainedSessionState,
    exit_code: Option<i32>,
}

impl RetainedOutput {
    pub(super) fn new(session_id: u64, limit: usize) -> Self {
        Self {
            session_id,
            limit,
            stdout: Stream::default(),
            stderr: Stream::default(),
            state: RetainedSessionState::Running,
            exit_code: None,
        }
    }

    pub(super) fn push(&mut self, stderr: bool, bytes: &[u8]) -> anyhow::Result<()> {
        let stream = if stderr {
            &mut self.stderr
        } else {
            &mut self.stdout
        };
        stream.push(bytes, self.limit)
    }

    pub(super) fn reader_complete(&mut self, stderr: bool) {
        if stderr {
            self.stderr.complete = true;
        } else {
            self.stdout.complete = true;
        }
    }

    pub(super) fn terminal(
        &mut self,
        state: RetainedSessionState,
        exit_code: Option<i32>,
    ) -> anyhow::Result<()> {
        ensure!(
            state != RetainedSessionState::Running,
            "terminal state required"
        );
        if self.state == RetainedSessionState::Running {
            self.state = state;
            self.exit_code = exit_code;
        }
        Ok(())
    }

    pub(super) fn status(&self) -> RetainedSessionStatus {
        RetainedSessionStatus {
            state: self.state.clone(),
            exit_code: self.exit_code,
            output_complete: self.complete(),
            stdout_bytes: self.stdout.end,
            stderr_bytes: self.stderr.end,
            dropped_bytes: self.stdout.floor.saturating_add(self.stderr.floor),
        }
    }

    fn complete(&self) -> bool {
        self.state != RetainedSessionState::Running && self.stdout.complete && self.stderr.complete
    }

    pub(super) fn read(
        &self,
        cursor: Option<&str>,
        max_bytes: usize,
    ) -> anyhow::Result<SessionOutputChunk> {
        ensure!(
            max_bytes > 0 && max_bytes <= 1024 * 1024,
            "invalid output cap"
        );
        let (out, err) = match cursor {
            None => (0, 0),
            Some(cursor) => self.parse_cursor(cursor)?,
        };
        ensure!(
            out <= self.stdout.end && err <= self.stderr.end,
            "future output cursor"
        );
        let trimmed = out < self.stdout.floor || err < self.stderr.floor;
        let out = out.max(self.stdout.floor);
        let err = err.max(self.stderr.floor);
        let out_start = usize::try_from(out - self.stdout.floor)?;
        let err_start = usize::try_from(err - self.stderr.floor)?;
        let out_len = (self.stdout.bytes.len() - out_start).min(max_bytes);
        let err_len = (self.stderr.bytes.len() - err_start).min(max_bytes - out_len);
        let next_out = out + u64::try_from(out_len)?;
        let next_err = err + u64::try_from(err_len)?;
        let output_complete = self.complete();
        Ok(SessionOutputChunk {
            stdout: self.stdout.bytes[out_start..out_start + out_len].to_vec(),
            stderr: self.stderr.bytes[err_start..err_start + err_len].to_vec(),
            next_cursor: format!("v1:{:x}:{:x}:{:x}", self.session_id, next_out, next_err),
            eof: output_complete && next_out == self.stdout.end && next_err == self.stderr.end,
            output_complete,
            trimmed,
            truncated: trimmed || next_out < self.stdout.end || next_err < self.stderr.end,
            dropped_bytes: self.stdout.floor.saturating_add(self.stderr.floor),
            stdout_bytes: self.stdout.end,
            stderr_bytes: self.stderr.end,
        })
    }

    fn parse_cursor(&self, cursor: &str) -> anyhow::Result<(u64, u64)> {
        ensure!(cursor.len() <= 128, "invalid output cursor");
        let mut parts = cursor.split(':');
        ensure!(parts.next() == Some("v1"), "invalid output cursor");
        let parse = |part: Option<&str>| -> anyhow::Result<u64> {
            let part = part.context("invalid output cursor")?;
            ensure!(
                !part.is_empty() && part.bytes().all(|c| c.is_ascii_hexdigit()),
                "invalid output cursor"
            );
            u64::from_str_radix(part, 16).map_err(|_| anyhow::anyhow!("invalid output cursor"))
        };
        let session = parse(parts.next())?;
        let stdout = parse(parts.next())?;
        let stderr = parse(parts.next())?;
        ensure!(
            parts.next().is_none() && session == self.session_id,
            "invalid output cursor"
        );
        Ok((stdout, stderr))
    }
}

#[cfg(test)]
#[path = "shell_output_tests.rs"]
mod tests;
