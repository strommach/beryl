//! Sandboxed derivation builder orchestrating DAG realization, environment isolation,
//! Linux namespace sandboxing (when available), and post-build reference scanning.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use beryl_eval::EvalResult;
use beryl_nix::derivation::{Derivation, OutputName};
use beryl_nix::nar_fs::{dump_nar, hash_nar_path};
use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::{BinaryCacheClient, LocalStore, Store, ValidPathInfo};
use tracing::{debug, info};

use crate::error::{BuildError, Result};
use crate::fetchurl::run_builtin_fetchurl;
use crate::scanner::scan_references;

/// Sandboxing policy for local derivation builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SandboxMode {
    /// Use Linux user+network namespaces when permitted by the kernel; fall back cleanly
    /// to isolated temp-dir + stripped-env execution in restricted containers or non-Linux hosts.
    #[default]
    Auto,
    /// Disable namespace isolation (still uses stripped env and isolated build directories).
    Disabled,
}

/// Realizes evaluated derivation DAGs into a `LocalStore`.
#[derive(Clone)]
pub struct Builder {
    store: LocalStore,
    cache: Option<BinaryCacheClient>,
    sandbox: SandboxMode,
}

impl Builder {
    pub fn new(store: LocalStore, cache: Option<BinaryCacheClient>) -> Self {
        Self {
            store,
            cache,
            sandbox: SandboxMode::Auto,
        }
    }

    pub fn with_sandbox(mut self, mode: SandboxMode) -> Self {
        self.sandbox = mode;
        self
    }

    /// Realize an entire `EvalResult` DAG into `self.store`, fetching any missing prebuilt
    /// `nixpkgs` inputs from `self.cache`, writing `.drv` files, executing FODs and builds
    /// in topological order, and returning the root output `StorePath`.
    pub async fn realise_eval(&self, eval: &EvalResult) -> Result<StorePath> {
        // 1. Ensure all prebuilt `input_sources` are present in the store (fetch from cache if needed).
        if let Some(ref cache) = self.cache {
            for src in &eval.input_sources {
                if !self.store.has_path(src).await? {
                    cache.fetch_closure(&self.store, src).await?;
                }
            }
        }

        // 2. Persist `.drv` files into the store.
        eval.write_drvs_to_store(&self.store).await?;

        // 3. Build each derivation in topological order.
        let ordered = topo_sort_drvs(&eval.derivations);
        for drv_path in ordered {
            let drv = &eval.derivations[&drv_path];
            self.build_derivation(drv, &eval.derivations).await?;
        }

        Ok(eval.root.out_path.clone())
    }

    /// Build a single `Derivation` assuming its `input_derivations` and `input_sources`
    /// are already realized in `self.store`.
    pub async fn build_derivation(
        &self,
        drv: &Derivation,
        all_drvs: &std::collections::BTreeMap<StorePath, Derivation>,
    ) -> Result<StorePath> {
        let out_path = drv
            .outputs()
            .get(&OutputName::out())
            .expect("derivation has `out` output")
            .clone();

        // Fast path: output already valid in store.
        if self.store.has_path(&out_path).await? {
            debug!(out = %out_path.to_absolute_path(), "build cache hit");
            return Ok(out_path);
        }

        // Fixed-output builtin:fetchurl
        if drv.outputs().is_fixed() {
            let cmd = drv.command();
            if cmd == "builtin:fetchurl" {
                run_builtin_fetchurl(&self.store, drv).await?;
                return Ok(out_path);
            }
            return Err(BuildError::UnsupportedBuiltin(cmd.to_string()));
        }

        // Collect all direct input store paths (both input_sources and input_derivations' outputs).
        let mut direct_inputs = BTreeSet::new();
        for src in drv.input_sources() {
            direct_inputs.insert(src.clone());
        }
        for (in_drv_path, out_names) in drv.input_derivations() {
            if let Some(in_drv) = all_drvs.get(in_drv_path) {
                for name in out_names {
                    if let Some(p) = in_drv.outputs().get(name) {
                        direct_inputs.insert(p.clone());
                    }
                }
            }
        }

        // Candidate references = transitive closure of all direct inputs + self.
        let mut allowed_refs = BTreeSet::new();
        allowed_refs.insert(out_path.clone());
        for inp in &direct_inputs {
            if self.store.has_path(inp).await? {
                let closure = self.store.query_closure(inp).await?;
                allowed_refs.extend(closure);
            }
        }

        // Execute builder in an isolated directory on a blocking thread.
        let store = self.store.clone();
        let drv_clone = drv.clone();
        let out_path_clone = out_path.clone();
        let direct_inputs_vec: Vec<StorePath> = direct_inputs.into_iter().collect();
        let sandbox = self.sandbox;

        let artifact = tokio::task::spawn_blocking(move || {
            execute_build_sync(
                &store,
                &drv_clone,
                &out_path_clone,
                &direct_inputs_vec,
                &allowed_refs,
                sandbox,
            )
        })
        .await
        .map_err(|e| BuildError::Join(e.to_string()))??;

        let info = ValidPathInfo {
            path: out_path.clone(),
            nar_hash: artifact.nar_hash,
            nar_size: artifact.nar_size,
            references: artifact.references,
            deriver: Some(drv.drv_path().clone()),
            sigs: vec![],
            ca: None,
            registration_time: 0,
        };

        self.store
            .add_nar(info, Cursor::new(artifact.nar_bytes))
            .await?;
        info!(out = %out_path.to_absolute_path(), "built and registered store path");
        Ok(out_path)
    }
}

struct BuiltArtifact {
    nar_bytes: Vec<u8>,
    nar_hash: [u8; 32],
    nar_size: u64,
    references: BTreeSet<StorePath>,
}

fn execute_build_sync(
    store: &LocalStore,
    drv: &Derivation,
    out_path: &StorePath,
    direct_inputs: &[StorePath],
    allowed_refs: &BTreeSet<StorePath>,
    sandbox: SandboxMode,
) -> Result<BuiltArtifact> {
    let build_tmp = tempfile::tempdir_in(store.tmp_dir())?;
    let work_dir = build_tmp.path().join("build");
    std::fs::create_dir(&work_dir)?;

    let out_staging = build_tmp.path().join("out");

    let builder_cmd = drv.command().to_string();
    let args: Vec<String> = drv
        .arguments()
        .iter()
        .map(|a| String::from_utf8_lossy(a.as_slice()).to_string())
        .collect();

    // Map logical `/nix/store/...` paths in env values to actual `store.real_path(...)`
    // when running with a non-`/nix` standalone local store root.
    let mut cmd = Command::new(&builder_cmd);
    cmd.args(&args).current_dir(&work_dir).env_clear();

    for (k, v) in drv.environment() {
        let key = String::from_utf8_lossy(k.as_slice()).to_string();
        if key == "out" {
            continue;
        }
        let val = String::from_utf8_lossy(v.as_slice()).to_string();
        let mapped = remap_store_path_if_local(store, &val);
        cmd.env(&key, mapped);
    }

    // Standard build environment variables
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    cmd.env("out", &out_staging);
    cmd.env("HOME", &work_dir);
    cmd.env("TMPDIR", &work_dir);
    cmd.env("NIX_BUILD_TOP", &work_dir);
    cmd.env("BERYL_BUILD_CORES", cores.to_string());

    // Build PATH / CPATH / LIBRARY_PATH / PKG_CONFIG_PATH / LDFLAGS from direct inputs
    let mut bin_dirs = Vec::new();
    let mut include_dirs = Vec::new();
    let mut lib_dirs = Vec::new();
    let mut pkgconfig_dirs = Vec::new();

    for inp in direct_inputs {
        let real = store.real_path(inp);
        let bin = real.join("bin");
        if bin.is_dir() {
            bin_dirs.push(bin);
        }
        let inc = real.join("include");
        if inc.is_dir() {
            include_dirs.push(inc);
        }
        let lib = real.join("lib");
        if lib.is_dir() {
            let pc = lib.join("pkgconfig");
            if pc.is_dir() {
                pkgconfig_dirs.push(pc);
            }
            lib_dirs.push(lib);
        }
    }

    // Preserve minimal system PATH only when builder is `/bin/sh` and inputs don't supply coreutils
    if bin_dirs.is_empty() {
        bin_dirs.push(PathBuf::from("/usr/bin"));
        bin_dirs.push(PathBuf::from("/bin"));
    }
    cmd.env("PATH", join_paths(&bin_dirs));
    if !include_dirs.is_empty() {
        cmd.env("CPATH", join_paths(&include_dirs));
    }
    if !lib_dirs.is_empty() {
        cmd.env("LIBRARY_PATH", join_paths(&lib_dirs));
        let ldflags = lib_dirs
            .iter()
            .map(|p| format!("-L{}", p.display()))
            .collect::<Vec<_>>()
            .join(" ");
        cmd.env("LDFLAGS", ldflags);
    }
    if !pkgconfig_dirs.is_empty() {
        cmd.env("PKG_CONFIG_PATH", join_paths(&pkgconfig_dirs));
    }

    #[cfg(target_os = "linux")]
    if sandbox == SandboxMode::Auto {
        configure_linux_sandbox(&mut cmd);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = sandbox;

    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(BuildError::BuilderFailed {
            drv: drv.drv_path().to_absolute_path(),
            code: output.status.code(),
            stderr,
        });
    }

    if !out_staging.exists() && !out_staging.is_symlink() {
        return Err(BuildError::OutputNotCreated {
            drv: drv.drv_path().to_absolute_path(),
            out: out_path.to_absolute_path(),
        });
    }

    // Post-build reference scan
    let references = scan_references(&out_staging, allowed_refs)?;

    // Dump & hash NAR for registration in LocalStore
    let hashed = hash_nar_path(&out_staging)?;
    let mut nar_bytes = Vec::with_capacity(hashed.size as usize);
    dump_nar(&out_staging, &mut nar_bytes)?;

    Ok(BuiltArtifact {
        nar_bytes,
        nar_hash: hashed.sha256,
        nar_size: hashed.size,
        references,
    })
}

#[cfg(target_os = "linux")]
fn configure_linux_sandbox(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;

    // Best-effort network + user namespace isolation in child process before exec.
    // Must stay strictly async-signal-safe (single raw `unshare` syscall, no allocations/logging).
    unsafe {
        cmd.pre_exec(|| {
            use nix::sched::{CloneFlags, unshare};
            let _ = unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNET);
            Ok(())
        });
    }
}

fn remap_store_path_if_local(store: &LocalStore, val: &str) -> String {
    if store.root() == Path::new("/nix") {
        return val.to_string();
    }
    if val.starts_with("/nix/store/")
        && let Ok(sp) = parse_store_path(val)
    {
        return store.real_path(&sp).display().to_string();
    }
    val.to_string()
}

fn join_paths(dirs: &[PathBuf]) -> OsString {
    std::env::join_paths(dirs).unwrap_or_default()
}

fn topo_sort_drvs(
    drvs: &std::collections::BTreeMap<StorePath, Derivation>,
) -> Vec<StorePath> {
    let mut visited = BTreeSet::new();
    let mut order = Vec::with_capacity(drvs.len());

    fn dfs(
        path: &StorePath,
        drvs: &std::collections::BTreeMap<StorePath, Derivation>,
        visited: &mut BTreeSet<StorePath>,
        order: &mut Vec<StorePath>,
    ) {
        if !visited.insert(path.clone()) {
            return;
        }
        if let Some(drv) = drvs.get(path) {
            for dep in drv.input_derivations().keys() {
                if drvs.contains_key(dep) {
                    dfs(dep, drvs, visited, order);
                }
            }
        }
        order.push(path.clone());
    }

    for path in drvs.keys() {
        dfs(path, drvs, &mut visited, &mut order);
    }
    order
}
