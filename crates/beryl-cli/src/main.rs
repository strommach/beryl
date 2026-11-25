use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command as StdCommand;
use std::sync::{Arc, Mutex};

use beryl_build::{Builder, SandboxMode};
use beryl_eval::{EvalError, EvalResult, Evaluator, NixpkgsLookup};
use beryl_index::{HydraConfig, Lockfile, NixpkgsResolver, current_system};
use beryl_nix::narinfo_ext::parse_verifying_key;
use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::{
    BinaryCacheClient, BinaryCacheConfig, DEFAULT_DAEMON_SOCKET, DaemonStore, LocalStore,
    ProfileManager, Store, collect_garbage,
};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "beryl",
    version,
    about = "A Lua-driven, /nix/store-compatible package manager"
)]
struct Cli {
    /// Root directory for the local store (contains `store/` and `var/nix/`)
    #[arg(long, env = "BERYL_STORE_DIR", default_value = "/nix", global = true)]
    store: PathBuf,

    /// Talk to a running `nix-daemon` instead of accessing the local store DB directly
    #[arg(long, env = "BERYL_USE_DAEMON", global = true)]
    daemon: bool,

    /// Unix socket path for `nix-daemon` (used with `--daemon`)
    #[arg(
        long,
        env = "BERYL_DAEMON_SOCKET",
        default_value = DEFAULT_DAEMON_SOCKET,
        global = true
    )]
    daemon_socket: PathBuf,

    /// Binary cache base URL
    #[arg(
        long,
        env = "BERYL_CACHE_URL",
        default_value = "https://cache.nixos.org",
        global = true
    )]
    cache: String,

    /// Additional trusted binary cache public keys (`name:base64`)
    #[arg(long = "trusted-key", global = true)]
    trusted_keys: Vec<String>,

    /// Profile name under `<store>/var/nix/profiles/`
    #[arg(long, default_value = "default", global = true)]
    profile: String,

    /// Path to `beryl.lock`
    #[arg(long, default_value = "beryl.lock", global = true)]
    lockfile: PathBuf,

    /// Target Nix system triple (defaults to host platform, e.g. `aarch64-linux`)
    #[arg(long, global = true)]
    system: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch a store path and its full runtime closure from a binary cache
    Fetch {
        /// Store path (`/nix/store/<hash>-<name>` or `<hash>-<name>`)
        store_path: String,
    },
    /// Query metadata and runtime references for a store path
    Info {
        /// Store path (`/nix/store/<hash>-<name>` or `<hash>-<name>`)
        store_path: String,
    },
    /// Resolve and pin nixpkgs attributes into `beryl.lock`
    Lock {
        /// Attribute names (e.g. `ripgrep` or `nixpkgs#ripgrep`)
        attrs: Vec<String>,
    },
    /// Refresh pinned nixpkgs attributes in `beryl.lock`
    Update {
        /// Specific attributes to refresh (defaults to all pinned packages for the current system)
        attrs: Vec<String>,
    },
    /// Evaluate a pkg.lua into Nix ATerm `.drv` files
    Eval {
        /// Path to `pkg.lua`
        path: PathBuf,
        /// Write evaluated `.drv` files into the local store
        #[arg(long)]
        write: bool,
        /// Print the root derivation's raw ATerm representation
        #[arg(long)]
        aterm: bool,
    },
    /// Evaluate a pkg.lua, fetch substitutes, and build into the store
    Build {
        /// Path to `pkg.lua`
        path: PathBuf,
        /// Disable Linux namespace sandboxing (still uses stripped env & isolated build dir)
        #[arg(long)]
        no_sandbox: bool,
    },
    /// Build/fetch a package (`nixpkgs#<attr>`, `/nix/store/...`, or `./pkg.lua`) and link into active profile
    Install { target: String },
    /// Spawn a subshell with requested store paths on PATH/CPATH/LIBRARY_PATH/LDFLAGS
    Shell {
        /// Packages to include (`ripgrep`, `nixpkgs#ripgrep`, or `/nix/store/...`)
        #[arg(required = true)]
        packages: Vec<String>,

        /// Optional command to run inside the shell instead of an interactive shell
        #[arg(long, short = 'c')]
        command: Option<String>,
    },
    /// Atomically revert the active profile to the previous generation
    Rollback,
    /// Sweep unreferenced paths from the store
    Gc,
}

fn build_cache_client(cli: &Cli) -> anyhow::Result<BinaryCacheClient> {
    let mut config = BinaryCacheConfig {
        base_url: cli.cache.clone(),
        ..BinaryCacheConfig::default()
    };
    for key_spec in &cli.trusted_keys {
        config.trusted_keys.push(parse_verifying_key(key_spec)?);
    }
    Ok(BinaryCacheClient::new(config)?)
}

fn strip_nixpkgs_prefix(spec: &str) -> &str {
    spec.strip_prefix("nixpkgs#").unwrap_or(spec)
}

async fn resolve_target(
    target: &str,
    system: &str,
    resolver: &NixpkgsResolver,
    lockfile: &mut Lockfile,
) -> anyhow::Result<StorePath> {
    if target.starts_with("/nix/store/") {
        return Ok(parse_store_path(target)?);
    }
    let attr = strip_nixpkgs_prefix(target);
    Ok(resolver.resolve(lockfile, system, attr).await?)
}

async fn eval_pkg_lua(
    cli: &Cli,
    system: &str,
    path: PathBuf,
    client: BinaryCacheClient,
) -> anyhow::Result<EvalResult> {
    let resolver = NixpkgsResolver::new(HydraConfig::default(), client)?;
    let lockfile = Arc::new(Mutex::new(Lockfile::load_or_default(&cli.lockfile)?));
    let handle = tokio::runtime::Handle::current();

    struct CliLookup {
        resolver: NixpkgsResolver,
        lockfile: Arc<Mutex<Lockfile>>,
        handle: tokio::runtime::Handle,
    }

    impl NixpkgsLookup for CliLookup {
        fn resolve_nixpkgs(&self, system: &str, attr: &str) -> beryl_eval::Result<StorePath> {
            let mut lf = self.lockfile.lock().expect("lockfile mutex poisoned");
            self.handle
                .block_on(self.resolver.resolve(&mut lf, system, attr))
                .map_err(EvalError::from)
        }
    }

    let lookup = Arc::new(CliLookup {
        resolver,
        lockfile: Arc::clone(&lockfile),
        handle,
    });
    let eval_system = system.to_string();
    let res = tokio::task::spawn_blocking(move || {
        let evaluator = Evaluator::new(eval_system, lookup);
        evaluator.eval_file(&path)
    })
    .await??;

    lockfile
        .lock()
        .expect("lockfile mutex poisoned")
        .save(&cli.lockfile)?;

    Ok(res)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let system = match &cli.system {
        Some(s) => s.clone(),
        None => current_system()?,
    };

    match &cli.command {
        Commands::Fetch { store_path } => {
            let client = build_cache_client(&cli)?;
            let parsed = parse_store_path(store_path)?;

            if cli.daemon {
                let store = DaemonStore::connect(&cli.daemon_socket).await?;
                let newly_installed = client.fetch_closure(&store, &parsed).await?;
                let full_closure = store.query_closure(&parsed).await?;
                println!(
                    "daemon fetch: {} new paths ({} total in closure) -> {}",
                    newly_installed.len(),
                    full_closure.len(),
                    store.real_path(&parsed).display()
                );
            } else {
                let store = LocalStore::open(&cli.store)?;
                let newly_installed = client.fetch_closure(&store, &parsed).await?;
                let full_closure = store.query_closure(&parsed).await?;

                if newly_installed.is_empty() {
                    println!(
                        "already present: {} ({} paths in closure)",
                        parsed.to_absolute_path(),
                        full_closure.len()
                    );
                } else {
                    println!(
                        "fetched {} new paths ({} total in closure) -> {}",
                        newly_installed.len(),
                        full_closure.len(),
                        store.real_path(&parsed).display()
                    );
                }
            }
            Ok(())
        }
        Commands::Info { store_path } => {
            let parsed = parse_store_path(store_path)?;
            let info = if cli.daemon {
                let store = DaemonStore::connect(&cli.daemon_socket).await?;
                store.query_path_info(&parsed).await?
            } else {
                let store = LocalStore::open(&cli.store)?;
                store.query_path_info(&parsed).await?
            };

            match info {
                Some(info) => {
                    println!("Path:       {}", info.path.to_absolute_path());
                    println!("NarSize:    {} bytes", info.nar_size);
                    println!("NarHash:    sha256:{}", hex::encode(info.nar_hash));
                    if let Some(drv) = &info.deriver {
                        println!("Deriver:    {}", drv.to_absolute_path());
                    }
                    if let Some(ca) = &info.ca {
                        println!("CA:         {ca}");
                    }
                    println!("References: {}", info.references.len());
                    for r in &info.references {
                        println!("  - {}", r.to_absolute_path());
                    }
                }
                None => {
                    anyhow::bail!("store path not found: {}", parsed.to_absolute_path());
                }
            }
            Ok(())
        }
        Commands::Lock { attrs } => {
            let client = build_cache_client(&cli)?;
            let resolver = NixpkgsResolver::new(HydraConfig::default(), client)?;
            let mut lockfile = Lockfile::load_or_default(&cli.lockfile)?;

            for raw in attrs {
                let attr = strip_nixpkgs_prefix(raw);
                let sp = resolver.resolve(&mut lockfile, &system, attr).await?;
                println!("locked {attr} ({system}) -> {}", sp.to_absolute_path());
            }
            lockfile.save(&cli.lockfile)?;
            Ok(())
        }
        Commands::Update { attrs } => {
            let client = build_cache_client(&cli)?;
            let resolver = NixpkgsResolver::new(HydraConfig::default(), client)?;
            let mut lockfile = Lockfile::load_or_default(&cli.lockfile)?;

            let targets: Vec<String> = if attrs.is_empty() {
                lockfile
                    .packages
                    .get(&system)
                    .map(|m| m.keys().cloned().collect())
                    .unwrap_or_default()
            } else {
                attrs
                    .iter()
                    .map(|s| strip_nixpkgs_prefix(s).to_string())
                    .collect()
            };

            for attr in targets {
                let sp = resolver.update(&mut lockfile, &system, &attr).await?;
                println!("updated {attr} ({system}) -> {}", sp.to_absolute_path());
            }
            lockfile.save(&cli.lockfile)?;
            Ok(())
        }
        Commands::Install { target } => {
            let store = LocalStore::open(&cli.store)?;
            let client = build_cache_client(&cli)?;

            let sp = if target.ends_with(".lua") {
                let eval_res =
                    eval_pkg_lua(&cli, &system, PathBuf::from(target), client.clone()).await?;
                let builder = Builder::new(store.clone(), Some(client));
                builder.realise_eval(&eval_res).await?
            } else {
                let resolver = NixpkgsResolver::new(HydraConfig::default(), client.clone())?;
                let mut lockfile = Lockfile::load_or_default(&cli.lockfile)?;
                let sp = resolve_target(target, &system, &resolver, &mut lockfile).await?;
                lockfile.save(&cli.lockfile)?;
                client.fetch_closure(&store, &sp).await?;
                sp
            };

            let profile = ProfileManager::new(store.clone(), &cli.profile)?;
            let gen_num = profile.install(&sp).await?;

            println!(
                "installed {} -> profile '{}' generation {} ({})",
                sp.to_absolute_path(),
                cli.profile,
                gen_num,
                profile.active_link().display()
            );
            Ok(())
        }
        Commands::Shell { packages, command } => {
            let store = LocalStore::open(&cli.store)?;
            let client = build_cache_client(&cli)?;
            let resolver = NixpkgsResolver::new(HydraConfig::default(), client.clone())?;
            let mut lockfile = Lockfile::load_or_default(&cli.lockfile)?;

            let mut bin_dirs = Vec::new();
            let mut include_dirs = Vec::new();
            let mut lib_dirs = Vec::new();
            let mut pkgconfig_dirs = Vec::new();

            for pkg in packages {
                let sp = resolve_target(pkg, &system, &resolver, &mut lockfile).await?;
                client.fetch_closure(&store, &sp).await?;
                let real = store.real_path(&sp);

                let bin = real.join("bin");
                if bin.is_dir() {
                    bin_dirs.push(bin);
                }
                let inc = real.join("include");
                if inc.is_dir() {
                    include_dirs.push(inc);
                }
                let lib = real.join("lib");
                if lib.is_dir() {
                    let pc = lib.join("pkgconfig");
                    if pc.is_dir() {
                        pkgconfig_dirs.push(pc);
                    }
                    lib_dirs.push(lib);
                }
            }
            lockfile.save(&cli.lockfile)?;

            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
            let mut cmd = StdCommand::new(&shell);
            if let Some(c) = command {
                cmd.args(["-c", c]);
            }

            if !bin_dirs.is_empty() {
                cmd.env("PATH", prepend_env_paths("PATH", &bin_dirs));
            }
            if !include_dirs.is_empty() {
                cmd.env("CPATH", prepend_env_paths("CPATH", &include_dirs));
            }
            if !lib_dirs.is_empty() {
                cmd.env("LIBRARY_PATH", prepend_env_paths("LIBRARY_PATH", &lib_dirs));
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
                cmd.env("LDFLAGS", combined);
            }
            if !pkgconfig_dirs.is_empty() {
                cmd.env(
                    "PKG_CONFIG_PATH",
                    prepend_env_paths("PKG_CONFIG_PATH", &pkgconfig_dirs),
                );
            }

            let status = cmd.status()?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
            Ok(())
        }
        Commands::Rollback => {
            let store = LocalStore::open(&cli.store)?;
            let profile = ProfileManager::new(store, &cli.profile)?;
            let gen_num = profile.rollback()?;
            println!(
                "rolled back profile '{}' to generation {} ({})",
                cli.profile,
                gen_num,
                profile.active_link().display()
            );
            Ok(())
        }
        Commands::Gc => {
            let store = LocalStore::open(&cli.store)?;
            let report = collect_garbage(&store, &[]).await?;
            println!(
                "deleted {} store paths, freed {} bytes ({} live paths kept)",
                report.deleted_paths.len(),
                report.freed_bytes,
                report.live_paths
            );
            Ok(())
        }
        Commands::Eval { path, write, aterm } => {
            let client = build_cache_client(&cli)?;
            let res = eval_pkg_lua(&cli, &system, path.clone(), client).await?;

            if *write {
                let store = LocalStore::open(&cli.store)?;
                res.write_drvs_to_store(&store).await?;
            }

            let drv_path = res.root.drv_path.as_ref().expect("root has drv_path");
            if *aterm {
                let bytes = res.derivations[drv_path].to_aterm_bytes();
                print!("{}", String::from_utf8_lossy(&bytes));
            } else {
                println!("drv: {}", drv_path.to_absolute_path());
                println!("out: {}", res.root.out_path.to_absolute_path());
                println!("derivations: {}", res.derivations.len());
            }
            Ok(())
        }
        Commands::Build { path, no_sandbox } => {
            let store = LocalStore::open(&cli.store)?;
            let client = build_cache_client(&cli)?;
            let eval_res = eval_pkg_lua(&cli, &system, path.clone(), client.clone()).await?;

            let sandbox = if *no_sandbox {
                SandboxMode::Disabled
            } else {
                SandboxMode::Auto
            };
            let builder = Builder::new(store.clone(), Some(client)).with_sandbox(sandbox);
            let out_path = builder.realise_eval(&eval_res).await?;

            println!(
                "built {} -> {}",
                out_path.to_absolute_path(),
                store.real_path(&out_path).display()
            );
            Ok(())
        }
    }
}

fn prepend_env_paths(var: &str, dirs: &[PathBuf]) -> OsString {
    let mut all: Vec<PathBuf> = dirs.to_vec();
    if let Some(existing) = std::env::var_os(var) {
        all.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(all).unwrap_or_default()
}
