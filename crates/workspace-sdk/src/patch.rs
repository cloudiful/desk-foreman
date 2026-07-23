mod apply;
mod commit;
mod matching;
mod parser;
mod plan;

#[cfg(test)]
mod tests;

pub(crate) use apply::apply_patch;
pub use parser::parse_patch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchOperation {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<Hunk>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub lines: Vec<HunkLine>,
    pub end_of_file: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    Context(String),
    Delete(String),
    Add(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPatch {
    pub operations: Vec<PatchOperation>,
}
