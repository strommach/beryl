//! Build evaluated `pkg.lua` derivation DAGs natively through a running `nix-daemon`.

use std::collections::{BTreeMap, BTreeSet};

use beryl_eval::EvalResult;
use beryl_nix::derivation::Derivation;
use beryl_nix::store_path::StorePath;
use beryl_store::{DaemonStore, Store};
use tracing::{debug, info};

use crate::error::{BuildError, Result};

/// Realize an `EvalResult` DAG via `nix-daemon`:
/// 1. Ensures all prebuilt `input_sources` exist in `/nix/store` (`EnsurePath`).
/// 2. Registers every `.drv` ATerm in topological order (`AddTextToStore`).
/// 3. Triggers a sandboxed build of the root derivation (`BuildPaths`).
pub async fn realise_eval_via_daemon(
    daemon: &DaemonStore,
    eval: &EvalResult,
) -> Result<StorePath> {
    // 1. Ensure prebuilt `nixpkgs` inputs are present in `/nix/store`.
    for src in &eval.input_sources {
        if !daemon.has_path(src).await? {
            info!(path = %src.to_absolute_path(), "ensuring input source via nix-daemon");
            daemon.ensure_path(src).await?;
        }
    }

    // 2. Register all `.drv` ATerm files in topological order.
    let ordered = topo_sort_drvs(&eval.derivations);
    for drv_path in ordered {
        let drv = &eval.derivations[&drv_path];
        let aterm_bytes = drv.to_aterm_bytes();
        let aterm_str = String::from_utf8_lossy(&aterm_bytes);

        let mut refs = BTreeSet::new();
        for in_drv in drv.input_derivations().keys() {
            refs.insert(in_drv.clone());
        }
        for in_src in drv.input_sources() {
            refs.insert(in_src.clone());
        }

        let registered = daemon
            .add_text_to_store(drv_path.name(), &aterm_str, &refs)
            .await?;
        debug!(
            expected = %drv_path.to_absolute_path(),
            registered = %registered.to_absolute_path(),
            "registered .drv with nix-daemon"
        );
    }

    // 3. Build root derivation via `BuildPaths` (`<drv_path>!out`).
    let root_drv = eval
        .root
        .drv_path
        .as_ref()
        .ok_or_else(|| BuildError::UnsupportedBuiltin("root has no drv_path".to_string()))?;

    let derived = format!("{}!out", root_drv.to_absolute_path());
    info!(%derived, "building derivation via nix-daemon");
    daemon.build_paths(&[derived]).await?;

    Ok(eval.root.out_path.clone())
}

fn topo_sort_drvs(drvs: &BTreeMap<StorePath, Derivation>) -> Vec<StorePath> {
    let mut visited = BTreeSet::new();
    let mut order = Vec::with_capacity(drvs.len());

    fn dfs(
        path: &StorePath,
        drvs: &BTreeMap<StorePath, Derivation>,
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
