//! Unit tests for the shared seed rules and manifest digest (issue 753 P2b).
//!
//! These are the pure-rule tests moved with the rules themselves: path
//! normalization, duplicate and ancestor collisions, the per-field and
//! aggregate ceilings, the NUL refusal, and the exact published v1 hash
//! vectors. The API side and the manager both depend on these bytes.

use super::{
    MAX_SEED_CONTENT_BYTES, MAX_SEED_ENTRIES, MAX_SEED_PATH_BYTES, SeedBundleError,
    SourceSeedSnapshotFile, has_ancestor_collision, is_normalized_relative_posix, manifest_sha256,
    validate_seed_bundle,
};

fn file(path: &str, content: &str) -> SourceSeedSnapshotFile {
    SourceSeedSnapshotFile::new(path, content)
}

#[test]
fn a_published_hash_vector_is_stable() {
    // The exact published v1 vectors. Pinning the bytes means a later
    // refactor cannot silently change the digest every stored snapshot and
    // every future content CAS depends on.
    let two = [file("a.txt", "a"), file("b.txt", "b")];
    assert_eq!(
        manifest_sha256(&two),
        "20410d9c695bacbbcd273369f4da3274431218c25f1a0975532b9be79e9f393b"
    );
    let named = [
        file("README.md", "# hi\n"),
        file("src/main.rs", "fn main() {}\n"),
    ];
    assert_eq!(
        validate_seed_bundle(&named).expect("plan").manifest_sha256,
        "d884c9b8dda5f737e27c9a67d06ee99fcdd85e03e3a7edbb184f8a84d0794a59"
    );
    // An empty workspace has its own fixed digest: the version prefix only.
    assert_eq!(
        validate_seed_bundle(&[]).expect("plan").manifest_sha256,
        "39ee0d00eb7cf8290dcdc707fcc0c30dec7ccd473023a5a4a38e734151ae35c7"
    );
}

#[test]
fn the_digest_is_independent_of_request_order() {
    let ordered = [file("a.txt", "a"), file("b.txt", "b")];
    let reversed = [file("b.txt", "b"), file("a.txt", "a")];
    assert_eq!(
        validate_seed_bundle(&ordered)
            .expect("plan")
            .manifest_sha256,
        validate_seed_bundle(&reversed)
            .expect("plan")
            .manifest_sha256,
    );
}

#[test]
fn the_digest_changes_when_content_or_path_changes() {
    let base = validate_seed_bundle(&[file("a.txt", "a")]).expect("plan");
    let other_content = validate_seed_bundle(&[file("a.txt", "b")]).expect("plan");
    let other_path = validate_seed_bundle(&[file("b.txt", "a")]).expect("plan");
    assert_ne!(base.manifest_sha256, other_content.manifest_sha256);
    assert_ne!(base.manifest_sha256, other_path.manifest_sha256);
}

#[test]
fn an_empty_list_is_a_valid_empty_workspace() {
    let plan = validate_seed_bundle(&[]).expect("empty plan");
    assert_eq!(plan.file_count, 0);
    assert_eq!(plan.total_bytes, 0);
    assert_eq!(plan.files.len(), 0);
    assert_eq!(plan.manifest_sha256.len(), 64);
}

#[test]
fn traversal_absolute_and_metadata_paths_are_refused() {
    for path in [
        "/etc/passwd",
        "a/../../b",
        "../up",
        ".",
        "..",
        "a//b",
        "a/./b",
        "a/",
        "C:/windows",
        "a\\b",
        ".git/config",
        "sub/.GIT/HEAD",
        ".git",
        "a/.git",
        "a\u{0}b",
        "a\u{1}b",
        "a\u{7f}b",
    ] {
        assert!(
            !is_normalized_relative_posix(path),
            "{path:?} must be refused"
        );
        assert_eq!(
            validate_seed_bundle(&[file(path, "x")]).unwrap_err(),
            SeedBundleError::InvalidPath,
            "{path:?} must plan as an invalid path"
        );
    }
}

#[test]
fn legitimate_paths_are_accepted() {
    for path in [
        "README.md",
        "src/main.rs",
        "a/b/c.txt",
        "dir/.github/workflow.yml",
        ".env",
    ] {
        assert!(
            is_normalized_relative_posix(path),
            "{path:?} must be accepted"
        );
    }
}

#[test]
fn duplicates_and_ancestor_collisions_are_refused() {
    assert_eq!(
        validate_seed_bundle(&[file("a.txt", "1"), file("a.txt", "2")]).unwrap_err(),
        SeedBundleError::DuplicatePath
    );
    assert_eq!(
        validate_seed_bundle(&[file("a", "1"), file("a/b", "2")]).unwrap_err(),
        SeedBundleError::AncestorCollision
    );
    assert_eq!(
        validate_seed_bundle(&[file("a/b/c", "1"), file("a/b", "2")]).unwrap_err(),
        SeedBundleError::AncestorCollision
    );
    // Shared prefix but not an ancestor: `ab` is not `a`.
    assert!(validate_seed_bundle(&[file("a", "1"), file("ab", "2")]).is_ok());
    assert!(!has_ancestor_collision(&[file("ab", "1"), file("ac", "2")]));
}

#[test]
fn per_field_and_aggregate_limits_are_enforced() {
    let long_path = "a".repeat(MAX_SEED_PATH_BYTES + 1);
    assert_eq!(
        validate_seed_bundle(&[file(&long_path, "x")]).unwrap_err(),
        SeedBundleError::PathTooLong
    );
    let big = "x".repeat(MAX_SEED_CONTENT_BYTES + 1);
    assert_eq!(
        validate_seed_bundle(&[file("a.txt", &big)]).unwrap_err(),
        SeedBundleError::ContentTooLarge
    );
    let many: Vec<SourceSeedSnapshotFile> = (0..=MAX_SEED_ENTRIES)
        .map(|index| file(&format!("f{index}.txt"), "x"))
        .collect();
    assert_eq!(
        validate_seed_bundle(&many).unwrap_err(),
        SeedBundleError::TooManyEntries
    );
}

#[test]
fn a_nul_content_is_refused_before_any_sql() {
    assert_eq!(
        validate_seed_bundle(&[file("a.txt", "a\0b")]).unwrap_err(),
        SeedBundleError::ContentNotSupported
    );
}
