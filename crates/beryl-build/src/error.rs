use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Nix compatibility error: {0}")]
    Nix(#[from] beryl_nix::NixCompatError),

    #[error("Store error: {0}")]
    Store(#[from] beryl_store::StoreError),

    #[error("Eval error: {0}")]
    Eval(#[from] beryl_eval::EvalError),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("missing required derivation env variable '{0}'")]
    MissingEnvVar(String),

    #[error("fixed-output hash mismatch for {path}: expected {expected}, got {actual}")]
    FixedOutputHashMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    #[error("build of '{drv}' failed with exit status {code:?}\n{stderr}")]
    BuilderFailed {
        drv: String,
        code: Option<i32>,
        stderr: String,
    },

    #[error("builder for '{drv}' succeeded but failed to produce output at {out}")]
    OutputNotCreated { drv: String, out: String },

    #[error("unsupported builtin builder '{0}'")]
    UnsupportedBuiltin(String),

    #[error("task join error: {0}")]
    Join(String),
}

pub type Result<T> = std::result::Result<T, BuildError>;
