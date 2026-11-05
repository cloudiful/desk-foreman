use std::fs::{self, File};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use std::os::unix::fs::PermissionsExt;

use super::import::RawGitSnapshotRequest;

pub(super) const HEAD_HEX: &str = "0123456789abcdef0123456789abcdef01234567";
pub(super) const OTHER_HEX: &str = "fedcba9876543210fedcba9876543210fedcba98";
pub(super) const LOOSE_BYTES: &[u8] = b"raw-loose-object-payload";
pub(super) const PACK_BYTES: &[u8] = b"raw-packfile-payload";

pub(super) struct Fixture {
    pub(super) temp: tempfile::TempDir,
    pub(super) workspace: PathBuf,
    pub(super) checkout: PathBuf,
    pub(super) quarantine: PathBuf,
    pub(super) loose_name: String,
    pub(super) pack_name: String,
    pub(super) hook_marker: PathBuf,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let checkout = workspace.join("checkout");
        let quarantine = temp.path().join("quarantine");
        let git = checkout.join(".git");

        fs::create_dir_all(git.join("objects/aa")).expect("loose object dir");
        fs::create_dir_all(git.join("objects/pack")).expect("pack dir");
        fs::create_dir_all(git.join("objects/info")).expect("info dir");
        fs::create_dir_all(git.join("refs/heads")).expect("refs dir");
        fs::create_dir_all(git.join("hooks")).expect("hooks dir");
        fs::create_dir_all(git.join("info")).expect("git info dir");
        fs::create_dir_all(git.join("modules")).expect("modules dir");
        fs::create_dir_all(git.join("logs")).expect("logs dir");
        fs::create_dir_all(&quarantine).expect("quarantine dir");
        fs::set_permissions(&quarantine, fs::Permissions::from_mode(0o700))
            .expect("quarantine mode");

        write(git.join("HEAD"), "ref: refs/heads/main\n");
        write(git.join("refs/heads/main"), format!("{HEAD_HEX}\n"));
        write(git.join("refs/heads/other"), format!("{OTHER_HEX}\n"));

        let loose_name = "b".repeat(38);
        write(git.join("objects/aa").join(&loose_name), LOOSE_BYTES);

        let pack_name = format!("pack-{}.pack", "c".repeat(40));
        write(git.join("objects/pack").join(&pack_name), PACK_BYTES);
        write(
            git.join("objects/pack")
                .join(format!("pack-{}.idx", "c".repeat(40))),
            b"raw-index-payload",
        );

        write(
            git.join("config"),
            b"[core]\n\trepositoryformatversion = 0\n\tfsmonitor = /nonexistent/canary\n[url \"ssh://canary\"]\n\tinsteadOf = https://canary\n[http]\n\tproxy = http://canary\n",
        );
        let hook_marker = temp.path().join("hook-executed");
        write(
            git.join("hooks").join("pre-commit"),
            format!("#!/bin/sh\n: > '{}'\n", hook_marker.display()),
        );
        fs::set_permissions(
            git.join("hooks").join("pre-commit"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("hook mode");
        write(git.join("info/attributes"), b"* canary\n");
        write(git.join("attributes"), b"* canary\n");
        write(git.join("modules/canary"), b"module canary\n");
        write(git.join("logs/HEAD"), b"log canary\n");
        write(git.join("info/include"), b"include canary\n");
        write(git.join("info/grafts"), b"graft canary\n");

        Self {
            temp,
            workspace,
            checkout,
            quarantine,
            loose_name,
            pack_name,
            hook_marker,
        }
    }

    pub(super) fn request(&self) -> RawGitSnapshotRequest {
        RawGitSnapshotRequest {
            workspace_root: open_dir(&self.workspace),
            quarantine_root: open_dir(&self.quarantine),
            checkout_relative: PathBuf::from("checkout"),
            pinned_branch: "main".to_owned(),
            expected_head: HEAD_HEX.to_owned(),
        }
    }

    pub(super) fn git(&self) -> PathBuf {
        self.checkout.join(".git")
    }

    pub(super) fn objects(&self) -> PathBuf {
        self.git().join("objects")
    }

    pub(super) fn loose_dir(&self) -> PathBuf {
        self.objects().join("aa")
    }

    pub(super) fn loose_file(&self) -> PathBuf {
        self.loose_dir().join(&self.loose_name)
    }

    pub(super) fn pack_dir(&self) -> PathBuf {
        self.objects().join("pack")
    }

    pub(super) fn snapshot_path(&self, snapshot_name: &str) -> PathBuf {
        self.quarantine.join(snapshot_name)
    }
}

pub(super) fn open_dir(path: &Path) -> OwnedFd {
    File::open(path).expect("open dir").into()
}

pub(super) fn write(path: PathBuf, bytes: impl AsRef<[u8]>) {
    fs::write(path, bytes).expect("write fixture file");
}

pub(super) fn add_loose_object(fixture: &Fixture, name: &str, bytes: &[u8]) -> PathBuf {
    let path = fixture.loose_dir().join(name);
    write(path.clone(), bytes);
    path
}

pub(super) fn read(path: &Path) -> Vec<u8> {
    fs::read(path).expect("read fixture file")
}
