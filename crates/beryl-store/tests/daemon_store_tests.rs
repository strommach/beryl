use std::path::Path;

use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::{DEFAULT_DAEMON_SOCKET, DaemonStore, Store};

#[tokio::test]
async fn daemon_store_live_handshake_and_query_if_available() {
    if !Path::new(DEFAULT_DAEMON_SOCKET).exists() {
        // Skip cleanly on systems without a running nix-daemon.
        return;
    }

    let daemon = DaemonStore::connect_default()
        .await
        .expect("connect to live nix-daemon");

    let (major, minor) = daemon.protocol_version().await;
    assert_eq!(major, 1);
    assert!(minor >= 33, "expected protocol >= 1.33, got {major}.{minor}");

    // Non-existent store path must return false / None
    let missing =
        parse_store_path("/nix/store/00000000000000000000000000000000-nonexistent-beryl").unwrap();
    assert!(!daemon.has_path(&missing).await.unwrap());
    assert!(daemon.query_path_info(&missing).await.unwrap().is_none());

    // Find a real store path in /nix/store (skipping `.drv` and `.lock` files)
    let real_entry = std::fs::read_dir("/nix/store")
        .ok()
        .and_then(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .find(|name| !name.ends_with(".drv") && !name.ends_with(".lock") && name.len() > 33)
                .and_then(|name| parse_store_path(&name).ok())
        });

    if let Some(sp) = real_entry
        && daemon.has_path(&sp).await.unwrap()
    {
        let info = daemon
            .query_path_info(&sp)
            .await
            .unwrap()
            .expect("valid path must have path info");
        assert_eq!(info.path, sp);
        assert!(info.nar_size > 0);

        let closure = daemon.query_closure(&sp).await.unwrap();
        assert!(closure.contains(&sp));
    }
}
