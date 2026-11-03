use crate::{PatchCode, PatchDiagnostic, PatchStage};

use super::{Hunk, HunkLine};

#[derive(Debug, Clone, Copy)]
enum MatchMode {
    Exact,
    Rstrip,
    Trim,
    UnicodePunctuation,
}

#[derive(Debug, Clone)]
pub(crate) struct TextDocument {
    pub lines: Vec<String>,
    pub trailing_newline: bool,
    pub newline: &'static str,
    pub bom: bool,
}

impl TextDocument {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, PatchDiagnostic> {
        let mut content = String::from_utf8(bytes.to_vec())
            .map_err(|_| PatchDiagnostic::new(PatchCode::PatchNotUtf8Text, PatchStage::Plan))?;
        let bom = content.starts_with('\u{feff}');
        if bom {
            content = content[3..].to_string();
        }
        let newline = if content.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        Ok(Self {
            trailing_newline: content.ends_with('\n'),
            lines: split_lines(&content),
            newline,
            bom,
        })
    }

    pub(crate) fn render(&self, lines: &[String]) -> String {
        let mut rendered = if lines.is_empty() {
            String::new()
        } else {
            lines.join(self.newline)
        };
        if self.trailing_newline && !lines.is_empty() {
            rendered.push_str(self.newline);
        }
        if self.bom {
            rendered.insert(0, '\u{feff}');
        }
        rendered
    }
}

pub(crate) fn apply_hunks(
    document: &TextDocument,
    hunks: &[Hunk],
) -> Result<String, PatchDiagnostic> {
    let mut result = Vec::<String>::new();
    let mut cursor = 0usize;

    for (index, hunk) in hunks.iter().enumerate() {
        let needle = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                HunkLine::Add(_) => None,
                HunkLine::Context(text) | HunkLine::Delete(text) => Some(text.clone()),
            })
            .collect::<Vec<_>>();
        let (start, mode) = find_match(&document.lines, &needle, cursor, hunk.end_of_file)
            .map_err(|code| {
                PatchDiagnostic::new(code, PatchStage::Plan).with_hunk(Some(index as u32))
            })?;

        result.extend(document.lines[cursor..start].iter().cloned());
        let mut position = start;
        for line in &hunk.lines {
            match line {
                HunkLine::Context(expected) => {
                    let Some(current) = document.lines.get(position) else {
                        return Err(hunk_failure(index, PatchCode::PatchContextNotFound));
                    };
                    if !matches_line(current, expected, mode) {
                        return Err(hunk_failure(index, PatchCode::PatchContextNotFound));
                    }
                    result.push(current.clone());
                    position += 1;
                }
                HunkLine::Delete(expected) => {
                    let Some(current) = document.lines.get(position) else {
                        return Err(hunk_failure(index, PatchCode::PatchContextNotFound));
                    };
                    if !matches_line(current, expected, mode) {
                        return Err(hunk_failure(index, PatchCode::PatchContextNotFound));
                    }
                    position += 1;
                }
                HunkLine::Add(text) => {
                    result.push(text.clone());
                }
            }
        }
        cursor = position;
    }

    result.extend(document.lines[cursor..].iter().cloned());
    Ok(document.render(&result))
}

fn hunk_failure(index: usize, code: PatchCode) -> PatchDiagnostic {
    PatchDiagnostic::new(code, PatchStage::Plan).with_hunk(Some(index as u32))
}

fn split_lines(input: &str) -> Vec<String> {
    if input.is_empty() {
        return Vec::new();
    }
    let input = input.strip_suffix('\n').unwrap_or(input);
    if input.is_empty() {
        return vec![String::new()];
    }
    input
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect()
}

fn find_match(
    original: &[String],
    needle: &[String],
    cursor: usize,
    end_of_file: bool,
) -> Result<(usize, MatchMode), PatchCode> {
    if needle.is_empty() {
        return if end_of_file {
            if cursor == original.len() {
                Ok((cursor, MatchMode::Exact))
            } else {
                Err(PatchCode::PatchContextNotFound)
            }
        } else {
            Ok((cursor, MatchMode::Exact))
        };
    }
    if needle.len() > original.len() {
        return Err(PatchCode::PatchContextNotFound);
    }

    let last_start = original.len() - needle.len();
    if cursor > last_start {
        return Err(PatchCode::PatchContextNotFound);
    }

    for mode in [
        MatchMode::Exact,
        MatchMode::Rstrip,
        MatchMode::Trim,
        MatchMode::UnicodePunctuation,
    ] {
        let candidates = (cursor..=last_start)
            .filter(|start| {
                if end_of_file && start + needle.len() != original.len() {
                    return false;
                }
                needle.iter().enumerate().all(|(offset, expected)| {
                    matches_line(&original[start + offset], expected, mode)
                })
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [] => {}
            [start] => return Ok((*start, mode)),
            _ => return Err(PatchCode::PatchContextAmbiguous),
        }
    }
    Err(PatchCode::PatchContextNotFound)
}

fn matches_line(actual: &str, expected: &str, mode: MatchMode) -> bool {
    match mode {
        MatchMode::Exact => actual == expected,
        MatchMode::Rstrip => actual.trim_end() == expected.trim_end(),
        MatchMode::Trim => actual.trim() == expected.trim(),
        MatchMode::UnicodePunctuation => {
            normalize_punctuation(actual.trim()) == normalize_punctuation(expected.trim())
        }
    }
}

fn normalize_punctuation(input: &str) -> String {
    input.chars().fold(String::new(), |mut output, character| {
        let character = if ('\u{ff01}'..='\u{ff5e}').contains(&character) {
            char::from_u32(character as u32 - 0xfee0).unwrap_or(character)
        } else if character == '\u{3000}' {
            ' '
        } else {
            character
        };
        let replacement = match character {
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => "'",
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => "\"",
            '\u{2013}' | '\u{2014}' | '\u{2015}' | '\u{2212}' => "-",
            '\u{2026}' => "...",
            '\u{00a0}' => " ",
            _ => {
                output.push(character);
                return output;
            }
        };
        output.push_str(replacement);
        output
    })
}
