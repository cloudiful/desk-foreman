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
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        let mut content = String::from_utf8(bytes.to_vec())
            .map_err(|_| "file is not valid UTF-8 text".to_string())?;
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
) -> Result<(String, usize, usize), String> {
    let mut result = Vec::<String>::new();
    let mut cursor = 0usize;
    let mut added_lines = 0usize;
    let mut deleted_lines = 0usize;

    for hunk in hunks {
        let needle = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                HunkLine::Add(_) => None,
                HunkLine::Context(text) | HunkLine::Delete(text) => Some(text.clone()),
            })
            .collect::<Vec<_>>();
        let (start, mode) = find_match(&document.lines, &needle, cursor, hunk.end_of_file)?;

        result.extend(document.lines[cursor..start].iter().cloned());
        let mut position = start;
        for line in &hunk.lines {
            match line {
                HunkLine::Context(expected) => {
                    let Some(current) = document.lines.get(position) else {
                        return Err("patch context ran past end of file".to_string());
                    };
                    if !matches_line(current, expected, mode) {
                        return Err(format!(
                            "context mismatch: expected {expected:?}, found {current:?}"
                        ));
                    }
                    result.push(current.clone());
                    position += 1;
                }
                HunkLine::Delete(expected) => {
                    let Some(current) = document.lines.get(position) else {
                        return Err("patch deletion ran past end of file".to_string());
                    };
                    if !matches_line(current, expected, mode) {
                        return Err(format!(
                            "delete mismatch: expected {expected:?}, found {current:?}"
                        ));
                    }
                    position += 1;
                    deleted_lines += 1;
                }
                HunkLine::Add(text) => {
                    result.push(text.clone());
                    added_lines += 1;
                }
            }
        }
        cursor = position;
    }

    result.extend(document.lines[cursor..].iter().cloned());
    Ok((document.render(&result), added_lines, deleted_lines))
}

pub(crate) fn count_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        0
    } else {
        bytes.iter().filter(|byte| **byte == b'\n').count() + usize::from(!bytes.ends_with(b"\n"))
    }
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
) -> Result<(usize, MatchMode), String> {
    if needle.is_empty() {
        return if end_of_file {
            if cursor == original.len() {
                Ok((cursor, MatchMode::Exact))
            } else {
                Err("end-of-file hunk does not start at end of file".to_string())
            }
        } else {
            Ok((cursor, MatchMode::Exact))
        };
    }
    if needle.len() > original.len() {
        return Err("patch pattern is longer than the file".to_string());
    }

    let last_start = original.len() - needle.len();
    if cursor > last_start {
        return Err("unable to find patch context".to_string());
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
            _ => {
                return Err(format!(
                    "ambiguous patch context: found {} matches",
                    candidates.len()
                ));
            }
        }
    }
    Err("unable to find patch context".to_string())
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
