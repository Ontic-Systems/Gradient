// gradient fetch — Download registry dependencies to cache
//
// Fetches all registry dependencies from the manifest, downloads them
// from the GitHub registry, and caches them locally.
//
// Usage: gradient fetch [package-name]
// Without argument: fetches all registry dependencies
// With argument: fetches only the specified package

use crate::manifest;
use crate::project::Project;
use crate::registry::{semver, GitHubClient};
use std::path::{Path, PathBuf};
use std::process;

/// Execute the `gradient fetch` subcommand.
pub fn execute(package: Option<&str>) {
    let project = match Project::find() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    println!("Fetching dependencies for '{}'...", project.name);

    // Create GitHub client
    let github = match GitHubClient::new() {
        Ok(client) => client,
        Err(e) => {
            eprintln!("Error: Failed to create GitHub client: {}", e);
            process::exit(1);
        }
    };

    if let Some(pkg_name) = package {
        // Fetch single package
        match fetch_single_package(&github, pkg_name, &project.manifest) {
            Ok(()) => println!("Successfully fetched '{}'", pkg_name),
            Err(e) => {
                eprintln!("Error: Failed to fetch '{}': {}", pkg_name, e);
                process::exit(1);
            }
        }
    } else {
        // Fetch all registry dependencies
        let registry_deps: Vec<_> = project
            .manifest
            .dependencies
            .iter()
            .filter(|(_, dep)| dep.registry().is_some())
            .collect();

        if registry_deps.is_empty() {
            println!("No registry dependencies to fetch.");
            return;
        }

        let mut fetched = 0;
        let mut failed = 0;

        for (name, dep) in registry_deps {
            let version_req = dep.version();
            match fetch_package(&github, name, version_req) {
                Ok(version) => {
                    println!("  {}@{}", name, version);
                    fetched += 1;
                }
                Err(e) => {
                    eprintln!("  Error: Failed to fetch '{}': {}", name, e);
                    failed += 1;
                }
            }
        }

        if failed > 0 {
            println!("\nFetched {} packages, {} failed", fetched, failed);
            process::exit(1);
        } else {
            println!("\nFetched {} packages", fetched);
        }
    }
}

/// Fetch a single package by name.
fn fetch_single_package(
    github: &GitHubClient,
    name: &str,
    manifest: &manifest::Manifest,
) -> Result<(), String> {
    // Look up the dependency in the manifest
    let dep = manifest
        .dependencies
        .get(name)
        .ok_or_else(|| format!("Package '{}' not found in dependencies", name))?;

    if dep.registry().is_none() {
        return Err(format!("'{}' is not a registry dependency", name));
    }

    let version_req = dep.version();
    let version = fetch_package(github, name, version_req)?;
    println!("Fetched {}@{}", name, version);
    Ok(())
}

/// Fetch a package from the registry.
fn fetch_package(
    github: &GitHubClient,
    name: &str,
    version_req: Option<&str>,
) -> Result<String, String> {
    // Use tokio runtime for async operations
    let rt =
        tokio::runtime::Runtime::new().map_err(|e| format!("Failed to create runtime: {}", e))?;
    rt.block_on(async_fetch_package(github, name, version_req))
}

/// Async version of fetch_package.
async fn async_fetch_package(
    github: &GitHubClient,
    name: &str,
    version_req: Option<&str>,
) -> Result<String, String> {
    // Fetch tags from GitHub
    let repo = format!("gradient-lang/{}", name);
    let tags = github
        .fetch_tags(&repo)
        .await
        .map_err(|e| format!("Failed to fetch tags: {}", e))?;

    // Parse tags as semver versions
    let versions: Vec<_> = tags
        .iter()
        .filter_map(|t| {
            let v_str = t.strip_prefix('v').unwrap_or(t);
            semver::parse_version(v_str).ok()
        })
        .collect();

    if versions.is_empty() {
        return Err("No valid semver tags found".to_string());
    }

    // Resolve version
    let resolved_version = if let Some(req_str) = version_req {
        let req = semver::parse_version_req(req_str)
            .map_err(|e| format!("Invalid version requirement: {}", e))?;
        semver::resolve_version(&versions, &req)
            .ok_or_else(|| format!("No matching version found for '{}'", req_str))?
    } else {
        semver::latest_version(&versions)
            .ok_or_else(|| "No valid semver versions found for package".to_string())?
    };

    let version_str = semver::version_to_string(&resolved_version);

    // Check if already cached
    if is_cached(name, &version_str) {
        println!("  {}@{} is already cached", name, version_str);
        return Ok(version_str);
    }

    // Download and cache
    println!("  Downloading {}@{}...", name, version_str);
    download_and_cache(github, name, &version_str, &repo).await?;

    Ok(version_str)
}

/// Check if a package version is already cached.
fn is_cached(name: &str, version: &str) -> bool {
    let cache_dir = get_cache_dir().map(|d| d.join(name).join(version));
    cache_dir
        .map(|p| p.is_dir() && p.join("gradient.toml").is_file())
        .unwrap_or(false)
}

/// Get the cache directory path.
fn get_cache_dir() -> Option<PathBuf> {
    let home_dir = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(PathBuf::from(home_dir).join(".gradient").join("cache"))
}

/// Download and cache a package.
async fn download_and_cache(
    github: &GitHubClient,
    name: &str,
    version: &str,
    repo: &str,
) -> Result<(), String> {
    let cache_dir = get_cache_dir()
        .ok_or_else(|| "Could not determine cache directory".to_string())?
        .join(name)
        .join(version);

    // Create cache directory
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("Failed to create cache directory: {}", e))?;

    // Download archive
    let tag = format!("v{}", version);
    let archive_data = github
        .download_archive(repo, &tag)
        .await
        .map_err(|e| format!("Failed to download archive: {}", e))?;

    // Extract archive
    extract_zip(&archive_data, &cache_dir)?;

    // Verify gradient.toml exists
    if !cache_dir.join("gradient.toml").is_file() {
        return Err("Downloaded package does not contain gradient.toml".to_string());
    }

    Ok(())
}

/// Extract a ZIP archive to a directory.
fn extract_zip(data: &[u8], dest: &Path) -> Result<(), String> {
    use std::io::Cursor;

    let reader = Cursor::new(data);
    let mut zip =
        zip::ZipArchive::new(reader).map_err(|e| format!("Failed to read ZIP archive: {}", e))?;

    // Extract files, stripping the top-level directory
    for i in 0..zip.len() {
        let mut file = zip
            .by_index(i)
            .map_err(|e| format!("Failed to access ZIP entry: {}", e))?;

        let name = file.name();

        // Skip macOS metadata files
        if name.contains("__MACOSX") || name.contains(".DS_Store") {
            continue;
        }

        // H-2: Security hardening - reject symlinks, canonicalize paths
        // In zip crate 0.6, symlinks are detected via unix_mode()
        if let Some(mode) = file.unix_mode() {
            if (mode & 0o170000) == 0o120000 {  // S_IFLNK
                return Err(format!(
                    "Invalid ZIP entry: symlinks are not allowed ('{}')",
                    name
                ));
            }
        }

        // Strip top-level directory (GitHub zipballs have format: owner-repo-tag/)
        let path_parts: Vec<&str> = name.split('/').collect();
        if path_parts.len() < 2 {
            continue; // Skip top-level directory entry
        }
        let stripped_name = path_parts[1..].join("/");

        if stripped_name.is_empty() {
            continue;
        }

        // Security: Prevent path traversal attacks
        if stripped_name.contains("..") || stripped_name.starts_with('/') {
            return Err(format!(
                "Invalid ZIP entry: potential path traversal detected in '{}'",
                name
            ));
        }

        // H-2: Additional hardening - reject backslash separators and absolute Windows paths
        if stripped_name.contains('\\') || stripped_name.starts_with("C:") {
            return Err(format!(
                "Invalid ZIP entry: illegal path component in '{}'",
                name
            ));
        }

        let out_path = dest.join(&stripped_name);

        // H-2: Canonicalize and verify the output path is within destination
        let canonical_out = out_path.canonicalize().unwrap_or_else(|_| out_path.clone());
        let canonical_dest = dest.canonicalize().unwrap_or_else(|_| dest.to_path_buf());
        if !canonical_out.starts_with(&canonical_dest) {
            return Err(format!(
                "Invalid ZIP entry: path escapes destination directory in '{}'",
                name
            ));
        }

        if file.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        } else {
            // Ensure parent directory exists
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directory: {}", e))?;
            }

            let mut out_file = std::fs::File::create(&out_path)
                .map_err(|e| format!("Failed to create file: {}", e))?;
            std::io::copy(&mut file, &mut out_file)
                .map_err(|e| format!("Failed to write file: {}", e))?;
        }
    }

    Ok(())
}
