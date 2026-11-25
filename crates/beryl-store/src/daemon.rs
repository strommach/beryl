//! `DaemonStore`: unprivileged client speaking the `nix-daemon` worker protocol
//! over `/nix/var/nix/daemon-socket/socket`.

use std::collections::{BTreeSet, VecDeque};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beryl_nix::store_path::StorePath;
use beryl_nix::store_path_ext::parse_store_path;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::{debug, trace};

use crate::error::{Result, StoreError};
use crate::store::{Store, ValidPathInfo};

/// Default Unix socket path for multi-user `nix-daemon`.
pub const DEFAULT_DAEMON_SOCKET: &str = "/nix/var/nix/daemon-socket/socket";

const WORKER_MAGIC_1: u64 = 0x6e69_7863; // "nixc"
const WORKER_MAGIC_2: u64 = 0x6478_696f; // "dxio"
const CLIENT_PROTOCOL_VERSION: u64 = (1 << 8) | 35; // 1.35

const OP_IS_VALID_PATH: u64 = 1;
const OP_ADD_TEXT_TO_STORE: u64 = 8;
const OP_BUILD_PATHS: u64 = 9;
const OP_ENSURE_PATH: u64 = 10;
const OP_ADD_INDIRECT_ROOT: u64 = 12;
const OP_QUERY_PATH_INFO: u64 = 26;
const OP_ADD_TO_STORE_NAR: u64 = 39;

const STDERR_NEXT: u64 = 0x6f6c_6d67;
const STDERR_ERROR: u64 = 0x6378_7470;
const STDERR_LAST: u64 = 0x616c_7473;
const STDERR_START_ACTIVITY: u64 = 0x5354_5254;
const STDERR_STOP_ACTIVITY: u64 = 0x5354_4f50;
const STDERR_RESULT: u64 = 0x5253_4c54;

const FRAMED_CHUNK_SIZE: usize = 64 * 1024;

struct DaemonConn {
    stream: UnixStream,
    protocol_version: u64,
    daemon_version: Option<String>,
}

/// Nix store backend communicating with a running `nix-daemon` over a Unix socket.
#[derive(Clone)]
pub struct DaemonStore {
    socket_path: PathBuf,
    conn: Arc<Mutex<DaemonConn>>,
}

impl DaemonStore {
    /// Connect to the default `/nix/var/nix/daemon-socket/socket`.
    pub async fn connect_default() -> Result<Self> {
        Self::connect(DEFAULT_DAEMON_SOCKET).await
    }

    /// Connect to a `nix-daemon` Unix socket and perform the worker protocol handshake.
    pub async fn connect(socket_path: impl Into<PathBuf>) -> Result<Self> {
        let socket_path = socket_path.into();
        let mut stream = UnixStream::connect(&socket_path).await?;

        // 1. Magic exchange
        write_u64(&mut stream, WORKER_MAGIC_1).await?;
        stream.flush().await?;

        let magic2 = read_u64(&mut stream).await?;
        if magic2 != WORKER_MAGIC_2 {
            return Err(StoreError::Daemon(format!(
                "invalid daemon magic: expected {WORKER_MAGIC_2:#x}, got {magic2:#x}"
            )));
        }

        let server_version = read_u64(&mut stream).await?;
        let negotiated = server_version.min(CLIENT_PROTOCOL_VERSION);

        write_u64(&mut stream, CLIENT_PROTOCOL_VERSION).await?;
        if negotiated >= version(1, 14) {
            write_u64(&mut stream, 0).await?; // obsolete CPU affinity
        }
        if negotiated >= version(1, 11) {
            write_u64(&mut stream, 0).await?; // obsolete reserveSpace
        }
        stream.flush().await?;

        let daemon_version = if negotiated >= version(1, 33) {
            Some(read_string(&mut stream).await?)
        } else {
            None
        };

        if negotiated >= version(1, 35) {
            let _trust = read_u64(&mut stream).await?;
        }

        process_stderr(&mut stream, negotiated).await?;

        debug!(
            socket = %socket_path.display(),
            protocol_minor = negotiated & 0xff,
            daemon_version = ?daemon_version,
            "connected to nix-daemon"
        );

        Ok(Self {
            socket_path,
            conn: Arc::new(Mutex::new(DaemonConn {
                stream,
                protocol_version: negotiated,
                daemon_version,
            })),
        })
    }

    /// Socket path this client is connected to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Negotiated protocol version `(major, minor)`.
    pub async fn protocol_version(&self) -> (u64, u64) {
        let v = self.conn.lock().await.protocol_version;
        (v >> 8, v & 0xff)
    }

    /// Daemon's reported Nix version string, if advertised.
    pub async fn daemon_version(&self) -> Option<String> {
        self.conn.lock().await.daemon_version.clone()
    }

    /// Ask `nix-daemon` to ensure `path` is present in `/nix/store` (substituting from
    /// configured binary caches if necessary).
    pub async fn ensure_path(&self, path: &StorePath) -> Result<()> {
        let mut guard = self.conn.lock().await;
        let proto = guard.protocol_version;
        write_u64(&mut guard.stream, OP_ENSURE_PATH).await?;
        write_string(&mut guard.stream, &path.to_absolute_path()).await?;
        guard.stream.flush().await?;

        process_stderr(&mut guard.stream, proto).await?;
        let _ok = read_u64(&mut guard.stream).await?;
        Ok(())
    }

    /// Add a text file (such as an ATerm `.drv`) to `/nix/store` via `nix-daemon`.
    ///
    /// `suffix` is the store path name (e.g. `"hello-1.0.drv"`).
    /// Returns the resulting `StorePath` registered by the daemon.
    pub async fn add_text_to_store(
        &self,
        suffix: &str,
        contents: &str,
        references: &BTreeSet<StorePath>,
    ) -> Result<StorePath> {
        let mut guard = self.conn.lock().await;
        let proto = guard.protocol_version;
        let refs: Vec<String> = references.iter().map(|r| r.to_absolute_path()).collect();

        write_u64(&mut guard.stream, OP_ADD_TEXT_TO_STORE).await?;
        write_string(&mut guard.stream, suffix).await?;
        write_string(&mut guard.stream, contents).await?;
        write_string_list(&mut guard.stream, &refs).await?;
        guard.stream.flush().await?;

        process_stderr(&mut guard.stream, proto).await?;
        let raw_path = read_string(&mut guard.stream).await?;
        Ok(parse_store_path(&raw_path)?)
    }

    /// Ask `nix-daemon` to build one or more derived paths (e.g. `"/nix/store/...-foo.drv!out"`).
    pub async fn build_paths(&self, derived_paths: &[String]) -> Result<()> {
        let mut guard = self.conn.lock().await;
        let proto = guard.protocol_version;

        write_u64(&mut guard.stream, OP_BUILD_PATHS).await?;
        write_string_list(&mut guard.stream, derived_paths).await?;
        write_u64(&mut guard.stream, 0).await?; // BuildMode::Normal
        guard.stream.flush().await?;

        process_stderr(&mut guard.stream, proto).await?;
        let _ok = read_u64(&mut guard.stream).await?;
        Ok(())
    }

    /// Register an indirect GC root symlink (e.g. a user profile generation link) with
    /// `nix-daemon` so `nix-collect-garbage` preserves the referenced store closure.
    pub async fn add_indirect_root(&self, symlink_path: &Path) -> Result<()> {
        let mut guard = self.conn.lock().await;
        let proto = guard.protocol_version;

        write_u64(&mut guard.stream, OP_ADD_INDIRECT_ROOT).await?;
        write_string(&mut guard.stream, &symlink_path.display().to_string()).await?;
        guard.stream.flush().await?;

        process_stderr(&mut guard.stream, proto).await?;
        let _ok = read_u64(&mut guard.stream).await?;
        Ok(())
    }
}

impl Store for DaemonStore {
    async fn has_path(&self, path: &StorePath) -> Result<bool> {
        let mut guard = self.conn.lock().await;
        let proto = guard.protocol_version;
        write_u64(&mut guard.stream, OP_IS_VALID_PATH).await?;
        write_string(&mut guard.stream, &path.to_absolute_path()).await?;
        guard.stream.flush().await?;

        process_stderr(&mut guard.stream, proto).await?;
        let valid = read_u64(&mut guard.stream).await?;
        Ok(valid != 0)
    }

    async fn query_path_info(&self, path: &StorePath) -> Result<Option<ValidPathInfo>> {
        let mut guard = self.conn.lock().await;
        let proto = guard.protocol_version;
        write_u64(&mut guard.stream, OP_QUERY_PATH_INFO).await?;
        write_string(&mut guard.stream, &path.to_absolute_path()).await?;
        guard.stream.flush().await?;

        process_stderr(&mut guard.stream, proto).await?;
        let valid = read_u64(&mut guard.stream).await?;
        if valid == 0 {
            return Ok(None);
        }

        let deriver_str = read_string(&mut guard.stream).await?;
        let nar_hash_hex = read_string(&mut guard.stream).await?;
        let ref_strs = read_string_set(&mut guard.stream).await?;
        let reg_time = read_u64(&mut guard.stream).await? as i64;
        let nar_size = read_u64(&mut guard.stream).await?;
        let _ultimate = read_u64(&mut guard.stream).await?;
        let sigs = read_string_set(&mut guard.stream).await?;
        let ca_str = read_string(&mut guard.stream).await?;

        let deriver = if deriver_str.is_empty() {
            None
        } else {
            Some(parse_store_path(&deriver_str)?)
        };

        let nar_hash_bytes = hex::decode(nar_hash_hex.strip_prefix("sha256:").unwrap_or(&nar_hash_hex))
            .map_err(|_| StoreError::Daemon(format!("invalid narHash hex: {nar_hash_hex}")))?;
        let nar_hash: [u8; 32] = nar_hash_bytes
            .try_into()
            .map_err(|_| StoreError::Daemon(format!("invalid narHash length: {nar_hash_hex}")))?;

        let mut references = BTreeSet::new();
        for r in ref_strs {
            references.insert(parse_store_path(&r)?);
        }

        let ca = if ca_str.is_empty() {
            None
        } else {
            Some(ca_str)
        };

        Ok(Some(ValidPathInfo {
            path: path.clone(),
            nar_hash,
            nar_size,
            references,
            deriver,
            sigs,
            ca,
            registration_time: reg_time,
        }))
    }

    fn real_path(&self, path: &StorePath) -> PathBuf {
        PathBuf::from(path.to_absolute_path())
    }

    async fn add_nar<R: BufRead + Send + 'static>(
        &self,
        info: ValidPathInfo,
        mut nar_reader: R,
    ) -> Result<()> {
        // Read NAR into memory on a blocking thread so we don't stall the async runtime.
        let nar_bytes = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
            let mut buf = Vec::with_capacity(info.nar_size as usize);
            nar_reader.read_to_end(&mut buf)?;
            Ok(buf)
        })
        .await
        .map_err(|e| StoreError::Join(e.to_string()))??;

        let mut guard = self.conn.lock().await;
        let proto = guard.protocol_version;

        write_u64(&mut guard.stream, OP_ADD_TO_STORE_NAR).await?;
        write_string(&mut guard.stream, &info.path.to_absolute_path()).await?;
        write_string(
            &mut guard.stream,
            &info
                .deriver
                .as_ref()
                .map(|d| d.to_absolute_path())
                .unwrap_or_default(),
        )
        .await?;
        write_string(&mut guard.stream, &hex::encode(info.nar_hash)).await?;

        let refs: Vec<String> = info
            .references
            .iter()
            .map(|r| r.to_absolute_path())
            .collect();
        write_string_list(&mut guard.stream, &refs).await?;
        write_u64(&mut guard.stream, info.registration_time.max(0) as u64).await?;
        write_u64(&mut guard.stream, info.nar_size).await?;
        write_u64(&mut guard.stream, 0).await?; // ultimate = false
        write_string_list(&mut guard.stream, &info.sigs).await?;
        write_string(&mut guard.stream, info.ca.as_deref().unwrap_or("")).await?;
        write_u64(&mut guard.stream, 0).await?; // repair = false
        write_u64(&mut guard.stream, 0).await?; // dontCheckSigs = false

        // Framed NAR transfer (protocol >= 1.23)
        for chunk in nar_bytes.chunks(FRAMED_CHUNK_SIZE) {
            write_u64(&mut guard.stream, chunk.len() as u64).await?;
            guard.stream.write_all(chunk).await?;
        }
        write_u64(&mut guard.stream, 0).await?; // EOF frame
        guard.stream.flush().await?;

        process_stderr(&mut guard.stream, proto).await?;
        Ok(())
    }

    async fn query_closure(&self, root: &StorePath) -> Result<BTreeSet<StorePath>> {
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(root.clone());

        while let Some(current) = queue.pop_front() {
            if !visited.insert(current.clone()) {
                continue;
            }
            let info = self
                .query_path_info(&current)
                .await?
                .ok_or_else(|| StoreError::NotFound(current.to_absolute_path()))?;
            for r in info.references {
                if !visited.contains(&r) {
                    queue.push_back(r);
                }
            }
        }
        Ok(visited)
    }
}

const fn version(major: u64, minor: u64) -> u64 {
    (major << 8) | minor
}

async fn process_stderr<R: AsyncRead + Unpin>(r: &mut R, proto: u64) -> Result<()> {
    loop {
        let msg = read_u64(r).await?;
        match msg {
            STDERR_LAST => return Ok(()),
            STDERR_NEXT => {
                let text = read_string(r).await?;
                trace!(daemon_msg = %text, "nix-daemon stderr");
            }
            STDERR_START_ACTIVITY => {
                let _act = read_u64(r).await?;
                let _lvl = read_u64(r).await?;
                let _ty = read_u64(r).await?;
                let _s = read_string(r).await?;
                skip_fields(r).await?;
                let _parent = read_u64(r).await?;
            }
            STDERR_STOP_ACTIVITY => {
                let _act = read_u64(r).await?;
            }
            STDERR_RESULT => {
                let _act = read_u64(r).await?;
                let _ty = read_u64(r).await?;
                skip_fields(r).await?;
            }
            STDERR_ERROR => {
                if proto >= version(1, 26) {
                    let _err_type = read_string(r).await?;
                    let _level = read_u64(r).await?;
                    let _name = read_string(r).await?;
                    let err_msg = read_string(r).await?;
                    let have_pos = read_u64(r).await?;
                    if have_pos != 0 {
                        let _pos = read_u64(r).await?;
                    }
                    let num_traces = read_u64(r).await?;
                    for _ in 0..num_traces {
                        let hp = read_u64(r).await?;
                        if hp != 0 {
                            let _pos = read_u64(r).await?;
                        }
                        let _trace_msg = read_string(r).await?;
                    }
                    return Err(StoreError::Daemon(err_msg));
                } else {
                    let err_msg = read_string(r).await?;
                    let _status = read_u64(r).await?;
                    return Err(StoreError::Daemon(err_msg));
                }
            }
            other => {
                return Err(StoreError::Daemon(format!(
                    "unexpected stderr opcode from nix-daemon: {other:#x}"
                )));
            }
        }
    }
}

async fn skip_fields<R: AsyncRead + Unpin>(r: &mut R) -> Result<()> {
    let count = read_u64(r).await?;
    for _ in 0..count {
        let field_type = read_u64(r).await?;
        match field_type {
            0 => {
                let _ = read_u64(r).await?;
            }
            1 => {
                let _ = read_string(r).await?;
            }
            other => {
                return Err(StoreError::Daemon(format!(
                    "unknown logger field type: {other}"
                )));
            }
        }
    }
    Ok(())
}

async fn write_u64<W: AsyncWrite + Unpin>(w: &mut W, val: u64) -> Result<()> {
    w.write_all(&val.to_le_bytes()).await?;
    Ok(())
}

async fn read_u64<R: AsyncRead + Unpin>(r: &mut R) -> Result<u64> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf).await?;
    Ok(u64::from_le_bytes(buf))
}

async fn write_string<W: AsyncWrite + Unpin>(w: &mut W, s: &str) -> Result<()> {
    let bytes = s.as_bytes();
    write_u64(w, bytes.len() as u64).await?;
    w.write_all(bytes).await?;
    let pad = (8 - (bytes.len() % 8)) % 8;
    if pad > 0 {
        w.write_all(&[0u8; 8][..pad]).await?;
    }
    Ok(())
}

async fn read_string<R: AsyncRead + Unpin>(r: &mut R) -> Result<String> {
    let len = read_u64(r).await? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    let pad = (8 - (len % 8)) % 8;
    if pad > 0 {
        let mut discard = [0u8; 8];
        r.read_exact(&mut discard[..pad]).await?;
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn read_string_set<R: AsyncRead + Unpin>(r: &mut R) -> Result<Vec<String>> {
    let count = read_u64(r).await? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_string(r).await?);
    }
    Ok(out)
}

async fn write_string_list<W: AsyncWrite + Unpin>(w: &mut W, items: &[String]) -> Result<()> {
    write_u64(w, items.len() as u64).await?;
    for item in items {
        write_string(w, item).await?;
    }
    Ok(())
}
