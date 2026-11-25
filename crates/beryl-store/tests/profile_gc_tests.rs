use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;

use beryl_nix::nar_fs::{dump_nar, hash_nar_path};
use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::{LocalStore, ProfileManager, Store, ValidPathInfo, collect_garbage};
use tempfile::TempDir;

async fn add_pkg(
    store: &LocalStore,
    sp_str: &str,
    bin_name: &str,
    refs: BTreeSet<StorePath>,
) -> StorePath {
    let tmp = TempDir::new().unwrap();
    let pkg = tmp.path().join("pkg");
    fs::create_dir_all(pkg.join("bin")).unwrap();
    let bin = pkg.join("bin").join(bin_name);
    fs::write(&bin, format!("#!/bin/sh\necho {bin_name}\n")).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();

    let mut nar = Vec::new();
    dump_nar(&pkg, &mut nar).unwrap();
    let hashed = hash_nar_path(&pkg).unwrap();

    let sp = parse_store_path(sp_str).unwrap();
    let info = ValidPathInfo {
        path: sp.clone(),
        nar_hash: hashed.sha256,
        nar_size: hashed.size,
        references: refs,
        deriver: None,
        sigs: vec![],
        ca: None,
        registration_time: 0,
    };
    store.add_nar(info, Cursor::new(nar)).await.unwrap();
    sp
}

#[tokio::test]
async fn profile_install_rollback_and_gc() {
    let store_root = TempDir::new().unwrap();
    let store = LocalStore::open(store_root.path()).unwrap();

    let pkg_a = add_pkg(
        &store,
        "/nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-tool-a-1.0",
        "tool-a",
        BTreeSet::new(),
    )
    .await;
    let pkg_b = add_pkg(
        &store,
        "/nix/store/11bgd045z0d4icpbc2yyz4gx48ak44la-tool-b-1.0",
        "tool-b",
        BTreeSet::new(),
    )
    .await;
    let orphan = add_pkg(
        &store,
        "/nix/store/22bgd045z0d4icpbc2yyz4gx48ak44la-orphan-1.0",
        "orphan",
        BTreeSet::new(),
    )
    .await;

    let profile = ProfileManager::new(store.clone(), "default").unwrap();
    assert_eq!(profile.current_generation().unwrap(), None);

    // Generation 1: install tool-a
    let gen1 = profile.install(&pkg_a).await.unwrap();
    assert_eq!(gen1, 1);
    assert!(profile.active_link().join("bin/tool-a").exists());
    assert!(!profile.active_link().join("bin/tool-b").exists());

    // Generation 2: install tool-b (both tool-a and tool-b present)
    let gen2 = profile.install(&pkg_b).await.unwrap();
    assert_eq!(gen2, 2);
    assert!(profile.active_link().join("bin/tool-a").exists());
    assert!(profile.active_link().join("bin/tool-b").exists());

    // Rollback to Generation 1
    let rolled = profile.rollback().unwrap();
    assert_eq!(rolled, 1);
    assert_eq!(profile.current_generation().unwrap(), Some(1));
    assert!(profile.active_link().join("bin/tool-a").exists());
    assert!(!profile.active_link().join("bin/tool-b").exists());

    // GC sweeps orphan, but keeps pkg_a, pkg_b, and both profile generations (referenced by -link)
    let report = collect_garbage(&store, &[]).await.unwrap();
    assert_eq!(report.deleted_paths, vec![orphan.clone()]);
    assert!(!store.has_path(&orphan).await.unwrap());
    assert!(!store.real_path(&orphan).exists());
    assert!(store.has_path(&pkg_a).await.unwrap());
    assert!(store.has_path(&pkg_b).await.unwrap());
}
