use std::collections::HashMap;
use std::sync::Arc;

use beryl_index::{
    HydraConfig, Lockfile, NixpkgsResolver, ResolutionSource, flake_ref_for_pin,
    hydra_job_for_channel,
};
use beryl_nix::narinfo::parse_keypair;
use beryl_nix::narinfo_ext::parse_narinfo;
use beryl_nix::nixbase32;
use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::{BinaryCacheClient, BinaryCacheConfig};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TEST_SECRET_KEY: &str = "beryl-test-1:R4/XHi9oGG2d42jPuUH6YG92wYBhNRZFFGs3Yzdp724F/EQJ2Opc8CzPz3Y2uRDs+/gCxNFD4kqeVsIWli3goA==";

async fn spawn_mock_http(routes: HashMap<String, Vec<u8>>) -> String {
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
                        .write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                }
            });
        }
    });

    format!("http://{addr}")
}

#[test]
fn lockfile_roundtrip() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("beryl.lock");

    let mut lf = Lockfile::load_or_default(&path).unwrap();
    let sp = parse_store_path("/nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-ripgrep-14.1.1").unwrap();
    lf.insert("aarch64-linux", "ripgrep", &sp, ResolutionSource::Hydra);
    lf.save(&path).unwrap();

    let reloaded = Lockfile::load_or_default(&path).unwrap();
    assert_eq!(reloaded.get("aarch64-linux", "ripgrep").unwrap(), Some(sp));
}

#[tokio::test]
async fn resolver_uses_lockfile_first_then_hydra_with_cache_verification() {
    let (signing_key, verifying_key) = parse_keypair(TEST_SECRET_KEY).unwrap();
    let rg_path =
        parse_store_path("/nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-ripgrep-14.1.1").unwrap();
    let rg_hash = nixbase32::encode(rg_path.digest());

    let narinfo_unsigned = format!(
        "StorePath: {}\nURL: nar/rg.nar.xz\nCompression: xz\nFileHash: sha256:1094wph9z4nwlgvsd53abfz8i117ykiv5dwnq9nnhz846s7xqd7d\nFileSize: 100\nNarHash: sha256:1094wph9z4nwlgvsd53abfz8i117ykiv5dwnq9nnhz846s7xqd7d\nNarSize: 200\nReferences: \n",
        rg_path.to_absolute_path()
    );
    let mut ni = parse_narinfo(&narinfo_unsigned).unwrap();
    ni.add_signature(&signing_key);

    let hydra_json = format!(
        r#"{{"buildoutputs":{{"out":{{"path":"{}"}}}}}}"#,
        rg_path.to_absolute_path()
    );

    let mut routes = HashMap::new();
    routes.insert(
        "/job/nixpkgs/trunk/ripgrep.aarch64-linux/latest".to_string(),
        hydra_json.into_bytes(),
    );
    routes.insert(format!("/{rg_hash}.narinfo"), ni.to_string().into_bytes());

    let mock_url = spawn_mock_http(routes).await;

    let cache = BinaryCacheClient::new(BinaryCacheConfig {
        base_url: mock_url.clone(),
        trusted_keys: vec![verifying_key],
        verify_signatures: true,
    })
    .unwrap();
    let hydra = HydraConfig {
        base_url: mock_url,
        project: Some("nixpkgs".to_string()),
        jobset: Some("trunk".to_string()),
    };

    let resolver = NixpkgsResolver::new(hydra, cache)
        .unwrap()
        .with_nix_eval(false);

    let mut lockfile = Lockfile::default();

    // Tier 2: Hydra + cache verification populates lockfile
    let resolved = resolver
        .resolve(&mut lockfile, "aarch64-linux", "ripgrep")
        .await
        .expect("resolve via Hydra");
    assert_eq!(resolved, rg_path);
    assert_eq!(
        lockfile.packages["aarch64-linux"]["ripgrep"].source,
        ResolutionSource::Hydra
    );

    // Tier 1: Subsequent resolution hits lockfile directly (even with broken Hydra URL)
    let offline_cache = BinaryCacheClient::new(BinaryCacheConfig {
        base_url: "http://127.0.0.1:1".to_string(),
        ..BinaryCacheConfig::default()
    })
    .unwrap();
    let offline_resolver = NixpkgsResolver::new(
        HydraConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            ..HydraConfig::default()
        },
        offline_cache,
    )
    .unwrap()
    .with_nix_eval(false);

    let pinned = offline_resolver
        .resolve(&mut lockfile, "aarch64-linux", "ripgrep")
        .await
        .expect("resolve offline from lockfile");
    assert_eq!(pinned, rg_path);
}

#[test]
fn channel_mapping_supports_unstable_stable_and_custom_rev() {
    assert_eq!(
        hydra_job_for_channel("nixos-unstable", "ripgrep", "aarch64-linux"),
        (
            "nixpkgs".to_string(),
            "trunk".to_string(),
            "ripgrep.aarch64-linux".to_string()
        )
    );
    assert_eq!(
        hydra_job_for_channel("nixos-25.05", "ripgrep", "x86_64-linux"),
        (
            "nixos".to_string(),
            "release-25.05".to_string(),
            "nixpkgs.ripgrep.x86_64-linux".to_string()
        )
    );
    assert_eq!(
        hydra_job_for_channel("nixpkgs-25.05-darwin", "ripgrep", "aarch64-darwin"),
        (
            "nixpkgs".to_string(),
            "nixpkgs-25.05-darwin".to_string(),
            "ripgrep.aarch64-darwin".to_string()
        )
    );

    assert_eq!(
        flake_ref_for_pin("nixos-25.05", None),
        "github:NixOS/nixpkgs/nixos-25.05"
    );
    assert_eq!(
        flake_ref_for_pin("nixos-25.05", Some("deadbeef")),
        "github:NixOS/nixpkgs/deadbeef"
    );
}
