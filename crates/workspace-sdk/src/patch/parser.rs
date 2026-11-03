use crate::{
    ApplyPatchOperation, Hunk, HunkLine, ParsedPatch, PatchCode, PatchDiagnostic, PatchOperation,
    PatchStage,
};

pub(crate) const MAX_PATTERN_LINES: usize = 10_000;

pub fn parse_patch(input: &str) -> Result<ParsedPatch, PatchDiagnostic> {
    let lines = input
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect::<Vec<_>>();

    if lines.first().map(String::as_str) != Some("*** Begin Patch") {
        return Err(parse_error(PatchCode::PatchBoundaryInvalid));
    }
    if lines.last().map(String::as_str) != Some("*** End Patch") {
        return Err(parse_error(PatchCode::PatchBoundaryInvalid));
    }

    let mut index = 1usize;
    let mut operations = Vec::new();
    while index + 1 < lines.len() {
        let line = &lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let path = non_empty_path(path)?;
            index += 1;
            let mut added = Vec::new();
            while index < lines.len()
                && lines[index] != "*** End Patch"
                && !lines[index].starts_with("*** ")
            {
                let line = &lines[index];
                let Some(content) = line.strip_prefix('+') else {
                    return Err(parse_error(PatchCode::PatchHunkInvalid)
                        .with_operation(Some(ApplyPatchOperation::Add)));
                };
                added.push(content.to_string());
                index += 1;
            }
            if added.is_empty() {
                return Err(parse_error(PatchCode::PatchHunkInvalid)
                    .with_operation(Some(ApplyPatchOperation::Add)));
            }
            operations.push(PatchOperation::Add {
                path: path.to_string(),
                lines: added,
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            let path = non_empty_path(path)?;
            operations.push(PatchOperation::Delete {
                path: path.to_string(),
            });
            index += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            let path = non_empty_path(path)?;
            index += 1;
            let mut move_to = None;
            if index < lines.len()
                && let Some(target) = lines[index].strip_prefix("*** Move to: ")
            {
                move_to = Some(non_empty_path(target)?.to_string());
                index += 1;
            }

            let mut hunks = Vec::new();
            let mut current_hunk: Option<Hunk> = None;
            while index < lines.len() && lines[index] != "*** End Patch" {
                let line = &lines[index];
                if line == "*** End of File" {
                    let Some(hunk) = current_hunk.as_mut() else {
                        return Err(parse_error(PatchCode::PatchHunkInvalid)
                            .with_operation(Some(ApplyPatchOperation::Update)));
                    };
                    hunk.end_of_file = true;
                    index += 1;
                    continue;
                }
                if line.starts_with("*** ") {
                    break;
                }
                if line == "@@" || line.starts_with("@@ ") {
                    if let Some(hunk) = current_hunk.take() {
                        validate_hunk(&hunk, hunks.len())?;
                        hunks.push(hunk);
                    }
                    current_hunk = Some(Hunk {
                        lines: Vec::new(),
                        end_of_file: false,
                    });
                    index += 1;
                    continue;
                }
                let hunk = current_hunk.get_or_insert_with(|| Hunk {
                    lines: Vec::new(),
                    end_of_file: false,
                });
                let parsed = match line.chars().next() {
                    Some(' ') => HunkLine::Context(line[1..].to_string()),
                    Some('-') => HunkLine::Delete(line[1..].to_string()),
                    Some('+') => HunkLine::Add(line[1..].to_string()),
                    _ => {
                        return Err(parse_error(PatchCode::PatchHunkInvalid)
                            .with_operation(Some(ApplyPatchOperation::Update))
                            .with_hunk(Some(hunks.len() as u32)));
                    }
                };
                hunk.lines.push(parsed);
                index += 1;
            }
            if let Some(hunk) = current_hunk.take() {
                validate_hunk(&hunk, hunks.len())?;
                hunks.push(hunk);
            }
            if hunks.is_empty() {
                return Err(parse_error(PatchCode::PatchHunkInvalid)
                    .with_operation(Some(ApplyPatchOperation::Update)));
            }

            operations.push(PatchOperation::Update {
                path: path.to_string(),
                move_to,
                hunks,
            });
            continue;
        }

        return Err(parse_error(PatchCode::PatchBoundaryInvalid));
    }

    if operations.is_empty() {
        return Err(PatchDiagnostic::new(
            PatchCode::PatchArgInvalid,
            PatchStage::Params,
        ));
    }
    Ok(ParsedPatch { operations })
}

fn non_empty_path(path: &str) -> Result<&str, PatchDiagnostic> {
    if path.is_empty() {
        return Err(parse_error(PatchCode::PatchArgInvalid));
    }
    Ok(path)
}

fn validate_hunk(hunk: &Hunk, index: usize) -> Result<(), PatchDiagnostic> {
    if hunk.lines.is_empty() || hunk.lines.len() > MAX_PATTERN_LINES {
        return Err(parse_error(PatchCode::PatchHunkInvalid)
            .with_operation(Some(ApplyPatchOperation::Update))
            .with_hunk(Some(index as u32)));
    }
    Ok(())
}

fn parse_error(code: PatchCode) -> PatchDiagnostic {
    PatchDiagnostic::new(code, PatchStage::Parse)
}
