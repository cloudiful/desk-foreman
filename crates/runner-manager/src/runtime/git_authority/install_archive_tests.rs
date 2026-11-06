use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;

use runner_protocol::GitObjectFormat;
use rustix::fs::{Mode, OFlags};

use super::install_archive::{ArchiveLimits, InstallArchiveError, extract, inspect_git};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

fn limits() -> ArchiveLimits {
    ArchiveLimits {
        worktree_bytes: 1024 * 1024,
        git_bytes: 1024 * 1024,
        metadata_bytes: 64 * 1024,
        max_entries: 128,
    }
}

fn root() -> (tempfile::TempDir, OwnedFd) {
    let temp = tempfile::tempdir().expect("temp");
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).expect("mode");
    let fd = rustix::fs::open(
        temp.path(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .expect("root fd");
    (temp, fd)
}

fn regular(builder: &mut tar::Builder<Vec<u8>>, path: &OsStr, body: &[u8], mode: u32) {
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(mode);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    builder
        .append_data(&mut header, path, body)
        .expect("append");
}

fn benign() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut dir = tar::Header::new_gnu();
    dir.set_entry_type(tar::EntryType::Directory);
    dir.set_mode(0o755);
    dir.set_size(0);
    dir.set_cksum();
    builder
        .append_data(&mut dir, "./work", &[][..])
        .expect("dir");
    regular(&mut builder, OsStr::new("./work/file.txt"), b"hello", 0o644);
    regular(&mut builder, OsStr::new("./run.sh"), b"#!/bin/sh\n", 0o755);
    let newline = OsStr::from_bytes(b"./work/new\nline");
    regular(&mut builder, newline, b"x", 0o644);
    let non_utf8 = OsStr::from_bytes(b"./work/\xff");
    regular(&mut builder, non_utf8, b"y", 0o644);
    regular(
        &mut builder,
        OsStr::new("./.git/HEAD"),
        b"ref: refs/heads/main\n",
        0o644,
    );
    regular(
        &mut builder,
        OsStr::new("./.git/config"),
        b"[core]\n\trepositoryformatversion = 0\n",
        0o644,
    );
    regular(
        &mut builder,
        OsStr::new("./.git/refs/heads/main"),
        format!("{HEAD}\n").as_bytes(),
        0o644,
    );
    builder.into_inner().expect("tar")
}

#[test]
fn benign_archive_extracts_and_metadata_matches() {
    let (_temp, fd) = root();
    let summary = extract(&benign(), &fd, &limits()).expect("extract");
    assert_eq!(summary.worktree_bytes, 5 + 10 + 1 + 1);
    assert!(summary.git_bytes > 0);
    inspect_git(&fd, "main", HEAD, GitObjectFormat::Sha1, 64 * 1024).expect("metadata");
}

#[test]
fn truncated_checksum_and_trailing_bytes_are_rejected() {
    let bytes = benign();
    assert!(matches!(
        extract(&bytes[..bytes.len() - 600], &root().1, &limits()),
        Err(InstallArchiveError::InvalidHeader | InstallArchiveError::Truncated)
    ));
    let mut corrupted = bytes.clone();
    corrupted[0] ^= 0xff;
    assert!(matches!(
        extract(&corrupted, &root().1, &limits()),
        Err(InstallArchiveError::InvalidHeader)
    ));
    let mut trailing = bytes;
    trailing.extend_from_slice(b"hidden");
    assert!(matches!(
        extract(&trailing, &root().1, &limits()),
        Err(InstallArchiveError::InvalidEntry)
    ));
}

#[test]
fn unsupported_types_duplicate_and_prefix_conflicts_are_rejected() {
    let (_temp, fd) = root();
    let mut builder = tar::Builder::new(Vec::new());
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Link);
    link.set_size(0);
    link.set_link_name("target").expect("link name");
    link.set_cksum();
    builder
        .append_data(&mut link, "./hard", &[][..])
        .expect("hardlink");
    assert!(matches!(
        extract(&builder.into_inner().expect("tar"), &fd, &limits()),
        Err(InstallArchiveError::UnsupportedEntry)
    ));

    let mut builder = tar::Builder::new(Vec::new());
    regular(&mut builder, OsStr::new("./dup"), b"a", 0o644);
    regular(&mut builder, OsStr::new("./dup"), b"b", 0o644);
    assert!(matches!(
        extract(&builder.into_inner().expect("tar"), &fd, &limits()),
        Err(InstallArchiveError::DuplicatePath)
    ));

    let mut builder = tar::Builder::new(Vec::new());
    regular(&mut builder, OsStr::new("./a"), b"a", 0o644);
    regular(&mut builder, OsStr::new("./a/b"), b"b", 0o644);
    assert!(matches!(
        extract(&builder.into_inner().expect("tar"), &fd, &limits()),
        Err(InstallArchiveError::PrefixConflict)
    ));

    let mut builder = tar::Builder::new(Vec::new());
    let mut symlink = tar::Header::new_gnu();
    symlink.set_entry_type(tar::EntryType::Symlink);
    symlink.set_size(0);
    symlink.set_link_name("target").expect("target");
    symlink.set_cksum();
    builder
        .append_data(&mut symlink, "./.git/link", &[][..])
        .expect("symlink");
    assert!(matches!(
        extract(&builder.into_inner().expect("tar"), &fd, &limits()),
        Err(InstallArchiveError::InvalidEntry)
    ));
}

#[test]
fn worktree_and_git_caps_are_independent() {
    let bytes = benign();
    let mut worktree = limits();
    worktree.worktree_bytes = 5;
    assert!(matches!(
        extract(&bytes, &root().1, &worktree),
        Err(InstallArchiveError::WorktreeLimit)
    ));
    worktree.worktree_bytes = 1024 * 1024;
    worktree.git_bytes = 1;
    assert!(matches!(
        extract(&bytes, &root().1, &worktree),
        Err(InstallArchiveError::GitLimit)
    ));
    let mut entries = limits();
    entries.max_entries = 2;
    assert!(matches!(
        extract(&bytes, &root().1, &entries),
        Err(InstallArchiveError::EntryLimit)
    ));
}

#[test]
fn git_metadata_rejects_remote_url_and_mismatched_head() {
    let (_temp, fd) = root();
    let mut builder = tar::Builder::new(Vec::new());
    regular(
        &mut builder,
        OsStr::new("./.git/HEAD"),
        b"ref: refs/heads/main\n",
        0o644,
    );
    regular(
        &mut builder,
        OsStr::new("./.git/config"),
        b"[remote \"origin\"]\n\turl = /git-bundle/bundle.git\n",
        0o644,
    );
    regular(
        &mut builder,
        OsStr::new("./.git/refs/heads/main"),
        format!("{HEAD}\n").as_bytes(),
        0o644,
    );
    let bytes = builder.into_inner().expect("tar");
    extract(&bytes, &fd, &limits()).expect("extract");
    assert!(matches!(
        inspect_git(&fd, "main", HEAD, GitObjectFormat::Sha1, 64 * 1024),
        Err(InstallArchiveError::MetadataMismatch)
    ));

    let (_temp, fd) = root();
    assert!(matches!(
        inspect_git(&fd, "main", HEAD, GitObjectFormat::Sha1, 64 * 1024),
        Err(InstallArchiveError::MetadataMismatch)
    ));
}

#[test]
fn pax_size_global_headers_and_sparse_entries_are_rejected() {
    let (_temp, fd) = root();
    let mut builder = tar::Builder::new(Vec::new());
    builder
        .append_pax_extensions([("size", b"5".as_slice())])
        .expect("pax");
    regular(&mut builder, OsStr::new("./file"), b"hello", 0o644);
    assert!(matches!(
        extract(&builder.into_inner().expect("tar"), &fd, &limits()),
        Err(InstallArchiveError::InvalidEntry)
    ));

    let mut builder = tar::Builder::new(Vec::new());
    let mut global = tar::Header::new_gnu();
    global.set_entry_type(tar::EntryType::XGlobalHeader);
    global.set_size(0);
    global.set_cksum();
    builder
        .append_data(&mut global, "./global", &[][..])
        .expect("global");
    assert!(matches!(
        extract(&builder.into_inner().expect("tar"), &fd, &limits()),
        Err(InstallArchiveError::UnsupportedEntry)
    ));

    let mut builder = tar::Builder::new(Vec::new());
    let mut sparse = tar::Header::new_gnu();
    sparse.set_entry_type(tar::EntryType::GNUSparse);
    sparse.set_size(0);
    sparse.set_cksum();
    builder
        .append_data(&mut sparse, "./sparse", &[][..])
        .expect("sparse");
    assert!(matches!(
        extract(&builder.into_inner().expect("tar"), &fd, &limits()),
        Err(InstallArchiveError::UnsupportedEntry)
    ));
}

#[test]
fn sha256_git_metadata_is_accepted() {
    let (_temp, fd) = root();
    let mut builder = tar::Builder::new(Vec::new());
    regular(
        &mut builder,
        OsStr::new("./.git/HEAD"),
        b"ref: refs/heads/main\n",
        0o644,
    );
    regular(
        &mut builder,
        OsStr::new("./.git/config"),
        b"[core]\n\trepositoryformatversion = 0\n[extensions]\n\tobjectformat = sha256\n",
        0o644,
    );
    regular(
        &mut builder,
        OsStr::new("./.git/refs/heads/main"),
        format!("{HEAD}\n").as_bytes(),
        0o644,
    );
    let bytes = builder.into_inner().expect("tar");
    extract(&bytes, &fd, &limits()).expect("extract");
    inspect_git(&fd, "main", HEAD, GitObjectFormat::Sha256, 64 * 1024).expect("sha256 metadata");
    assert!(matches!(
        inspect_git(&fd, "main", HEAD, GitObjectFormat::Sha1, 64 * 1024),
        Err(InstallArchiveError::MetadataMismatch)
    ));
}

#[test]
fn acl_xattr_pax_keys_are_rejected_and_long_names_are_accepted() {
    let (_temp, fd) = root();
    let mut builder = tar::Builder::new(Vec::new());
    builder
        .append_pax_extensions([("SCHILY.acl.access", b"user::rwx".as_slice())])
        .expect("pax");
    regular(&mut builder, OsStr::new("./file"), b"x", 0o644);
    assert!(matches!(
        extract(&builder.into_inner().expect("tar"), &fd, &limits()),
        Err(InstallArchiveError::InvalidEntry)
    ));

    let (_temp, fd) = root();
    let long = format!("./{}", "a".repeat(200));
    let mut builder = tar::Builder::new(Vec::new());
    regular(&mut builder, OsStr::new(&long), b"x", 0o644);
    let bytes = builder.into_inner().expect("tar");
    extract(&bytes, &fd, &limits()).expect("long name");
}
