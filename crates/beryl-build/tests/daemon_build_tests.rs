use std::fs;
use std::path::Path;
use std::sync::Arc;

use beryl_build::realise_eval_via_daemon;
use beryl_eval::{EvalError, Evaluator, NixpkgsLookup};
use beryl_index::current_system;
use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::{DEFAULT_DAEMON_SOCKET, DaemonStore, Store};

struct BashLookup(StorePath);

impl NixpkgsLookup for BashLookup {
    fn resolve_nixpkgs(&self, _system: &str, attr: &str) -> beryl_eval::Result<StorePath> {
        if attr == "bash" {
            Ok(self.0.clone())
        } else {
            Err(EvalError::Derivation(format!("unexpected attr {attr}")))
        }
    }
}

#[tokio::test]
async fn daemon_builds_lua_derivation_in_nix_sandbox_if_available() {
    if !Path::new(DEFAULT_DAEMON_SOCKET).exists() {
        return;
    }

    // Locate an existing bash in /nix/store to use as the derivation builder inside nix-daemon's sandbox
    let bash_sp = std::fs::read_dir("/nix/store")
        .ok()
        .and_then(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .find(|name| {
                    !name.ends_with(".drv")
                        && name.contains("-bash-")
                        && Path::new("/nix/store").join(name).join("bin/sh").exists()
                })
                .and_then(|name| parse_store_path(&name).ok())
        });

    let Some(bash_sp) = bash_sp else {
        return;
    };

    let system = current_system().unwrap();
    let pkg_lua = r#"
local bash = beryl.nixpkgs("bash")
return beryl.derivation({
  name = "beryl-daemon-hello",
  version = "1.0",
  builder = bash .. "/bin/sh",
  inputs = { bash },
  build = [[
    echo "built by nix-daemon from Lua!" > "$out"
  ]],
})
"#;

    let evaluator = Evaluator::new(system, Arc::new(BashLookup(bash_sp)));
    let eval_res = evaluator.eval_str(pkg_lua, "pkg.lua").expect("eval pkg.lua");

    let daemon = DaemonStore::connect_default()
        .await
        .expect("connect to nix-daemon");

    let out_path = realise_eval_via_daemon(&daemon, &eval_res)
        .await
        .expect("realise_eval_via_daemon");

    assert_eq!(out_path, eval_res.root.out_path);
    assert!(daemon.has_path(&out_path).await.unwrap());
    assert_eq!(
        fs::read_to_string(daemon.real_path(&out_path)).unwrap(),
        "built by nix-daemon from Lua!\n"
    );
}
