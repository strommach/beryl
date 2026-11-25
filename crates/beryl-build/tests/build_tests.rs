use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::sync::Arc;

use beryl_build::{Builder, SandboxMode, scan_references};
use beryl_eval::{EvalError, Evaluator, NixpkgsLookup};
use beryl_nix::nixbase32;
use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::{LocalStore, Store};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct EmptyLookup;

impl NixpkgsLookup for EmptyLookup {
    fn resolve_nixpkgs(&self, _system: &str, attr: &str) -> beryl_eval::Result<StorePath> {
        Err(EvalError::Derivation(format!("unexpected nixpkgs lookup: {attr}")))
    }
}

async fn serve_single_file(body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });

    format!("http://{addr}/hello.txt")
}

#[test]
fn scanner_finds_refs_across_files_symlinks_and_chunk_boundaries() {
    let sp1 = parse_store_path("/nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-dep-one-1.0").unwrap();
    let sp2 = parse_store_path("/nix/store/11bgd045z0d4icpbc2yyz4gx48ak44la-dep-two-1.0").unwrap();
    let sp3 = parse_store_path("/nix/store/22bgd045z0d4icpbc2yyz4gx48ak44la-unused-1.0").unwrap();

    let dir = TempDir::new().unwrap();
    // Place sp1's 32-char hash right across the 64 KiB read chunk boundary
    let mut big = vec![b'A'; 65520];
    big.extend_from_slice(nixbase32::encode(sp1.digest()).as_bytes());
    big.extend_from_slice(&vec![b'B'; 1024]);
    fs::write(dir.path().join("binary.bin"), &big).unwrap();

    // Symlink referencing sp2
    symlink(sp2.to_absolute_path(), dir.path().join("liblink")).unwrap();

    let candidates = BTreeSet::from([sp1.clone(), sp2.clone(), sp3]);
    let found = scan_references(dir.path(), &candidates).unwrap();

    assert_eq!(found, BTreeSet::from([sp1, sp2]));
}

#[tokio::test]
async fn end_to_end_lua_fetch_url_and_build() {
    let payload = b"hello from beryl source\n".to_vec();
    let sha256_hex = hex::encode(Sha256::digest(&payload));
    let url = serve_single_file(payload.clone()).await;

    let pkg_lua = format!(
        r#"
return beryl.derivation({{
  name = "greet",
  version = "1.0",
  src = beryl.fetch_url({{
    url = "{url}",
    sha256 = "{sha256_hex}",
  }}),
  build = [[
    mkdir -p "$out/bin"
    cat "$src" > "$out/message.txt"
    printf '#!/bin/sh\ncat "%s"\n' "$src" > "$out/bin/greet"
    chmod +x "$out/bin/greet"
  ]],
}})
"#
    );

    let evaluator = Evaluator::new("aarch64-linux", Arc::new(EmptyLookup));
    let eval_res = evaluator.eval_str(&pkg_lua, "pkg.lua").expect("eval pkg.lua");

    let store_root = TempDir::new().unwrap();
    let store = LocalStore::open(store_root.path()).unwrap();

    let builder = Builder::new(store.clone(), None).with_sandbox(SandboxMode::Auto);
    let out_path = builder
        .realise_eval(&eval_res)
        .await
        .expect("realise_eval");

    assert_eq!(out_path, eval_res.root.out_path);
    assert!(store.has_path(&out_path).await.unwrap());

    let real_out = store.real_path(&out_path);
    assert_eq!(fs::read(real_out.join("message.txt")).unwrap(), payload);
    assert!(real_out.join("bin/greet").exists());

    // Reference scanner must have automatically detected the `$src` store path embedded in `bin/greet`!
    let info = store.query_path_info(&out_path).await.unwrap().unwrap();
    assert_eq!(info.references.len(), 1, "must record runtime reference to src");

    // Second realisation is an instant cache hit
    let out_path_2 = builder.realise_eval(&eval_res).await.unwrap();
    assert_eq!(out_path_2, out_path);
}
