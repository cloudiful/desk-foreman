//! Exact-name Docker volume lifecycle for one Git clone operation.
//!
//! Only a generated, bounded, operation-owned name can be created or removed.
//! There is no list, inspect, or sweep operation.

use std::time::Duration;

use uuid::Uuid;

pub(crate) const VOLUME_NAME_PREFIX: &str = "dfgit-";
pub(crate) const VOLUME_NAME_MAX_LEN: usize = 63;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitVolumePlan {
    pub(crate) name: String,
    pub(crate) bytes: u64,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) deadline: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitVolumeError {
    Name,
    Size,
    Limits,
}

impl std::fmt::Display for GitVolumeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Name => "git clone volume name is invalid",
            Self::Size => "git clone volume size is invalid",
            Self::Limits => "git clone volume limits are invalid",
        })
    }
}

impl std::error::Error for GitVolumeError {}

impl GitVolumePlan {
    /// Generate one volume plan from a job/operation identity.
    pub(crate) fn generate(
        job_id: Uuid,
        operation_id: Uuid,
        bytes: u64,
        uid: u32,
        gid: u32,
        deadline: Duration,
    ) -> Result<Self, GitVolumeError> {
        if bytes == 0 || deadline.is_zero() {
            return Err(GitVolumeError::Limits);
        }
        let suffix = format!(
            "{}{}",
            job_id
                .simple()
                .to_string()
                .get(..12)
                .unwrap_or("000000000000"),
            operation_id
                .simple()
                .to_string()
                .get(..12)
                .unwrap_or("000000000000")
        );
        let name = format!("{VOLUME_NAME_PREFIX}{suffix}");
        let plan = Self {
            name,
            bytes,
            uid,
            gid,
            deadline,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub(crate) fn validate(&self) -> Result<(), GitVolumeError> {
        if self.bytes == 0 || self.deadline.is_zero() {
            return Err(GitVolumeError::Limits);
        }
        if self.name.len() > VOLUME_NAME_MAX_LEN
            || !self.name.starts_with(VOLUME_NAME_PREFIX)
            || !self
                .name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || self.name.ends_with('-')
            || self.name.contains("--")
        {
            return Err(GitVolumeError::Name);
        }
        Ok(())
    }

    /// Exact create argv. The `o=` value is one validated argument.
    pub(crate) fn create_args(&self, docker_cli: &str) -> Result<Vec<String>, GitVolumeError> {
        self.validate()?;
        let options = format!(
            "size={},uid={},gid={},mode=0700,nosuid,nodev,noexec",
            self.bytes, self.uid, self.gid
        );
        Ok(vec![
            docker_cli.to_string(),
            "volume".to_string(),
            "create".to_string(),
            "--driver".to_string(),
            "local".to_string(),
            "--opt".to_string(),
            "type=tmpfs".to_string(),
            "--opt".to_string(),
            "device=tmpfs".to_string(),
            "--opt".to_string(),
            format!("o={options}"),
            self.name.clone(),
        ])
    }

    /// Exact removal argv for the same owned name.
    pub(crate) fn remove_args(&self, docker_cli: &str) -> Result<Vec<String>, GitVolumeError> {
        self.validate()?;
        Ok(vec![
            docker_cli.to_string(),
            "volume".to_string(),
            "rm".to_string(),
            "-f".to_string(),
            self.name.clone(),
        ])
    }
}

/// Byte source of the create result. Only the exact owned name is accepted.
pub(crate) fn create_stdout_matches(stdout: &[u8], name: &str) -> bool {
    String::from_utf8_lossy(stdout).trim() == name
}

/// Narrow missing-volume classification: the message must identify a missing
/// volume and the exact owned name. A generic Docker "not found" is not enough.
pub(crate) fn is_missing_volume_error(message: &str, name: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("no such volume") && message.contains(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_name_is_owned_bounded_and_stable() {
        let job = Uuid::new_v4();
        let op = Uuid::new_v4();
        let plan = GitVolumePlan::generate(job, op, 4096, 1000, 1001, Duration::from_secs(60))
            .expect("plan");
        assert!(plan.name.starts_with(VOLUME_NAME_PREFIX));
        assert!(plan.name.len() <= VOLUME_NAME_MAX_LEN);
        assert_eq!(
            plan.name,
            GitVolumePlan::generate(job, op, 4096, 1000, 1001, Duration::from_secs(60))
                .expect("same plan")
                .name
        );
        let create = plan.create_args("docker").expect("create");
        assert_eq!(create[0], "docker");
        assert_eq!(create[1], "volume");
        assert!(create.iter().any(|arg| arg == "type=tmpfs"));
        assert!(
            create
                .iter()
                .any(|arg| arg.starts_with("o=size=4096,uid=1000,gid=1001,"))
        );
        assert!(
            create
                .iter()
                .any(|arg| arg == "mode=0700,nosuid,nodev,noexec"
                    || arg.ends_with(",mode=0700,nosuid,nodev,noexec"))
        );
        assert_eq!(
            plan.remove_args("docker").expect("remove"),
            vec!["docker", "volume", "rm", "-f", plan.name.as_str()]
        );
    }

    #[test]
    fn zero_size_or_deadline_is_rejected() {
        let job = Uuid::new_v4();
        let op = Uuid::new_v4();
        assert_eq!(
            GitVolumePlan::generate(job, op, 0, 1, 1, Duration::from_secs(1)),
            Err(GitVolumeError::Limits)
        );
        assert_eq!(
            GitVolumePlan::generate(job, op, 1, 1, 1, Duration::ZERO),
            Err(GitVolumeError::Limits)
        );
    }

    #[test]
    fn missing_volume_requires_the_exact_owned_name() {
        let name = "dfgit-abc";
        assert!(is_missing_volume_error("no such volume: dfgit-abc", name));
        assert!(!is_missing_volume_error(
            "no such container: dfgit-abc",
            name
        ));
        assert!(!is_missing_volume_error("no such volume: other", name));
        assert!(!is_missing_volume_error("not found", name));
        assert!(create_stdout_matches(b"dfgit-abc\n", name));
        assert!(!create_stdout_matches(b"dfgit-abcd\n", name));
    }
}
