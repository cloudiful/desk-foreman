use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkspaceSdkError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    PatchContextNotFound(String),
    #[error("{context}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
}

impl WorkspaceSdkError {
    pub(crate) fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }

    pub(crate) fn patch_context_not_found(message: impl Into<String>) -> Self {
        Self::PatchContextNotFound(message.into())
    }

    pub(crate) fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::PatchContextNotFound(_) => "patch_context_not_found",
            Self::Io { .. } => "io_error",
        }
    }
}
