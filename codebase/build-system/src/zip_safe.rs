//! Hardened ZIP extraction shared by `fetch` and the registry resolver.
//!
//! Defends against classic archive attacks:
//!   * zip-bomb (uncompressed-size blow-up)
//!   * path traversal via `..`, absolute paths, Windows drive prefixes,
//!     backslash separators, or NUL bytes
//!   * symlink entries (rejected outright; never written)
//!   * runaway entry counts and directory depth
//!   * partial / interrupted writes (extraction goes to a tempdir and is
//!     atomically renamed into place; the destination only ever sees a
//!     fully-validated tree)
//!
//! The check strategy is deliberately string-/component-based instead of
//! relying on `Path::canonicalize`: canonicalize touches the filesystem and
//! can be racy, and we want to reject malicious entries before we create
//! anything on disk.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

/// Numerical limits applied to a ZIP archive during extraction.
///
/// Defaults are tuned for "small source-only Gradient packages" — they should
/// fit comfortably inside any sane registry payload while still tripping on
/// pathological archives.
#[derive(Debug, Clone, Copy)]
pub struct ExtractLimits {
    /// Maximum total uncompressed bytes written across all entries.
    pub max_total_uncompressed: u64,
    /// Maximum uncompressed bytes for a single entry.
    pub max_entry_uncompressed: u64,
    /// Maximum number of entries (files + directories) processed.
    pub max_entries: u64,
    /// Maximum directory depth (number of path components) for any entry.
    pub max_depth: usize,
}

impl Default for ExtractLimits {
    fn default() -> Self {
        ExtractLimits {
            // 256 MiB total, generous but bounded.
            max_total_uncompressed: 256 * 1024 * 1024,
            // 64 MiB per file.
            max_entry_uncompressed: 64 * 1024 * 1024,
            // 10k entries.
            max_entries: 10_000,
            // 32 directory levels.
            max_depth: 32,
        }
    }
}

/// Errors produced by [`safe_extract`].
#[derive(Debug)]
pub enum ExtractError {
    /// Underlying I/O failure.
    Io(io::Error),
    /// `zip` crate failed to read the archive structure.
    Zip(zip::result::ZipError),
    /// Total uncompressed payload exceeded `max_total_uncompressed`.
    TotalSizeExceeded { limit: u64 },
    /// A single entry's uncompressed size exceeded `max_entry_uncompressed`.
    EntrySizeExceeded { name: String, limit: u64 },
    /// Number of entries exceeded `max_entries`.
    EntryCountExceeded { limit: u64 },
    /// Path depth exceeded `max_depth`.
    DepthExceeded { name: String, limit: usize },
    /// Entry path is not safe (traversal, absolute, drive prefix, NUL, …).
    UnsafePath { name: String, reason: &'static str },
    /// Symlink entry encountered (always rejected).
    SymlinkRejected { name: String },
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Io(e) => write!(f, "i/o error during zip extraction: {}", e),
            ExtractError::Zip(e) => write!(f, "malformed zip archive: {}", e),
            ExtractError::TotalSizeExceeded { limit } => write!(
                f,
                "zip extraction aborted: total uncompressed size exceeds {} bytes",
                limit
            ),
            ExtractError::EntrySizeExceeded { name, limit } => write!(
                f,
                "zip entry '{}' exceeds per-entry uncompressed limit of {} bytes",
                name, limit
            ),
            ExtractError::EntryCountExceeded { limit } => {
                write!(f, "zip archive contains more than {} entries", limit)
            }
            ExtractError::DepthExceeded { name, limit } => write!(
                f,
                "zip entry '{}' exceeds max path depth of {}",
                name, limit
            ),
            ExtractError::UnsafePath { name, reason } => {
                write!(f, "zip entry '{}' rejected: {}", name, reason)
            }
            ExtractError::SymlinkRejected { name } => {
                write!(f, "zip entry '{}' rejected: symlinks are not allowed", name)
            }
        }
    }
}

impl std::error::Error for ExtractError {}

impl From<io::Error> for ExtractError {
    fn from(e: io::Error) -> Self {
        ExtractError::Io(e)
    }
}

impl From<zip::result::ZipError> for ExtractError {
    fn from(e: zip::result::ZipError) -> Self {
        ExtractError::Zip(e)
    }
}

/// Options controlling how entry paths are interpreted.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExtractOptions {
    /// If true, drop the first path component of every entry. GitHub zipballs
    /// wrap their content in a `owner-repo-sha/` directory; callers fetching
    /// from GitHub should set this.
    pub strip_top_level: bool,
}

/// Extract `archive_bytes` into `final_dest`, atomically.
///
/// The function first extracts into a sibling temporary directory, validates
/// every entry against `limits`, and only then renames the staged tree onto
/// `final_dest`. If `final_dest` already exists it is removed first; if any
/// step fails the staging directory is cleaned up and `final_dest` is left
/// untouched.
pub fn safe_extract(
    archive_bytes: &[u8],
    final_dest: &Path,
    limits: ExtractLimits,
    opts: ExtractOptions,
) -> Result<(), ExtractError> {
    // Stage extraction in a sibling tempdir of `final_dest`. Using the same
    // parent guarantees the final rename is on the same filesystem and is
    // therefore atomic.
    let parent = final_dest.parent().ok_or_else(|| {
        ExtractError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination has no parent directory",
        ))
    })?;
    fs::create_dir_all(parent)?;

    let staging = tempfile::Builder::new()
        .prefix(".zip-safe-")
        .tempdir_in(parent)?;

    extract_into(archive_bytes, staging.path(), limits, opts)?;

    // Atomic swap: remove existing dest, then rename. The remove+rename pair
    // isn't atomic, but is the best we can do across platforms and is
    // crash-safe in the sense that a partial state is detectable (no dest
    // dir or a complete dest dir; never a half-written one).
    if final_dest.exists() {
        if final_dest.is_dir() {
            fs::remove_dir_all(final_dest)?;
        } else {
            fs::remove_file(final_dest)?;
        }
    }
    let staged = staging.keep(); // disarm auto-cleanup
    if let Err(e) = fs::rename(&staged, final_dest) {
        // Best-effort cleanup if the rename fails so we don't leak the
        // staging directory.
        let _ = fs::remove_dir_all(&staged);
        return Err(ExtractError::Io(e));
    }
    Ok(())
}

/// Lower-level entry point: extract directly into `dest_root` (which must
/// already exist and be empty). Performs all validation but no atomic rename.
fn extract_into(
    archive_bytes: &[u8],
    dest_root: &Path,
    limits: ExtractLimits,
    opts: ExtractOptions,
) -> Result<(), ExtractError> {
    let reader = io::Cursor::new(archive_bytes);
    let mut zip = zip::ZipArchive::new(reader)?;

    let entry_count = zip.len() as u64;
    if entry_count > limits.max_entries {
        return Err(ExtractError::EntryCountExceeded {
            limit: limits.max_entries,
        });
    }

    let mut total_written: u64 = 0;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let raw_name = entry.name().to_string();

        // Skip noise files commonly produced by zip-on-macOS.
        if raw_name.contains("__MACOSX") || raw_name.ends_with(".DS_Store") {
            continue;
        }

        // S_IFLNK → symlink. Reject before doing anything else.
        if let Some(mode) = entry.unix_mode() {
            if (mode & 0o170000) == 0o120000 {
                return Err(ExtractError::SymlinkRejected { name: raw_name });
            }
        }

        let rel = match sanitize_entry_name(&raw_name, opts.strip_top_level, limits.max_depth)? {
            Some(p) => p,
            None => continue, // empty / top-level-only entry → skip
        };

        let out_path = dest_root.join(&rel);

        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }

        // Per-entry size cap, checked against the declared uncompressed
        // size first…
        let declared = entry.size();
        if declared > limits.max_entry_uncompressed {
            return Err(ExtractError::EntrySizeExceeded {
                name: raw_name,
                limit: limits.max_entry_uncompressed,
            });
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut out_file = fs::File::create(&out_path)?;
        // …and then enforced again as we stream, so a lying header can't
        // sneak past us.
        let written = copy_with_caps(
            &mut entry,
            &mut out_file,
            limits.max_entry_uncompressed,
            limits.max_total_uncompressed.saturating_sub(total_written),
            &raw_name,
        )?;
        total_written = total_written.saturating_add(written);
        if total_written > limits.max_total_uncompressed {
            return Err(ExtractError::TotalSizeExceeded {
                limit: limits.max_total_uncompressed,
            });
        }
    }

    Ok(())
}

/// Copy `entry → out` while enforcing per-entry and remaining-total caps.
fn copy_with_caps<R: Read, W: Write>(
    entry: &mut R,
    out: &mut W,
    per_entry_limit: u64,
    remaining_total: u64,
    name: &str,
) -> Result<u64, ExtractError> {
    let mut buf = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        let n = entry.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let n_u64 = n as u64;
        if written.saturating_add(n_u64) > per_entry_limit {
            return Err(ExtractError::EntrySizeExceeded {
                name: name.to_string(),
                limit: per_entry_limit,
            });
        }
        if n_u64 > remaining_total.saturating_sub(written) {
            return Err(ExtractError::TotalSizeExceeded {
                limit: remaining_total,
            });
        }
        out.write_all(&buf[..n])?;
        written += n_u64;
    }
    Ok(written)
}

/// Validate a raw entry name from a zip archive and turn it into a safe
/// relative `PathBuf`. Returns `Ok(None)` for entries that should be
/// skipped (empty or pure top-level dir when stripping).
fn sanitize_entry_name(
    raw: &str,
    strip_top_level: bool,
    max_depth: usize,
) -> Result<Option<PathBuf>, ExtractError> {
    if raw.is_empty() {
        return Ok(None);
    }

    if raw.contains('\0') {
        return Err(ExtractError::UnsafePath {
            name: raw.to_string(),
            reason: "contains NUL byte",
        });
    }
    // Backslashes are never legal path separators on POSIX and are how a
    // lot of Windows-flavoured traversal attempts smuggle themselves in.
    if raw.contains('\\') {
        return Err(ExtractError::UnsafePath {
            name: raw.to_string(),
            reason: "contains backslash separator",
        });
    }
    // Drive-letter prefixes (`C:`, `c:foo`, …).
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return Err(ExtractError::UnsafePath {
            name: raw.to_string(),
            reason: "Windows drive prefix",
        });
    }
    if raw.starts_with('/') {
        return Err(ExtractError::UnsafePath {
            name: raw.to_string(),
            reason: "absolute path",
        });
    }

    // Component walk: this is the canonical safe way to validate a relative
    // path. Anything that isn't a `Normal` component is suspicious.
    let mut parts: Vec<&str> = Vec::new();
    for comp in Path::new(raw).components() {
        match comp {
            Component::Normal(s) => {
                let s = s.to_str().ok_or(ExtractError::UnsafePath {
                    name: raw.to_string(),
                    reason: "non-utf8 component",
                })?;
                parts.push(s);
            }
            Component::CurDir => { /* "./" — harmless, drop it */ }
            Component::ParentDir => {
                return Err(ExtractError::UnsafePath {
                    name: raw.to_string(),
                    reason: "parent-directory component '..'",
                });
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ExtractError::UnsafePath {
                    name: raw.to_string(),
                    reason: "absolute or prefixed component",
                });
            }
        }
    }

    if strip_top_level {
        if parts.len() <= 1 {
            // The top-level dir itself, or a stray bare name → skip.
            return Ok(None);
        }
        parts.remove(0);
    }

    if parts.is_empty() {
        return Ok(None);
    }

    if parts.len() > max_depth {
        return Err(ExtractError::DepthExceeded {
            name: raw.to_string(),
            limit: max_depth,
        });
    }

    let mut out = PathBuf::new();
    for p in parts {
        out.push(p);
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::FileOptions;

    // ---- helpers ----------------------------------------------------------

    fn build_zip(entries: &[(&str, ZipEntry)]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(io::Cursor::new(&mut buf));
            for (name, kind) in entries {
                match kind {
                    ZipEntry::File(data) => {
                        w.start_file(*name, FileOptions::default()).unwrap();
                        w.write_all(data).unwrap();
                    }
                    ZipEntry::Dir => {
                        w.add_directory(*name, FileOptions::default()).unwrap();
                    }
                    ZipEntry::Symlink(target) => {
                        // Use the zip crate's dedicated helper, which sets the
                        // S_IFLNK bits in the external attributes correctly.
                        w.add_symlink(*name, *target, FileOptions::default()).unwrap();
                    }
                }
            }
            w.finish().unwrap();
        }
        buf
    }

    enum ZipEntry<'a> {
        File(&'a [u8]),
        Dir,
        Symlink(&'a str),
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gradient-zipsafe-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    // ---- happy path -------------------------------------------------------

    #[test]
    fn extracts_simple_archive_with_top_level_strip() {
        let zip = build_zip(&[
            ("pkg-1.0.0/", ZipEntry::Dir),
            ("pkg-1.0.0/gradient.toml", ZipEntry::File(b"[package]\n")),
            ("pkg-1.0.0/src/main.gr", ZipEntry::File(b"mod main\n")),
        ]);

        let base = tmpdir("ok");
        let dest = base.join("out");
        safe_extract(
            &zip,
            &dest,
            ExtractLimits::default(),
            ExtractOptions { strip_top_level: true },
        )
        .unwrap();

        assert!(dest.join("gradient.toml").is_file());
        assert!(dest.join("src/main.gr").is_file());
        assert_eq!(
            fs::read(dest.join("gradient.toml")).unwrap(),
            b"[package]\n"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rename_overwrites_existing_dest() {
        let zip = build_zip(&[("root/file", ZipEntry::File(b"new"))]);
        let base = tmpdir("overwrite");
        let dest = base.join("out");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("stale"), b"old").unwrap();
        safe_extract(
            &zip,
            &dest,
            ExtractLimits::default(),
            ExtractOptions { strip_top_level: true },
        )
        .unwrap();
        assert!(dest.join("file").is_file());
        assert!(!dest.join("stale").exists(), "stale file must be gone");
        let _ = fs::remove_dir_all(&base);
    }

    // ---- traversal fixture ------------------------------------------------

    #[test]
    fn rejects_dotdot_traversal() {
        let zip = build_zip(&[("root/../escape.txt", ZipEntry::File(b"x"))]);
        let base = tmpdir("trav");
        let dest = base.join("out");
        let err = safe_extract(
            &zip,
            &dest,
            ExtractLimits::default(),
            ExtractOptions { strip_top_level: true },
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::UnsafePath { .. }), "{:?}", err);
        // No partial files should leak: dest must not exist.
        assert!(!dest.exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_absolute_path() {
        let zip = build_zip(&[("/etc/passwd", ZipEntry::File(b"x"))]);
        let base = tmpdir("abs");
        let dest = base.join("out");
        let err = safe_extract(&zip, &dest, ExtractLimits::default(), ExtractOptions::default())
            .unwrap_err();
        assert!(matches!(err, ExtractError::UnsafePath { .. }), "{:?}", err);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_windows_drive_prefix() {
        let zip = build_zip(&[("C:/evil", ZipEntry::File(b"x"))]);
        let base = tmpdir("drive");
        let dest = base.join("out");
        let err = safe_extract(&zip, &dest, ExtractLimits::default(), ExtractOptions::default())
            .unwrap_err();
        assert!(matches!(err, ExtractError::UnsafePath { .. }), "{:?}", err);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_backslash_separator() {
        let zip = build_zip(&[("root\\..\\evil", ZipEntry::File(b"x"))]);
        let base = tmpdir("bslash");
        let dest = base.join("out");
        let err = safe_extract(
            &zip,
            &dest,
            ExtractLimits::default(),
            ExtractOptions { strip_top_level: true },
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::UnsafePath { .. }), "{:?}", err);
        let _ = fs::remove_dir_all(&base);
    }

    // ---- symlink fixture --------------------------------------------------

    #[test]
    fn rejects_symlink_entry() {
        let zip = build_zip(&[
            ("root/", ZipEntry::Dir),
            ("root/link", ZipEntry::Symlink("/etc/passwd")),
        ]);
        let base = tmpdir("sym");
        let dest = base.join("out");
        let err = safe_extract(
            &zip,
            &dest,
            ExtractLimits::default(),
            ExtractOptions { strip_top_level: true },
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::SymlinkRejected { .. }), "{:?}", err);
        assert!(!dest.exists());
        let _ = fs::remove_dir_all(&base);
    }

    // ---- oversize / bomb fixtures ----------------------------------------

    #[test]
    fn rejects_oversize_single_entry() {
        let big = vec![b'A'; 2048];
        let zip = build_zip(&[("root/big", ZipEntry::File(&big))]);
        let limits = ExtractLimits {
            max_entry_uncompressed: 1024,
            ..ExtractLimits::default()
        };
        let base = tmpdir("entrysize");
        let dest = base.join("out");
        let err = safe_extract(
            &zip,
            &dest,
            limits,
            ExtractOptions { strip_top_level: true },
        )
        .unwrap_err();
        assert!(
            matches!(err, ExtractError::EntrySizeExceeded { .. }),
            "{:?}",
            err
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_zip_bomb_total_size() {
        // Lots of small files; each fits the per-entry cap but they blow the
        // total cap collectively. This is the "many tiny files" flavour of
        // bomb; the streaming `copy_with_caps` plus `total_written` check
        // catches it.
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        for i in 0..200 {
            entries.push((format!("root/f{}", i), vec![b'A'; 1024]));
        }
        let entry_refs: Vec<(&str, ZipEntry)> = entries
            .iter()
            .map(|(n, d)| (n.as_str(), ZipEntry::File(d.as_slice())))
            .collect();
        let zip = build_zip(&entry_refs);
        let limits = ExtractLimits {
            max_total_uncompressed: 50 * 1024, // 50 KiB
            max_entry_uncompressed: 64 * 1024,
            ..ExtractLimits::default()
        };
        let base = tmpdir("bomb");
        let dest = base.join("out");
        let err = safe_extract(
            &zip,
            &dest,
            limits,
            ExtractOptions { strip_top_level: true },
        )
        .unwrap_err();
        assert!(
            matches!(err, ExtractError::TotalSizeExceeded { .. }),
            "{:?}",
            err
        );
        assert!(!dest.exists(), "no partial extraction must leak");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_too_many_entries() {
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        for i in 0..20 {
            entries.push((format!("root/f{}", i), vec![]));
        }
        let entry_refs: Vec<(&str, ZipEntry)> = entries
            .iter()
            .map(|(n, d)| (n.as_str(), ZipEntry::File(d.as_slice())))
            .collect();
        let zip = build_zip(&entry_refs);
        let limits = ExtractLimits {
            max_entries: 5,
            ..ExtractLimits::default()
        };
        let base = tmpdir("count");
        let dest = base.join("out");
        let err = safe_extract(
            &zip,
            &dest,
            limits,
            ExtractOptions { strip_top_level: true },
        )
        .unwrap_err();
        assert!(
            matches!(err, ExtractError::EntryCountExceeded { .. }),
            "{:?}",
            err
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_excessive_depth() {
        let deep = "root/".to_string() + &"a/".repeat(40) + "leaf";
        let zip = build_zip(&[(deep.as_str(), ZipEntry::File(b"x"))]);
        let limits = ExtractLimits {
            max_depth: 8,
            ..ExtractLimits::default()
        };
        let base = tmpdir("depth");
        let dest = base.join("out");
        let err = safe_extract(
            &zip,
            &dest,
            limits,
            ExtractOptions { strip_top_level: true },
        )
        .unwrap_err();
        assert!(
            matches!(err, ExtractError::DepthExceeded { .. }),
            "{:?}",
            err
        );
        let _ = fs::remove_dir_all(&base);
    }

    // ---- sanitizer unit tests --------------------------------------------

    #[test]
    fn sanitize_strips_top_level() {
        let p = sanitize_entry_name("root/src/main.gr", true, 16).unwrap().unwrap();
        assert_eq!(p, PathBuf::from("src/main.gr"));
    }

    #[test]
    fn sanitize_skips_top_level_only() {
        let p = sanitize_entry_name("root/", true, 16).unwrap();
        assert!(p.is_none());
    }

    #[test]
    fn sanitize_rejects_nul() {
        let err = sanitize_entry_name("foo\0bar", false, 16).unwrap_err();
        assert!(matches!(err, ExtractError::UnsafePath { .. }));
    }
}
