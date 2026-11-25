//! Filesystem <-> NAR streaming utilities (unpack, dump, and single-pass NAR hashing).

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use nix_compat::nar::reader::{self, Node as ReadNode};
use nix_compat::nar::writer::{self, Node as WriteNode};
use sha2::{Digest, Sha256};

use crate::error::{NixCompatError, Result};

/// Result of hashing a filesystem path as a NAR archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NarHashResult {
    /// SHA-256 digest of the canonical NAR byte stream.
    pub sha256: [u8; 32],
    /// Total length of the canonical NAR byte stream in bytes.
    pub size: u64,
}

/// Unpack a NAR stream into `dest`.
///
/// `dest` must not already exist (or must be removable/creatable at the target path).
/// Rejects path traversal entries (`.`, `..`, or names containing `/` or NUL).
pub fn unpack_nar<R: BufRead + Send>(mut reader: R, dest: &Path) -> Result<()> {
    let node = reader::open(&mut reader)?;
    unpack_node(node, dest)
}

fn unpack_node(node: ReadNode<'_, '_>, dest: &Path) -> Result<()> {
    match node {
        ReadNode::Symlink { target } => {
            let target_os = OsStr::from_bytes(&target);
            symlink(target_os, dest)?;
            Ok(())
        }
        ReadNode::File {
            executable,
            mut reader,
        } => {
            let mut file = File::create(dest)?;
            reader.copy(&mut file)?;
            let mode = if executable { 0o755 } else { 0o644 };
            fs::set_permissions(dest, fs::Permissions::from_mode(mode))?;
            Ok(())
        }
        ReadNode::Directory(mut dir_reader) => {
            fs::create_dir(dest)?;
            while let Some(entry) = dir_reader.next()? {
                validate_entry_name(entry.name)?;
                let child_path = dest.join(OsStr::from_bytes(entry.name));
                unpack_node(entry.node, &child_path)?;
            }
            Ok(())
        }
    }
}

fn validate_entry_name(name: &[u8]) -> Result<()> {
    if name.is_empty()
        || name == b"."
        || name == b".."
        || name.contains(&b'/')
        || name.contains(&0)
    {
        return Err(NixCompatError::InvalidNarEntryName(name.to_vec()));
    }
    Ok(())
}

/// Dump a filesystem path (file, symlink, or directory) into a canonical NAR stream.
pub fn dump_nar<W: Write>(src: &Path, writer: &mut W) -> Result<()> {
    let node = writer::open(writer)?;
    dump_node(src, node)
}

fn dump_node<W: Write>(src: &Path, node: WriteNode<'_, W>) -> Result<()> {
    let meta = fs::symlink_metadata(src)?;
    let ft = meta.file_type();

    if ft.is_symlink() {
        let target = fs::read_link(src)?;
        node.symlink(target.as_os_str().as_bytes())?;
        Ok(())
    } else if ft.is_file() {
        let executable = (meta.permissions().mode() & 0o111) != 0;
        let size = meta.len();
        let file = File::open(src)?;
        let mut reader = BufReader::new(file);
        node.file(executable, size, &mut reader)?;
        Ok(())
    } else if ft.is_dir() {
        let mut entries: Vec<(Vec<u8>, std::path::PathBuf)> = Vec::new();
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let name = entry.file_name().as_bytes().to_vec();
            entries.push((name, entry.path()));
        }
        // NAR spec requires directory entries to be sorted lexicographically by raw byte order.
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let mut dir = node.directory()?;
        for (name, path) in entries {
            let child_node = dir.entry(&name)?;
            dump_node(&path, child_node)?;
        }
        dir.close()?;
        Ok(())
    } else {
        Err(NixCompatError::UnsupportedFsEntry(src.to_path_buf()))
    }
}

/// Compute the canonical NAR SHA-256 hash and byte size of `src` in a single streaming pass.
pub fn hash_nar_path(src: &Path) -> Result<NarHashResult> {
    struct HashWriter {
        hasher: Sha256,
        size: u64,
    }

    impl Write for HashWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.hasher.update(buf);
            self.size += buf.len() as u64;
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut hw = HashWriter {
        hasher: Sha256::new(),
        size: 0,
    };
    dump_nar(src, &mut hw)?;

    Ok(NarHashResult {
        sha256: hw.hasher.finalize().into(),
        size: hw.size,
    })
}
