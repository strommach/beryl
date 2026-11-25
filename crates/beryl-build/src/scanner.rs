//! Post-build 32-character Nix base32 store-hash reference scanner.
//!
//! Scans all files and symlink targets in a build output against the set of candidate
//! store paths (transitive input closure + self) using Aho-Corasick multi-pattern matching
//! with overlapping chunk reads so hashes spanning buffer boundaries are never missed.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use aho_corasick::AhoCorasick;
use beryl_nix::nixbase32;
use beryl_nix::store_path::StorePath;

use crate::error::Result;

const CHUNK_SIZE: usize = 64 * 1024;
const HASH_LEN: usize = 32;

/// Scan `output_path` (file, symlink, or directory tree) for occurrences of any
/// 32-char Nix base32 digest belonging to `candidates`.
pub fn scan_references(
    output_path: &Path,
    candidates: &BTreeSet<StorePath>,
) -> Result<BTreeSet<StorePath>> {
    if candidates.is_empty() {
        return Ok(BTreeSet::new());
    }

    let ordered: Vec<StorePath> = candidates.iter().cloned().collect();
    let patterns: Vec<String> = ordered
        .iter()
        .map(|sp| nixbase32::encode(sp.digest()))
        .collect();

    let ac = AhoCorasick::new(&patterns).expect("valid 32-byte ASCII patterns");
    let mut found = BTreeSet::new();

    scan_recursive(output_path, &ac, &ordered, &mut found)?;
    Ok(found)
}

fn scan_recursive(
    path: &Path,
    ac: &AhoCorasick,
    ordered: &[StorePath],
    found: &mut BTreeSet<StorePath>,
) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    let ft = meta.file_type();

    if ft.is_symlink() {
        let target = fs::read_link(path)?;
        scan_bytes(target.as_os_str().as_bytes(), ac, ordered, found);
    } else if ft.is_file() {
        let mut file = File::open(path)?;
        let mut buf = vec![0u8; CHUNK_SIZE + HASH_LEN];
        let mut carry = 0usize;

        loop {
            let n = file.read(&mut buf[carry..])?;
            if n == 0 {
                break;
            }
            let total = carry + n;
            scan_bytes(&buf[..total], ac, ordered, found);

            // Preserve the last (HASH_LEN - 1) bytes for boundary-spanning matches.
            let keep = total.min(HASH_LEN - 1);
            buf.copy_within(total - keep..total, 0);
            carry = keep;
        }
    } else if ft.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            scan_recursive(&entry.path(), ac, ordered, found)?;
        }
    }

    Ok(())
}

fn scan_bytes(
    data: &[u8],
    ac: &AhoCorasick,
    ordered: &[StorePath],
    found: &mut BTreeSet<StorePath>,
) {
    for mat in ac.find_overlapping_iter(data) {
        let idx = mat.pattern().as_usize();
        found.insert(ordered[idx].clone());
    }
}
