use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

use beryl_eval::{EvalError, Evaluator, NixpkgsLookup, Result};
use beryl_nix::derivation::UnverifiedDerivation;
use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::{LocalStore, Store};
use tempfile::TempDir;

struct MockLookup {
    map: HashMap<String, StorePath>,
}

impl NixpkgsLookup for MockLookup {
    fn resolve_nixpkgs(&self, _system: &str, attr: &str) -> Result<StorePath> {
        self.map
            .get(attr)
            .cloned()
            .ok_or_else(|| EvalError::Derivation(format!("unknown mock attr {attr}")))
    }
}

#[tokio::test]
async fn eval_readme_style_pkg_lua_and_write_drvs() {
    let openssl_sp =
        parse_store_path("/nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-openssl-3.5.9").unwrap();
    let zlib_sp =
        parse_store_path("/nix/store/11bgd045z0d4icpbc2yyz4gx48ak44la-zlib-1.3.1").unwrap();

    let mut map = HashMap::new();
    map.insert("openssl_3".to_string(), openssl_sp.clone());
    map.insert("zlib".to_string(), zlib_sp.clone());

    let evaluator = Evaluator::new("aarch64-linux", Arc::new(MockLookup { map }));

    let pkg_lua = r#"
local openssl = beryl.nixpkgs("openssl_3")
local zlib    = beryl.nixpkgs("zlib")

return beryl.derivation({
  name = "my-custom-curl",
  version = "8.7.1",

  src = beryl.fetch_url({
    url = "https://curl.se/download/curl-8.7.1.tar.xz",
    sha256 = "05a89d6b3b283f22b4d78b915f22a00000000000000000000000000000000000",
  }),

  inputs = { openssl, zlib },

  build = [[
    tar -xf $src --strip-components=1
    ./configure --prefix=$out --with-openssl=${openssl} --with-zlib=${zlib}
    make -j$BERYL_BUILD_CORES
    make install
  ]],
})
"#;

    let res1 = evaluator
        .eval_str(pkg_lua, "pkg.lua")
        .expect("eval pkg.lua");
    let res2 = evaluator
        .eval_str(pkg_lua, "pkg.lua")
        .expect("eval pkg.lua second time");

    // 100% deterministic output & drv paths
    assert_eq!(res1.root, res2.root);
    assert_eq!(res1.derivations.len(), 2); // fetch_url FOD + my-custom-curl

    let root_drv_path = res1.root.drv_path.as_ref().expect("root has drv_path");
    assert!(
        root_drv_path
            .to_absolute_path()
            .ends_with("-my-custom-curl-8.7.1.drv")
    );
    assert!(
        res1.root
            .out_path
            .to_absolute_path()
            .ends_with("-my-custom-curl-8.7.1")
    );

    let root_drv = &res1.derivations[root_drv_path];
    assert_eq!(root_drv.input_derivations().len(), 1);
    assert!(root_drv.input_sources().contains(&openssl_sp));
    assert!(root_drv.input_sources().contains(&zlib_sp));
    assert_eq!(
        root_drv.environment().get(b"openssl".as_slice()).unwrap(),
        openssl_sp.to_absolute_path().as_bytes()
    );
    assert_eq!(
        root_drv.environment().get(b"zlib".as_slice()).unwrap(),
        zlib_sp.to_absolute_path().as_bytes()
    );

    // ATerm bytes parse back cleanly via nix-rs-compat
    let aterm = root_drv.to_aterm_bytes();
    let reparsed =
        UnverifiedDerivation::from_aterm_bytes(&aterm).expect("reparse emitted ATerm .drv");
    assert_eq!(reparsed.to_aterm_bytes(), aterm);

    // Write both .drv files into a LocalStore
    let store_root = TempDir::new().unwrap();
    let store = LocalStore::open(store_root.path()).unwrap();
    let written = res1
        .write_drvs_to_store(&store)
        .await
        .expect("write drvs to store");
    assert_eq!(written.len(), 2);
    assert!(store.has_path(root_drv_path).await.unwrap());
    assert_eq!(fs::read(store.real_path(root_drv_path)).unwrap(), aterm);
}
