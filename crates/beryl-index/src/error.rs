use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("TOML parse error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("TOML serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Nix compatibility error: {0}")]
    Nix(#[from] beryl_nix::NixCompatError),

    #[error("Store/cache error: {0}")]
    Store(#[from] beryl_store::StoreError),

    #[error("attribute '{attr}' for system '{system}' not found in index and `nix` is unavailable")]
    NotFound { attr: String, system: String },

    #[error("resolved path '{path}' for '{attr}' is not available in binary cache")]
    MissingInCache { attr: String, path: String },

    #[error("`nix eval` failed for '{attr}': {stderr}")]
    NixEvalFailed { attr: String, stderr: String },

    #[error("unsupported host platform: {os}/{arch}")]
    UnsupportedPlatform { os: String, arch: String },
}

pub type Result<T> = std::result::Result<T, IndexError>;
