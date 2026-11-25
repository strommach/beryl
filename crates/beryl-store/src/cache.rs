//! Async Nix binary cache client (`.narinfo` + Ed25519 verification + NAR decompression).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{BufReader, Cursor};

use beryl_nix::narinfo::VerifyingKey;
use beryl_nix::narinfo_ext::{default_nixos_verifying_key, parse_narinfo, verify_narinfo};
use beryl_nix::nixbase32;
use beryl_nix::store_path::StorePath;
use reqwest::StatusCode;
use tracing::{debug, info};

use crate::error::{Result, StoreError};
use crate::store::{Store, ValidPathInfo};

/// Configuration for a Nix HTTP binary cache (e.g. `https://cache.nixos.org`).
#[derive(Debug, Clone)]
pub struct BinaryCacheConfig {
    pub base_url: String,
    pub trusted_keys: Vec<VerifyingKey>,
    pub verify_signatures: bool,
}

impl Default for BinaryCacheConfig {
    fn default() -> Self {
        Self {
            base_url: "https://cache.nixos.org".to_string(),
            trusted_keys: vec![default_nixos_verifying_key()],
            verify_signatures: true,
        }
    }
}

/// Parsed and signature-verified `.narinfo` metadata ready to download.
#[derive(Debug, Clone)]
pub struct FetchedNarInfo {
    pub info: ValidPathInfo,
    pub url: String,
    pub compression: String,
}

/// Async client for fetching and verifying store paths from a Nix binary cache.
#[derive(Debug, Clone)]
pub struct BinaryCacheClient {
    http: reqwest::Client,
    config: BinaryCacheConfig,
}

impl BinaryCacheClient {
    pub fn new(config: BinaryCacheConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("beryl/0.1")
            .build()?;
        Ok(Self { http, config })
    }

    pub fn nixos_default() -> Result<Self> {
        Self::new(BinaryCacheConfig::default())
    }

    pub fn base_url(&self) -> &str {
        self.config.base_url.trim_end_matches('/')
    }

    /// Fetch the raw `.narinfo` text for a 32-char Nix base32 store hash.
    pub async fn fetch_narinfo_raw(&self, hash_part: &str) -> Result<Option<String>> {
        let url = format!("{}/{hash_part}.narinfo", self.base_url());
        debug!(%url, "fetching narinfo");

        let resp = self.http.get(&url).send().await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        Ok(Some(resp.text().await?))
    }

    /// Fetch, parse, and verify the `.narinfo` for `store_path`.
    pub async fn fetch_path_info(&self, store_path: &StorePath) -> Result<Option<FetchedNarInfo>> {
        let hash_part = nixbase32::encode(store_path.digest());
        let Some(raw) = self.fetch_narinfo_raw(&hash_part).await? else {
            return Ok(None);
        };

        let narinfo = parse_narinfo(&raw)?;
        if self.config.verify_signatures && !verify_narinfo(&narinfo, &self.config.trusted_keys) {
            return Err(StoreError::InvalidSignature(store_path.to_absolute_path()));
        }

        let references: BTreeSet<StorePath> =
            narinfo.references.iter().map(|r| r.to_owned()).collect();
        let deriver = narinfo.deriver.as_ref().map(|d| d.to_owned());
        let sigs = narinfo.signatures.iter().map(|s| s.to_string()).collect();
        let ca = narinfo.ca.as_ref().map(|c| c.to_string());
        let compression = narinfo.compression.unwrap_or("none").to_string();

        let info = ValidPathInfo {
            path: narinfo.store_path.to_owned(),
            nar_hash: narinfo.nar_hash,
            nar_size: narinfo.nar_size,
            references,
            deriver,
            sigs,
            ca,
            registration_time: 0,
        };

        Ok(Some(FetchedNarInfo {
            info,
            url: narinfo.url.to_string(),
            compression,
        }))
    }

    /// Download, decompress, verify, and install a single path into `store`.
    pub async fn fetch_and_install_single<S: Store>(
        &self,
        store: &S,
        fetched: &FetchedNarInfo,
    ) -> Result<()> {
        let nar_url = format!("{}/{}", self.base_url(), fetched.url.trim_start_matches('/'));
        info!(path = %fetched.info.path.to_absolute_path(), compression = %fetched.compression, "downloading NAR");

        let bytes = self
            .http
            .get(&nar_url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let info = fetched.info.clone();
        match fetched.compression.as_str() {
            "" | "none" => {
                store.add_nar(info, Cursor::new(bytes)).await?;
            }
            "xz" => {
                let decoder = xz2::read::XzDecoder::new(Cursor::new(bytes));
                store.add_nar(info, BufReader::new(decoder)).await?;
            }
            "zstd" => {
                let decoder = zstd::stream::read::Decoder::new(Cursor::new(bytes))?;
                store.add_nar(info, BufReader::new(decoder)).await?;
            }
            other => {
                return Err(StoreError::UnsupportedCompression(other.to_string()));
            }
        }

        Ok(())
    }

    /// Fetch `root` and its entire missing runtime closure into `store`,
    /// installing dependencies before dependents.
    ///
    /// Returns the list of newly installed store paths in installation order.
    pub async fn fetch_closure<S: Store>(
        &self,
        store: &S,
        root: &StorePath,
    ) -> Result<Vec<StorePath>> {
        let mut to_fetch: BTreeMap<StorePath, FetchedNarInfo> = BTreeMap::new();
        let mut queue: VecDeque<StorePath> = VecDeque::new();
        queue.push_back(root.clone());

        // 1. Discover missing closure via BFS over `.narinfo` references.
        while let Some(path) = queue.pop_front() {
            if to_fetch.contains_key(&path) || store.has_path(&path).await? {
                continue;
            }
            let fetched = self
                .fetch_path_info(&path)
                .await?
                .ok_or_else(|| StoreError::NotFound(path.to_absolute_path()))?;

            for dep in &fetched.info.references {
                if dep != &path && !to_fetch.contains_key(dep) {
                    queue.push_back(dep.clone());
                }
            }
            to_fetch.insert(path, fetched);
        }

        if to_fetch.is_empty() {
            return Ok(Vec::new());
        }

        // 2. Topologically sort missing paths so dependencies are registered first.
        let ordered = topo_sort_missing(&to_fetch);

        // 3. Download and install each path in dependency order.
        let mut installed = Vec::with_capacity(ordered.len());
        for path in ordered {
            let fetched = &to_fetch[&path];
            self.fetch_and_install_single(store, fetched).await?;
            installed.push(path);
        }

        Ok(installed)
    }
}

fn topo_sort_missing(nodes: &BTreeMap<StorePath, FetchedNarInfo>) -> Vec<StorePath> {
    let mut visited = BTreeSet::new();
    let mut result = Vec::with_capacity(nodes.len());

    fn dfs(
        path: &StorePath,
        nodes: &BTreeMap<StorePath, FetchedNarInfo>,
        visited: &mut BTreeSet<StorePath>,
        result: &mut Vec<StorePath>,
    ) {
        if !visited.insert(path.clone()) {
            return;
        }
        if let Some(fetched) = nodes.get(path) {
            for dep in &fetched.info.references {
                if dep != path && nodes.contains_key(dep) {
                    dfs(dep, nodes, visited, result);
                }
            }
        }
        result.push(path.clone());
    }

    for path in nodes.keys() {
        dfs(path, nodes, &mut visited, &mut result);
    }

    result
}
