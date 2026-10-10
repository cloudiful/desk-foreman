//! Durable execution-environment generation barrier (issue 742 P1).
//!
//! Persists, per environment and derived only from the environment UUID, a
//! monotonic generation high-water plus an optional stopped generation. The
//! barrier survives store reopen and manager restart so a late ensure can
//! never restart a closed or lower generation. Writes are atomic
//! (same-directory temp file, file sync, rename, directory sync); corrupted,
//! invalid, or oversize state fails closed instead of being reset.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

const BARRIER_FILE: &str = "barrier.json";
const LOCK_FILE: &str = "lock";
const BARRIER_VERSION: u32 = 1;
/// Upper bound for one serialized barrier file; larger state fails closed.
const MAX_BARRIER_BYTES: u64 = 4096;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Durable per-environment generation barrier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EnvironmentBarrier {
    /// Highest generation ever persisted by an ensure or stop attempt.
    pub(crate) high_water: i64,
    /// Highest generation durably closed by a stop barrier.
    pub(crate) stopped_generation: Option<i64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BarrierRecord {
    version: u32,
    high_water: i64,
    #[serde(default)]
    stopped_generation: Option<i64>,
}

/// Durable barrier store rooted in one dedicated state directory.
pub(crate) struct TombstoneStore {
    root: PathBuf,
}

impl TombstoneStore {
    /// Open the store for one environment. Only the validated environment
    /// UUID selects the directory; no request-supplied path is accepted.
    pub(crate) fn open(state_dir: &Path, environment_id: &str) -> anyhow::Result<Self> {
        if state_dir.as_os_str().is_empty() {
            anyhow::bail!("execution environment state directory is empty");
        }
        runner_protocol::execution_environment::validate_environment_id(environment_id)
            .map_err(|error| anyhow::anyhow!(error))?;
        let simple: String = environment_id
            .chars()
            .filter(|c| *c != '-')
            .collect::<String>()
            .to_ascii_lowercase();
        let root = state_dir.join(simple);
        std::fs::create_dir_all(&root).map_err(|error| {
            anyhow::anyhow!(
                "failed to create execution environment state directory {}: {error}",
                root.display()
            )
        })?;
        Ok(Self { root })
    }

    pub(crate) fn barrier_path(&self) -> PathBuf {
        self.root.join(BARRIER_FILE)
    }

    pub(crate) fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }

    /// Read the current barrier. A missing file is an empty barrier; a
    /// present but unreadable, corrupt, invalid, or oversize file is an
    /// error and is never silently reset.
    pub(crate) fn load(&self) -> anyhow::Result<EnvironmentBarrier> {
        let path = self.barrier_path();
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(EnvironmentBarrier::default());
            }
            Err(error) => {
                anyhow::bail!(
                    "failed to open execution environment barrier {}: {error}",
                    path.display()
                );
            }
        };
        let mut raw = Vec::new();
        (&mut file)
            .take(MAX_BARRIER_BYTES + 1)
            .read_to_end(&mut raw)
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to read execution environment barrier {}: {error}",
                    path.display()
                )
            })?;
        if raw.len() as u64 > MAX_BARRIER_BYTES {
            anyhow::bail!(
                "execution environment barrier {} exceeds the {MAX_BARRIER_BYTES} byte bound",
                path.display()
            );
        }
        let record: BarrierRecord = serde_json::from_slice(&raw).map_err(|error| {
            anyhow::anyhow!(
                "execution environment barrier {} is corrupt: {error}",
                path.display()
            )
        })?;
        if record.version != BARRIER_VERSION {
            anyhow::bail!(
                "execution environment barrier {} has unsupported version {}",
                path.display(),
                record.version
            );
        }
        let barrier = EnvironmentBarrier {
            high_water: record.high_water,
            stopped_generation: record.stopped_generation,
        };
        validate_barrier(&barrier).map_err(|error| {
            anyhow::anyhow!(
                "execution environment barrier {} is invalid: {error}",
                path.display()
            )
        })?;
        Ok(barrier)
    }

    /// Atomically persist a barrier. Callers always derive it from the
    /// loaded value with the monotonic helpers, so state never rewinds.
    pub(crate) fn save(&self, barrier: EnvironmentBarrier) -> anyhow::Result<()> {
        validate_barrier(&barrier)?;
        let record = BarrierRecord {
            version: BARRIER_VERSION,
            high_water: barrier.high_water,
            stopped_generation: barrier.stopped_generation,
        };
        let bytes = serde_json::to_vec(&record).map_err(|error| {
            anyhow::anyhow!("failed to encode execution environment barrier: {error}")
        })?;
        if bytes.len() as u64 > MAX_BARRIER_BYTES {
            anyhow::bail!(
                "execution environment barrier serialization exceeds the {MAX_BARRIER_BYTES} byte bound"
            );
        }
        let path = self.barrier_path();
        let tmp = self.root.join(format!(
            ".{BARRIER_FILE}.{}.{}.tmp",
            std::process::id(),
            unique_suffix()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|error| {
                anyhow::anyhow!(
                    "failed to create execution environment barrier temp file {}: {error}",
                    tmp.display()
                )
            })?;
        if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!(
                "failed to write execution environment barrier {}: {error}",
                path.display()
            );
        }
        drop(file);
        if let Err(error) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!(
                "failed to commit execution environment barrier {}: {error}",
                path.display()
            );
        }
        let dir = File::open(&self.root).map_err(|error| {
            anyhow::anyhow!(
                "failed to open execution environment state directory {}: {error}",
                self.root.display()
            )
        })?;
        dir.sync_all().map_err(|error| {
            anyhow::anyhow!(
                "failed to sync execution environment state directory {}: {error}",
                self.root.display()
            )
        })?;
        Ok(())
    }
}

fn validate_barrier(barrier: &EnvironmentBarrier) -> anyhow::Result<()> {
    if barrier.high_water < 0 {
        anyhow::bail!(
            "execution environment barrier high-water must be nonnegative, got {}",
            barrier.high_water
        );
    }
    if let Some(stopped) = barrier.stopped_generation {
        if stopped < 0 {
            anyhow::bail!(
                "execution environment stopped generation must be nonnegative, got {stopped}"
            );
        }
        if stopped > barrier.high_water {
            anyhow::bail!(
                "execution environment stopped generation {stopped} exceeds high-water {}",
                barrier.high_water
            );
        }
    }
    Ok(())
}

/// Refuse a generation that a durable barrier has already closed or that
/// sits below an already attempted higher generation.
pub(crate) fn ensure_generation_admissible(
    barrier: &EnvironmentBarrier,
    generation: i64,
) -> anyhow::Result<()> {
    if let Some(stopped) = barrier.stopped_generation {
        if stopped >= generation {
            anyhow::bail!(
                "execution environment generation {generation} is durable-stopped at generation {stopped}; refusing to run"
            );
        }
    }
    if barrier.high_water > generation {
        anyhow::bail!(
            "execution environment generation {generation} is below the durable high-water generation {}; refusing to run",
            barrier.high_water
        );
    }
    Ok(())
}

/// Monotonically raise the high-water to include an attempted generation.
pub(crate) fn advance_high_water(
    barrier: EnvironmentBarrier,
    generation: i64,
) -> EnvironmentBarrier {
    EnvironmentBarrier {
        high_water: barrier.high_water.max(generation),
        stopped_generation: barrier.stopped_generation,
    }
}

/// Monotonically record durable stop intent for one generation.
pub(crate) fn record_stopped(barrier: EnvironmentBarrier, generation: i64) -> EnvironmentBarrier {
    EnvironmentBarrier {
        high_water: barrier.high_water.max(generation),
        stopped_generation: Some(
            barrier
                .stopped_generation
                .map_or(generation, |stopped| stopped.max(generation)),
        ),
    }
}

fn unique_suffix() -> u64 {
    let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    counter ^ nanos
}
