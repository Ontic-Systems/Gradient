//! GitHub API client for fetching package metadata and manifests

use crate::registry::client::RegistryClient;
use serde::Deserialize;

/// Repository information returned from GitHub API
#[derive(Debug, Deserialize)]
pub struct RepoInfo {
    /// Default branch name (e.g., "main", "master")
    pub default_branch: String,
    /// Repository description (optional)
    pub description: Option<String>,
    /// Repository full name (owner/repo)
    pub full_name: String,
}

/// Git tag information
#[derive(Debug, Deserialize)]
struct TagInfo {
    /// Tag name (e.g., "v1.0.0")
    name: String,
}

/// Response shape of `GET /repos/:owner/:repo/git/ref/tags/:tag`.
/// Used by GRA-176 SHA-anchored fetches.
#[derive(Debug, Deserialize)]
struct GitRefResponse {
    object: GitRefObject,
}

#[derive(Debug, Deserialize)]
struct GitRefObject {
    sha: String,
    /// Either `"commit"` (lightweight tag) or `"tag"` (annotated tag).
    /// For annotated tags we need a second hop to the tag object to get
    /// the underlying commit SHA.
    #[serde(rename = "type")]
    object_type: String,
}

#[derive(Debug, Deserialize)]
struct AnnotatedTagResponse {
    object: AnnotatedTagObject,
}

#[derive(Debug, Deserialize)]
struct AnnotatedTagObject {
    sha: String,
}

/// Client for interacting with GitHub repositories
#[derive(Debug)]
pub struct GitHubClient {
    /// Inner registry client for HTTP operations
    inner: RegistryClient,
}

impl GitHubClient {
    /// Create a new GitHub client, wrapping a RegistryClient
    pub fn new() -> Result<Self, String> {
        let inner = RegistryClient::new()?;
        Ok(GitHubClient { inner })
    }

    /// Fetch repository information from GitHub API
    ///
    /// # Arguments
    /// * `repo` - Repository name in format "owner/repo"
    ///
    /// # Returns
    /// * `RepoInfo` - Repository metadata including default branch and description
    pub async fn fetch_repo_info(&self, repo: &str) -> Result<RepoInfo, String> {
        let url = format!("https://api.github.com/repos/{}", repo);

        let response = self.inner.get(&url).await?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to fetch repo info: HTTP {} for {}",
                response.status(),
                repo
            ));
        }

        let repo_info: RepoInfo = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse repo info: {}", e))?;

        Ok(repo_info)
    }

    /// Fetch all tags for a repository
    ///
    /// # Arguments
    /// * `repo` - Repository name in format "owner/repo"
    ///
    /// # Returns
    /// * `Vec<String>` - List of tag names (sorted by GitHub API, typically newest first)
    pub async fn fetch_tags(&self, repo: &str) -> Result<Vec<String>, String> {
        let url = format!("https://api.github.com/repos/{}/tags", repo);

        let response = self.inner.get(&url).await?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to fetch tags: HTTP {} for {}",
                response.status(),
                repo
            ));
        }

        let tags: Vec<TagInfo> = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse tags: {}", e))?;

        let tag_names: Vec<String> = tags.into_iter().map(|tag| tag.name).collect();

        Ok(tag_names)
    }

    /// Fetch the gradient.toml manifest from a specific tag/ref
    ///
    /// # Arguments
    /// * `repo` - Repository name in format "owner/repo"
    /// * `tag` - Tag or branch name to fetch from
    ///
    /// # Returns
    /// * `String` - Contents of gradient.toml file as string
    pub async fn fetch_manifest(&self, repo: &str, tag: &str) -> Result<String, String> {
        let url = format!(
            "https://raw.githubusercontent.com/{}/{}/gradient.toml",
            repo, tag
        );

        let response = self.inner.get(&url).await?;

        if response.status() == 404 {
            return Err(format!(
                "No gradient.toml found in {}/{} - not a valid Gradient package",
                repo, tag
            ));
        }

        if !response.status().is_success() {
            return Err(format!(
                "Failed to fetch manifest: HTTP {} for {}/{} gradient.toml",
                response.status(),
                repo,
                tag
            ));
        }

        let manifest_content = response
            .text()
            .await
            .map_err(|e| format!("Failed to read manifest content: {}", e))?;

        Ok(manifest_content)
    }

    /// Fetch a file from a repository at a specific ref
    ///
    /// # Arguments
    /// * `repo` - Repository name in format "owner/repo"
    /// * `ref_name` - Tag, branch, or commit SHA
    /// * `path` - File path within the repository
    ///
    /// # Returns
    /// * `String` - File contents as string
    pub async fn fetch_file(
        &self,
        repo: &str,
        ref_name: &str,
        path: &str,
    ) -> Result<String, String> {
        let url = format!(
            "https://raw.githubusercontent.com/{}/{}/{}",
            repo, ref_name, path
        );

        let response = self.inner.get(&url).await?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to fetch file: HTTP {} for {}/{}/{}",
                response.status(),
                repo,
                ref_name,
                path
            ));
        }

        let content = response
            .text()
            .await
            .map_err(|e| format!("Failed to read file content: {}", e))?;

        Ok(content)
    }

    /// Maximum archive size: 100MB
    const MAX_ARCHIVE_SIZE: usize = 100 * 1024 * 1024;

    /// Download a tarball/zipball of the repository at a specific ref
    ///
    /// # Arguments
    /// * `repo` - Repository name in format "owner/repo"
    /// * `ref_name` - Tag, branch, or commit SHA
    ///
    /// # Returns
    /// * `Vec<u8>` - Archive data as bytes
    pub async fn download_archive(&self, repo: &str, ref_name: &str) -> Result<Vec<u8>, String> {
        let url = format!("https://api.github.com/repos/{}/zipball/{}", repo, ref_name);

        let response = self.inner.get(&url).await?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to download archive: HTTP {} for {}/{}",
                response.status(),
                repo,
                ref_name
            ));
        }

        // Check content length before downloading
        if let Some(content_length) = response.content_length() {
            if content_length as usize > Self::MAX_ARCHIVE_SIZE {
                return Err(format!(
                    "Archive too large: {} bytes exceeds maximum of {} bytes",
                    content_length,
                    Self::MAX_ARCHIVE_SIZE
                ));
            }
        }

        let archive_data = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read archive data: {}", e))?;

        // Double-check size after download
        if archive_data.len() > Self::MAX_ARCHIVE_SIZE {
            return Err(format!(
                "Archive too large: {} bytes exceeds maximum of {} bytes",
                archive_data.len(),
                Self::MAX_ARCHIVE_SIZE
            ));
        }

        Ok(archive_data.to_vec())
    }

    /// Get the latest release tag for a repository (if releases exist)
    ///
    /// # Arguments
    /// * `repo` - Repository name in format "owner/repo"
    ///
    /// # Returns
    /// * `Option<String>` - Latest release tag name, or None if no releases
    pub async fn fetch_latest_release(&self, repo: &str) -> Result<Option<String>, String> {
        let url = format!("https://api.github.com/repos/{}/releases/latest", repo);

        let response = self.inner.get(&url).await?;

        // 404 means no releases exist
        if response.status() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(format!(
                "Failed to fetch latest release: HTTP {} for {}",
                response.status(),
                repo
            ));
        }

        #[derive(Deserialize)]
        struct ReleaseInfo {
            tag_name: String,
        }

        let release: ReleaseInfo = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse release info: {}", e))?;

        Ok(Some(release.tag_name))
    }

    /// GRA-176: Resolve a git tag to the commit SHA it currently points at.
    ///
    /// Hits `GET /repos/:owner/:repo/git/ref/tags/:tag`. For annotated
    /// tags (`object.type == "tag"`) this returns the SHA of the tag
    /// object itself; we then dereference once via
    /// `GET /repos/:owner/:repo/git/tags/:tag_sha` to get the commit
    /// SHA. For lightweight tags (`object.type == "commit"`) the first
    /// response already contains the commit SHA.
    pub async fn resolve_tag_to_sha(&self, repo: &str, tag: &str) -> Result<String, String> {
        let url = format!("https://api.github.com/repos/{}/git/ref/tags/{}", repo, tag);

        let response = self.inner.get(&url).await?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to resolve tag '{}' in '{}': HTTP {}",
                tag,
                repo,
                response.status()
            ));
        }

        let git_ref: GitRefResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse git ref response: {}", e))?;

        match git_ref.object.object_type.as_str() {
            "commit" => Ok(git_ref.object.sha),
            "tag" => {
                // Annotated tag — dereference once.
                let tag_url = format!(
                    "https://api.github.com/repos/{}/git/tags/{}",
                    repo, git_ref.object.sha
                );
                let tag_resp = self.inner.get(&tag_url).await?;
                if !tag_resp.status().is_success() {
                    return Err(format!(
                        "Failed to dereference annotated tag '{}' in '{}': HTTP {}",
                        tag,
                        repo,
                        tag_resp.status()
                    ));
                }
                let annotated: AnnotatedTagResponse = tag_resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse annotated tag response: {}", e))?;
                Ok(annotated.object.sha)
            }
            other => Err(format!(
                "Unexpected git ref object type '{}' for tag '{}' in '{}'",
                other, tag, repo
            )),
        }
    }

    /// GRA-176: Download a zipball pinned to a specific commit SHA.
    ///
    /// Unlike `download_archive(repo, tag)`, the URL is content-addressed:
    /// `https://github.com/{owner}/{repo}/archive/{sha}.zip`. Once GitHub
    /// has served the bytes for a given commit SHA they are immutable.
    pub async fn download_archive_by_sha(
        &self,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<u8>, String> {
        // Reuse the existing /zipball/<ref> endpoint, which accepts a SHA.
        // This goes through the same auth/size-limit code path as
        // `download_archive`.
        self.download_archive(repo, sha).await
    }

    /// Access the inner registry client for direct cache operations
    pub fn registry_client(&self) -> &RegistryClient {
        &self.inner
    }
}
