//! GRA-176: SHA-anchored registry fetch.
//!
//! Tag-based fetches are vulnerable to silent content substitution: a
//! malicious or compromised upstream can move `v1.2.0` to point at a
//! different commit and lockfile-blind installs will pick up the new
//! contents without any signal.
//!
//! This module pins registry dependencies to an immutable commit SHA at
//! fetch time and verifies the archive against a SHA-256 the lockfile
//! captured. The flow is:
//!
//! 1. Resolve `tag` → `commit_sha` via the GitHub API
//!    (`GET /repos/:owner/:repo/git/ref/tags/:tag`).
//! 2. Compare against any existing `commit_sha` in the lockfile. A drift
//!    aborts the install unless `update = true` was passed (i.e. the
//!    user explicitly opted into picking up the new commit).
//! 3. Download the zipball **by SHA** (`/archive/<sha>.zip`) — this URL
//!    is content-addressed: GitHub will not silently substitute it.
//! 4. Hash the bytes; if the lockfile has an `archive_sha256`, the new
//!    download must match it. A mismatch aborts the install.
//!
//! The `GitHubApi` trait isolates the network calls so this logic can be
//! unit-tested without a live registry.

use sha2::{Digest, Sha256};

/// Errors produced while anchoring a registry dependency.
#[derive(Debug, PartialEq, Eq)]
pub enum AnchorError {
    /// The GitHub API call to resolve the tag failed.
    TagResolutionFailed { tag: String, message: String },
    /// The archive download failed.
    ArchiveDownloadFailed { sha: String, message: String },
    /// The lockfile records `commit_sha = X` but the registry now reports `Y`.
    /// Refuses install unless `--update` was passed.
    TagMoved {
        repo: String,
        tag: String,
        locked_sha: String,
        upstream_sha: String,
    },
    /// The downloaded archive's SHA-256 does not match what the lockfile
    /// recorded on the previous install.
    ArchiveShaMismatch {
        repo: String,
        commit_sha: String,
        expected: String,
        actual: String,
    },
}

impl std::fmt::Display for AnchorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnchorError::TagResolutionFailed { tag, message } => {
                write!(f, "Failed to resolve tag '{}' to a commit SHA: {}", tag, message)
            }
            AnchorError::ArchiveDownloadFailed { sha, message } => {
                write!(f, "Failed to download archive for commit '{}': {}", sha, message)
            }
            AnchorError::TagMoved {
                repo,
                tag,
                locked_sha,
                upstream_sha,
            } => write!(
                f,
                "Refusing install: tag '{tag}' on '{repo}' moved from \
                 locked commit {locked_sha} to {upstream_sha}. \
                 Re-run with `gradient update` to accept the new commit."
            ),
            AnchorError::ArchiveShaMismatch {
                repo,
                commit_sha,
                expected,
                actual,
            } => write!(
                f,
                "Archive checksum mismatch for {repo}@{commit_sha}: \
                 expected {expected}, downloaded archive hashes to {actual}. \
                 The cached or upstream archive may have been tampered with."
            ),
        }
    }
}

impl std::error::Error for AnchorError {}

/// Minimal GitHub-shaped API surface required for SHA-anchored fetches.
///
/// Implemented for real by `GitHubClient`; the unit tests in this module
/// implement it against in-memory fixtures so the test suite stays
/// network-free.
pub trait GitHubApi {
    /// Resolve `tag` (e.g. `"v1.2.0"`) to the commit SHA the tag points
    /// at *right now*.
    fn resolve_tag_to_sha(
        &self,
        repo: &str,
        tag: &str,
    ) -> Result<String, String>;

    /// Download the zipball at a specific commit SHA. The URL
    /// `https://github.com/{owner}/{repo}/archive/{sha}.zip` is
    /// content-addressed and immutable.
    fn download_archive_by_sha(
        &self,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<u8>, String>;
}

/// Result of a successful anchor operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchoredArchive {
    /// Commit SHA the tag was resolved to.
    pub commit_sha: String,
    /// SHA-256 of the downloaded archive bytes (hex-encoded, with
    /// `sha256:` prefix to match the existing lockfile convention).
    pub archive_sha256: String,
    /// Raw archive bytes — caller is responsible for caching/extraction.
    pub bytes: Vec<u8>,
}

/// What an existing lockfile entry told us about this dependency.
///
/// Either field may be missing for a legacy (v1) lockfile entry that was
/// written before SHA anchoring was implemented; in that case anchoring
/// proceeds and the fields are populated for the next install.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LockedAnchor<'a> {
    pub commit_sha: Option<&'a str>,
    pub archive_sha256: Option<&'a str>,
}

/// Resolve a tag to a SHA, download the SHA-pinned archive, verify it
/// against any existing lockfile state, and return the anchored archive.
///
/// Trust model:
///
/// - If `locked.commit_sha` is `Some` and the upstream tag now resolves
///   to a different SHA, the install is refused unless `update = true`
///   (the caller passed `--update` / `gradient update`).
/// - If `locked.archive_sha256` is `Some`, the freshly downloaded
///   archive must hash to that value, regardless of `update`. An
///   archive-hash mismatch is *always* an error: the SHA-pinned URL is
///   content-addressed, so a mismatch implies either cache corruption
///   or an upstream tampering with a specific commit's archive.
pub fn anchor_registry_dep(
    api: &dyn GitHubApi,
    repo: &str,
    tag: &str,
    locked: LockedAnchor<'_>,
    update: bool,
) -> Result<AnchoredArchive, AnchorError> {
    // Step 1: tag → SHA.
    let upstream_sha = api
        .resolve_tag_to_sha(repo, tag)
        .map_err(|message| AnchorError::TagResolutionFailed {
            tag: tag.to_string(),
            message,
        })?;

    // Step 2: refuse silent tag movement.
    if let Some(locked_sha) = locked.commit_sha {
        if locked_sha != upstream_sha && !update {
            return Err(AnchorError::TagMoved {
                repo: repo.to_string(),
                tag: tag.to_string(),
                locked_sha: locked_sha.to_string(),
                upstream_sha,
            });
        }
    }

    // Step 3: download by SHA.
    let bytes = api
        .download_archive_by_sha(repo, &upstream_sha)
        .map_err(|message| AnchorError::ArchiveDownloadFailed {
            sha: upstream_sha.clone(),
            message,
        })?;

    // Step 4: hash and verify.
    let archive_sha256 = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));

    if let Some(expected) = locked.archive_sha256 {
        // Only enforce when the lockfile commit SHA matches the upstream
        // commit SHA we just downloaded. If the user opted into a tag
        // move via `update = true`, the recorded archive hash refers to
        // a *different* commit, so it's not meaningful to compare.
        let same_commit = locked
            .commit_sha
            .map(|s| s == upstream_sha)
            .unwrap_or(true);
        if same_commit && expected != archive_sha256 {
            return Err(AnchorError::ArchiveShaMismatch {
                repo: repo.to_string(),
                commit_sha: upstream_sha,
                expected: expected.to_string(),
                actual: archive_sha256,
            });
        }
    }

    Ok(AnchoredArchive {
        commit_sha: upstream_sha,
        archive_sha256,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// In-memory `GitHubApi` for hermetic tests.
    struct MockGitHub {
        /// (repo, tag) → sha
        tags: RefCell<HashMap<(String, String), String>>,
        /// (repo, sha) → archive bytes
        archives: RefCell<HashMap<(String, String), Vec<u8>>>,
    }

    impl MockGitHub {
        fn new() -> Self {
            Self {
                tags: RefCell::new(HashMap::new()),
                archives: RefCell::new(HashMap::new()),
            }
        }
        fn set_tag(&self, repo: &str, tag: &str, sha: &str) {
            self.tags
                .borrow_mut()
                .insert((repo.to_string(), tag.to_string()), sha.to_string());
        }
        fn set_archive(&self, repo: &str, sha: &str, bytes: Vec<u8>) {
            self.archives
                .borrow_mut()
                .insert((repo.to_string(), sha.to_string()), bytes);
        }
    }

    impl GitHubApi for MockGitHub {
        fn resolve_tag_to_sha(&self, repo: &str, tag: &str) -> Result<String, String> {
            self.tags
                .borrow()
                .get(&(repo.to_string(), tag.to_string()))
                .cloned()
                .ok_or_else(|| format!("no such tag {}/{}", repo, tag))
        }
        fn download_archive_by_sha(&self, repo: &str, sha: &str) -> Result<Vec<u8>, String> {
            self.archives
                .borrow()
                .get(&(repo.to_string(), sha.to_string()))
                .cloned()
                .ok_or_else(|| format!("no archive for {}@{}", repo, sha))
        }
    }

    fn sha256_of(bytes: &[u8]) -> String {
        format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
    }

    #[test]
    fn first_install_populates_anchor_fields() {
        let api = MockGitHub::new();
        api.set_tag("gradient-lang/math", "v1.2.0", "aaa111");
        api.set_archive("gradient-lang/math", "aaa111", b"original-bytes".to_vec());

        let result = anchor_registry_dep(
            &api,
            "gradient-lang/math",
            "v1.2.0",
            LockedAnchor::default(),
            false,
        )
        .expect("first install should succeed");

        assert_eq!(result.commit_sha, "aaa111");
        assert_eq!(result.archive_sha256, sha256_of(b"original-bytes"));
        assert_eq!(result.bytes, b"original-bytes");
    }

    #[test]
    fn second_install_with_unchanged_tag_passes() {
        let api = MockGitHub::new();
        api.set_tag("gradient-lang/math", "v1.2.0", "aaa111");
        api.set_archive("gradient-lang/math", "aaa111", b"original-bytes".to_vec());

        let original_hash = sha256_of(b"original-bytes");
        let result = anchor_registry_dep(
            &api,
            "gradient-lang/math",
            "v1.2.0",
            LockedAnchor {
                commit_sha: Some("aaa111"),
                archive_sha256: Some(&original_hash),
            },
            false,
        )
        .expect("re-install with matching anchor should succeed");

        assert_eq!(result.commit_sha, "aaa111");
        assert_eq!(result.archive_sha256, original_hash);
    }

    /// Tag-moved fixture. After a successful install pinned `v1.2.0` to
    /// commit `aaa111`, the upstream silently re-points `v1.2.0` at
    /// `bbb222`. Without `--update` the install must be rejected.
    #[test]
    fn tag_moved_between_installs_is_rejected() {
        let api = MockGitHub::new();
        api.set_tag("gradient-lang/math", "v1.2.0", "bbb222"); // moved!
        api.set_archive("gradient-lang/math", "bbb222", b"new-bytes".to_vec());

        let original_hash = sha256_of(b"original-bytes");
        let err = anchor_registry_dep(
            &api,
            "gradient-lang/math",
            "v1.2.0",
            LockedAnchor {
                commit_sha: Some("aaa111"),
                archive_sha256: Some(&original_hash),
            },
            false, // no --update
        )
        .expect_err("tag movement without --update must fail");

        match err {
            AnchorError::TagMoved {
                ref repo,
                ref tag,
                ref locked_sha,
                ref upstream_sha,
            } => {
                assert_eq!(repo, "gradient-lang/math");
                assert_eq!(tag, "v1.2.0");
                assert_eq!(locked_sha, "aaa111");
                assert_eq!(upstream_sha, "bbb222");
            }
            other => panic!("expected TagMoved, got {:?}", other),
        }

        // With `update = true` the same call must succeed and produce
        // the *new* anchor.
        let ok = anchor_registry_dep(
            &api,
            "gradient-lang/math",
            "v1.2.0",
            LockedAnchor {
                commit_sha: Some("aaa111"),
                archive_sha256: Some(&original_hash),
            },
            true,
        )
        .expect("tag movement with --update is allowed");
        assert_eq!(ok.commit_sha, "bbb222");
        assert_eq!(ok.archive_sha256, sha256_of(b"new-bytes"));
    }

    /// SHA-mismatch fixture. The lockfile says
    /// `commit_sha = aaa111, archive_sha256 = sha256(original-bytes)`,
    /// but the SHA-pinned URL now serves *different* bytes (cache
    /// poisoning, upstream tampering with archive generation, etc.).
    /// The install must be rejected, with or without `--update`.
    #[test]
    fn archive_sha_mismatch_at_pinned_commit_is_rejected() {
        let api = MockGitHub::new();
        api.set_tag("gradient-lang/math", "v1.2.0", "aaa111");
        // Same commit SHA, but the archive bytes have changed.
        api.set_archive("gradient-lang/math", "aaa111", b"tampered-bytes".to_vec());

        let original_hash = sha256_of(b"original-bytes");
        let err = anchor_registry_dep(
            &api,
            "gradient-lang/math",
            "v1.2.0",
            LockedAnchor {
                commit_sha: Some("aaa111"),
                archive_sha256: Some(&original_hash),
            },
            false,
        )
        .expect_err("archive sha mismatch must always fail");

        match err {
            AnchorError::ArchiveShaMismatch {
                ref expected,
                ref actual,
                ref commit_sha,
                ..
            } => {
                assert_eq!(commit_sha, "aaa111");
                assert_eq!(expected, &original_hash);
                assert_eq!(actual, &sha256_of(b"tampered-bytes"));
            }
            other => panic!("expected ArchiveShaMismatch, got {:?}", other),
        }

        // `--update` does NOT relax the archive-hash check at the same
        // commit: there is no legitimate reason for the SHA-pinned URL
        // to serve different bytes than it did last time.
        let err2 = anchor_registry_dep(
            &api,
            "gradient-lang/math",
            "v1.2.0",
            LockedAnchor {
                commit_sha: Some("aaa111"),
                archive_sha256: Some(&original_hash),
            },
            true,
        )
        .expect_err("archive sha mismatch must fail even with --update");
        assert!(matches!(err2, AnchorError::ArchiveShaMismatch { .. }));
    }

    #[test]
    fn tag_resolution_failure_is_surfaced() {
        let api = MockGitHub::new(); // no tags configured
        let err = anchor_registry_dep(
            &api,
            "gradient-lang/math",
            "v9.9.9",
            LockedAnchor::default(),
            false,
        )
        .expect_err("missing tag must fail");
        assert!(matches!(err, AnchorError::TagResolutionFailed { .. }));
    }
}
