use std::io;
use std::result::Result as StdResult;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum InkDripError {
    #[error("Book not found: {0}")]
    BookNotFound(String),

    #[error("Feed not found: {0}")]
    FeedNotFound(String),

    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Split error: {0}")]
    SplitError(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Ambiguous ID prefix '{0}': multiple matches")]
    AmbiguousId(String),

    #[error("Book already exists (ID: {0})")]
    DuplicateBook(String),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = StdResult<T, InkDripError>;
