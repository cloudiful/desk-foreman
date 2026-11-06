use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rustix::fs::{FileType, Mode, mkfifoat};

use super::fixtures::{
    Fixture, HEAD_HEX, LOOSE_BYTES, OTHER_HEX, PACK_BYTES, add_loose_object, open_dir, read, write,
};
use super::import::seam;
use super::import::seam::Step;
use super::import::{
    GitImportError, RawGitSnapshot, RawGitSnapshotRequest, stage_raw_git_snapshot,
};
use super::limits::GitImportLimits;

fn limits() -> GitImportLimits {
    GitImportLimits {
        max_entries: 256,
        max_files: 256,
        max_file_bytes: 1024 * 1024,
        max_total_bytes: 1024 * 1024,
        max_metadata_bytes: 4096,
    }
}

fn stage(fixture: &Fixture) -> Result<RawGitSnapshot, GitImportError> {
    stage_raw_git_snapshot(fixture.request(), limits())
}

fn stage_with(
    fixture: &Fixture,
    limits: GitImportLimits,
) -> Result<RawGitSnapshot, GitImportError> {
    stage_raw_git_snapshot(fixture.request(), limits)
}

fn snapshot_root(fixture: &Fixture, snapshot: &RawGitSnapshot) -> PathBuf {
    fixture.snapshot_path(&snapshot.snapshot_name)
}

fn quarantine_is_empty(fixture: &Fixture) -> bool {
    fs::read_dir(&fixture.quarantine)
        .expect("quarantine")
        .next()
        .is_none()
}

fn canary(fixture: &Fixture, name: &str, bytes: &[u8]) -> PathBuf {
    let path = fixture.temp.path().join(name);
    write(path.clone(), bytes);
    path
}

#[test]
fn stages_loose_and_pack_objects_with_counts_and_pinned_head() {
    let fixture = Fixture::new();
    let snapshot = stage(&fixture).expect("raw snapshot");

    let expected_object_bytes =
        (LOOSE_BYTES.len() + PACK_BYTES.len() + b"raw-index-payload".len()) as u64;
    let expected_metadata_bytes = ("ref: refs/heads/main\n".len() + HEAD_HEX.len() + 1) as u64;
    assert_eq!(snapshot.object_files, 3);
    assert_eq!(snapshot.object_bytes, expected_object_bytes);
    assert_eq!(snapshot.metadata_files, 2);
    assert_eq!(snapshot.metadata_bytes, expected_metadata_bytes);
    assert_eq!(snapshot.captured_head, HEAD_HEX);
    assert_eq!(snapshot.pinned_branch, "main");
    assert!(snapshot.snapshot_name.starts_with("raw-git-"));

    let root = snapshot_root(&fixture, &snapshot);
    assert_eq!(
        read(&root.join("objects/aa").join(&fixture.loose_name)),
        LOOSE_BYTES
    );
    assert_eq!(
        read(&root.join("objects/pack").join(&fixture.pack_name)),
        PACK_BYTES
    );
    assert_eq!(
        read(
            &root
                .join("objects/pack")
                .join(format!("pack-{}.idx", "c".repeat(40)))
        ),
        b"raw-index-payload"
    );
    assert_eq!(read(&root.join("HEAD")), b"ref: refs/heads/main\n");
    assert_eq!(
        read(&root.join("refs/heads/main")),
        format!("{HEAD_HEX}\n").as_bytes()
    );

    for ignored in [
        "config",
        "hooks",
        "hooks/pre-commit",
        "info/attributes",
        "attributes",
        "modules",
        "logs",
        "info/include",
        "info/grafts",
        "objects/info",
        "refs/heads/other",
    ] {
        assert!(
            !root.join(ignored).exists(),
            "{ignored} must not be copied into quarantine"
        );
    }
    assert_eq!(
        read(&fixture.git().join("refs/heads/other")),
        format!("{OTHER_HEX}\n").as_bytes()
    );
    assert!(
        !fixture.hook_marker.exists(),
        "repository hook must not run"
    );
    assert_eq!(
        read(&fixture.git().join("config")),
        b"[core]\n\trepositoryformatversion = 0\n\tfsmonitor = /nonexistent/canary\n[url \"ssh://canary\"]\n\tinsteadOf = https://canary\n[http]\n\tproxy = http://canary\n"
    );
}

#[test]
fn detached_head_matches_expected_without_ref_metadata() {
    let fixture = Fixture::new();
    write(fixture.git().join("HEAD"), format!("{HEAD_HEX}\n"));
    let snapshot = stage(&fixture).expect("detached head");
    assert_eq!(snapshot.metadata_files, 1);
    assert_eq!(snapshot.metadata_bytes, (HEAD_HEX.len() + 1) as u64);
    let root = snapshot_root(&fixture, &snapshot);
    assert_eq!(read(&root.join("HEAD")), format!("{HEAD_HEX}\n").as_bytes());
    assert!(!root.join("refs").exists());
}

#[test]
fn sha256_object_format_uses_sixty_four_hex_names() {
    let fixture = Fixture::new();
    let head = "a".repeat(64);
    write(fixture.git().join("refs/heads/main"), format!("{head}\n"));
    fs::remove_file(fixture.loose_file()).expect("remove loose");
    let loose_name = "b".repeat(62);
    write(fixture.loose_dir().join(&loose_name), LOOSE_BYTES);
    fs::remove_dir_all(fixture.pack_dir()).expect("remove pack dir");
    fs::create_dir_all(fixture.pack_dir()).expect("pack dir");
    write(
        fixture
            .pack_dir()
            .join(format!("pack-{}.pack", "c".repeat(64))),
        PACK_BYTES,
    );
    let mut request = fixture.request();
    request.expected_head = head.clone();
    let snapshot = stage_raw_git_snapshot(request, limits()).expect("sha256 raw snapshot");
    assert_eq!(snapshot.captured_head, head);
    assert_eq!(snapshot.object_files, 2);
    assert_eq!(
        snapshot.object_bytes,
        (LOOSE_BYTES.len() + PACK_BYTES.len()) as u64
    );
}

#[test]
fn branch_ref_mismatch_is_rejected() {
    let fixture = Fixture::new();
    write(
        fixture.git().join("refs/heads/main"),
        format!("{OTHER_HEX}\n"),
    );
    assert_eq!(stage(&fixture), Err(GitImportError::HeadMismatch));
    assert!(quarantine_is_empty(&fixture));
}

#[test]
fn changed_head_after_copy_is_rejected_and_cleaned() {
    let fixture = Fixture::new();
    let ref_path = fixture.git().join("refs/heads/main");
    let _guard = seam::install(move |step| {
        if step == Step::RecheckHead {
            fs::write(&ref_path, format!("{OTHER_HEX}\n")).expect("change head");
        }
    });
    assert_eq!(stage(&fixture), Err(GitImportError::HeadChanged));
    assert!(quarantine_is_empty(&fixture));
}

#[test]
fn namespace_swap_between_enumeration_and_open_never_reads_outside() {
    let fixture = Fixture::new();
    let outside = fixture.temp.path().join("outside");
    fs::create_dir_all(outside.join("aa")).expect("outside dir");
    write(outside.join("aa").join(&fixture.loose_name), b"outside");
    let loose_dir = fixture.loose_dir();
    let moved = fixture.objects().join("aa-old");
    let target = outside.clone();
    let _guard = seam::install(move |step| {
        if step == Step::OpenObjectDirectory {
            fs::rename(&loose_dir, &moved).expect("move object dir");
            symlink(&target, &loose_dir).expect("swap object dir");
        }
    });
    assert_eq!(stage(&fixture), Err(GitImportError::UnsupportedLayout));
    assert_eq!(
        read(&outside.join("aa").join(&fixture.loose_name)),
        b"outside"
    );
    assert!(quarantine_is_empty(&fixture));
}

#[test]
fn replaced_checkout_parent_uses_pinned_descriptor() {
    let fixture = Fixture::new();
    let outside = fixture.temp.path().join("outside-checkout");
    fs::create_dir_all(outside.join(".git/objects/aa")).expect("outside git");
    write(
        outside.join(".git/objects/aa").join(&fixture.loose_name),
        b"outside",
    );
    let checkout = fixture.checkout.clone();
    let moved = fixture.workspace.join("checkout-moved");
    let target = outside.clone();
    let _guard = seam::install(move |step| {
        if step == Step::OpenGitDir {
            fs::rename(&checkout, &moved).expect("move checkout");
            symlink(&target, &checkout).expect("replace checkout");
        }
    });
    let snapshot = stage(&fixture).expect("pinned checkout descriptor");
    let root = snapshot_root(&fixture, &snapshot);
    assert_eq!(
        read(&root.join("objects/aa").join(&fixture.loose_name)),
        LOOSE_BYTES
    );
    assert_eq!(
        read(&outside.join(".git/objects/aa").join(&fixture.loose_name)),
        b"outside"
    );
}

#[test]
fn symlinked_object_and_git_dir_are_rejected() {
    let fixture = Fixture::new();
    let outside = canary(&fixture, "outside-object", b"outside");
    fs::remove_file(fixture.loose_file()).expect("remove loose");
    symlink(&outside, fixture.loose_file()).expect("symlink object");
    assert_eq!(stage(&fixture), Err(GitImportError::UnsafeEntry));
    assert!(quarantine_is_empty(&fixture));

    let fixture = Fixture::new();
    let git = fixture.git();
    let moved = fixture.checkout.join("git-moved");
    fs::rename(&git, &moved).expect("move git");
    symlink(&moved, &git).expect("symlink git");
    assert_eq!(stage(&fixture), Err(GitImportError::UnsupportedLayout));

    let fixture = Fixture::new();
    let outside = fixture.temp.path().join("outside-checkout");
    fs::create_dir_all(outside.join(".git")).expect("outside git");
    let link = fixture.workspace.join("linked-checkout");
    symlink(&outside, &link).expect("link checkout");
    let request = RawGitSnapshotRequest {
        workspace_root: open_dir(&fixture.workspace),
        quarantine_root: open_dir(&fixture.quarantine),
        checkout_relative: PathBuf::from("linked-checkout"),
        pinned_branch: "main".to_owned(),
        expected_head: HEAD_HEX.to_owned(),
    };
    assert_eq!(
        stage_raw_git_snapshot(request, limits()),
        Err(GitImportError::UnsupportedLayout)
    );
}

#[test]
fn git_pointer_commondir_worktrees_and_alternates_are_rejected() {
    let fixture = Fixture::new();
    write(fixture.git().join("commondir"), b"../elsewhere\n");
    assert_eq!(stage(&fixture), Err(GitImportError::UnsupportedLayout));

    let fixture = Fixture::new();
    fs::create_dir_all(fixture.git().join("worktrees/one")).expect("worktrees");
    assert_eq!(stage(&fixture), Err(GitImportError::UnsupportedLayout));

    let fixture = Fixture::new();
    write(
        fixture.git().join("objects/info/alternates"),
        b"/outside/objects\n",
    );
    assert_eq!(stage(&fixture), Err(GitImportError::UnsupportedLayout));

    let fixture = Fixture::new();
    let git = fixture.git();
    let moved = fixture.checkout.join("git-real");
    fs::rename(&git, &moved).expect("move git");
    write(git, format!("gitdir: {}\n", moved.display()));
    assert_eq!(stage(&fixture), Err(GitImportError::UnsupportedLayout));
}

#[test]
fn fifo_and_socket_candidates_are_rejected_without_blocking() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.loose_file()).expect("remove loose");
    let directory = open_dir(&fixture.loose_dir());
    mkfifoat(
        &directory,
        fixture.loose_name.as_str(),
        Mode::from_bits_truncate(0o600),
    )
    .expect("fifo");
    assert_eq!(stage(&fixture), Err(GitImportError::UnsafeEntry));

    let fixture = Fixture::new();
    fs::remove_file(fixture.loose_file()).expect("remove loose");
    let _listener = UnixListener::bind(fixture.loose_file()).expect("socket");
    assert_eq!(stage(&fixture), Err(GitImportError::UnsafeEntry));
}

#[test]
fn non_regular_candidate_types_are_rejected() {
    use super::import::is_regular_candidate;
    assert!(is_regular_candidate(FileType::RegularFile, 1));
    assert!(!is_regular_candidate(FileType::RegularFile, 2));
    assert!(!is_regular_candidate(FileType::Directory, 1));
    assert!(!is_regular_candidate(FileType::Fifo, 1));
    assert!(!is_regular_candidate(FileType::Socket, 1));
    assert!(!is_regular_candidate(FileType::CharacterDevice, 1));
    assert!(!is_regular_candidate(FileType::BlockDevice, 1));
    assert!(!is_regular_candidate(FileType::Symlink, 1));
}

#[test]
fn hardlinked_object_is_rejected() {
    let fixture = Fixture::new();
    let outside = canary(&fixture, "outside-hardlink", b"outside");
    fs::remove_file(fixture.loose_file()).expect("remove loose");
    fs::hard_link(&outside, fixture.loose_file()).expect("hardlink");
    assert_eq!(stage(&fixture), Err(GitImportError::UnsafeEntry));
}

#[test]
fn growing_candidate_is_bounded_and_partial_quarantine_is_cleaned() {
    let fixture = Fixture::new();
    let outside = canary(&fixture, "outside-canary", b"do not touch");
    let loose = fixture.loose_file();
    let fired = Arc::new(AtomicBool::new(false));
    let hook_fired = Arc::clone(&fired);
    let _guard = seam::install(move |step| {
        if step == Step::CopyObjectFile && !hook_fired.swap(true, Ordering::SeqCst) {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(&loose)
                .expect("open loose");
            use std::io::Write;
            file.write_all(b"x").expect("grow object");
        }
    });
    let mut small = limits();
    small.max_file_bytes = LOOSE_BYTES.len() as u64;
    assert_eq!(
        stage_with(&fixture, small),
        Err(GitImportError::LimitExceeded)
    );
    assert!(quarantine_is_empty(&fixture));
    assert_eq!(read(&outside), b"do not touch");
}

#[test]
fn truncated_candidate_copies_only_observed_bytes() {
    let fixture = Fixture::new();
    let loose = fixture.loose_file();
    let fired = Arc::new(AtomicBool::new(false));
    let hook_fired = Arc::clone(&fired);
    let _guard = seam::install(move |step| {
        if step == Step::CopyObjectFile && !hook_fired.swap(true, Ordering::SeqCst) {
            fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&loose)
                .expect("truncate loose")
                .set_len(4)
                .expect("truncate");
        }
    });
    let snapshot = stage(&fixture).expect("truncated copy");
    assert_eq!(snapshot.object_files, 3);
    assert_eq!(
        snapshot.object_bytes,
        (4 + PACK_BYTES.len() + b"raw-index-payload".len()) as u64
    );
    let root = snapshot_root(&fixture, &snapshot);
    assert_eq!(
        read(&root.join("objects/aa").join(&fixture.loose_name)).len(),
        4
    );
}

#[test]
fn every_limit_rejects_exhaustion_and_invalid_values() {
    let fixture = Fixture::new();

    let mut small = limits();
    small.max_entries = 2;
    assert_eq!(
        stage_with(&fixture, small),
        Err(GitImportError::LimitExceeded)
    );

    let mut small = limits();
    small.max_files = 4;
    assert_eq!(
        stage_with(&fixture, small),
        Err(GitImportError::LimitExceeded)
    );

    let mut small = limits();
    small.max_file_bytes = PACK_BYTES.len() as u64 - 1;
    assert_eq!(
        stage_with(&fixture, small),
        Err(GitImportError::LimitExceeded)
    );

    let mut small = limits();
    small.max_total_bytes = (LOOSE_BYTES.len() + PACK_BYTES.len()) as u64;
    assert_eq!(
        stage_with(&fixture, small),
        Err(GitImportError::LimitExceeded)
    );

    let mut small = limits();
    small.max_metadata_bytes = 1;
    assert_eq!(
        stage_with(&fixture, small),
        Err(GitImportError::LimitExceeded)
    );
    assert!(quarantine_is_empty(&fixture));

    for invalid in [
        GitImportLimits {
            max_entries: 0,
            ..limits()
        },
        GitImportLimits {
            max_files: 0,
            ..limits()
        },
        GitImportLimits {
            max_file_bytes: 0,
            ..limits()
        },
        GitImportLimits {
            max_total_bytes: 0,
            ..limits()
        },
        GitImportLimits {
            max_metadata_bytes: 0,
            ..limits()
        },
        GitImportLimits {
            max_entries: u64::MAX,
            ..limits()
        },
        GitImportLimits {
            max_total_bytes: u64::MAX,
            ..limits()
        },
    ] {
        assert_eq!(
            stage_with(&fixture, invalid),
            Err(GitImportError::InvalidLimits)
        );
    }
}

#[test]
fn invalid_request_metadata_is_rejected_before_any_staging() {
    let fixture = Fixture::new();

    let mut request = fixture.request();
    request.expected_head = "abc".to_owned();
    assert_eq!(
        stage_raw_git_snapshot(request, limits()),
        Err(GitImportError::InvalidExpectedHead)
    );

    let mut request = fixture.request();
    request.expected_head = "z".repeat(40);
    assert_eq!(
        stage_raw_git_snapshot(request, limits()),
        Err(GitImportError::InvalidExpectedHead)
    );

    for branch in ["", "main\n", "../main", "main/../x", "/main", "main//x"] {
        let mut request = fixture.request();
        request.pinned_branch = branch.to_owned();
        assert_eq!(
            stage_raw_git_snapshot(request, limits()),
            Err(GitImportError::InvalidBranch),
            "branch {branch:?}"
        );
    }

    for checkout in ["", ".", "..", "../outside", "/absolute"] {
        let mut request = fixture.request();
        request.checkout_relative = PathBuf::from(checkout);
        assert_eq!(
            stage_raw_git_snapshot(request, limits()),
            Err(GitImportError::InvalidCheckoutLocation),
            "checkout {checkout:?}"
        );
    }
}

#[test]
fn existing_destination_file_or_symlink_is_never_overwritten() {
    let fixture = Fixture::new();
    let name = "fixed-snapshot";
    let existing = fixture.quarantine.join(name);
    write(existing.clone(), b"existing");
    let _guard = seam::snapshot_name(name);
    assert_eq!(stage(&fixture), Err(GitImportError::UnsafeDestination));
    assert_eq!(read(&existing), b"existing");

    let fixture = Fixture::new();
    let name = "fixed-snapshot";
    let existing = fixture.quarantine.join(name);
    symlink(fixture.temp.path().join("outside"), &existing).expect("destination symlink");
    let _guard = seam::snapshot_name(name);
    assert_eq!(stage(&fixture), Err(GitImportError::UnsafeDestination));
    assert!(
        fs::symlink_metadata(&existing)
            .expect("destination")
            .file_type()
            .is_symlink()
    );
}

#[test]
fn unexpected_object_layouts_are_rejected() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.objects().join("zz")).expect("unexpected dir");
    assert_eq!(stage(&fixture), Err(GitImportError::UnsupportedLayout));

    let fixture = Fixture::new();
    write(fixture.pack_dir().join("multi-pack-index"), b"unexpected");
    assert_eq!(stage(&fixture), Err(GitImportError::UnsupportedLayout));

    let fixture = Fixture::new();
    write(fixture.loose_dir().join("not-a-hash"), b"unexpected");
    assert_eq!(stage(&fixture), Err(GitImportError::UnsupportedLayout));

    let fixture = Fixture::new();
    add_loose_object(&fixture, &"d".repeat(38), b"second object");
    let snapshot = stage(&fixture).expect("multiple loose objects");
    assert_eq!(snapshot.object_files, 4);
}

#[test]
fn multiple_local_branches_are_preserved_and_ignored() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.git().join("refs/heads/feature")).expect("feature dir");
    write(
        fixture.git().join("refs/heads/feature/x"),
        format!("{OTHER_HEX}\n"),
    );
    let snapshot = stage(&fixture).expect("extra branches");
    let root = snapshot_root(&fixture, &snapshot);
    assert!(!root.join("refs/heads/other").exists());
    assert!(!root.join("refs/heads/feature").exists());
    assert_eq!(
        read(&fixture.git().join("refs/heads/feature/x")),
        format!("{OTHER_HEX}\n").as_bytes()
    );
}

#[test]
fn destination_files_and_directories_are_owner_only() {
    let fixture = Fixture::new();
    let snapshot = stage(&fixture).expect("raw snapshot");
    let root = snapshot_root(&fixture, &snapshot);
    let mode = |path: &PathBuf| {
        fs::symlink_metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&root.join("objects")), 0o700);
    assert_eq!(mode(&root.join("HEAD")), 0o600);
    assert_eq!(
        mode(&root.join("objects/aa").join(&fixture.loose_name)),
        0o600
    );
}

#[test]
fn objects_info_entries_are_not_read_or_copied() {
    let fixture = Fixture::new();
    write(fixture.git().join("objects/info/packs"), b"ignored");
    write(fixture.git().join("objects/info/commit-graph"), b"ignored");
    let snapshot = stage(&fixture).expect("objects info ignored");
    let root = snapshot_root(&fixture, &snapshot);
    assert!(!root.join("objects/info").exists());
}
