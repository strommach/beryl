//! Fixed-output derivation executor for `builtin:fetchurl`.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Cursor, Write};

use beryl_nix::derivation::{Derivation, OutputName};
use beryl_nix::nar_fs::{dump_nar, hash_nar_path};
use beryl_store::{LocalStore, Store, ValidPathInfo};
use sha2::{Digest, Sha256};
use tracing::info;

use crate::error::{BuildError, Result};

/// Execute a `builtin:fetchurl` fixed-output derivation, verify its SHA-256 hash,
/// and register the resulting file in `store`.
pub async fn run_builtin_fetchurl(store: &LocalStore, drv: &Derivation) -> Result<()> {
    let out_path = drv
        .outputs()
        .get(&OutputName::out())
        .expect("FOD has `out` output")
        .clone();

    if store.has_path(&out_path).await? {
        return Ok(());
    }

    let url = get_env_str(drv, "url")?;
    let expected_hex = get_env_str(drv, "outputHash")?;

    info!(%url, out = %out_path.to_absolute_path(), "fetching fixed-output source");

    let http = reqwest::Client::builder()
        .user_agent("beryl/0.1")
        .build()?;
    let bytes = http
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    let actual_digest: [u8; 32] = Sha256::digest(&bytes).into();
    let actual_hex = hex::encode(actual_digest);

    if actual_hex != expected_hex.to_lowercase() {
        return Err(BuildError::FixedOutputHashMismatch {
            path: out_path.to_absolute_path(),
            expected: expected_hex,
            actual: actual_hex,
        });
    }

    let tmp = tempfile::tempdir_in(store.tmp_dir())?;
    let file_path = tmp.path().join("fetched");
    {
        let mut f = File::create(&file_path)?;
        f.write_all(&bytes)?;
    }

    let hashed = hash_nar_path(&file_path)?;
    let mut nar = Vec::with_capacity(hashed.size as usize);
    dump_nar(&file_path, &mut nar)?;

    let info = ValidPathInfo {
        path: out_path,
        nar_hash: hashed.sha256,
        nar_size: hashed.size,
        references: BTreeSet::new(),
        deriver: Some(drv.drv_path().clone()),
        sigs: vec![],
        ca: Some(format!("fixed:sha256:{actual_hex}")),
        registration_time: 0,
    };

    store.add_nar(info, Cursor::new(nar)).await?;
    Ok(())
}

fn get_env_str(drv: &Derivation, key: &str) -> Result<String> {
    drv.environment()
        .get(key.as_bytes())
        .map(|v| String::from_utf8_lossy(v.as_slice()).to_string())
        .ok_or_else(|| BuildError::MissingEnvVar(key.to_string()))
}
