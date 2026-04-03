// resolver.rs — Dependency resolution for Gradient projects
//
// Walks the dependency graph starting from the root manifest, resolves
// path-based dependencies, detects cycles, and returns an ordered list
// of dependencies to compile.

use crate::lockfile::{compute_directory_checksum, LockedPackage, Lockfile};
use crate::manifest::{self, Manifest};
use crate::registry::{semver, GitHubClient, Version};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// A resolved dependency with all information needed for compilation.
#[derive(Debug, Clone)]
pub struct ResolvedDependency {
    /// The dependency name.
    pub name: String,
    /// The version from its manifest.
    pub version: String,
    /// The absolute path to the dependency root directory.
    pub root: PathBuf,
    /// Absolute paths to all `.gr` source files in this dependency.
    pub source_files: Vec<PathBuf>,
}

/// The result of resolving all dependencies for a project.
#[derive(Debug)]
pub struct ResolvedGraph {
    /// Dependencies in topological order (leaves first, so they can be
    /// compiled before the packages that depend on them).
    pub dependencies: Vec<ResolvedDependency>,
    /// The generated/updated lockfile.
    pub lockfile: Lockfile,
}

/// Errors that can occur during dependency resolution.
#[derive(Debug)]
pub enum ResolveError {
    /// A dependency cycle was detected.
    CyclicDependency {
        /// The chain of package names forming the cycle.
        cycle: Vec<String>,
    },
    /// A path dependency could not be found on disk.
    DependencyNotFound {
        name: String,
        path: PathBuf,
        referenced_from: String,
    },
    /// A dependency has no path (e.g., version-only deps are not yet supported).
    UnsupportedDependency {
        name: String,
        referenced_from: String,
    },
    /// Failed to parse a dependency's manifest.
    ManifestError {
        name: String,
        path: PathBuf,
        error: String,
    },
    /// Registry error when fetching package information.
    RegistryError { name: String, message: String },
    /// Version resolution failed - no matching version found.
    VersionResolutionFailed {
        name: String,
        requested: String,
        available: Vec<String>,
    },
    /// I/O or other error.
    Other(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::CyclicDependency { cycle } => {
                write!(f, "Dependency cycle detected: {}", cycle.join(" -> "))
            }
            ResolveError::DependencyNotFound {
                name,
                path,
                referenced_from,
            } => {
                write!(
                    f,
                    "Dependency '{}' not found at path '{}' (referenced from '{}')",
                    name,
                    path.display(),
                    referenced_from
                )
            }
            ResolveError::UnsupportedDependency {
                name,
                referenced_from,
            } => {
                write!(
                    f,
                    "Dependency '{}' (referenced from '{}') is a registry dependency, \
                     which is not yet supported. Use a path dependency instead.",
                    name, referenced_from
                )
            }
            ResolveError::ManifestError { name, path, error } => {
                write!(
                    f,
                    "Failed to parse manifest for '{}' at '{}': {}",
                    name,
                    path.display(),
                    error
                )
            }
            ResolveError::RegistryError { name, message } => {
                write!(f, "Registry error for package '{}': {}", name, message)
            }
            ResolveError::VersionResolutionFailed {
                name,
                requested,
                available,
            } => {
                let available_str = if available.is_empty() {
                    "none".to_string()
                } else {
                    available.join(", ")
                };
                write!(
                    f,
                    "Could not resolve version '{}' for '{}'. Available versions: {}",
                    requested, name, available_str
                )
            }
            ResolveError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Resolve all dependencies for a project starting from its root directory.
///
/// This function:
/// 1. Reads the root `gradient.toml`
/// 2. Recursively resolves path dependencies
/// 3. Detects circular dependencies
/// 4. Returns dependencies in compilation order (leaves first)
/// 5. Generates a lockfile with checksums
pub fn resolve(project_dir: &Path) -> Result<ResolvedGraph, ResolveError> {
    let manifest = manifest::load(project_dir).map_err(|e| ResolveError::Other(e.to_string()))?;
    resolve_from_manifest(&manifest, project_dir)
}

/// Resolve dependencies given an already-parsed manifest and project root.
pub fn resolve_from_manifest(
    manifest: &Manifest,
    project_dir: &Path,
) -> Result<ResolvedGraph, ResolveError> {
    let root_name = manifest.package.name.clone();

    // Collected resolved deps in topological order
    let mut resolved: Vec<ResolvedDependency> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut in_progress: Vec<String> = Vec::new();

    // All manifests we've loaded, keyed by canonical path
    let mut manifest_cache: BTreeMap<PathBuf, Manifest> = BTreeMap::new();

    resolve_recursive(
        &root_name,
        manifest,
        project_dir,
        &mut resolved,
        &mut visited,
        &mut in_progress,
        &mut manifest_cache,
    )?;

    // Build lockfile
    let mut lockfile = Lockfile::new();
    for dep in &resolved {
        let rel_path = pathdiff(project_dir, &dep.root);
        let checksum = compute_directory_checksum(&dep.root)
            .map_err(|e| ResolveError::Other(format!("Failed to compute checksum: {}", e)))?;

        lockfile.add_package(LockedPackage {
            name: dep.name.clone(),
            version: dep.version.clone(),
            source: format!("path:{}", rel_path),
            checksum,
        });
    }
    lockfile.sort();

    Ok(ResolvedGraph {
        dependencies: resolved,
        lockfile,
    })
}

/// Recursively resolve dependencies using DFS, detecting cycles.
fn resolve_recursive(
    parent_name: &str,
    manifest: &Manifest,
    manifest_dir: &Path,
    resolved: &mut Vec<ResolvedDependency>,
    visited: &mut HashSet<String>,
    in_progress: &mut Vec<String>,
    manifest_cache: &mut BTreeMap<PathBuf, Manifest>,
) -> Result<(), ResolveError> {
    for (dep_name, dep) in &manifest.dependencies {
        // Already fully resolved
        if visited.contains(dep_name) {
            continue;
        }

        // Cycle detection
        if in_progress.contains(dep_name) {
            let mut cycle: Vec<String> = in_progress
                .iter()
                .skip_while(|n| *n != dep_name)
                .cloned()
                .collect();
            cycle.push(dep_name.clone());
            return Err(ResolveError::CyclicDependency { cycle });
        }

        // Get the path for this dependency
        let rel_path = dep
            .path()
            .ok_or_else(|| ResolveError::UnsupportedDependency {
                name: dep_name.clone(),
                referenced_from: parent_name.to_string(),
            })?;

        let dep_dir = manifest_dir.join(rel_path);
        let dep_dir = dep_dir
            .canonicalize()
            .map_err(|_| ResolveError::DependencyNotFound {
                name: dep_name.clone(),
                path: dep_dir.clone(),
                referenced_from: parent_name.to_string(),
            })?;

        if !dep_dir.join("gradient.toml").is_file() {
            return Err(ResolveError::DependencyNotFound {
                name: dep_name.clone(),
                path: dep_dir,
                referenced_from: parent_name.to_string(),
            });
        }

        // Load the dependency's manifest
        let dep_manifest = if let Some(cached) = manifest_cache.get(&dep_dir) {
            cached.clone()
        } else {
            let m = manifest::load(&dep_dir).map_err(|e| ResolveError::ManifestError {
                name: dep_name.clone(),
                path: dep_dir.clone(),
                error: e.to_string(),
            })?;
            manifest_cache.insert(dep_dir.clone(), m.clone());
            m
        };

        // Mark as in-progress and recurse into transitive deps
        in_progress.push(dep_name.clone());

        resolve_recursive(
            dep_name,
            &dep_manifest,
            &dep_dir,
            resolved,
            visited,
            in_progress,
            manifest_cache,
        )?;

        in_progress.pop();

        // Collect source files
        let source_files = collect_source_files(&dep_dir);

        // Add to resolved list (post-order: dependencies first)
        resolved.push(ResolvedDependency {
            name: dep_name.clone(),
            version: dep_manifest.package.version.clone(),
            root: dep_dir,
            source_files,
        });
        visited.insert(dep_name.clone());
    }

    Ok(())
}

/// Collect all `.gr` source files under a directory.
fn collect_source_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_source_files_recursive(dir, &mut files);
    files.sort();
    files
}

fn collect_source_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with('.') && name_str != "target" {
                collect_source_files_recursive(&path, files);
            }
        } else if path.extension().is_some_and(|ext| ext == "gr") {
            files.push(path);
        }
    }
}

/// Compute a relative path from `base` to `target`.
/// Falls back to returning the target's display string if relative path
/// computation fails.
fn pathdiff(base: &Path, target: &Path) -> String {
    // Try to compute relative path
    let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let target = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());

    // Simple approach: strip common prefix
    let base_components: Vec<_> = base.components().collect();
    let target_components: Vec<_> = target.components().collect();

    let common_len = base_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let ups = base_components.len() - common_len;
    let mut result = String::new();
    for _ in 0..ups {
        if !result.is_empty() {
            result.push('/');
        }
        result.push_str("..");
    }
    for comp in &target_components[common_len..] {
        if !result.is_empty() {
            result.push('/');
        }
        result.push_str(&comp.as_os_str().to_string_lossy());
    }

    if result.is_empty() {
        ".".to_string()
    } else {
        result
    }
}

/// Resolver for handling both path and registry dependencies
#[derive(Debug)]
pub struct Resolver {
    /// Project root directory
    project_dir: PathBuf,
    /// GitHub client for fetching packages from registry
    github_client: Option<GitHubClient>,
}

impl Resolver {
    /// Create a new resolver for the given project directory
    pub fn new(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
            github_client: None,
        }
    }

    /// Set the GitHub client for resolving registry dependencies
    pub fn with_github(mut self, client: GitHubClient) -> Self {
        self.github_client = Some(client);
        self
    }

    /// Check if a package version exists in the local cache
    fn check_cache(&self, name: &str, version: &Version) -> Option<PathBuf> {
        // Use the same cache directory as RegistryClient: ~/.gradient/cache
        let home_dir = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()?;
        let cache_dir = PathBuf::from(home_dir).join(".gradient").join("cache");
        let cache_path = cache_dir.join(name).join(version.to_string());
        if cache_path.is_dir() && cache_path.join("gradient.toml").is_file() {
            Some(cache_path)
        } else {
            None
        }
    }

    /// Resolve a registry dependency by fetching tags, resolving version, and checking cache
    pub async fn resolve_registry_dep(
        &self,
        name: &str,
        version_req: Option<&str>,
    ) -> Result<ResolvedDependency, ResolveError> {
        let github = self
            .github_client
            .as_ref()
            .ok_or_else(|| ResolveError::RegistryError {
                name: name.to_string(),
                message: "No GitHub client configured".to_string(),
            })?;

        // Fetch tags from GitHub for gradient-lang/{name}
        let repo = format!("gradient-lang/{}", name);
        let tags = github
            .fetch_tags(&repo)
            .await
            .map_err(|e| ResolveError::RegistryError {
                name: name.to_string(),
                message: format!("Failed to fetch tags: {}", e),
            })?;

        // Parse tags as semver versions
        let versions = parse_tags_as_versions(&tags);

        if versions.is_empty() {
            return Err(ResolveError::RegistryError {
                name: name.to_string(),
                message: "No valid semver tags found in repository".to_string(),
            });
        }

        // Resolve to specific version
        let resolved_version = if let Some(req_str) = version_req {
            let req =
                semver::parse_version_req(req_str).map_err(|e| ResolveError::RegistryError {
                    name: name.to_string(),
                    message: e,
                })?;
            semver::resolve_version(&versions, &req).ok_or_else(|| {
                let available: Vec<String> = versions
                    .iter()
                    .map(|v| semver::version_to_string(v))
                    .collect();
                ResolveError::VersionResolutionFailed {
                    name: name.to_string(),
                    requested: req_str.to_string(),
                    available,
                }
            })?
        } else {
            // No version requirement specified, use latest
            semver::latest_version(&versions).expect("versions is not empty")
        };

        // Check cache for resolved version
        let cache_path = self.check_cache(name, &resolved_version);

        if let Some(cached_path) = cache_path {
            // Load the manifest from cached path
            let dep_manifest =
                manifest::load(&cached_path).map_err(|e| ResolveError::ManifestError {
                    name: name.to_string(),
                    path: cached_path.clone(),
                    error: e.to_string(),
                })?;

            // Collect source files
            let source_files = collect_source_files(&cached_path);

            Ok(ResolvedDependency {
                name: name.to_string(),
                version: dep_manifest.package.version.clone(),
                root: cached_path,
                source_files,
            })
        } else {
            // Not cached - return error indicating download needed (Workstream 3)
            Err(ResolveError::RegistryError {
                name: name.to_string(),
                message: format!(
                    "Version {} is not cached. Run 'gradient fetch' to download.",
                    semver::version_to_string(&resolved_version)
                ),
            })
        }
    }
}

/// Parse git tags as semver versions
fn parse_tags_as_versions(tags: &[String]) -> Vec<Version> {
    tags.iter()
        .filter_map(|t| {
            // Strip 'v' prefix if present
            let v_str = t.strip_prefix('v').unwrap_or(t);
            Version::parse(v_str).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a minimal Gradient project in a temp directory.
    fn create_test_project(base: &Path, name: &str, deps: &[(&str, &str)]) {
        let dir = base.join(name);
        fs::create_dir_all(dir.join("src")).unwrap();

        let mut dep_lines = String::new();
        for (dep_name, dep_path) in deps {
            dep_lines.push_str(&format!("{} = {{ path = \"{}\" }}\n", dep_name, dep_path));
        }

        let manifest = format!(
            "[package]\nname = \"{}\"\nversion = \"0.1.0\"\n\n[dependencies]\n{}",
            name, dep_lines
        );

        fs::write(dir.join("gradient.toml"), manifest).unwrap();
        fs::write(
            dir.join("src/main.gr"),
            format!(
                "mod {}\n\nfn main() -> !{{IO}} ():\n    print(\"hello\")\n",
                name
            ),
        )
        .unwrap();
    }

    #[test]
    fn resolve_single_path_dependency() {
        let tmp = std::env::temp_dir().join("gradient_test_resolve_single");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        create_test_project(&tmp, "dep-a", &[]);
        create_test_project(&tmp, "root", &[("dep-a", "../dep-a")]);

        let result = resolve(&tmp.join("root")).unwrap();
        assert_eq!(result.dependencies.len(), 1);
        assert_eq!(result.dependencies[0].name, "dep-a");
        assert!(!result.dependencies[0].source_files.is_empty());

        assert_eq!(result.lockfile.packages.len(), 1);
        assert_eq!(result.lockfile.packages[0].name, "dep-a");
        assert!(result.lockfile.packages[0].checksum.starts_with("sha256:"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_transitive_dependencies() {
        let tmp = std::env::temp_dir().join("gradient_test_resolve_transitive");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // root -> dep-b -> dep-a
        create_test_project(&tmp, "dep-a", &[]);
        create_test_project(&tmp, "dep-b", &[("dep-a", "../dep-a")]);
        create_test_project(&tmp, "root", &[("dep-b", "../dep-b")]);

        let result = resolve(&tmp.join("root")).unwrap();
        assert_eq!(result.dependencies.len(), 2);
        // dep-a should come first (leaf-first order)
        assert_eq!(result.dependencies[0].name, "dep-a");
        assert_eq!(result.dependencies[1].name, "dep-b");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_circular_dependency() {
        let tmp = std::env::temp_dir().join("gradient_test_resolve_cycle");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // root -> dep-a -> dep-b -> dep-a (cycle)
        create_test_project(&tmp, "dep-a", &[("dep-b", "../dep-b")]);
        create_test_project(&tmp, "dep-b", &[("dep-a", "../dep-a")]);
        create_test_project(&tmp, "root", &[("dep-a", "../dep-a")]);

        let result = resolve(&tmp.join("root"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ResolveError::CyclicDependency { cycle } => {
                assert!(cycle.contains(&"dep-a".to_string()));
                assert!(cycle.contains(&"dep-b".to_string()));
            }
            other => panic!("Expected CyclicDependency, got: {}", other),
        }

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_diamond_dependency() {
        let tmp = std::env::temp_dir().join("gradient_test_resolve_diamond");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // root -> dep-b, dep-c; dep-b -> dep-a; dep-c -> dep-a
        create_test_project(&tmp, "dep-a", &[]);
        create_test_project(&tmp, "dep-b", &[("dep-a", "../dep-a")]);
        create_test_project(&tmp, "dep-c", &[("dep-a", "../dep-a")]);
        create_test_project(
            &tmp,
            "root",
            &[("dep-b", "../dep-b"), ("dep-c", "../dep-c")],
        );

        let result = resolve(&tmp.join("root")).unwrap();
        // dep-a should appear only once
        let names: Vec<&str> = result
            .dependencies
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(
            names.iter().filter(|n| **n == "dep-a").count(),
            1,
            "dep-a should only appear once"
        );
        assert_eq!(result.dependencies.len(), 3);
        // dep-a should be first (it's a leaf)
        assert_eq!(names[0], "dep-a");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_no_dependencies() {
        let tmp = std::env::temp_dir().join("gradient_test_resolve_none");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        create_test_project(&tmp, "root", &[]);

        let result = resolve(&tmp.join("root")).unwrap();
        assert!(result.dependencies.is_empty());
        assert!(result.lockfile.packages.is_empty());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn unsupported_version_dependency() {
        let tmp = std::env::temp_dir().join("gradient_test_resolve_unsupported");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let dir = tmp.join("root");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("gradient.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\n\n[dependencies]\njson = \"1.0\"\n",
        )
        .unwrap();
        fs::write(dir.join("src/main.gr"), "mod main\n").unwrap();

        let result = resolve(&dir);
        assert!(result.is_err());
        match result.unwrap_err() {
            ResolveError::UnsupportedDependency { name, .. } => {
                assert_eq!(name, "json");
            }
            other => panic!("Expected UnsupportedDependency, got: {}", other),
        }

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn build_with_path_dependency_generates_lockfile() {
        // Integration test: resolve deps, generate lockfile, save it, reload it,
        // and verify the whole pipeline is consistent.
        let tmp = std::env::temp_dir().join("gradient_test_build_with_deps");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // Create a library dependency
        let lib_dir = tmp.join("math-lib");
        fs::create_dir_all(lib_dir.join("src")).unwrap();
        fs::write(
            lib_dir.join("gradient.toml"),
            "[package]\nname = \"math-lib\"\nversion = \"0.3.0\"\n",
        )
        .unwrap();
        fs::write(
            lib_dir.join("src/lib.gr"),
            "mod math\n\nfn add(a: Int, b: Int) -> Int:\n    a + b\n",
        )
        .unwrap();

        // Create root project depending on math-lib
        let root_dir = tmp.join("app");
        fs::create_dir_all(root_dir.join("src")).unwrap();
        fs::write(
            root_dir.join("gradient.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n\n[dependencies]\nmath-lib = { path = \"../math-lib\" }\n",
        )
        .unwrap();
        fs::write(
            root_dir.join("src/main.gr"),
            "mod main\n\nfn main() -> !{IO} ():\n    print(\"hello\")\n",
        )
        .unwrap();

        // Resolve
        let graph = resolve(&root_dir).unwrap();
        assert_eq!(graph.dependencies.len(), 1);
        assert_eq!(graph.dependencies[0].name, "math-lib");
        assert_eq!(graph.dependencies[0].version, "0.3.0");
        assert!(!graph.dependencies[0].source_files.is_empty());

        // Save lockfile
        graph.lockfile.save(&root_dir).unwrap();
        assert!(root_dir.join("gradient.lock").is_file());

        // Reload lockfile and verify
        let loaded = crate::lockfile::Lockfile::load(&root_dir).unwrap();
        assert_eq!(loaded.packages.len(), 1);
        assert_eq!(loaded.packages[0].name, "math-lib");
        assert_eq!(loaded.packages[0].version, "0.3.0");
        assert!(loaded.packages[0].source.starts_with("path:"));
        assert!(loaded.packages[0].checksum.starts_with("sha256:"));

        // Validate checksums pass
        let mismatches = loaded.validate_checksums(&root_dir).unwrap();
        assert!(
            mismatches.is_empty(),
            "Checksums should match immediately after generation"
        );

        let _ = fs::remove_dir_all(&tmp);
    }
}
