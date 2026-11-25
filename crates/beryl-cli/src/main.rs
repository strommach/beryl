use std::path::PathBuf;

use beryl_nix::narinfo_ext::parse_verifying_key;
use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::{BinaryCacheClient, BinaryCacheConfig, LocalStore, Store};
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
    /// Evaluate a pkg.lua, fetch substitutes, and build into the store
    Build { path: String },
    /// Build/fetch a package and link it into the active user profile
    Install { target: String },
    /// Spawn a subshell with requested store paths on PATH/CPATH/LDFLAGS
    Shell { packages: Vec<String> },
    /// Atomically revert the active profile to the previous generation
    Rollback,
    /// Sweep unreferenced paths from the store
    Gc,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Fetch { store_path } => {
            let store = LocalStore::open(&cli.store)?;
            let mut config = BinaryCacheConfig {
                base_url: cli.cache,
                ..BinaryCacheConfig::default()
            };
            for key_spec in &cli.trusted_keys {
                config.trusted_keys.push(parse_verifying_key(key_spec)?);
            }
            let client = BinaryCacheClient::new(config)?;
            let parsed = parse_store_path(&store_path)?;

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
            Ok(())
        }
        Commands::Build { path } => anyhow::bail!("`beryl build {path}` is not implemented yet"),
        Commands::Install { target } => {
            anyhow::bail!("`beryl install {target}` is not implemented yet")
        }
        Commands::Shell { packages } => {
            anyhow::bail!("`beryl shell {:?}` is not implemented yet", packages)
        }
        Commands::Rollback => anyhow::bail!("`beryl rollback` is not implemented yet"),
        Commands::Gc => anyhow::bail!("`beryl gc` is not implemented yet"),
    }
}
