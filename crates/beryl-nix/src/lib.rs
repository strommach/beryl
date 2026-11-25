//! Nix format and compatibility primitives for Beryl, backed by `nix-rs-compat`.

pub mod error;
pub mod nar_fs;
pub mod narinfo_ext;
pub mod store_path_ext;

pub use error::{NixCompatError, Result};
pub use nix_compat::{
    self, derivation, nar, narinfo, nixbase32, nixhash, path_info, store_path,
};
