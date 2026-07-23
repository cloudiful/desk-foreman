use crate::{Hunk, HunkLine, ParsedPatch, PatchOperation, WorkspaceSdkError};

pub(crate) const MAX_PATTERN_LINES: usize = 10_000;

pub fn parse_patch(input: &str) -> Result<ParsedPatch, WorkspaceSdkError> {
    let lines = input
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect::<Vec<_>>();

    if lines.first().map(String::as_str) != Some("*** Begin Patch") {
        return Err(WorkspaceSdkError::invalid_input(
            "patch must start with *** Begin Patch",
        ));
    }
    if lines.last().map(String::as_str) != Some("*** End Patch") {
        return Err(WorkspaceSdkError::invalid_input(
            "patch must end with *** End Patch",
        ));
    }

    let mut index = 1usize;
    let mut operations = Vec::new();
    while index + 1 < lines.len() {
        let line = &lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let path = non_empty_path(path, "add file")?;
            index += 1;
            let mut added = Vec::new();
            while index < lines.len()
                && lines[index] != "*** End Patch"
                && !lines[index].starts_with("*** ")
            {
                let line = &lines[index];
                let Some(content) = line.strip_prefix('+') else {
                    return Err(WorkspaceSdkError::invalid_input(
                        "add file hunks may only contain + lines",
                    ));
                };
                added.push(content.to_string());
                index += 1;
            }
            if added.is_empty() {
                return Err(WorkspaceSdkError::invalid_input(format!(
                    "add file has no content: {path}"
                )));
            }
            operations.push(PatchOperation::Add {
                path: path.to_string(),
                lines: added,
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            let path = non_empty_path(path, "delete file")?;
            operations.push(PatchOperation::Delete {
                path: path.to_string(),
            });
            index += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            let path = non_empty_path(path, "update file")?;
            index += 1;
            let mut move_to = None;
            if index < lines.len()
                && let Some(target) = lines[index].strip_prefix("*** Move to: ")
            {
                move_to = Some(non_empty_path(target, "move target")?.to_string());
                index += 1;
            }

            let mut hunks = Vec::new();
            let mut current_hunk: Option<Hunk> = None;
            while index < lines.len() && lines[index] != "*** End Patch" {
                let line = &lines[index];
                if line == "*** End of File" {
                    let Some(hunk) = current_hunk.as_mut() else {
                        return Err(WorkspaceSdkError::invalid_input(
                            "*** End of File must follow an update hunk",
                        ));
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
                        validate_hunk(&hunk)?;
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
                        return Err(WorkspaceSdkError::invalid_input(format!(
                            "invalid update hunk line: {line}"
                        )));
                    }
                };
                hunk.lines.push(parsed);
                index += 1;
            }
            if let Some(hunk) = current_hunk.take() {
                validate_hunk(&hunk)?;
                hunks.push(hunk);
            }
            if hunks.is_empty() {
                return Err(WorkspaceSdkError::invalid_input(format!(
                    "update file has no hunks: {path}"
                )));
            }

            operations.push(PatchOperation::Update {
                path: path.to_string(),
                move_to,
                hunks,
            });
            continue;
        }

        return Err(WorkspaceSdkError::invalid_input(format!(
            "unknown patch directive: {line}"
        )));
    }

    if operations.is_empty() {
        return Err(WorkspaceSdkError::invalid_input(
            "patch must contain at least one file operation",
        ));
    }
    Ok(ParsedPatch { operations })
}

fn non_empty_path<'a>(path: &'a str, kind: &str) -> Result<&'a str, WorkspaceSdkError> {
    if path.is_empty() {
        return Err(WorkspaceSdkError::invalid_input(format!(
            "{kind} path must not be empty"
        )));
    }
    Ok(path)
}

fn validate_hunk(hunk: &Hunk) -> Result<(), WorkspaceSdkError> {
    if hunk.lines.is_empty() {
        return Err(WorkspaceSdkError::invalid_input(
            "update hunk must contain at least one line",
        ));
    }
    if hunk.lines.len() > MAX_PATTERN_LINES {
        return Err(WorkspaceSdkError::invalid_input(format!(
            "update hunk is too large; maximum is {MAX_PATTERN_LINES} lines"
        )));
    }
    Ok(())
}
