use std::fmt;

use crate::DatasetId;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("a command with id `{0}` is already registered")]
    DuplicateCommand(String),
    #[error("invalid command definition `{id}`: {reason}")]
    InvalidSpec { id: String, reason: String },
    #[error("missing required parameter `{param}` for `{command}`")]
    MissingArg { command: String, param: String },
    #[error("`{command}` has no parameter `{param}`")]
    UnknownArg { command: String, param: String },
    #[error("invalid value for `{param}`: {reason}")]
    InvalidArg { param: String, reason: String },
    #[error("not permitted: {0}")]
    Denied(String),
    #[error("cancelled")]
    Cancelled,
    #[error("no open image with id {0}")]
    NoDataset(DatasetId),
    #[error("nothing to undo")]
    NothingToUndo,
    #[error("nothing to redo")]
    NothingToRedo,
    #[error(transparent)]
    Core(#[from] imager_core::Error),
    #[error("{0}")]
    Failed(String),
}

impl Error {
    /// A command failure with a message for the user.
    pub fn failed(message: impl fmt::Display) -> Error {
        Error::Failed(message.to_string())
    }

    pub(crate) fn invalid_arg(param: &str, reason: impl fmt::Display) -> Error {
        Error::InvalidArg {
            param: param.to_string(),
            reason: reason.to_string(),
        }
    }
}
