//! Project manifest (`beryl.lua`) and `beryl.shell(...)` definitions.

use std::collections::{BTreeMap, BTreeSet};

use beryl_nix::derivation::Derivation;
use beryl_nix::store_path::StorePath;
use mlua::UserData;

use crate::store_ref::StoreRef;

/// Specification of a Beryl dev shell produced by `beryl.shell({ ... })`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DevShellSpec {
    /// Tool packages whose `bin/` (and `lib/pkgconfig`) are added to the shell environment.
    pub packages: Vec<StoreRef>,
    /// Library packages whose `include/`, `lib/`, `pkgconfig/`, and `-L` flags are wired into
    /// `CPATH`, `LIBRARY_PATH`, `PKG_CONFIG_PATH`, and `LDFLAGS`.
    pub libs: Vec<StoreRef>,
    /// Extra environment variables exported inside the dev shell.
    pub env: BTreeMap<String, String>,
    /// Optional shell hook executed upon entering the dev shell.
    pub hook: Option<String>,
}

/// Lua `UserData` wrapper for `DevShellSpec`.
#[derive(Debug, Clone)]
pub struct DevShellRef(pub DevShellSpec);

impl UserData for DevShellRef {}

/// Evaluated `beryl.lua` project manifest (analogous to a Nix `flake.nix`).
#[derive(Debug, Clone)]
pub struct ProjectManifest {
    pub description: Option<String>,
    pub shells: BTreeMap<String, DevShellSpec>,
    pub packages: BTreeMap<String, StoreRef>,
    pub derivations: BTreeMap<StorePath, Derivation>,
    pub input_sources: BTreeSet<StorePath>,
}
