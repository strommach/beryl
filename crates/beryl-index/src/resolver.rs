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
    pub project: String,
    pub jobset: String,
}

impl Default for HydraConfig {
    fn default() -> Self {
        Self {
            base_url: "https://hydra.nixos.org".to_string(),
            project: "nixpkgs".to_string(),
            jobset: "trunk".to_string(),
        }
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
        // Tier 2: Upstream Hydra JSON endpoint (skipped when user pinned a custom nixpkgs git rev)
        if lockfile.nixpkgs.rev.is_none()
            && let Some(path) = self.query_hydra(system, attr).await?
        {
            // Confirm .narinfo exists and signature verifies on the configured binary cache.
            if self.cache.fetch_path_info(&path).await?.is_none() {
                return Err(IndexError::MissingInCache {
                    attr: attr.to_string(),
                    path: path.to_absolute_path(),
                });
            }
            info!(%system, %attr, path = %path.to_absolute_path(), "resolved via Hydra + cache.nixos.org");
            lockfile.insert(system, attr, &path, ResolutionSource::Hydra);
            return Ok(path);
        }

        // Tier 3: `nix eval` fallback
        if self.enable_nix_eval
            && let Some(path) = self
                .query_nix_eval(lockfile.nixpkgs.rev.as_deref(), system, attr)
                .await?
        {
            info!(%system, %attr, path = %path.to_absolute_path(), "resolved via `nix eval`");
            lockfile.insert(system, attr, &path, ResolutionSource::NixEval);
            return Ok(path);
        }

        Err(IndexError::NotFound {
            attr: attr.to_string(),
            system: system.to_string(),
        })
    }

    async fn query_hydra(&self, system: &str, attr: &str) -> Result<Option<StorePath>> {
        let base = self.hydra.base_url.trim_end_matches('/');
        let url = format!(
            "{base}/job/{}/{}/{attr}.{system}/latest",
            self.hydra.project, self.hydra.jobset
        );
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
        rev: Option<&str>,
        system: &str,
        attr: &str,
    ) -> Result<Option<StorePath>> {
        let flake_ref = match rev {
            Some(r) => format!("github:NixOS/nixpkgs/{r}"),
            None => "nixpkgs".to_string(),
        };
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
