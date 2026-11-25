use std::io;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NixCompatError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("invalid store path '{path}': {reason}")]
    InvalidStorePath { path: String, reason: String },

    #[error("invalid NAR entry name: {0:?}")]
    InvalidNarEntryName(Vec<u8>),

    #[error("unsupported filesystem entry type at {0}")]
    UnsupportedFsEntry(PathBuf),

    #[error("invalid verifying key: {0}")]
    InvalidVerifyingKey(String),

    #[error("invalid narinfo: {0}")]
    InvalidNarInfo(String),

    #[error("derivation error: {0}")]
    Derivation(String),
}

pub type Result<T> = std::result::Result<T, NixCompatError>;
