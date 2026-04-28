//! HTTP client for package registry operations with local caching

use crate::name_validation::{safe_cache_path, NameError};
use std::env;
use std::fs;
use std::path::PathBuf;

/// HTTP client for fetching packages with local caching support
#[derive(Debug)]
pub struct RegistryClient {
    /// Cache directory for downloaded packages (~/.gradient/cache)
    pub cache_dir: PathBuf,
    /// GitHub API token from GITHUB_TOKEN env var
    pub github_token: Option<String>,
    /// HTTP client for making requests
    http_client: reqwest::Client,
}

impl RegistryClient {
    /// Create a new RegistryClient, initializing cache directory if needed
    pub fn new() -> Result<Self, String> {
        // Determine cache directory: ~/.gradient/cache
        let home_dir = env::var("HOME")
            .or_else(|_| env::var("USERPROFILE"))
            .map_err(|_| "Could not determine home directory".to_string())?;

        let cache_dir = PathBuf::from(home_dir).join(".gradient").join("cache");

        // Create cache directory if it doesn't exist
        if !cache_dir.exists() {
            fs::create_dir_all(&cache_dir)
                .map_err(|e| format!("Failed to create cache directory: {}", e))?;
        }

        // Get GitHub token from environment
        let github_token = env::var("GITHUB_TOKEN").ok();

        // Create HTTP client
        let http_client = reqwest::Client::builder()
            .user_agent("gradient-build-system/0.1.0")
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        Ok(RegistryClient {
            cache_dir,
            github_token,
            http_client,
        })
    }

    /// Perform an HTTP GET request, with optional GitHub authentication
    pub async fn get(&self, url: &str) -> Result<reqwest::Response, String> {
        let mut request = self.http_client.get(url);

        // Add GitHub authorization header if token is present and URL is GitHub
        if let Some(ref token) = self.github_token {
            if url.contains("github.com") {
                request = request.header("Authorization", format!("Bearer {}", token));
            }
        }

        request
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))
    }

    /// Get the cache path for a specific package version, validating
    /// `name` and `version` against the strict allowlist (issue #177).
    ///
    /// Returns: `~/.gradient/cache/{name}/{version}` on success, or a
    /// [`NameError`] if either component fails validation or the result
    /// would escape the cache root.
    pub fn cache_path(&self, name: &str, version: &str) -> Result<PathBuf, NameError> {
        safe_cache_path(&self.cache_dir, name, version)
    }

    /// Check if a package version is already cached. Returns `false`
    /// (not cached) for names/versions that fail validation, on the
    /// principle that an unsafe input is never "in cache".
    pub fn is_cached(&self, name: &str, version: &str) -> bool {
        match self.cache_path(name, version) {
            Ok(p) => p.exists(),
            Err(_) => false,
        }
    }

    /// Write data to cache for a specific package version. Validates
    /// `name` and `version` before any filesystem operation.
    /// Returns the path where data was written.
    pub fn write_cache(&self, name: &str, version: &str, data: &[u8]) -> Result<PathBuf, String> {
        let version_dir = self
            .cache_path(name, version)
            .map_err(|e| format!("Invalid package name or version: {}", e))?;
        let package_dir = version_dir
            .parent()
            .ok_or_else(|| "cache path has no parent directory".to_string())?
            .to_path_buf();

        // Create package directory if it doesn't exist
        if !package_dir.exists() {
            fs::create_dir_all(&package_dir)
                .map_err(|e| format!("Failed to create package directory: {}", e))?;
        }

        // Create version directory if it doesn't exist
        if !version_dir.exists() {
            fs::create_dir_all(&version_dir)
                .map_err(|e| format!("Failed to create version directory: {}", e))?;
        }

        // Write data to a temporary file first, then rename for atomicity
        let temp_path = version_dir.join(".tmp_download");
        let final_path = version_dir.join("package");

        fs::write(&temp_path, data).map_err(|e| format!("Failed to write cache file: {}", e))?;

        fs::rename(&temp_path, &final_path)
            .map_err(|e| format!("Failed to finalize cache file: {}", e))?;

        Ok(version_dir)
    }

    /// Read cached data for a specific package version.
    pub fn read_cache(&self, name: &str, version: &str) -> Result<Vec<u8>, String> {
        let path = self
            .cache_path(name, version)
            .map_err(|e| format!("Invalid package name or version: {}", e))?
            .join("package");

        if !path.exists() {
            return Err(format!("Cache not found for {}@{}", name, version));
        }

        fs::read(&path).map_err(|e| format!("Failed to read cache file: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_path_construction() {
        let client = RegistryClient::new().unwrap();
        let path = client.cache_path("my-package", "1.0.0").unwrap();

        assert!(path.to_string_lossy().contains(".gradient"));
        assert!(path.to_string_lossy().contains("cache"));
        assert!(path.to_string_lossy().contains("my-package"));
        assert!(path.to_string_lossy().contains("1.0.0"));
    }

    #[test]
    fn test_cache_path_rejects_traversal() {
        let client = RegistryClient::new().unwrap();
        assert!(client.cache_path("../etc", "1.0.0").is_err());
        assert!(client.cache_path("my-package", "../1.0.0").is_err());
        assert!(client.cache_path("foo/bar", "1.0.0").is_err());
        assert!(client.cache_path("foo\0bar", "1.0.0").is_err());
    }

    #[test]
    fn test_is_cached_returns_false_for_invalid_names() {
        let client = RegistryClient::new().unwrap();
        assert!(!client.is_cached("../etc", "1.0.0"));
        assert!(!client.is_cached("my-pkg", "../1.0.0"));
    }

    #[test]
    fn test_write_cache_rejects_invalid_names() {
        let client = RegistryClient::new().unwrap();
        assert!(client.write_cache("../etc", "1.0.0", b"data").is_err());
        assert!(client.write_cache("pkg", "../1.0", b"data").is_err());
        assert!(client.write_cache("Foo", "1.0.0", b"data").is_err());
    }
}
