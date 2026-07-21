//! Scanning a math folder into a manifest and hashing its files.
//!
//! A math folder is content-addressed: every file is identified by the SHA-256
//! of its bytes, so the server can tell which blobs it already has. Scanning
//! and hashing are kept free of any network or CLI-error types to stay unit
//! testable.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

/// 64 KiB read buffer — large enough to keep syscall overhead negligible on the
/// multi-hundred-MB `books_*.jsonl.zst` files, small enough to stay off the
/// stack and out of the way of the OS page cache.
const READ_BUF: usize = 64 * 1024;

/// Max files in a front bundle. Mirrors the server's `MAX_BUNDLE_FILES`
/// (`api::shares`): a web build is many small assets, so it is far larger than a
/// math manifest, but still bounded. Enforced client-side for a clear error
/// before the commit round-trips to a 422.
pub const MAX_BUNDLE_FILES: usize = 2000;

/// One file discovered under the math folder, before it is hashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Path relative to the math folder, always forward-slashed.
    pub rel_path: String,
    /// Path on disk used to open the file (may be relative to the cwd).
    pub path: PathBuf,
    pub size: u64,
}

/// A [`ManifestEntry`] with its content hash filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedFile {
    pub rel_path: String,
    pub path: PathBuf,
    pub size: u64,
    /// Lowercase 64-character hex SHA-256 of the file's bytes.
    pub hash: String,
}

/// Why a folder could not be turned into a manifest.
#[derive(Debug, Error)]
pub enum ScanError {
    #[error("{0} is not a directory")]
    NotADirectory(String),
    #[error("not a math folder: index.json is missing")]
    MissingIndex,
    #[error("not a front bundle: index.html is missing at the root")]
    MissingIndexHtml,
    #[error("a front bundle may contain at most {max} files (found {found})")]
    TooManyFiles { found: usize, max: usize },
    #[error("refusing to follow symlink: {0}")]
    Symlink(String),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// Walks a math folder recursively and returns its files as a sorted manifest.
///
/// Enforces the math-folder contract: `index.json` must exist at the root;
/// dotfiles and dot-directories are skipped; symlinks are rejected outright
/// (a CI checkout should never contain them, and following them could smuggle
/// bytes from outside the folder). Entries are sorted by their relative path so
/// a manifest is deterministic regardless of readdir order.
pub fn scan_manifest(root: &Path) -> Result<Vec<ManifestEntry>, ScanError> {
    ensure_dir(root)?;
    if !root.join("index.json").is_file() {
        return Err(ScanError::MissingIndex);
    }
    walk_sorted(root)
}

/// Walks a front-bundle folder (a web build) into a sorted manifest.
///
/// Same content-addressing and path rules as [`scan_manifest`] — dotfiles
/// skipped, symlinks rejected, forward-slashed relative paths — but the required
/// root file is `index.html` (the bundle's SPA entry) rather than `index.json`,
/// and the bundle is capped at [`MAX_BUNDLE_FILES`] files.
pub fn scan_front_manifest(root: &Path) -> Result<Vec<ManifestEntry>, ScanError> {
    ensure_dir(root)?;
    if !root.join("index.html").is_file() {
        return Err(ScanError::MissingIndexHtml);
    }
    let out = walk_sorted(root)?;
    if out.len() > MAX_BUNDLE_FILES {
        return Err(ScanError::TooManyFiles {
            found: out.len(),
            max: MAX_BUNDLE_FILES,
        });
    }
    Ok(out)
}

fn ensure_dir(root: &Path) -> Result<(), ScanError> {
    if root.is_dir() {
        Ok(())
    } else {
        Err(ScanError::NotADirectory(root.display().to_string()))
    }
}

/// The shared recursive walk: skip dotfiles, reject symlinks, collect regular
/// files as [`ManifestEntry`]s, and sort by relative path for determinism.
fn walk_sorted(root: &Path) -> Result<Vec<ManifestEntry>, ScanError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name();
            // Dotfiles/dirs (`.git`, `.DS_Store`, …) are never part of a bundle.
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            // `symlink_metadata` does not follow the link, so we can detect and
            // reject one instead of silently traversing it.
            let md = std::fs::symlink_metadata(&path)?;
            let ft = md.file_type();
            if ft.is_symlink() {
                return Err(ScanError::Symlink(path.display().to_string()));
            }
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                out.push(ManifestEntry {
                    rel_path: rel_path(root, &path)?,
                    path,
                    size: md.len(),
                });
            }
            // Anything else (fifo, socket, …) is not a regular file: skip it.
        }
    }

    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// Streams a file through SHA-256 and returns the lowercase hex digest.
///
/// Reads in [`READ_BUF`]-sized chunks so arbitrarily large books hash in
/// constant memory.
pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; READ_BUF];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Renders bytes as lowercase hex without pulling in the `hex` crate.
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// Relative path from `root` to `path`, joined with forward slashes so the wire
/// manifest is identical on Windows and Unix.
fn rel_path(root: &Path, path: &Path) -> Result<String, ScanError> {
    let rel = path
        .strip_prefix(root)
        .map_err(|e| ScanError::Io(io::Error::other(e.to_string())))?;
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn hashes_known_vector() {
        // NIST/RFC vector: SHA-256("abc").
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f.bin");
        write(&f, b"abc");
        assert_eq!(
            hash_file(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hashes_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("empty");
        write(&f, b"");
        assert_eq!(
            hash_file(&f).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn scans_sorted_relative_forward_slash_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("index.json"), b"{}");
        write(&root.join("lookuptable_base.csv"), b"a");
        write(&root.join("nested").join("books_base.jsonl.zst"), b"b");

        let entries = scan_manifest(root).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.rel_path.as_str()).collect();
        // Sorted, relative, forward-slashed — even the nested one.
        assert_eq!(
            paths,
            [
                "index.json",
                "lookuptable_base.csv",
                "nested/books_base.jsonl.zst"
            ]
        );
    }

    #[test]
    fn skips_dotfiles_and_dot_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("index.json"), b"{}");
        write(&root.join(".hidden"), b"x");
        write(&root.join(".git").join("config"), b"y");

        let paths: Vec<String> = scan_manifest(root)
            .unwrap()
            .into_iter()
            .map(|e| e.rel_path)
            .collect();
        assert_eq!(paths, ["index.json"]);
    }

    #[test]
    fn requires_index_json() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("lookuptable_base.csv"), b"a");
        assert!(matches!(
            scan_manifest(dir.path()),
            Err(ScanError::MissingIndex)
        ));
    }

    #[test]
    fn front_scan_requires_index_html() {
        let dir = tempfile::tempdir().unwrap();
        // A build without a root index.html is not a front bundle.
        write(&dir.path().join("assets").join("app.js"), b"a");
        assert!(matches!(
            scan_front_manifest(dir.path()),
            Err(ScanError::MissingIndexHtml)
        ));
        // index.json alone does not satisfy a front bundle either.
        write(&dir.path().join("index.json"), b"{}");
        assert!(matches!(
            scan_front_manifest(dir.path()),
            Err(ScanError::MissingIndexHtml)
        ));
    }

    #[test]
    fn front_scan_accepts_index_html_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("index.html"), b"<html></html>");
        write(&root.join("assets").join("app.js"), b"a");
        write(&root.join("assets").join("style.css"), b"b");

        let paths: Vec<String> = scan_front_manifest(root)
            .unwrap()
            .into_iter()
            .map(|e| e.rel_path)
            .collect();
        assert_eq!(paths, ["assets/app.js", "assets/style.css", "index.html"]);
    }

    #[test]
    fn front_scan_rejects_too_many_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("index.html"), b"<html></html>");
        // index.html + MAX_BUNDLE_FILES extra files = one over the cap.
        for i in 0..MAX_BUNDLE_FILES {
            write(&root.join(format!("f{i}.txt")), b"x");
        }
        assert!(matches!(
            scan_front_manifest(root),
            Err(ScanError::TooManyFiles { max, .. }) if max == MAX_BUNDLE_FILES
        ));
    }

    #[test]
    fn rejects_a_non_directory() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("index.json");
        write(&f, b"{}");
        assert!(matches!(
            scan_manifest(&f),
            Err(ScanError::NotADirectory(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("index.json"), b"{}");
        write(&root.join("real.csv"), b"a");
        symlink(root.join("real.csv"), root.join("link.csv")).unwrap();
        assert!(matches!(scan_manifest(root), Err(ScanError::Symlink(_))));
    }
}
