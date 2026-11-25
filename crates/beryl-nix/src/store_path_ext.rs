//! Convenience helpers for Nix store paths and Nix base32 encoding.

use nix_compat::store_path::{StorePath, StorePathRef};

use crate::error::{NixCompatError, Result};

/// Parse a Nix store path, accepting either an absolute path (`/nix/store/<hash>-<name>`)
/// or a bare store path basename (`<hash>-<name>`).
pub fn parse_store_path(input: &str) -> Result<StorePath> {
    if input.starts_with('/') {
        StorePath::from_absolute_path(input.as_bytes()).map_err(|e| {
            NixCompatError::InvalidStorePath {
                path: input.to_string(),
                reason: e.to_string(),
            }
        })
    } else {
        StorePath::from_bytes(input.as_bytes()).map_err(|e| NixCompatError::InvalidStorePath {
            path: input.to_string(),
            reason: e.to_string(),
        })
    }
}

/// Format a `StorePathRef` as an absolute `/nix/store/<hash>-<name>` string.
pub fn to_absolute_string(path: StorePathRef<'_>) -> String {
    path.to_absolute_path()
}
