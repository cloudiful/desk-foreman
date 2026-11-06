use std::os::unix::fs::PermissionsExt;

use runner_protocol::{GitReceiptPhase, RunnerOwner};
use uuid::Uuid;

use super::receipt::{
    GIT_RECEIPT_VERSION, GitReceipt, GitReceiptError, read_receipt, receipt_path,
    workspace_root_sha256, write_receipt,
};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const BASE: &str = "fedcba9876543210fedcba9876543210fedcba98";

fn receipt() -> GitReceipt {
    GitReceipt {
        version: GIT_RECEIPT_VERSION,
        job_id: Uuid::new_v4(),
        operation_id: Uuid::new_v4(),
        owner: RunnerOwner::ApplicationPrincipal {
            application_id: 7,
            workspace_binding_id: 11,
            principal_digest: "a".repeat(64),
        },
        canonical_branch: "main".to_owned(),
        base: Some(BASE.to_owned()),
        head: HEAD.to_owned(),
        workspace_root_sha256: workspace_root_sha256("/srv/workspaces/binding-11"),
        phase: GitReceiptPhase::Completed,
        published: true,
        cleanup_confirmed: true,
    }
}

fn private_root() -> tempfile::TempDir {
    let base = std::env::var_os("HOME").expect("home directory");
    let root = tempfile::Builder::new().tempdir_in(&base).expect("root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("root mode");
    root
}

fn request_for(receipt: &GitReceipt) -> runner_protocol::GitReceiptReadRequest {
    receipt.read_request(9, "/srv/workspaces/binding-11", 1_800_000_000_000)
}

#[test]
fn receipt_roundtrips_strictly() {
    let receipt = receipt();
    let json = serde_json::to_string(&receipt).expect("serialize");
    let decoded: GitReceipt = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, receipt);
    decoded.validate().expect("valid");
    assert!(!json.contains("password"));
    assert!(!json.contains("/srv/"));
}

#[test]
fn receipt_requires_present_base_field() {
    let receipt = receipt();
    let json = serde_json::to_string(&receipt).expect("serialize");
    let missing = json.replace("\"base\":\"fedcba9876543210fedcba9876543210fedcba98\",", "");
    assert!(serde_json::from_str::<GitReceipt>(&missing).is_err());
    let null = json.replace(
        "\"base\":\"fedcba9876543210fedcba9876543210fedcba98\"",
        "\"base\":null",
    );
    let decoded: GitReceipt = serde_json::from_str(&null).expect("first clone receipt");
    assert!(decoded.base.is_none());
}

#[test]
fn receipt_rejects_unknown_fields() {
    let json = serde_json::to_string(&receipt()).expect("serialize");
    let unknown = json.replace('{', "{\"path\":\"/tmp/receipt\",");
    assert!(serde_json::from_str::<GitReceipt>(&unknown).is_err());
}

#[test]
fn receipt_requires_version_one() {
    let mut bad = receipt();
    bad.version = 2;
    assert_eq!(bad.validate(), Err(GitReceiptError::InvalidVersion));
}

#[test]
fn receipt_requires_non_nil_identity() {
    let mut bad = receipt();
    bad.job_id = Uuid::nil();
    assert_eq!(bad.validate(), Err(GitReceiptError::InvalidIdentity));
    let mut bad = receipt();
    bad.operation_id = Uuid::nil();
    assert_eq!(bad.validate(), Err(GitReceiptError::InvalidIdentity));
}

#[test]
fn receipt_requires_application_binding_owner() {
    let mut bad = receipt();
    bad.owner = RunnerOwner::WorkspaceBinding {
        workspace_binding_id: 11,
    };
    assert_eq!(bad.validate(), Err(GitReceiptError::InvalidOwner));
}

#[test]
fn receipt_requires_canonical_head_and_base() {
    let mut bad = receipt();
    bad.head = "A".repeat(40);
    assert_eq!(bad.validate(), Err(GitReceiptError::InvalidHead));
    let mut bad = receipt();
    bad.base = Some("b".repeat(41));
    assert_eq!(bad.validate(), Err(GitReceiptError::InvalidBase));
}

#[test]
fn receipt_requires_canonical_branch_and_root_digest() {
    let mut bad = receipt();
    bad.canonical_branch = "../main".to_owned();
    assert_eq!(bad.validate(), Err(GitReceiptError::InvalidBranch));
    let mut bad = receipt();
    bad.workspace_root_sha256 = "A".repeat(64);
    assert_eq!(bad.validate(), Err(GitReceiptError::InvalidRootDigest));
}

#[test]
fn completed_receipt_requires_publication_and_cleanup() {
    let mut bad = receipt();
    bad.published = false;
    assert_eq!(bad.validate(), Err(GitReceiptError::Unpublished));
    let mut bad = receipt();
    bad.cleanup_confirmed = false;
    assert_eq!(bad.validate(), Err(GitReceiptError::Unpublished));
    let mut verified = receipt();
    verified.phase = GitReceiptPhase::Verified;
    verified.published = false;
    verified.cleanup_confirmed = false;
    verified.validate().expect("verified");
    assert!(!verified.is_confirmed());
}

#[test]
fn io_roundtrip_is_private_and_phase_monotonic() {
    let root = private_root();
    eprintln!(
        "ROOT {:?} VALIDATE {:?}",
        root.path(),
        super::runtime_config::validate_private_root(root.path(), super::receipt::current_euid())
    );
    let mut receipt = receipt();
    receipt.phase = GitReceiptPhase::Verified;
    receipt.published = false;
    receipt.cleanup_confirmed = false;
    write_receipt(root.path(), &receipt).expect("write verified");
    let request = request_for(&receipt);
    assert_eq!(read_receipt(root.path(), &request).expect("read"), receipt);

    let mut completed = receipt.clone();
    completed.phase = GitReceiptPhase::Completed;
    completed.published = true;
    completed.cleanup_confirmed = true;
    write_receipt(root.path(), &completed).expect("complete");
    assert!(
        read_receipt(root.path(), &request)
            .expect("read")
            .is_confirmed()
    );

    let path = receipt_path(root.path(), receipt.job_id);
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(&path).expect("metadata");
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.nlink(), 1);

    let mut downgrade = completed.clone();
    downgrade.phase = GitReceiptPhase::Verified;
    downgrade.published = false;
    downgrade.cleanup_confirmed = false;
    assert_eq!(
        write_receipt(root.path(), &downgrade),
        Err(GitReceiptError::Conflict)
    );
}

#[test]
fn io_rejects_identity_and_root_mismatch() {
    let root = private_root();
    let receipt = receipt();
    write_receipt(root.path(), &receipt).expect("write");
    let mut request = request_for(&receipt);
    request.operation_id = Uuid::new_v4();
    assert_eq!(
        read_receipt(root.path(), &request),
        Err(GitReceiptError::Conflict)
    );
    let mut request = request_for(&receipt);
    request.workspace_root = "/srv/workspaces/binding-12".to_owned();
    assert_eq!(
        read_receipt(root.path(), &request),
        Err(GitReceiptError::Conflict)
    );
}

#[test]
fn io_rejects_wrong_mode_and_symlink() {
    let root = private_root();
    let first = receipt();
    write_receipt(root.path(), &first).expect("write");
    let path = receipt_path(root.path(), first.job_id);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("mode");
    assert_eq!(
        read_receipt(root.path(), &request_for(&first)),
        Err(GitReceiptError::Corrupt)
    );

    let root = private_root();
    let second = receipt();
    let receipts = root.path().join("receipts");
    std::fs::create_dir(&receipts).expect("receipts");
    std::fs::set_permissions(&receipts, std::fs::Permissions::from_mode(0o700)).expect("mode");
    std::os::unix::fs::symlink("/tmp/elsewhere", receipt_path(root.path(), second.job_id))
        .expect("symlink");
    assert_eq!(
        read_receipt(root.path(), &request_for(&second)),
        Err(GitReceiptError::Corrupt)
    );
}

#[test]
fn io_missing_file_is_unavailable_and_read_never_unlinks() {
    let root = private_root();
    let receipt = receipt();
    let receipts = root.path().join("receipts");
    std::fs::create_dir(&receipts).expect("receipts");
    std::fs::set_permissions(&receipts, std::fs::Permissions::from_mode(0o700)).expect("mode");
    assert_eq!(
        read_receipt(root.path(), &request_for(&receipt)),
        Err(GitReceiptError::Unavailable)
    );
    write_receipt(root.path(), &receipt).expect("write");
    let path = receipt_path(root.path(), receipt.job_id);
    assert!(read_receipt(root.path(), &request_for(&receipt)).is_ok());
    assert!(path.exists());
    assert_eq!(
        std::fs::read_dir(root.path().join("receipts"))
            .expect("dir")
            .count(),
        1
    );
}
