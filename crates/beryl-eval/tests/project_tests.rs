use std::collections::HashMap;
use std::sync::Arc;

use beryl_eval::{EvalError, Evaluator, NixpkgsLookup, Result};
use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;

struct MockLookup(HashMap<String, StorePath>);

impl NixpkgsLookup for MockLookup {
    fn resolve_nixpkgs(&self, _system: &str, attr: &str) -> Result<StorePath> {
        self.0
            .get(attr)
            .cloned()
            .ok_or_else(|| EvalError::Derivation(format!("unknown attr {attr}")))
    }
}

#[test]
fn eval_beryl_lua_project_manifest_with_shells_and_env() {
    let rustc = parse_store_path("/nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-rustc-1.99.1").unwrap();
    let sqlite = parse_store_path("/nix/store/11bgd045z0d4icpbc2yyz4gx48ak44la-sqlite-3.54.0").unwrap();
    let libclang =
        parse_store_path("/nix/store/22bgd045z0d4icpbc2yyz4gx48ak44la-libclang-21.1.8").unwrap();

    let mut map = HashMap::new();
    map.insert("rustc".to_string(), rustc.clone());
    map.insert("sqlite".to_string(), sqlite.clone());
    map.insert("libclang.lib".to_string(), libclang.clone());

    let evaluator = Evaluator::new("aarch64-linux", Arc::new(MockLookup(map)));

    let beryl_lua = r#"
local p = beryl.nixpkgs

return {
  description = "Beryl dev env",
  shells = {
    default = beryl.shell({
      packages = { p("rustc") },
      libs = { p("sqlite") },
      env = {
        LIBCLANG_PATH = p("libclang.lib") .. "/lib",
        RUST_BACKTRACE = "1",
      },
      hook = [[echo "welcome"]],
    }),
  },
}
"#;

    let manifest = evaluator
        .eval_project_str(beryl_lua, "beryl.lua")
        .expect("eval project manifest");

    assert_eq!(manifest.description.as_deref(), Some("Beryl dev env"));
    let default_shell = manifest.shells.get("default").expect("has default shell");
    assert_eq!(default_shell.packages.len(), 1);
    assert_eq!(default_shell.packages[0].out_path, rustc);
    assert_eq!(default_shell.libs.len(), 1);
    assert_eq!(default_shell.libs[0].out_path, sqlite);
    assert_eq!(
        default_shell.env.get("LIBCLANG_PATH").unwrap(),
        &format!("{}/lib", libclang.to_absolute_path())
    );
    assert_eq!(default_shell.env.get("RUST_BACKTRACE").unwrap(), "1");
    assert_eq!(default_shell.hook.as_deref(), Some("echo \"welcome\""));
}
