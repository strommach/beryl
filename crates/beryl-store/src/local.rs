//! Standalone `LocalStore` backed by a configurable root directory and Nix-compatible SQLite DB.

use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::io::{self, BufRead, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use beryl_nix::nar_fs::unpack_nar;
use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::error::{Result, StoreError};
use crate::store::{Store, ValidPathInfo};

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS ValidPaths (
    id               INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    path             TEXT UNIQUE NOT NULL,
    hash             TEXT NOT NULL,
    registrationTime INTEGER NOT NULL,
    deriver          TEXT,
    narSize          INTEGER,
    ultimate         INTEGER,
    sigs             TEXT,
    ca               TEXT
);

CREATE TABLE IF NOT EXISTS Refs (
    referrer  INTEGER NOT NULL,
    reference INTEGER NOT NULL,
    PRIMARY KEY (referrer, reference),
    FOREIGN KEY (referrer) REFERENCES ValidPaths(id) ON DELETE CASCADE,
    FOREIGN KEY (reference) REFERENCES ValidPaths(id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS IndexReferrer ON Refs(referrer);
CREATE INDEX IF NOT EXISTS IndexReference ON Refs(reference);

CREATE TABLE IF NOT EXISTS DerivationOutputs (
    drv  INTEGER NOT NULL,
    id   TEXT NOT NULL,
    path TEXT NOT NULL,
    PRIMARY KEY (drv, id),
    FOREIGN KEY (drv) REFERENCES ValidPaths(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS IndexDerivationOutputs ON DerivationOutputs(path);
"#;

/// Standalone Nix-compatible store rooted at a configurable directory (default `/nix`).
#[derive(Clone)]
pub struct LocalStore {
    root: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

impl LocalStore {
    /// Open or initialize a `LocalStore` rooted at `root`.
    ///
    /// Layout:
    /// - `<root>/store/<hash>-<name>`
    /// - `<root>/var/nix/db/db.sqlite`
    /// - `<root>/var/nix/tmp/`
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let store_dir = root.join("store");
        let db_dir = root.join("var/nix/db");
        let tmp_dir = root.join("var/nix/tmp");

        fs::create_dir_all(&store_dir)?;
        fs::create_dir_all(&db_dir)?;
        fs::create_dir_all(&tmp_dir)?;

        let db_path = db_dir.join("db.sqlite");
        let conn = Connection::open(&db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(SCHEMA_SQL)?;

        Ok(Self {
            root,
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Root directory of this store (e.g. `/nix` or a temporary test directory).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory containing unpacked store objects (`<root>/store`).
    pub fn store_dir(&self) -> PathBuf {
        self.root.join("store")
    }

    /// Staging directory on the same filesystem for atomic renames (`<root>/var/nix/tmp`).
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("var/nix/tmp")
    }

    /// Return all registered store paths in this store.
    pub fn list_valid_paths(&self) -> Result<Vec<StorePath>> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let mut stmt = conn.prepare("SELECT path FROM ValidPaths ORDER BY id ASC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(parse_store_path(&row?)?);
        }
        Ok(out)
    }

    fn has_path_sync(&self, path: &StorePath) -> Result<bool> {
        let abs = path.to_absolute_path();
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let exists: Option<i64> = conn
            .query_row(
                "SELECT id FROM ValidPaths WHERE path = ?1",
                params![abs],
                |row| row.get(0),
            )
            .optional()?;
        Ok(exists.is_some())
    }

    fn query_path_info_sync(&self, path: &StorePath) -> Result<Option<ValidPathInfo>> {
        let abs = path.to_absolute_path();
        let conn = self.conn.lock().expect("sqlite mutex poisoned");

        let row = conn
            .query_row(
                "SELECT id, path, hash, registrationTime, deriver, narSize, sigs, ca
                 FROM ValidPaths WHERE path = ?1",
                params![abs],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()?;

        let Some((id, path_str, hash_str, reg_time, deriver_str, nar_size, sigs_str, ca)) = row
        else {
            return Ok(None);
        };

        let mut ref_stmt = conn.prepare(
            "SELECT vp.path
             FROM Refs r
             JOIN ValidPaths vp ON vp.id = r.reference
             WHERE r.referrer = ?1",
        )?;
        let ref_rows = ref_stmt.query_map(params![id], |r| r.get::<_, String>(0))?;
        let mut references = BTreeSet::new();
        for r in ref_rows {
            references.insert(parse_store_path(&r?)?);
        }

        let nar_hash = decode_db_sha256(&hash_str, &path_str)?;
        let deriver = match deriver_str {
            Some(s) if !s.is_empty() => Some(parse_store_path(&s)?),
            _ => None,
        };
        let sigs = sigs_str
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default();

        Ok(Some(ValidPathInfo {
            path: parse_store_path(&path_str)?,
            nar_hash,
            nar_size: nar_size.unwrap_or(0) as u64,
            references,
            deriver,
            sigs,
            ca,
            registration_time: reg_time,
        }))
    }

    fn register_valid_path_sync(&self, info: &ValidPathInfo) -> Result<()> {
        let mut conn = self.conn.lock().expect("sqlite mutex poisoned");
        let tx = conn.transaction()?;

        let abs = info.path.to_absolute_path();
        let hash_str = format!("sha256:{}", hex::encode(info.nar_hash));
        let deriver_str = info.deriver.as_ref().map(|d| d.to_absolute_path());
        let sigs_str = if info.sigs.is_empty() {
            None
        } else {
            Some(info.sigs.join(" "))
        };
        let reg_time = if info.registration_time > 0 {
            info.registration_time
        } else {
            now_unix_secs()
        };

        tx.execute(
            "INSERT OR IGNORE INTO ValidPaths
             (path, hash, registrationTime, deriver, narSize, ultimate, sigs, ca)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7)",
            params![
                abs,
                hash_str,
                reg_time,
                deriver_str,
                info.nar_size as i64,
                sigs_str,
                info.ca,
            ],
        )?;

        let referrer_id: i64 = tx.query_row(
            "SELECT id FROM ValidPaths WHERE path = ?1",
            params![abs],
            |row| row.get(0),
        )?;

        for reference in &info.references {
            let ref_abs = reference.to_absolute_path();
            let ref_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM ValidPaths WHERE path = ?1",
                    params![ref_abs],
                    |row| row.get(0),
                )
                .optional()?;

            let ref_id = ref_id.ok_or_else(|| StoreError::MissingReference {
                referrer: abs.clone(),
                reference: ref_abs,
            })?;

            tx.execute(
                "INSERT OR IGNORE INTO Refs (referrer, reference) VALUES (?1, ?2)",
                params![referrer_id, ref_id],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    fn add_nar_sync<R: BufRead + Send>(&self, info: ValidPathInfo, nar_reader: R) -> Result<()> {
        // Fast path: already registered in DB.
        if self.has_path_sync(&info.path)? {
            return Ok(());
        }

        let staging_dir = tempfile::tempdir_in(self.tmp_dir())?;
        let staging_target = staging_dir.path().join("out");

        let mut hashing_reader = HashingBufReader::new(nar_reader);
        unpack_nar(&mut hashing_reader, &staging_target)?;

        let (actual_hash, actual_size) = hashing_reader.finish();
        if actual_size != info.nar_size {
            return Err(StoreError::SizeMismatch {
                path: info.path.to_absolute_path(),
                expected: info.nar_size,
                actual: actual_size,
            });
        }
        if actual_hash != info.nar_hash {
            return Err(StoreError::HashMismatch {
                path: info.path.to_absolute_path(),
                expected: hex::encode(info.nar_hash),
                actual: hex::encode(actual_hash),
            });
        }

        let final_path = self.real_path(&info.path);
        if final_path.exists() {
            // Another writer may have raced or left an unregistered copy; replace cleanly.
            make_writable_recursive(&final_path)?;
            if final_path.is_dir() && !final_path.is_symlink() {
                fs::remove_dir_all(&final_path)?;
            } else {
                fs::remove_file(&final_path)?;
            }
        }
        fs::rename(&staging_target, &final_path)?;

        // Make the published artifact tree read-only after renaming (Linux requires write
        // permission on a directory to update its `..` link during rename).
        make_read_only_recursive(&final_path)?;

        self.register_valid_path_sync(&info)?;
        Ok(())
    }
}

impl Store for LocalStore {
    async fn has_path(&self, path: &StorePath) -> Result<bool> {
        let store = self.clone();
        let path = path.clone();
        tokio::task::spawn_blocking(move || store.has_path_sync(&path))
            .await
            .map_err(|e| StoreError::Join(e.to_string()))?
    }

    async fn query_path_info(&self, path: &StorePath) -> Result<Option<ValidPathInfo>> {
        let store = self.clone();
        let path = path.clone();
        tokio::task::spawn_blocking(move || store.query_path_info_sync(&path))
            .await
            .map_err(|e| StoreError::Join(e.to_string()))?
    }

    fn real_path(&self, path: &StorePath) -> PathBuf {
        self.store_dir().join(path.to_string())
    }

    async fn add_nar<R: BufRead + Send + 'static>(
        &self,
        info: ValidPathInfo,
        nar_reader: R,
    ) -> Result<()> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.add_nar_sync(info, nar_reader))
            .await
            .map_err(|e| StoreError::Join(e.to_string()))?
    }

    async fn query_closure(&self, root: &StorePath) -> Result<BTreeSet<StorePath>> {
        let store = self.clone();
        let root = root.clone();
        tokio::task::spawn_blocking(move || {
            let mut visited = BTreeSet::new();
            let mut queue = VecDeque::new();
            queue.push_back(root);

            while let Some(current) = queue.pop_front() {
                if !visited.insert(current.clone()) {
                    continue;
                }
                let info = store
                    .query_path_info_sync(&current)?
                    .ok_or_else(|| StoreError::NotFound(current.to_absolute_path()))?;
                for r in info.references {
                    if !visited.contains(&r) {
                        queue.push_back(r);
                    }
                }
            }
            Ok(visited)
        })
        .await
        .map_err(|e| StoreError::Join(e.to_string()))?
    }
}

/// `BufRead` wrapper that computes SHA-256 and total byte count of all consumed bytes.
struct HashingBufReader<R> {
    inner: R,
    hasher: Sha256,
    total_bytes: u64,
}

impl<R> HashingBufReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            total_bytes: 0,
        }
    }

    fn finish(self) -> ([u8; 32], u64) {
        (self.hasher.finalize().into(), self.total_bytes)
    }
}

impl<R: Read> Read for HashingBufReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.total_bytes += n as u64;
        Ok(n)
    }
}

impl<R: BufRead> BufRead for HashingBufReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        if amt > 0
            && let Ok(buf) = self.inner.fill_buf()
        {
            let slice = &buf[..amt.min(buf.len())];
            self.hasher.update(slice);
            self.total_bytes += slice.len() as u64;
        }
        self.inner.consume(amt);
    }
}

fn decode_db_sha256(hash_str: &str, path: &str) -> Result<[u8; 32]> {
    let hex_part = hash_str.strip_prefix("sha256:").unwrap_or(hash_str);
    let bytes = hex::decode(hex_part).map_err(|_| StoreError::HashMismatch {
        path: path.to_string(),
        expected: "64-char hex sha256".to_string(),
        actual: hash_str.to_string(),
    })?;
    bytes.try_into().map_err(|_| StoreError::HashMismatch {
        path: path.to_string(),
        expected: "32-byte sha256".to_string(),
        actual: hash_str.to_string(),
    })
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn make_read_only_recursive(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            make_read_only_recursive(&entry?.path())?;
        }
    }
    let mode = meta.permissions().mode() & !0o222;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn make_writable_recursive(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let mode = meta.permissions().mode() | 0o200;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            make_writable_recursive(&entry?.path())?;
        }
    }
    Ok(())
}
