//! Mark-and-sweep garbage collector for `LocalStore`.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;
use tracing::{debug, info};

use crate::error::Result;
use crate::local::LocalStore;
use crate::store::Store;

/// Summary of a garbage collection run.
#[derive(Debug, Clone, Default)]
pub struct GcReport {
    pub deleted_paths: Vec<StorePath>,
    pub freed_bytes: u64,
    pub live_paths: usize,
}

/// Discover GC roots from `<root>/var/nix/profiles/*-link` and `<root>/var/nix/gcroots/*`.
pub fn collect_gc_roots(store: &LocalStore) -> Result<BTreeSet<StorePath>> {
    let mut roots = BTreeSet::new();
    let dirs = [
        store.root().join("var/nix/profiles"),
        store.root().join("var/nix/gcroots"),
    ];

    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_symlink() {
                continue;
            }
            if let Ok(resolved) = fs::canonicalize(&path)
                && let Some(sp) = extract_store_path_from_real(store, &resolved)
            {
                debug!(root = %sp.to_absolute_path(), link = %path.display(), "found GC root");
                roots.insert(sp);
            }
        }
    }

    Ok(roots)
}

fn extract_store_path_from_real(store: &LocalStore, resolved: &Path) -> Option<StorePath> {
    let store_dir = fs::canonicalize(store.store_dir()).ok()?;
    let rel = resolved.strip_prefix(&store_dir).ok()?;
    let first_component = rel.components().next()?;
    let name = first_component.as_os_str().to_str()?;
    parse_store_path(name).ok()
}

/// Run mark-and-sweep garbage collection on `store`, keeping all paths reachable from
/// profile/GC roots plus `extra_roots`.
pub async fn collect_garbage(store: &LocalStore, extra_roots: &[StorePath]) -> Result<GcReport> {
    let mut roots = collect_gc_roots(store)?;
    roots.extend(extra_roots.iter().cloned());

    // Mark phase: compute transitive closure of all roots
    let mut live = BTreeSet::new();
    for root in &roots {
        if store.has_path(root).await? {
            let closure = store.query_closure(root).await?;
            live.extend(closure);
        }
    }

    // Sweep phase: find all valid paths not in `live`
    let all_paths = store.list_valid_paths()?;
    let dead: Vec<StorePath> = all_paths
        .into_iter()
        .filter(|p| !live.contains(p))
        .collect();

    let (deleted_paths, freed_bytes) = store.delete_dead_paths(&dead)?;
    info!(
        deleted = deleted_paths.len(),
        freed_bytes,
        live = live.len(),
        "garbage collection complete"
    );

    Ok(GcReport {
        deleted_paths,
        freed_bytes,
        live_paths: live.len(),
    })
}
