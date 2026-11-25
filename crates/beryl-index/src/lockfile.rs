//! `beryl.lock` persistence for pinned `nixpkgs` store paths.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Origin of a pinned `nixpkgs` store path in `beryl.lock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionSource {
    Hydra,
    NixEval,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPackage {
    pub store_path: String,
    pub source: ResolutionSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NixpkgsPin {
    #[serde(default = "default_channel")]
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

fn default_channel() -> String {
    "nixos-unstable".to_string()
}

impl Default for NixpkgsPin {
    fn default() -> Self {
        Self {
            channel: default_channel(),
            rev: None,
        }
    }
}

/// On-disk `beryl.lock` schema (TOML).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub nixpkgs: NixpkgsPin,
    /// Map of `system` (e.g. `"aarch64-linux"`) -> `attr` (e.g. `"ripgrep"`) -> `LockedPackage`.
    #[serde(default)]
    pub packages: BTreeMap<String, BTreeMap<String, LockedPackage>>,
}

fn default_version() -> u32 {
    1
}

impl Default for Lockfile {
    fn default() -> Self {
        Self {
            version: default_version(),
            nixpkgs: NixpkgsPin::default(),
            packages: BTreeMap::new(),
        }
    }
}

impl Lockfile {
    /// Load `beryl.lock` from `path`, or return a fresh default lockfile if it does not exist.
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path)?;
        let lockfile: Self = toml::from_str(&text)?;
        Ok(lockfile)
    }

    /// Atomically write `beryl.lock` to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let encoded = toml::to_string_pretty(self)?;
        let tmp_path = path.with_extension("lock.tmp");
        fs::write(&tmp_path, encoded)?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    }

    /// Look up a pinned store path for `(system, attr)`.
    pub fn get(&self, system: &str, attr: &str) -> Result<Option<StorePath>> {
        let Some(pkg) = self.packages.get(system).and_then(|m| m.get(attr)) else {
            return Ok(None);
        };
        Ok(Some(parse_store_path(&pkg.store_path)?))
    }

    /// Pin `(system, attr) -> store_path` in the lockfile.
    pub fn insert(&mut self, system: &str, attr: &str, path: &StorePath, source: ResolutionSource) {
        self.packages
            .entry(system.to_string())
            .or_default()
            .insert(
                attr.to_string(),
                LockedPackage {
                    store_path: path.to_absolute_path(),
                    source,
                },
            );
    }
}
