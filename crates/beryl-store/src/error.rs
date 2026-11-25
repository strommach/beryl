use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Nix compatibility error: {0}")]
    Nix(#[from] beryl_nix::NixCompatError),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("store path not found: {0}")]
    NotFound(String),

    #[error("NAR hash mismatch for {path}: expected {expected}, got {actual}")]
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    #[error("NAR size mismatch for {path}: expected {expected} bytes, got {actual} bytes")]
    SizeMismatch {
        path: String,
        expected: u64,
        actual: u64,
    },

    #[error("untrusted or invalid binary cache signature for {0}")]
    InvalidSignature(String),

    #[error("unsupported NAR compression '{0}'")]
    UnsupportedCompression(String),

    #[error("missing reference '{reference}' required by '{referrer}'")]
    MissingReference { referrer: String, reference: String },

    #[error("no previous generation available for profile '{0}'")]
    NoPreviousGeneration(String),

    #[error("nix-daemon error: {0}")]
    Daemon(String),

    #[error("task join error: {0}")]
    Join(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;
