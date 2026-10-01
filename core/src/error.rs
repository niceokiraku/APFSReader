use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid format: {0}")]
    Format(&'static str),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("read out of range (offset {offset}, len {len})")]
    OutOfRange { offset: u64, len: usize },
}

pub type Result<T> = std::result::Result<T, Error>;
