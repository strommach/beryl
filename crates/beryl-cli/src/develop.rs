//! `beryl develop`: enter a project dev shell defined in `beryl.lua` (or emit `--export` env).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use beryl_build::{Builder, realise_eval_via_daemon};
use beryl_eval::{DevShellSpec, EvalResult, ProjectManifest};
use beryl_nix::store_path::StorePath;
use beryl_store::{BinaryCacheClient, DaemonStore, LocalStore, Store};

/// Realized environment variables ready to spawn a subshell or print via `--export`.
#[derive(Debug, Default)]
pub struct DevShellEnv {
    pub vars: BTreeMap<String, String>,
    pub hook: Option<String>,
}

/// Realize all packages and libs referenced by `shell` and compute the dev shell environment.
pub async fn prepare_dev_shell(
    manifest: &ProjectManifest,
    shell: &DevShellSpec,
    use_daemon: bool,
    daemon_socket: &Path,
    store_dir: &Path,
    cache: BinaryCacheClient,
) -> anyhow::Result<DevShellEnv> {
    let mut bin_dirs = Vec::new();
    let mut include_dirs = Vec::new();
    let mut lib_dirs = Vec::new();
    let mut pkgconfig_dirs = Vec::new();

    if use_daemon {
        let daemon = DaemonStore::connect(daemon_socket).await?;

        for pkg in &shell.packages {
            let real = realise_ref_daemon(&daemon, manifest, &pkg.out_path, pkg.drv_path.as_ref())
                .await?;
            collect_tool_dirs(&real, &mut bin_dirs, &mut pkgconfig_dirs);
        }
        for lib in &shell.libs {
            let real = realise_ref_daemon(&daemon, manifest, &lib.out_path, lib.drv_path.as_ref())
                .await?;
            collect_lib_dirs(&real, &mut include_dirs, &mut lib_dirs, &mut pkgconfig_dirs);
        }
    } else {
        let store = LocalStore::open(store_dir)?;
        let builder = Builder::new(store.clone(), Some(cache.clone()));

        for pkg in &shell.packages {
            let real = realise_ref_local(
                &store,
                &cache,
                &builder,
                manifest,
                &pkg.out_path,
                pkg.drv_path.as_ref(),
            )
            .await?;
            collect_tool_dirs(&real, &mut bin_dirs, &mut pkgconfig_dirs);
        }
        for lib in &shell.libs {
            let real = realise_ref_local(
                &store,
                &cache,
                &builder,
                manifest,
                &lib.out_path,
                lib.drv_path.as_ref(),
            )
            .await?;
            collect_lib_dirs(&real, &mut include_dirs, &mut lib_dirs, &mut pkgconfig_dirs);
        }
    }

    let mut vars = BTreeMap::new();
    vars.insert("IN_BERYL_SHELL".to_string(), "1".to_string());

    if !bin_dirs.is_empty() {
        vars.insert("PATH".to_string(), prepend_path_string("PATH", &bin_dirs));
    }
    if !include_dirs.is_empty() {
        vars.insert(
            "CPATH".to_string(),
            prepend_path_string("CPATH", &include_dirs),
        );
    }
    if !lib_dirs.is_empty() {
        vars.insert(
            "LIBRARY_PATH".to_string(),
            prepend_path_string("LIBRARY_PATH", &lib_dirs),
        );
        let ldflags = lib_dirs
            .iter()
            .map(|p| format!("-L{}", p.display()))
            .collect::<Vec<_>>()
            .join(" ");
        let existing = std::env::var("LDFLAGS").unwrap_or_default();
        let combined = if existing.is_empty() {
            ldflags
        } else {
            format!("{ldflags} {existing}")
        };
        vars.insert("LDFLAGS".to_string(), combined);
    }
    if !pkgconfig_dirs.is_empty() {
        vars.insert(
            "PKG_CONFIG_PATH".to_string(),
            prepend_path_string("PKG_CONFIG_PATH", &pkgconfig_dirs),
        );
    }

    // Apply user-defined `env = { ... }` overrides last
    for (k, v) in &shell.env {
        vars.insert(k.clone(), v.clone());
    }

    Ok(DevShellEnv {
        vars,
        hook: shell.hook.clone(),
    })
}

/// Print POSIX-compatible `export KEY='VALUE'` lines (for `eval "$(beryl develop --export)"` / direnv).
pub fn print_export_script(env: &DevShellEnv) {
    for (k, v) in &env.vars {
        println!("export {k}={}", shell_quote(v));
    }
    if let Some(ref hook) = env.hook
        && !hook.trim().is_empty()
    {
        println!("{}", hook.trim());
    }
}

/// Spawn an interactive subshell (or run `-c <command>`) with `env` applied.
pub fn spawn_dev_shell(env: &DevShellEnv, command: Option<&str>) -> anyhow::Result<()> {
    let shell_bin = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut cmd = StdCommand::new(&shell_bin);

    for (k, v) in &env.vars {
        cmd.env(k, v);
    }

    let _rc_guard;
    if let Some(user_cmd) = command {
        let script = match &env.hook {
            Some(h) if !h.trim().is_empty() => format!("{}\n{user_cmd}", h.trim()),
            _ => user_cmd.to_string(),
        };
        cmd.args(["-c", &script]);
        _rc_guard = None;
    } else if let Some(ref hook) = env.hook
        && !hook.trim().is_empty()
        && shell_bin.ends_with("bash")
    {
        // For interactive bash, source ~/.bashrc (if present) and run the shell hook via --rcfile
        let tmp = tempfile::Builder::new()
            .prefix("beryl-rc-")
            .suffix(".sh")
            .tempfile()?;
        let rc_content = format!(
            "[ -f \"$HOME/.bashrc\" ] && source \"$HOME/.bashrc\"\n{}\n",
            hook.trim()
        );
        std::fs::write(tmp.path(), rc_content)?;
        cmd.args(["--rcfile", &tmp.path().display().to_string(), "-i"]);
        _rc_guard = Some(tmp);
    } else {
        _rc_guard = None;
    }

    let status = cmd.status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

async fn realise_ref_daemon(
    daemon: &DaemonStore,
    manifest: &ProjectManifest,
    out_path: &StorePath,
    drv_path: Option<&StorePath>,
) -> anyhow::Result<PathBuf> {
    if !daemon.has_path(out_path).await? {
        if let Some(drv_sp) = drv_path {
            let sub_eval = EvalResult {
                root: beryl_eval::StoreRef::from_derivation(
                    out_path.clone(),
                    drv_sp.clone(),
                    out_path.name(),
                ),
                derivations: manifest.derivations.clone(),
                input_sources: manifest.input_sources.clone(),
            };
            realise_eval_via_daemon(daemon, &sub_eval).await?;
        } else {
            daemon.ensure_path(out_path).await?;
        }
    }
    Ok(daemon.real_path(out_path))
}

async fn realise_ref_local(
    store: &LocalStore,
    cache: &BinaryCacheClient,
    builder: &Builder,
    manifest: &ProjectManifest,
    out_path: &StorePath,
    drv_path: Option<&StorePath>,
) -> anyhow::Result<PathBuf> {
    if !store.has_path(out_path).await? {
        if let Some(drv_sp) = drv_path {
            let sub_eval = EvalResult {
                root: beryl_eval::StoreRef::from_derivation(
                    out_path.clone(),
                    drv_sp.clone(),
                    out_path.name(),
                ),
                derivations: manifest.derivations.clone(),
                input_sources: manifest.input_sources.clone(),
            };
            builder.realise_eval(&sub_eval).await?;
        } else {
            cache.fetch_closure(store, out_path).await?;
        }
    }
    Ok(store.real_path(out_path))
}

fn collect_tool_dirs(real: &Path, bin_dirs: &mut Vec<PathBuf>, pkgconfig_dirs: &mut Vec<PathBuf>) {
    let bin = real.join("bin");
    if bin.is_dir() {
        bin_dirs.push(bin);
    }
    for rel in ["lib/pkgconfig", "share/pkgconfig"] {
        let pc = real.join(rel);
        if pc.is_dir() {
            pkgconfig_dirs.push(pc);
        }
    }
}

fn collect_lib_dirs(
    real: &Path,
    include_dirs: &mut Vec<PathBuf>,
    lib_dirs: &mut Vec<PathBuf>,
    pkgconfig_dirs: &mut Vec<PathBuf>,
) {
    let inc = real.join("include");
    if inc.is_dir() {
        include_dirs.push(inc);
    }
    let lib = real.join("lib");
    if lib.is_dir() {
        lib_dirs.push(lib);
    }
    for rel in ["lib/pkgconfig", "share/pkgconfig"] {
        let pc = real.join(rel);
        if pc.is_dir() {
            pkgconfig_dirs.push(pc);
        }
    }
}

fn prepend_path_string(var: &str, dirs: &[PathBuf]) -> String {
    let mut all: Vec<PathBuf> = dirs.to_vec();
    if let Some(existing) = std::env::var_os(var) {
        all.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(all)
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
