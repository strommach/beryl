use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Lua error: {0}")]
    Lua(#[from] mlua::Error),

    #[error("Nix compatibility error: {0}")]
    Nix(#[from] beryl_nix::NixCompatError),

    #[error("Store error: {0}")]
    Store(#[from] beryl_store::StoreError),

    #[error("Index error: {0}")]
    Index(#[from] beryl_index::IndexError),

    #[error("derivation error: {0}")]
    Derivation(String),

    #[error("invalid sha256 hash '{0}'")]
    InvalidSha256(String),

    #[error("pkg.lua must return a beryl.derivation(...) value")]
    InvalidReturnValue,
}

pub type Result<T> = std::result::Result<T, EvalError>;
