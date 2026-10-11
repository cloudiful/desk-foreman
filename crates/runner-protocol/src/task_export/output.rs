use std::collections::HashSet;

use super::{
    ExportNativeTaskRequest, NativeTaskExportContent, NativeTaskExportSelection,
    NativeTaskExportValidationError as Error, TASK_EXPORT_OUTPUT_PREFIX, ValidatedNativeTaskExport,
    validate_native_task_export_selection,
};
use crate::{SourceSeedSnapshotFile, validate_seed_bundle};

struct CanonicalOutput {
    files: Vec<SourceSeedSnapshotFile>,
    missing_paths: Vec<String>,
    total_bytes: u32,
}

pub fn validate_native_task_export_content(
    request: &ExportNativeTaskRequest,
    content: &NativeTaskExportContent,
) -> Result<ValidatedNativeTaskExport, Error> {
    let selection = validate_native_task_export_selection(&request.paths)?;
    if request.selection_sha256 != selection.selection_sha256 {
        return Err(Error::SelectionDigestMismatch);
    }
    if content.task_id != request.task.task_id
        || content.stop_job_id != request.stop_job_id
        || content.selection_sha256 != selection.selection_sha256
    {
        return Err(Error::IdentityMismatch);
    }
    let canonical = canonical_output(&selection, &content.files, &content.missing_paths)?;
    let output_manifest_sha256 = output_hash(&selection.paths, &canonical.files);
    if content.output_manifest_sha256 != output_manifest_sha256 {
        return Err(Error::OutputManifestMismatch);
    }
    let canonical_content = NativeTaskExportContent {
        task_id: content.task_id,
        stop_job_id: content.stop_job_id,
        selection_sha256: selection.selection_sha256,
        output_manifest_sha256,
        files: canonical.files,
        missing_paths: canonical.missing_paths,
    };
    Ok(ValidatedNativeTaskExport {
        file_count: u32::try_from(canonical_content.files.len()).unwrap_or_default(),
        missing_count: u32::try_from(canonical_content.missing_paths.len()).unwrap_or_default(),
        total_bytes: canonical.total_bytes,
        content: canonical_content,
    })
}

/// Compute the canonical output digest shared by the manager producer and
/// control-plane upload validator.
pub fn native_task_export_output_manifest_sha256(
    selection: &NativeTaskExportSelection,
    files: &[SourceSeedSnapshotFile],
    missing_paths: &[String],
) -> Result<String, Error> {
    let checked = validate_native_task_export_selection(&selection.paths)?;
    if checked != *selection {
        return Err(Error::SelectionDigestMismatch);
    }
    let canonical = canonical_output(&checked, files, missing_paths)?;
    Ok(output_hash(&checked.paths, &canonical.files))
}

fn canonical_output(
    selection: &NativeTaskExportSelection,
    files: &[SourceSeedSnapshotFile],
    missing_paths: &[String],
) -> Result<CanonicalOutput, Error> {
    let files = validate_seed_bundle(files).map_err(Error::OutputBounds)?;
    let missing_inputs = missing_paths
        .iter()
        .map(|path| SourceSeedSnapshotFile::new(path.clone(), ""))
        .collect::<Vec<_>>();
    let missing_paths = if missing_inputs.is_empty() {
        Vec::new()
    } else {
        validate_seed_bundle(&missing_inputs)
            .map_err(Error::InvalidMissingPaths)?
            .files
            .into_iter()
            .map(|file| file.path)
            .collect()
    };
    if files.files.len() + missing_paths.len() > super::MAX_TASK_EXPORT_PATHS {
        return Err(Error::TooManyPaths);
    }

    let present: HashSet<_> = files.files.iter().map(|file| file.path.as_str()).collect();
    if missing_paths
        .iter()
        .any(|path| present.contains(path.as_str()))
    {
        return Err(Error::OutputSetMismatch);
    }
    let supplied_paths = files
        .files
        .iter()
        .map(|file| file.path.clone())
        .chain(missing_paths.iter().cloned())
        .collect::<Vec<_>>();
    let supplied = validate_native_task_export_selection(&supplied_paths)
        .map_err(|_| Error::OutputSetMismatch)?;
    if supplied.paths != selection.paths {
        return Err(Error::OutputSetMismatch);
    }

    Ok(CanonicalOutput {
        files: files.files,
        missing_paths,
        total_bytes: files.total_bytes,
    })
}

fn output_hash(paths: &[String], files: &[SourceSeedSnapshotFile]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(TASK_EXPORT_OUTPUT_PREFIX);
    let mut file_index = 0;
    for path in paths {
        hasher.update((path.len() as u64).to_be_bytes());
        hasher.update(path.as_bytes());
        if files.get(file_index).is_some_and(|file| file.path == *path) {
            let file = &files[file_index];
            hasher.update([1]);
            hasher.update((file.content.len() as u64).to_be_bytes());
            hasher.update(file.content.as_bytes());
            file_index += 1;
        } else {
            hasher.update([0]);
        }
    }
    let mut output = String::with_capacity(64);
    for byte in hasher.finalize() {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
