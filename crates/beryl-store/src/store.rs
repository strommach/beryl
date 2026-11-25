//! Core store types and the `Store` trait abstraction.

use std::collections::BTreeSet;
use std::future::Future;
use std::io::BufRead;
use std::path::PathBuf;

use beryl_nix::store_path::StorePath;

use crate::error::Result;

/// Metadata for a valid path registered in the store (mirrors Nix's `ValidPaths` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidPathInfo {
    /// Logical store path (`/nix/store/<hash>-<name>`).
    pub path: StorePath,
    /// SHA-256 digest of the unpacked NAR stream.
    pub nar_hash: [u8; 32],
    /// Size of the unpacked NAR stream in bytes.
    pub nar_size: u64,
    /// Direct runtime references of this store path.
    pub references: BTreeSet<StorePath>,
    /// Optional `.drv` path that produced this output.
    pub deriver: Option<StorePath>,
    /// Signatures (`name:base64`).
    pub sigs: Vec<String>,
    /// Content-address string (e.g. `fixed:sha256:...`), if any.
    pub ca: Option<String>,
    /// Registration timestamp (Unix seconds).
    pub registration_time: i64,
}

/// Abstract interface for a Nix-compatible store backend.
///
/// Implemented by `LocalStore` (standalone SQLite + filesystem) and, in a later phase,
/// `DaemonStore` (`nix-daemon` worker protocol).
pub trait Store: Send + Sync {
    /// Check whether a store path is registered and valid in this store.
    fn has_path(&self, path: &StorePath) -> impl Future<Output = Result<bool>> + Send;

    /// Query metadata for a registered store path.
    fn query_path_info(
        &self,
        path: &StorePath,
    ) -> impl Future<Output = Result<Option<ValidPathInfo>>> + Send;

    /// Resolve the on-disk path for a logical `StorePath`.
    fn real_path(&self, path: &StorePath) -> PathBuf;

    /// Unpack a NAR stream, verify its SHA-256 hash & size against `info`,
    /// atomically move it into the store, and register it in the store DB.
    fn add_nar<R: BufRead + Send + 'static>(
        &self,
        info: ValidPathInfo,
        nar_reader: R,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Compute the full transitive runtime closure of `root` (including `root`).
    fn query_closure(
        &self,
        root: &StorePath,
    ) -> impl Future<Output = Result<BTreeSet<StorePath>>> + Send;
}
