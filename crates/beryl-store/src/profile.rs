//! Atomic user profiles (`/nix/var/nix/profiles/...`) with symlink farms and rollbacks.

use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use beryl_nix::nar_fs::{dump_nar, hash_nar_path};
use beryl_nix::nixhash::NixHash;
use beryl_nix::store_path::{StorePath, build_ca_path};
use beryl_nix::store_path_ext::parse_store_path;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::{Result, StoreError};
use crate::local::LocalStore;
use crate::store::{Store, ValidPathInfo};

const MANIFEST_FILENAME: &str = "beryl-manifest.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProfileManifest {
    paths: Vec<String>,
}

/// Manages generations and atomic symlink flips for a named profile (e.g. `"default"`).
#[derive(Clone)]
pub struct ProfileManager {
    store: LocalStore,
    profiles_dir: PathBuf,
    name: String,
}

impl ProfileManager {
    pub fn new(store: LocalStore, name: impl Into<String>) -> Result<Self> {
        let profiles_dir = store.root().join("var/nix/profiles");
        fs::create_dir_all(&profiles_dir)?;
        Ok(Self {
            store,
            profiles_dir,
            name: name.into(),
        })
    }

    /// Path to the active profile symlink (e.g. `<root>/var/nix/profiles/default`).
    pub fn active_link(&self) -> PathBuf {
        self.profiles_dir.join(&self.name)
    }

    /// Path to a specific generation symlink (e.g. `<root>/var/nix/profiles/default-3-link`).
    pub fn generation_link(&self, gen_num: u64) -> PathBuf {
        self.profiles_dir
            .join(format!("{}-{gen_num}-link", self.name))
    }

    /// List all existing generation numbers for this profile in ascending order.
    pub fn list_generations(&self) -> Result<Vec<u64>> {
        let prefix = format!("{}-", self.name);
        let suffix = "-link";
        let mut gens = Vec::new();

        for entry in fs::read_dir(&self.profiles_dir)? {
            let entry = entry?;
            let fname = entry.file_name();
            let Some(name_str) = fname.to_str() else {
                continue;
            };
            if let Some(rest) = name_str.strip_prefix(&prefix)
                && let Some(num_str) = rest.strip_suffix(suffix)
                && let Ok(num) = num_str.parse::<u64>()
            {
                gens.push(num);
            }
        }
        gens.sort_unstable();
        Ok(gens)
    }

    /// Return the currently active generation number, if any.
    pub fn current_generation(&self) -> Result<Option<u64>> {
        let active = self.active_link();
        if !active.is_symlink() {
            return Ok(None);
        }
        let target = fs::read_link(&active)?;
        let Some(fname) = target.file_name().and_then(|s| s.to_str()) else {
            return Ok(None);
        };
        let prefix = format!("{}-", self.name);
        let suffix = "-link";
        let gen_num = fname
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
            .and_then(|num| num.parse::<u64>().ok());
        Ok(gen_num)
    }

    /// Read the set of installed `StorePath`s in the currently active generation.
    pub fn installed_paths(&self) -> Result<BTreeSet<StorePath>> {
        let active = self.active_link();
        if !active.is_symlink() {
            return Ok(BTreeSet::new());
        }
        let manifest_path = active.join(MANIFEST_FILENAME);
        if !manifest_path.exists() {
            return Ok(BTreeSet::new());
        }
        let text = fs::read_to_string(&manifest_path)?;
        let manifest: ProfileManifest = serde_json::from_str(&text)?;
        let mut set = BTreeSet::new();
        for raw in manifest.paths {
            set.insert(parse_store_path(&raw)?);
        }
        Ok(set)
    }

    /// Install `path` into this profile, creating a new immutable `beryl-env` generation
    /// and atomically flipping the active profile symlink.
    ///
    /// Returns the new generation number.
    pub async fn install(&self, path: &StorePath) -> Result<u64> {
        if !self.store.has_path(path).await? {
            return Err(StoreError::NotFound(path.to_absolute_path()));
        }

        let mut paths = self.installed_paths()?;
        paths.insert(path.clone());

        let env_store_path = self.build_env_store_path(&paths).await?;

        let next_gen = self.list_generations()?.into_iter().max().unwrap_or(0) + 1;
        let gen_link = self.generation_link(next_gen);
        let env_real = self.store.real_path(&env_store_path);
        atomic_symlink(&env_real, &gen_link)?;

        let gen_link_name = gen_link
            .file_name()
            .expect("generation link has a file name");
        atomic_symlink(Path::new(gen_link_name), &self.active_link())?;

        info!(
            profile = %self.name,
            generation = next_gen,
            env = %env_store_path.to_absolute_path(),
            "switched profile generation"
        );
        Ok(next_gen)
    }

    /// Atomically roll back the active profile to the previous generation.
    ///
    /// Returns the generation number that is now active.
    pub fn rollback(&self) -> Result<u64> {
        let current = self
            .current_generation()?
            .ok_or_else(|| StoreError::NoPreviousGeneration(self.name.clone()))?;

        let gens = self.list_generations()?;
        let prev = gens
            .into_iter()
            .rev()
            .find(|&g| g < current)
            .ok_or_else(|| StoreError::NoPreviousGeneration(self.name.clone()))?;

        let target_name = format!("{}-{prev}-link", self.name);
        atomic_symlink(Path::new(&target_name), &self.active_link())?;

        info!(profile = %self.name, from = current, to = prev, "rolled back profile");
        Ok(prev)
    }

    /// Build a content-addressed `beryl-env` symlink farm and register it in `LocalStore`.
    async fn build_env_store_path(&self, paths: &BTreeSet<StorePath>) -> Result<StorePath> {
        let staging = tempfile::tempdir_in(self.store.tmp_dir())?;
        let env_dir = staging.path().join("beryl-env");
        fs::create_dir(&env_dir)?;

        // Write manifest listing installed store paths
        let manifest = ProfileManifest {
            paths: paths.iter().map(|p| p.to_absolute_path()).collect(),
        };
        fs::write(
            env_dir.join(MANIFEST_FILENAME),
            serde_json::to_string_pretty(&manifest)?,
        )?;

        // Merge standard top-level directories into the symlink farm
        for sp in paths {
            let real = self.store.real_path(sp);
            if real.is_dir() {
                merge_symlink_tree(&real, &env_dir)?;
            }
        }

        // Compute NAR hash + CA store path for `beryl-env`
        let hashed = hash_nar_path(&env_dir)?;
        let mut nar_bytes = Vec::with_capacity(hashed.size as usize);
        dump_nar(&env_dir, &mut nar_bytes)?;

        let refs_iter = paths.iter().map(|p| p.as_ref());
        let env_sp: StorePath = build_ca_path(
            "beryl-env",
            true,
            &NixHash::Sha256(hashed.sha256),
            refs_iter,
            false,
        )
        .map_err(|e| beryl_nix::NixCompatError::InvalidStorePath {
            path: "beryl-env".to_string(),
            reason: e.to_string(),
        })?
        .to_owned();

        let info = ValidPathInfo {
            path: env_sp.clone(),
            nar_hash: hashed.sha256,
            nar_size: hashed.size,
            references: paths.clone(),
            deriver: None,
            sigs: vec![],
            ca: Some(format!("fixed:r:sha256:{}", hex::encode(hashed.sha256))),
            registration_time: 0,
        };

        self.store.add_nar(info, Cursor::new(nar_bytes)).await?;
        Ok(env_sp)
    }
}

/// Recursively merge `src` directory into `dst` by creating symlinks for files and
/// recursing into shared subdirectories (e.g. `bin/`, `lib/`, `include/`, `share/`).
fn merge_symlink_tree(src: &Path, dst: &Path) -> Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == MANIFEST_FILENAME {
            continue;
        }
        let src_child = entry.path();
        let dst_child = dst.join(&name);

        let src_meta = fs::symlink_metadata(&src_child)?;
        if src_meta.is_dir() {
            if !dst_child.exists() && !dst_child.is_symlink() {
                fs::create_dir(&dst_child)?;
            }
            if dst_child.is_dir() && !dst_child.is_symlink() {
                merge_symlink_tree(&src_child, &dst_child)?;
            }
        } else if !dst_child.exists() && !dst_child.is_symlink() {
            symlink(&src_child, &dst_child)?;
        }
    }
    Ok(())
}

/// Atomically create or replace a symlink at `link_path` pointing to `target`.
fn atomic_symlink(target: &Path, link_path: &Path) -> Result<()> {
    let parent = link_path
        .parent()
        .expect("symlink path must have a parent directory");
    let unique = format!(
        ".{}.tmp.{}",
        link_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("link"),
        std::process::id()
    );
    let tmp = parent.join(unique);
    if tmp.exists() || tmp.is_symlink() {
        fs::remove_file(&tmp)?;
    }
    symlink(target, &tmp)?;
    fs::rename(&tmp, link_path)?;
    Ok(())
}
