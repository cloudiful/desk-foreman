//! Per-generation service credential storage (issue 749 P2a).
//!
//! Each `(environment, generation)` gets one random credential, generated on
//! the manager host and handed to the container only through a Docker
//! `--env-file` path. The token itself never appears in an argv, a label, a
//! volume, a DTO, or a diagnostic.
//!
//! The credential is drawn with the workspace `uuid` crate's v4 generator,
//! so it is a well-formed UUIDv4 backed by a real CSPRNG rather than a
//! hand-rolled one. The file that holds it is the trust boundary:
//!
//! - created through a `create_new` temporary file at mode `0600`, written,
//!   fsynced, atomically renamed into place, with the directory fsynced;
//! - stored under the manager's own state root in an owner-only directory
//!   whose chain is re-validated on every access;
//! - re-validated on every read against the *opened handle* — regular
//!   file, mode `0600`, owned by this manager's effective uid, bounded size,
//!   and exactly the expected `NAME=token` line.
//!
//! A file that fails any check is refused and left untouched: it is never
//! re-permissioned, rewritten, or deleted. A file that passes is reused,
//! which is what makes the credential stable across a restart of the same
//! generation and distinct across generations and environments.
//!
//! See [`super::fs_guard`] for exactly which race windows are closed by
//! handle-based checks and which are covered by the trusted-state-root
//! assumption.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::fs_guard::{
    CREDENTIAL_MODE, assert_private_directory, prepare_private_directory,
    read_credential_from_handle, read_credential_token_from_handle, refuse_link_or_irregular,
    validate_directory_chain,
};
use super::host_identity::UidSource;

/// Directory under the per-environment root that holds credentials.
const CREDENTIAL_DIR: &str = "service";

/// A validated credential, reduced to what a launch actually needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceCredential {
    path: PathBuf,
}

impl ServiceCredential {
    /// Host path passed to `docker run --env-file`.
    pub fn env_file(&self) -> &Path {
        &self.path
    }
}

/// Resolves and materialises credentials under one manager state root.
#[derive(Clone, Debug)]
pub struct ServiceCredentialStore {
    root: PathBuf,
    uid_source: UidSource,
}

impl ServiceCredentialStore {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            root: state_dir.to_path_buf(),
            uid_source: UidSource::Proc,
        }
    }

    /// Build a store with an explicit uid source. Test-only: the store's
    /// ownership check accepts a uid only through this type, so a test can
    /// simulate an unavailable or different owner without any way to weaken
    /// the production path.
    #[cfg(test)]
    pub(crate) fn with_uid_source(state_dir: &Path, uid_source: UidSource) -> Self {
        Self {
            root: state_dir.to_path_buf(),
            uid_source,
        }
    }

    /// Credential path for one `(environment, generation)`.
    ///
    /// Derived only from values the manager already owns, under the same
    /// per-environment directory the durable barrier uses, so no request can
    /// select a path and the two live together. The identifier is validated
    /// and normalized exactly as the barrier store normalizes it.
    pub fn credential_path(
        &self,
        environment_id: &str,
        generation: i64,
    ) -> anyhow::Result<PathBuf> {
        runner_protocol::execution_environment::validate_environment_id(environment_id)
            .map_err(|error| anyhow::anyhow!(error))?;
        let simple: String = environment_id
            .chars()
            .filter(|c| *c != '-')
            .collect::<String>()
            .to_ascii_lowercase();
        Ok(self
            .root
            .join(simple)
            .join(CREDENTIAL_DIR)
            .join(format!("{generation}.env")))
    }

    /// Read and validate an existing credential, failing closed when it is
    /// absent, when ownership cannot be established, or when any check
    /// fails. Used on the reuse and restart branches, where minting a fresh
    /// value would leave a running agent holding a credential nothing else
    /// knows.
    pub fn existing(
        &self,
        environment_id: &str,
        generation: i64,
        env_name: &str,
    ) -> anyhow::Result<ServiceCredential> {
        let uid = self.uid_source.effective_uid()?;
        let path = self.credential_path(environment_id, generation)?;
        let directory = self.credential_directory(&path)?;
        // Chain first: a writable parent would let the file be swapped no
        // matter how narrow the file's own mode is.
        validate_directory_chain(&self.root, &directory)?;
        assert_private_directory(&directory, uid)?;
        refuse_link_or_irregular(&path)?;
        let file = File::open(&path)
            .map_err(|error| anyhow::anyhow!("service credential is not available: {error}"))?;
        read_credential_from_handle(&file, env_name, uid)?;
        Ok(ServiceCredential { path })
    }

    /// Read the raw token for the actual relay consumer, failing closed
    /// exactly like [`ServiceCredentialStore::existing`].
    ///
    /// Crate-private on purpose: the value must reach only the manager-side
    /// Basic-auth injection, never a DTO, a `Debug`/`Display` format, an
    /// error message, or a log line. Callers must treat the return as a
    /// secret and never record it.
    pub(crate) fn read_token(
        &self,
        environment_id: &str,
        generation: i64,
        env_name: &str,
    ) -> anyhow::Result<String> {
        let uid = self.uid_source.effective_uid()?;
        let path = self.credential_path(environment_id, generation)?;
        let directory = self.credential_directory(&path)?;
        validate_directory_chain(&self.root, &directory)?;
        assert_private_directory(&directory, uid)?;
        refuse_link_or_irregular(&path)?;
        let file = File::open(&path)
            .map_err(|error| anyhow::anyhow!("service credential is not available: {error}"))?;
        read_credential_token_from_handle(&file, env_name, uid)
    }

    /// Return the existing credential, creating it exactly once when it is
    /// genuinely absent. A file that exists but fails validation is an
    /// error, never an overwrite.
    pub fn obtain(
        &self,
        environment_id: &str,
        generation: i64,
        env_name: &str,
    ) -> anyhow::Result<ServiceCredential> {
        let path = self.credential_path(environment_id, generation)?;
        if path.symlink_metadata().is_ok() {
            return self.existing(environment_id, generation, env_name);
        }
        self.create(environment_id, generation, env_name)
    }

    fn create(
        &self,
        environment_id: &str,
        generation: i64,
        env_name: &str,
    ) -> anyhow::Result<ServiceCredential> {
        let uid = self.uid_source.effective_uid()?;
        let path = self.credential_path(environment_id, generation)?;
        let directory = self.credential_directory(&path)?;
        prepare_private_directory(&self.root, &directory)?;
        assert_private_directory(&directory, uid)?;

        // Same-directory temporary file so the rename cannot cross a
        // filesystem, and `create_new` so two writers cannot both believe
        // they won.
        let temporary = directory.join(format!(".{generation}.env.{}", std::process::id()));
        let contents = format!("{env_name}={}\n", Uuid::new_v4());
        let written = (|| -> anyhow::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(CREDENTIAL_MODE)
                .open(&temporary)?;
            // `open` masks the requested mode with the process umask, so
            // the owner-only mode is set explicitly rather than trusted.
            file.set_permissions(fs::Permissions::from_mode(CREDENTIAL_MODE))?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &path)?;
            super::fs_guard::sync_directory(&directory)
        })();
        if let Err(error) = written {
            // Remove only our own incomplete temporary file; an existing
            // credential is never cleaned up here.
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(ServiceCredential { path })
    }

    fn credential_directory(&self, path: &Path) -> anyhow::Result<PathBuf> {
        path.parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("credential path has no parent directory"))
    }
}
