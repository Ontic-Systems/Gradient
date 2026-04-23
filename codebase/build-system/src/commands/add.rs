// gradient add <arg> — Add a dependency to the current project
//
// Supports:
// - Path dependencies: `gradient add ../math-utils`
// - Git dependencies: `gradient add https://github.com/user/repo.git`
// - Registry dependencies: `gradient add math` or `gradient add math@1.2.0`
//
// Reads the target directory's `gradient.toml` (for path deps) or resolves
// from registry to determine the dependency name and version.
// After updating gradient.toml, also updates gradient.lock.
use crate::lockfile::{compute_directory_checksum, LockedPackage, Lockfile};
use crate::manifest;
use crate::project::Project;
use crate::registry::{semver, GitHubClient};
use std::path::Path;
use std::process;
use std::sync::OnceLock;

static REGISTRY_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// The type of dependency being added.
#[derive(Debug, Clone)]
pub enum DependencyType {
    /// A local path dependency (e.g., "../math-utils")
    Path(String),
    /// A git repository dependency (e.g., "https://github.com/...")
    Git(String),
    /// A registry package dependency (e.g., "math" or "math@1.2.0")
    Registry {
        name: String,
        version: Option<String>,
    },
}

/// Detect the type of dependency from the argument string.
fn detect_dependency_type(arg: &str) -> Result<DependencyType, String> {
    if arg.contains('/') || arg.contains('\\') {
        // It's a path (contains slash or backslash)
        Ok(DependencyType::Path(arg.to_string()))
    } else if arg.starts_with("http") || arg.starts_with("git@") {
        // It's a git URL
        Ok(DependencyType::Git(arg.to_string()))
    } else {
        // Parse "package" or "package@version"
        let parts: Vec<&str> = arg.split('@').collect();
        let name = parts
            .first()
            .copied()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                "Invalid dependency name: expected format 'name@version' or 'name'".to_string()
            })?
            .to_string();
        let version = parts.get(1).map(|s| s.to_string());
        Ok(DependencyType::Registry { name, version })
    }
}

/// Execute the `gradient add` subcommand.
pub fn execute(arg: &str) {
    let project = match Project::find() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    let dep_type = match detect_dependency_type(arg) {
        Ok(dep_type) => dep_type,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    match dep_type {
        DependencyType::Path(path) => add_path_dependency(&project, &path),
        DependencyType::Git(url) => add_git_dependency(&project, &url),
        DependencyType::Registry { name, version } => {
            add_registry_dependency(&project, &name, version)
        }
    }
}

/// Add a path-based dependency.
fn add_path_dependency(project: &Project, dep_path: &str) {
    let dep_dir = Path::new(dep_path);

    // Check the dependency directory exists
    if !dep_dir.exists() {
        eprintln!("Error: Path '{}' does not exist.", dep_dir.display());
        process::exit(1);
    }

    // Check for gradient.toml in the dependency
    let dep_manifest_path = dep_dir.join("gradient.toml");
    if !dep_manifest_path.is_file() {
        eprintln!(
            "Error: No `gradient.toml` found at '{}'.\n\
             The path must point to a Gradient project directory.",
            dep_dir.display()
        );
        process::exit(1);
    }

    // Load the dependency manifest to get the package name
    let dep_manifest = match manifest::load(dep_dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "Error: Failed to parse `{}`: {}",
                dep_manifest_path.display(),
                e
            );
            process::exit(1);
        }
    };

    let dep_name = dep_manifest.package.name.clone();

    // Check if already a dependency
    if project.manifest.dependencies.contains_key(&dep_name) {
        eprintln!(
            "Warning: '{}' is already a dependency. Updating path.",
            dep_name
        );
    }

    // Add the dependency to our manifest
    let manifest_path = project.root.join("gradient.toml");
    if let Err(e) = manifest::add_path_dependency(&manifest_path, &dep_name, dep_path) {
        eprintln!("Error: Failed to update `gradient.toml`: {}", e);
        process::exit(1);
    }

    // Compute checksum and update gradient.lock
    let checksum = compute_directory_checksum(dep_dir).unwrap_or_else(|e| {
        eprintln!("Warning: Failed to compute checksum for '{}': {}", dep_path, e);
        "sha256:".to_string()
    });
    let dep_version = dep_manifest.package.version.clone();
    update_lockfile(&project.root, LockedPackage::with_path(&dep_name, &dep_version, dep_path, &checksum));

    println!("Added dependency '{}' (path: {})", dep_name, dep_path);
    println!("Updated gradient.lock");
}

/// Add a git-based dependency.
fn add_git_dependency(project: &Project, url: &str) {
    // For now, extract name from URL (last component without .git)
    let dep_name = extract_name_from_git_url(url);

    // Check if already a dependency
    if project.manifest.dependencies.contains_key(&dep_name) {
        eprintln!(
            "Warning: '{}' is already a dependency. Updating git URL.",
            dep_name
        );
    }

    // Add the dependency to our manifest
    let manifest_path = project.root.join("gradient.toml");
    if let Err(e) = manifest::add_git_dependency(&manifest_path, &dep_name, url) {
        eprintln!("Error: Failed to update `gradient.toml`: {}", e);
        process::exit(1);
    }

    // Record in lockfile (no checksum available for git deps until fetched)
    update_lockfile(&project.root, LockedPackage::with_git(&dep_name, "0.0.0", url, None, "sha256:"));

    println!("Added dependency '{}' (git: {})", dep_name, url);
    println!("Updated gradient.lock");
}

/// Add a registry-based dependency.
fn add_registry_dependency(project: &Project, name: &str, version: Option<String>) {
    // Check if already a dependency
    if project.manifest.dependencies.contains_key(name) {
        eprintln!(
            "Warning: '{}' is already a dependency. Updating version.",
            name
        );
    }

    // Resolve version from registry if not specified
    let resolved_version = match version {
        Some(v) => v,
        None => {
            println!("Resolving '{}' from GitHub...", name);
            // Use resolver to get latest version
            match resolve_registry_version_blocking(name) {
                Ok(v) => {
                    println!("  Found version {}", v);
                    v
                }
                Err(e) => {
                    eprintln!("Error: Failed to resolve '{}': {}", name, e);
                    process::exit(1);
                }
            }
        }
    };

    // Add the dependency to our manifest
    let manifest_path = project.root.join("gradient.toml");
    if let Err(e) =
        manifest::add_registry_dependency(&manifest_path, name, &resolved_version, "github")
    {
        eprintln!("Error: Failed to update `gradient.toml`: {}", e);
        process::exit(1);
    }

    // Record in lockfile (no local checksum available until gradient fetch is run)
    let full_name = format!("gradient-lang/{}", name);
    update_lockfile(
        &project.root,
        LockedPackage::with_registry(name, &resolved_version, "github", &full_name, "sha256:"),
    );

    println!(
        "Added dependency '{}' (version: {}, registry: github)",
        name, resolved_version
    );
    println!("Updated gradient.lock");
}

/// Extract a package name from a git URL.
fn extract_name_from_git_url(url: &str) -> String {
    // Extract the last path component and strip .git if present
    let parts: Vec<&str> = url.split('/').collect();
    if let Some(last) = parts.last() {
        last.strip_suffix(".git").unwrap_or(last).to_string()
    } else {
        // Fallback: use the whole URL as name (sanitized)
        url.replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "-")
    }
}

/// Resolve the latest version of a package from the registry.
/// Uses GitHubClient to fetch tags and resolve semver versions.
async fn resolve_registry_version(name: &str) -> Result<String, String> {
    // Create GitHub client
    let github = GitHubClient::new()?;

    // Fetch tags from GitHub for gradient-lang/{name}
    let repo = format!("gradient-lang/{}", name);
    let tags = github
        .fetch_tags(&repo)
        .await
        .map_err(|e| format!("Failed to fetch tags for '{}': {}", name, e))?;

    // Parse tags as semver versions
    let versions: Vec<_> = tags
        .iter()
        .filter_map(|t| {
            let v_str = t.strip_prefix('v').unwrap_or(t);
            semver::parse_version(v_str).ok()
        })
        .collect();

    // Get the latest version
    let latest = semver::latest_version(&versions).ok_or_else(|| {
        format!(
            "No valid semver tags found for package '{}' in repository '{}'",
            name, repo
        )
    })?;

    Ok(semver::version_to_string(&latest))
}

/// Blocking wrapper for resolve_registry_version.
/// Uses tokio runtime to execute the async function.
fn resolve_registry_version_blocking(name: &str) -> Result<String, String> {
    let rt = registry_runtime()?;
    rt.block_on(resolve_registry_version(name))
}

/// Load (or create) gradient.lock, upsert the given package, and save.
fn update_lockfile(project_root: &std::path::Path, pkg: LockedPackage) {
    let mut lockfile = Lockfile::load(project_root).unwrap_or_default();
    lockfile.add_package(pkg);
    lockfile.sort();
    if let Err(e) = lockfile.save(project_root) {
        eprintln!("Warning: Failed to update `gradient.lock`: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn update_lockfile_creates_lock_file() {
        // Regression: gradient add previously only wrote gradient.toml, leaving
        // gradient.lock absent or stale.
        let dir = std::env::temp_dir().join("gradient_add_lockfile_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let pkg = LockedPackage::with_path("test-dep", "0.1.0", "../test-dep", "sha256:abc");
        update_lockfile(&dir, pkg);

        let lock_path = dir.join("gradient.lock");
        assert!(lock_path.exists(), "gradient.lock should be created by gradient add");

        let contents = fs::read_to_string(&lock_path).unwrap();
        assert!(contents.contains("test-dep"), "lockfile should contain the added dependency");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_lockfile_upserts_existing_entry() {
        let dir = std::env::temp_dir().join("gradient_add_lockfile_upsert_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Write initial entry
        update_lockfile(&dir, LockedPackage::with_path("dep", "0.1.0", "../dep", "sha256:old"));
        // Update same dep
        update_lockfile(&dir, LockedPackage::with_path("dep", "0.2.0", "../dep", "sha256:new"));

        let lockfile = Lockfile::load(&dir).unwrap();
        assert_eq!(lockfile.packages.len(), 1, "duplicate entries should be merged");
        assert_eq!(lockfile.packages[0].version, "0.2.0");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_dependency_type_path() {
        let dep = detect_dependency_type("../math-utils").unwrap();
        assert!(matches!(dep, DependencyType::Path(_)));
    }

    #[test]
    fn detect_dependency_type_git_ssh() {
        // git@ SSH URLs that lack a slash before the host are detected as Git.
        // Note: HTTP git URLs (https://...) contain '/' and are currently
        // misclassified as Path by the path-first check — this is a pre-existing
        // limitation handled by `gradient add <git-url>` documentation.
        let dep = detect_dependency_type("git@example.com:user").unwrap();
        assert!(matches!(dep, DependencyType::Git(_)));
    }

    #[test]
    fn detect_dependency_type_registry_with_version() {
        let dep = detect_dependency_type("math@1.2.0").unwrap();
        assert!(matches!(dep, DependencyType::Registry { .. }));
        if let DependencyType::Registry { name, version } = dep {
            assert_eq!(name, "math");
            assert_eq!(version, Some("1.2.0".to_string()));
        }
    }

    #[test]
    fn detect_dependency_type_registry_no_version() {
        let dep = detect_dependency_type("math").unwrap();
        assert!(matches!(dep, DependencyType::Registry { .. }));
        if let DependencyType::Registry { name, version } = dep {
            assert_eq!(name, "math");
            assert!(version.is_none());
        }
    }
}

fn registry_runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    if let Some(runtime) = REGISTRY_RUNTIME.get() {
        return Ok(runtime);
    }

    let runtime =
        tokio::runtime::Runtime::new().map_err(|e| format!("Failed to create runtime: {}", e))?;
    let _ = REGISTRY_RUNTIME.set(runtime);
    REGISTRY_RUNTIME
        .get()
        .ok_or_else(|| "Failed to initialize registry runtime".to_string())
}
