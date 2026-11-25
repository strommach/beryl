use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;

use beryl_nix::nar_fs::{dump_nar, hash_nar_path};
use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::{LocalStore, Store, StoreError, ValidPathInfo};
use tempfile::TempDir;

fn make_test_nar(content: &[u8]) -> (Vec<u8>, [u8; 32], u64) {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("pkg");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("hello.txt"), content).unwrap();

    let mut nar_bytes = Vec::new();
    dump_nar(&dir, &mut nar_bytes).unwrap();
    let hashed = hash_nar_path(&dir).unwrap();
    (nar_bytes, hashed.sha256, hashed.size)
}

#[tokio::test]
async fn local_store_add_query_and_closure() {
    let store_root = TempDir::new().unwrap();
    let store = LocalStore::open(store_root.path()).expect("open local store");

    let dep_path =
        parse_store_path("/nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-dep-1.0").unwrap();
    let app_path =
        parse_store_path("/nix/store/11bgd045z0d4icpbc2yyz4gx48ak44la-app-1.0").unwrap();

    let (dep_nar, dep_hash, dep_size) = make_test_nar(b"dep library");
    let dep_info = ValidPathInfo {
        path: dep_path.clone(),
        nar_hash: dep_hash,
        nar_size: dep_size,
        references: BTreeSet::from([dep_path.clone()]), // self-reference
        deriver: None,
        sigs: vec!["cache.nixos.org-1:dummy".to_string()],
        ca: None,
        registration_time: 1_700_000_000,
    };

    assert!(!store.has_path(&dep_path).await.unwrap());
    store
        .add_nar(dep_info.clone(), Cursor::new(dep_nar))
        .await
        .expect("add dep nar");
    assert!(store.has_path(&dep_path).await.unwrap());

    // Verify unpacked file exists and is read-only
    let real_dep = store.real_path(&dep_path);
    assert_eq!(fs::read(real_dep.join("hello.txt")).unwrap(), b"dep library");
    let mode = fs::metadata(real_dep.join("hello.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o222, 0, "store file must be read-only");

    // Add app referencing dep
    let (app_nar, app_hash, app_size) = make_test_nar(b"app binary");
    let app_info = ValidPathInfo {
        path: app_path.clone(),
        nar_hash: app_hash,
        nar_size: app_size,
        references: BTreeSet::from([dep_path.clone()]),
        deriver: None,
        sigs: vec![],
        ca: None,
        registration_time: 1_700_000_001,
    };
    store
        .add_nar(app_info.clone(), Cursor::new(app_nar))
        .await
        .expect("add app nar");

    // Query path info roundtrip
    let queried = store
        .query_path_info(&app_path)
        .await
        .unwrap()
        .expect("app info present");
    assert_eq!(queried, app_info);

    // Closure of app includes both app and dep
    let closure = store.query_closure(&app_path).await.unwrap();
    assert_eq!(closure, BTreeSet::from([dep_path, app_path]));
}

#[tokio::test]
async fn local_store_rejects_hash_mismatch() {
    let store_root = TempDir::new().unwrap();
    let store = LocalStore::open(store_root.path()).expect("open local store");

    let path =
        parse_store_path("/nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-corrupt-1.0").unwrap();
    let (nar_bytes, mut hash, size) = make_test_nar(b"good data");
    hash[0] ^= 0xff; // corrupt expected hash

    let info = ValidPathInfo {
        path: path.clone(),
        nar_hash: hash,
        nar_size: size,
        references: BTreeSet::new(),
        deriver: None,
        sigs: vec![],
        ca: None,
        registration_time: 0,
    };

    let err = store
        .add_nar(info, Cursor::new(nar_bytes))
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::HashMismatch { .. }));
    assert!(!store.has_path(&path).await.unwrap());
    assert!(!store.real_path(&path).exists());
}
