use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "beryl",
    version,
    about = "A Lua-driven, /nix/store-compatible package manager"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
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
