// gradient add <arg> — Add a dependency to the current project
//
// Supports:
// - Path dependencies: `gradient add ../math-utils`
// - Git dependencies: `gradient add https://github.com/user/repo.git`
// - Registry dependencies: `gradient add math` or `gradient add math@1.2.0`
//
// Reads the target directory's `gradient.toml` (for path deps) or resolves
// from registry to determine the dependency name and version.
use crate::manifest;
use crate::project::Project;
use crate::registry::{semver, GitHubClient};
use std::path::Path;
use std::process;

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
fn detect_dependency_type(arg: &str) -> DependencyType {
    if arg.contains('/') || arg.contains('\\') {
        // It's a path (contains slash or backslash)
        DependencyType::Path(arg.to_string())
    } else if arg.starts_with("http") || arg.starts_with("git@") {
        // It's a git URL
        DependencyType::Git(arg.to_string())
    } else {
        // Parse "package" or "package@version"
        let parts: Vec<&str> = arg.split('@').collect();
        let name = parts[0].to_string();
        let version = parts.get(1).map(|s| s.to_string());
        DependencyType::Registry { name, version }
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

    let dep_type = detect_dependency_type(arg);

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

    println!("Added dependency '{}' (path: {})", dep_name, dep_path);
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

    println!("Added dependency '{}' (git: {})", dep_name, url);
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

    println!(
        "Added dependency '{}' (version: {}, registry: github)",
        name, resolved_version
    );
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
    let latest = semver::latest_version(&versions)
        .ok_or_else(|| format!(
            "No valid semver tags found for package '{}' in repository '{}'",
            name, repo
        ))?;

    Ok(semver::version_to_string(&latest))
}

/// Blocking wrapper for resolve_registry_version.
/// Uses tokio runtime to execute the async function.
fn resolve_registry_version_blocking(name: &str) -> Result<String, String> {
    // Create a new runtime for the async operation
    let rt =
        tokio::runtime::Runtime::new().map_err(|e| format!("Failed to create runtime: {}", e))?;
    rt.block_on(resolve_registry_version(name))
}
