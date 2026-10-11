#[path = "native_task_export_tests/http.rs"]
mod http;
#[path = "native_task_export_tests/pg.rs"]
mod pg;

use runner_protocol::{
    ExportNativeTaskRequest, ExportNativeTaskUpload, NativeTaskExportContent,
    NativeTaskRuntimeIdentity, RunnerOwner, SourceSeedSnapshotFile,
};

fn upload(text: String) -> ExportNativeTaskUpload {
    let selection =
        runner_protocol::validate_native_task_export_selection(&["text".into()]).unwrap();
    let files = vec![SourceSeedSnapshotFile::new("text", text)];
    let digest =
        runner_protocol::native_task_export_output_manifest_sha256(&selection, &files, &[])
            .unwrap();
    let task_id = uuid::Uuid::new_v4();
    let stop_job_id = uuid::Uuid::new_v4();
    ExportNativeTaskUpload {
        request: ExportNativeTaskRequest {
            owner: RunnerOwner::ExecutionEnvironment {
                environment_id: uuid::Uuid::new_v4().to_string(),
                generation: 1,
            },
            task: NativeTaskRuntimeIdentity {
                task_id,
                workspace_binding_id: 1,
                manifest_sha256: "a".repeat(64),
                preparation_generation: 1,
            },
            stop_job_id,
            selection_sha256: selection.selection_sha256.clone(),
            paths: selection.paths,
        },
        content: NativeTaskExportContent {
            task_id,
            stop_job_id,
            selection_sha256: selection.selection_sha256,
            output_manifest_sha256: digest,
            files,
            missing_paths: vec![],
        },
    }
}

#[test]
fn native_task_export_transport_preserves_exact_upload_identity() {
    let upload = upload("edited\n".into());
    let bytes = super::bounded_body(&upload).unwrap();
    assert_eq!(
        serde_json::from_slice::<ExportNativeTaskUpload>(&bytes).unwrap(),
        upload
    );
}

#[test]
fn native_task_export_transport_rejects_json_escape_expansion() {
    let mut upload = upload("".into());
    upload.content.files = (0..8)
        .map(|index| {
            SourceSeedSnapshotFile::new(
                format!("file{index}"),
                "\u{1}".repeat(runner_protocol::MAX_SEED_CONTENT_BYTES),
            )
        })
        .collect();
    // Raw text fits the aggregate cap; six-byte JSON escapes do not.
    assert!(super::bounded_body(&upload).is_err());
}
