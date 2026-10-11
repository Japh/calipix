use crate::DType;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("shape mismatch: {0}")]
    Shape(String),
    #[error("pixel type mismatch: expected {expected}, found {found}")]
    DType { expected: DType, found: DType },
    #[error("out of bounds: {0}")]
    OutOfBounds(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Other(String),
}
