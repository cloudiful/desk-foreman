use uuid::Uuid;

use crate::{
    NativeTaskRuntimeIdentity, RunnerOwner, SourceSeedSnapshotFile,
    native_task_export_output_manifest_sha256, selection_manifest_sha256,
    validate_native_task_export_content, validate_native_task_export_selection,
};

use super::{ExportNativeTaskRequest, NativeTaskExportContent, NativeTaskExportValidationError};

fn identity() -> NativeTaskRuntimeIdentity {
    NativeTaskRuntimeIdentity {
        task_id: Uuid::from_u128(1),
        workspace_binding_id: 7,
        manifest_sha256: "a".repeat(64),
        preparation_generation: 4,
    }
}

fn request(paths: &[&str]) -> ExportNativeTaskRequest {
    let paths = paths
        .iter()
        .map(|path| path.to_string())
        .collect::<Vec<_>>();
    let selection = validate_native_task_export_selection(&paths).unwrap();
    ExportNativeTaskRequest {
        owner: RunnerOwner::ExecutionEnvironment {
            environment_id: Uuid::from_u128(2).to_string(),
            generation: 4,
        },
        task: identity(),
        stop_job_id: Uuid::from_u128(3),
        selection_sha256: selection.selection_sha256,
        paths: selection.paths,
    }
}

#[test]
fn selection_digest_is_order_independent_and_rejects_unsafe_path_sets() {
    let first = validate_native_task_export_selection(&[
        "src/main.rs".to_string(),
        "README.md".to_string(),
    ])
    .unwrap();
    let replay = validate_native_task_export_selection(&[
        "README.md".to_string(),
        "src/main.rs".to_string(),
    ])
    .unwrap();
    assert_eq!(first.paths, ["README.md", "src/main.rs"]);
    assert_eq!(first, replay);
    assert_eq!(
        first.selection_sha256,
        selection_manifest_sha256(&first.paths)
    );
    assert_eq!(
        validate_native_task_export_selection(&[]),
        Err(NativeTaskExportValidationError::EmptySelection)
    );
    for paths in [
        vec!["a".to_string(), "a".to_string()],
        vec!["a".to_string(), "a/b".to_string()],
        vec!["../secret".to_string()],
        vec![".git/config".to_string()],
    ] {
        assert!(validate_native_task_export_selection(&paths).is_err());
    }
}

#[test]
fn canonical_output_digest_is_shared_with_content_validation() {
    let request = request(&["src/main.rs", "empty.txt", "missing.txt"]);
    let files = vec![
        SourceSeedSnapshotFile::new("src/main.rs", "contents"),
        SourceSeedSnapshotFile::new("empty.txt", ""),
    ];
    let missing = vec!["missing.txt".to_string()];
    let selection = validate_native_task_export_selection(&request.paths).unwrap();
    let digest = native_task_export_output_manifest_sha256(&selection, &files, &missing).unwrap();
    let content = NativeTaskExportContent {
        task_id: request.task.task_id,
        stop_job_id: request.stop_job_id,
        selection_sha256: request.selection_sha256.clone(),
        output_manifest_sha256: digest,
        files: vec![files[1].clone(), files[0].clone()],
        missing_paths: missing,
    };
    let validated = validate_native_task_export_content(&request, &content).unwrap();
    assert_eq!(validated.content.files[0].path, "empty.txt");
    assert_eq!(validated.content.files[1].path, "src/main.rs");
    assert_eq!(validated.content.missing_paths, ["missing.txt"]);
    assert_eq!(validated.file_count, 2);
    assert_eq!(validated.missing_count, 1);
    assert_eq!(validated.total_bytes, 8);
}

#[test]
fn a_present_empty_file_and_a_missing_path_have_different_manifests() {
    let request = request(&["file.txt"]);
    let selection = validate_native_task_export_selection(&request.paths).unwrap();
    let present = vec![SourceSeedSnapshotFile::new("file.txt", "")];
    let missing = vec!["file.txt".to_string()];
    let present_digest =
        native_task_export_output_manifest_sha256(&selection, &present, &[]).unwrap();
    let missing_digest =
        native_task_export_output_manifest_sha256(&selection, &[], &missing).unwrap();
    assert_ne!(present_digest, missing_digest);
}

#[test]
fn output_set_identity_and_manifest_must_match_the_job() {
    let request = request(&["a.txt", "b.txt"]);
    let partial = NativeTaskExportContent {
        task_id: request.task.task_id,
        stop_job_id: request.stop_job_id,
        selection_sha256: request.selection_sha256.clone(),
        output_manifest_sha256: "0".repeat(64),
        files: vec![SourceSeedSnapshotFile::new("a.txt", "a")],
        missing_paths: Vec::new(),
    };
    assert_eq!(
        validate_native_task_export_content(&request, &partial),
        Err(NativeTaskExportValidationError::OutputSetMismatch)
    );

    let selection = validate_native_task_export_selection(&request.paths).unwrap();
    let files = vec![
        SourceSeedSnapshotFile::new("a.txt", "a"),
        SourceSeedSnapshotFile::new("b.txt", "b"),
    ];
    let digest = native_task_export_output_manifest_sha256(&selection, &files, &[]).unwrap();
    let mut content = partial;
    content.files = files;
    content.output_manifest_sha256 = digest;
    content.stop_job_id = Uuid::from_u128(99);
    assert_eq!(
        validate_native_task_export_content(&request, &content),
        Err(NativeTaskExportValidationError::IdentityMismatch)
    );
}

#[test]
fn export_protocol_shapes_reject_unowned_fields() {
    let request = request(&["a.txt"]);
    let mut value = serde_json::to_value(request).unwrap();
    value["host_path"] = serde_json::json!("/etc");
    assert!(serde_json::from_value::<ExportNativeTaskRequest>(value).is_err());
}
