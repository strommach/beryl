use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::sync::Arc;

use beryl_nix::nar_fs::{dump_nar, hash_nar_path};
use beryl_nix::narinfo::parse_keypair;
use beryl_nix::narinfo_ext::parse_narinfo;
use beryl_nix::nixbase32;
use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::{BinaryCacheClient, BinaryCacheConfig, LocalStore, Store, StoreError};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TEST_SECRET_KEY: &str = "beryl-test-1:R4/XHi9oGG2d42jPuUH6YG92wYBhNRZFFGs3Yzdp724F/EQJ2Opc8CzPz3Y2uRDs+/gCxNFD4kqeVsIWli3goA==";

fn build_nar(content: &[u8]) -> (Vec<u8>, [u8; 32], u64) {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("pkg");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("data.txt"), content).unwrap();

    let mut nar = Vec::new();
    dump_nar(&dir, &mut nar).unwrap();
    let hashed = hash_nar_path(&dir).unwrap();
    (nar, hashed.sha256, hashed.size)
}

async fn spawn_mock_cache(routes: HashMap<String, Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let routes = Arc::new(routes);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let routes = Arc::clone(&routes);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let Ok(n) = socket.read(&mut buf).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");

                if let Some(body) = routes.get(path) {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(body).await;
                } else {
                    let _ = socket
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                }
            });
        }
    });

    format!("http://{addr}")
}

#[tokio::test]
async fn fetch_closure_verifies_signatures_and_decompresses_xz_and_zstd() {
    let (signing_key, verifying_key) = parse_keypair(TEST_SECRET_KEY).unwrap();

    let dep_path =
        parse_store_path("/nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-libdep-1.0").unwrap();
    let app_path =
        parse_store_path("/nix/store/11bgd045z0d4icpbc2yyz4gx48ak44la-myapp-1.0").unwrap();

    // 1. Compress dep with xz
    let (dep_nar, dep_hash, dep_size) = build_nar(b"dep payload");
    let mut dep_xz = Vec::new();
    {
        let mut enc = xz2::write::XzEncoder::new(&mut dep_xz, 1);
        enc.write_all(&dep_nar).unwrap();
        enc.finish().unwrap();
    }
    let dep_file_hash: [u8; 32] = Sha256::digest(&dep_xz).into();
    let dep_hash_b32 = nixbase32::encode(dep_path.digest());
    let dep_narinfo_unsigned = format!(
        "StorePath: {}\nURL: nar/dep.nar.xz\nCompression: xz\nFileHash: sha256:{}\nFileSize: {}\nNarHash: sha256:{}\nNarSize: {}\nReferences: {}\n",
        dep_path.to_absolute_path(),
        nixbase32::encode(&dep_file_hash),
        dep_xz.len(),
        nixbase32::encode(&dep_hash),
        dep_size,
        dep_path,
    );
    let mut dep_ni = parse_narinfo(&dep_narinfo_unsigned).unwrap();
    dep_ni.add_signature(&signing_key);

    // 2. Compress app with zstd (referencing dep)
    let (app_nar, app_hash, app_size) = build_nar(b"app payload");
    let app_zst = zstd::stream::encode_all(app_nar.as_slice(), 1).unwrap();
    let app_file_hash: [u8; 32] = Sha256::digest(&app_zst).into();
    let app_hash_b32 = nixbase32::encode(app_path.digest());
    let app_narinfo_unsigned = format!(
        "StorePath: {}\nURL: nar/app.nar.zst\nCompression: zstd\nFileHash: sha256:{}\nFileSize: {}\nNarHash: sha256:{}\nNarSize: {}\nReferences: {}\n",
        app_path.to_absolute_path(),
        nixbase32::encode(&app_file_hash),
        app_zst.len(),
        nixbase32::encode(&app_hash),
        app_size,
        dep_path,
    );
    let mut app_ni = parse_narinfo(&app_narinfo_unsigned).unwrap();
    app_ni.add_signature(&signing_key);

    let mut routes = HashMap::new();
    routes.insert(format!("/{dep_hash_b32}.narinfo"), dep_ni.to_string().into_bytes());
    routes.insert("/nar/dep.nar.xz".to_string(), dep_xz);
    routes.insert(format!("/{app_hash_b32}.narinfo"), app_ni.to_string().into_bytes());
    routes.insert("/nar/app.nar.zst".to_string(), app_zst);

    let base_url = spawn_mock_cache(routes).await;

    // Untrusted client (default cache.nixos.org key) must reject our test signature
    let untrusted_store_root = TempDir::new().unwrap();
    let untrusted_store = LocalStore::open(untrusted_store_root.path()).unwrap();
    let untrusted_client = BinaryCacheClient::new(BinaryCacheConfig {
        base_url: base_url.clone(),
        ..BinaryCacheConfig::default()
    })
    .unwrap();
    let err = untrusted_client
        .fetch_closure(&untrusted_store, &app_path)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::InvalidSignature(_)));

    // Trusted client fetches dep first, then app
    let store_root = TempDir::new().unwrap();
    let store = LocalStore::open(store_root.path()).unwrap();
    let client = BinaryCacheClient::new(BinaryCacheConfig {
        base_url,
        trusted_keys: vec![verifying_key],
        verify_signatures: true,
    })
    .unwrap();

    let installed = client
        .fetch_closure(&store, &app_path)
        .await
        .expect("fetch closure");
    assert_eq!(installed, vec![dep_path.clone(), app_path.clone()]);

    assert_eq!(
        fs::read(store.real_path(&dep_path).join("data.txt")).unwrap(),
        b"dep payload"
    );
    assert_eq!(
        fs::read(store.real_path(&app_path).join("data.txt")).unwrap(),
        b"app payload"
    );
    assert_eq!(
        store.query_closure(&app_path).await.unwrap(),
        BTreeSet::from([dep_path, app_path.clone()])
    );

    // Second fetch is a no-op
    let second = client.fetch_closure(&store, &app_path).await.unwrap();
    assert!(second.is_empty());
}
