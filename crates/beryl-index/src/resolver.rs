//! Three-tier `nixpkgs` attribute resolver:
//! 1. `beryl.lock` (zero network, zero Nix)
//! 2. Upstream Hydra JSON job endpoint + `cache.nixos.org` `.narinfo` verification (zero Nix)
//! 3. `nix eval --raw` fallback (arbitrary attributes / custom nixpkgs git commits)

use std::collections::BTreeMap;

use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;
use beryl_store::BinaryCacheClient;
use reqwest::StatusCode;
use serde::Deserialize;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::error::{IndexError, Result};
use crate::lockfile::{Lockfile, ResolutionSource};

/// Detect the current Nix system triple (e.g. `aarch64-linux`, `x86_64-linux`, `aarch64-darwin`).
pub fn current_system() -> Result<String> {
    let arch = std::env::consts::ARCH;
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        other => {
            return Err(IndexError::UnsupportedPlatform {
                os: other.to_string(),
                arch: arch.to_string(),
            });
        }
    };
    Ok(format!("{arch}-{os}"))
}

/// Configuration for the upstream Hydra JSON index (Tier 2).
#[derive(Debug, Clone)]
pub struct HydraConfig {
    pub base_url: String,
    /// Optional explicit Hydra project override (when `None`, derived from `lockfile.nixpkgs.channel`).
    pub project: Option<String>,
    /// Optional explicit Hydra jobset override (when `None`, derived from `lockfile.nixpkgs.channel`).
    pub jobset: Option<String>,
}

impl Default for HydraConfig {
    fn default() -> Self {
        Self {
            base_url: "https://hydra.nixos.org".to_string(),
            project: None,
            jobset: None,
        }
    }
}

/// Map a Nixpkgs channel name (e.g. `nixos-unstable`, `nixos-25.05`, `nixpkgs-25.05-darwin`)
/// to its Hydra `(project, jobset, job_name)` tuple.
pub fn hydra_job_for_channel(channel: &str, attr: &str, system: &str) -> (String, String, String) {
    let ch = channel.trim();
    match ch {
        "" | "unstable" | "nixos-unstable" | "nixpkgs-unstable" => (
            "nixpkgs".to_string(),
            "trunk".to_string(),
            format!("{attr}.{system}"),
        ),
        _ if ch.starts_with("nixos-") => {
            // Stable NixOS channels (e.g. `nixos-25.05` -> `nixos/release-25.05`, job `nixpkgs.<attr>.<system>`)
            let ver = ch.trim_start_matches("nixos-");
            (
                "nixos".to_string(),
                format!("release-{ver}"),
                format!("nixpkgs.{attr}.{system}"),
            )
        }
        _ if ch.starts_with("release-") => (
            "nixos".to_string(),
            ch.to_string(),
            format!("nixpkgs.{attr}.{system}"),
        ),
        other => (
            "nixpkgs".to_string(),
            other.to_string(),
            format!("{attr}.{system}"),
        ),
    }
}

/// Compute the `nix eval` flake reference for a channel and optional pinned git commit.
pub fn flake_ref_for_pin(channel: &str, rev: Option<&str>) -> String {
    if let Some(r) = rev.filter(|s| !s.trim().is_empty()) {
        return format!("github:NixOS/nixpkgs/{}", r.trim());
    }
    let ch = channel.trim();
    match ch {
        "" | "unstable" => "github:NixOS/nixpkgs/nixos-unstable".to_string(),
        other => format!("github:NixOS/nixpkgs/{other}"),
    }
}

#[derive(Debug, Deserialize)]
struct HydraBuildResponse {
    #[serde(default)]
    buildoutputs: BTreeMap<String, HydraBuildOutput>,
}

#[derive(Debug, Deserialize)]
struct HydraBuildOutput {
    path: String,
}

/// Three-tier resolver mapping `(system, attr)` -> `/nix/store/<hash>-<name>`.
#[derive(Debug, Clone)]
pub struct NixpkgsResolver {
    http: reqwest::Client,
    hydra: HydraConfig,
    cache: BinaryCacheClient,
    enable_nix_eval: bool,
}

impl NixpkgsResolver {
    pub fn new(hydra: HydraConfig, cache: BinaryCacheClient) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("beryl/0.1")
            .build()?;
        Ok(Self {
            http,
            hydra,
            cache,
            enable_nix_eval: true,
        })
    }

    pub fn with_nix_eval(mut self, enabled: bool) -> Self {
        self.enable_nix_eval = enabled;
        self
    }

    /// Resolve `(system, attr)` using the 3-tier chain, persisting newly discovered pins
    /// into `lockfile`.
    pub async fn resolve(
        &self,
        lockfile: &mut Lockfile,
        system: &str,
        attr: &str,
    ) -> Result<StorePath> {
        // Tier 1: beryl.lock
        if let Some(pinned) = lockfile.get(system, attr)? {
            debug!(%system, %attr, path = %pinned.to_absolute_path(), "resolved from beryl.lock");
            return Ok(pinned);
        }

        self.resolve_uncached(lockfile, system, attr).await
    }

    /// Bypass Tier 1 (`beryl.lock`) to refresh/update the pin for `(system, attr)`.
    pub async fn update(
        &self,
        lockfile: &mut Lockfile,
        system: &str,
        attr: &str,
    ) -> Result<StorePath> {
        self.resolve_uncached(lockfile, system, attr).await
    }

    async fn resolve_uncached(
        &self,
        lockfile: &mut Lockfile,
        system: &str,
        attr: &str,
    ) -> Result<StorePath> {
        let channel = lockfile.nixpkgs.channel.clone();

        // Tier 2: Upstream Hydra JSON endpoint (skipped when user pinned a custom nixpkgs git rev)
        if lockfile.nixpkgs.rev.is_none()
            && let Some(path) = self.query_hydra(&channel, system, attr).await?
        {
            // Confirm .narinfo exists and signature verifies on the configured binary cache.
            if self.cache.fetch_path_info(&path).await?.is_none() {
                return Err(IndexError::MissingInCache {
                    attr: attr.to_string(),
                    path: path.to_absolute_path(),
                });
            }
            info!(%channel, %system, %attr, path = %path.to_absolute_path(), "resolved via Hydra + cache.nixos.org");
            lockfile.insert(system, attr, &path, ResolutionSource::Hydra);
            return Ok(path);
        }

        // Tier 3: `nix eval` fallback
        if self.enable_nix_eval
            && let Some(path) = self
                .query_nix_eval(&channel, lockfile.nixpkgs.rev.as_deref(), system, attr)
                .await?
        {
            info!(%channel, %system, %attr, path = %path.to_absolute_path(), "resolved via `nix eval`");
            lockfile.insert(system, attr, &path, ResolutionSource::NixEval);
            return Ok(path);
        }

        Err(IndexError::NotFound {
            attr: attr.to_string(),
            system: system.to_string(),
        })
    }

    async fn query_hydra(
        &self,
        channel: &str,
        system: &str,
        attr: &str,
    ) -> Result<Option<StorePath>> {
        let base = self.hydra.base_url.trim_end_matches('/');
        let (default_project, default_jobset, job_name) =
            hydra_job_for_channel(channel, attr, system);
        let project = self.hydra.project.as_deref().unwrap_or(&default_project);
        let jobset = self.hydra.jobset.as_deref().unwrap_or(&default_jobset);

        let url = format!("{base}/job/{project}/{jobset}/{job_name}/latest");
        debug!(%url, "querying Hydra job endpoint");

        let resp = match self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(%url, error = %e, "Hydra request failed; will try fallback");
                return Ok(None);
            }
        };

        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            warn!(%url, status = %resp.status(), "Hydra returned non-success status; will try fallback");
            return Ok(None);
        }

        let text = resp.text().await?;
        let body: HydraBuildResponse = serde_json::from_str(&text)?;
        let Some(out) = body
            .buildoutputs
            .get("out")
            .or_else(|| body.buildoutputs.values().next())
        else {
            return Ok(None);
        };

        Ok(Some(parse_store_path(&out.path)?))
    }

    async fn query_nix_eval(
        &self,
        channel: &str,
        rev: Option<&str>,
        system: &str,
        attr: &str,
    ) -> Result<Option<StorePath>> {
        let flake_ref = flake_ref_for_pin(channel, rev);
        let expr = format!("{flake_ref}#legacyPackages.{system}.{attr}.outPath");
        debug!(%expr, "running nix eval fallback");

        let output = match Command::new("nix")
            .args(["eval", "--raw", &expr])
            .output()
            .await
        {
            Ok(out) => out,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(IndexError::Io(e)),
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(IndexError::NixEvalFailed {
                attr: attr.to_string(),
                stderr,
            });
        }

        let raw_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(Some(parse_store_path(&raw_path)?))
    }
}
