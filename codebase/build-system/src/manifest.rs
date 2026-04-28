// manifest.rs — Parsing and generation of `gradient.toml` project manifests
//
// Supports the enhanced manifest format with path-based, git, and registry dependencies:
//
// ```toml
// [package]
// name = "my-project"
// version = "0.1.0"
//
// [dependencies]
// math-utils = { path = "../math-utils" }
// json-lib = { version = "1.2.0", registry = "github" }
// git-dep = { git = "https://github.com/user/repo.git" }
// ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The top-level manifest read from `gradient.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub package: Package,
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
}

/// The `[package]` section of a Gradient manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edition: Option<String>,
}

/// A single dependency entry in the `[dependencies]` table.
///
/// Supports three forms:
/// - Path dependency: `dep-name = { path = "../dep" }`
/// - Version dependency: `dep-name = "1.0"` or `dep-name = { version = "1.0", registry = "github" }`
/// - Git dependency: `dep-name = { git = "https://..." }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    /// A simple version string: `dep = "1.0"`
    Simple(String),
    /// A detailed dependency specification with optional fields.
    Detailed(DetailedDependency),
}

/// Detailed dependency with optional path, version, git, registry, and rev fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailedDependency {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>, // None = local/git, "github" = GitHub
    /// H-1: Commit SHA for git dependencies (required for security)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

impl Dependency {
    /// Returns the path if this is a path-based dependency.
    pub fn path(&self) -> Option<&str> {
        match self {
            Dependency::Simple(_) => None,
            Dependency::Detailed(d) => d.path.as_deref(),
        }
    }

    /// Returns the version string if specified.
    pub fn version(&self) -> Option<&str> {
        match self {
            Dependency::Simple(v) => Some(v.as_str()),
            Dependency::Detailed(d) => d.version.as_deref(),
        }
    }

    /// Returns the git URL if specified.
    pub fn git(&self) -> Option<&str> {
        match self {
            Dependency::Simple(_) => None,
            Dependency::Detailed(d) => d.git.as_deref(),
        }
    }

    /// Returns the registry name if specified.
    pub fn registry(&self) -> Option<&str> {
        match self {
            Dependency::Simple(_) => None,
            Dependency::Detailed(d) => d.registry.as_deref(),
        }
    }

    /// H-1: Returns the commit SHA (rev) if specified.
    pub fn rev(&self) -> Option<&str> {
        match self {
            Dependency::Simple(_) => None,
            Dependency::Detailed(d) => d.rev.as_deref(),
        }
    }

    /// Create a new path dependency.
    pub fn from_path(path: &str) -> Self {
        Dependency::Detailed(DetailedDependency {
            path: Some(path.to_string()),
            version: None,
            git: None,
            registry: None,
            rev: None,
        })
    }

    /// Create a new git dependency.
    pub fn from_git(url: &str) -> Self {
        Dependency::Detailed(DetailedDependency {
            path: None,
            version: None,
            git: Some(url.to_string()),
            registry: None,
            rev: None,
        })
    }

    /// Create a new registry dependency.
    pub fn from_registry(version: &str, registry: &str) -> Self {
        Dependency::Detailed(DetailedDependency {
            path: None,
            version: Some(version.to_string()),
            git: None,
            registry: Some(registry.to_string()),
            rev: None,
        })
    }
}

/// Load and parse a `gradient.toml` manifest from the given directory.
///
/// Looks for `gradient.toml` in `project_dir` and deserializes it
/// into a `Manifest`.
pub fn load(project_dir: &Path) -> Result<Manifest, Box<dyn std::error::Error>> {
    let manifest_path = project_dir.join("gradient.toml");
    let contents = std::fs::read_to_string(&manifest_path)?;
    let manifest: Manifest = toml::from_str(&contents)?;
    Ok(manifest)
}

/// Parse a manifest from a TOML string directly.
pub fn parse(contents: &str) -> Result<Manifest, Box<dyn std::error::Error>> {
    let manifest: Manifest = toml::from_str(contents)?;
    validate(&manifest)?;
    Ok(manifest)
}

/// M-1: Validate package name against regex ^[a-zA-Z][a-zA-Z0-9_-]{0,63}$
fn validate_package_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Package name cannot be empty".to_string());
    }
    if name.len() > 64 {
        return Err(format!("Package name '{}' exceeds 64 characters", name));
    }
    // Check first character is alphabetic
    let first = name.chars().next().unwrap();
    if !first.is_ascii_alphabetic() {
        return Err(format!(
            "Package name '{}' must start with a letter (a-z, A-Z)",
            name
        ));
    }
    // Check all characters are valid
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '_' && c != '-' {
            return Err(format!(
                "Package name '{}' contains invalid character '{}'. Only letters, digits, underscores, and hyphens are allowed",
                name, c
            ));
        }
    }
    // M-1: Reject flag-shaped names (start with -)
    if name.starts_with('-') {
        return Err(format!("Package name '{}' cannot start with a hyphen", name));
    }
    Ok(())
}

/// H-1: Validate that git dependencies have a rev (commit SHA)
fn validate_git_dependency(name: &str, dep: &Dependency) -> Result<(), String> {
    if let Some(git_url) = dep.git() {
        if dep.rev().is_none() {
            return Err(format!(
                "Git dependency '{}' from '{}' must specify a commit SHA via 'rev'. \
                Use: {} = {{ git = \"{}\", rev = \"<40-char-sha>\" }}",
                name, git_url, name, git_url
            ));
        }
        // Validate SHA format (40 hex chars)
        let rev = dep.rev().unwrap();
        if rev.len() != 40 || !rev.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "Git dependency '{}' has invalid rev '{}'. Must be a 40-character hex SHA",
                name, rev
            ));
        }
    }
    Ok(())
}

/// Validate a manifest according to M-1 and H-1 rules.
pub fn validate(manifest: &Manifest) -> Result<(), Box<dyn std::error::Error>> {
    // M-1: Validate package name
    validate_package_name(&manifest.package.name)
        .map_err(|e| format!("Invalid [package].name: {}", e))?;

    // H-1: Validate all dependencies
    for (name, dep) in &manifest.dependencies {
        validate_git_dependency(name, dep)
            .map_err(|e| format!("Invalid dependency '{}': {}", name, e))?;
    }

    Ok(())
}

/// Generate a default `gradient.toml` manifest for a new project.
///
/// Returns the TOML content as a string with the project name filled in.
pub fn create_default(project_name: &str) -> String {
    let manifest = Manifest {
        package: Package {
            name: project_name.to_string(),
            version: "0.1.0".to_string(),
            edition: Some("2026".to_string()),
        },
        dependencies: BTreeMap::new(),
    };
    toml::to_string_pretty(&manifest).expect("failed to serialize default manifest")
}

/// Add a path dependency to the manifest TOML file, preserving formatting.
/// Uses `toml_edit` for minimal-disruption editing.
pub fn add_path_dependency(
    manifest_path: &Path,
    dep_name: &str,
    dep_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(manifest_path)?;
    let mut doc = contents.parse::<toml_edit::DocumentMut>()?;

    // Ensure [dependencies] table exists
    if doc.get("dependencies").is_none() {
        doc["dependencies"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    // Build the inline table for { path = "..." }
    let mut inline = toml_edit::InlineTable::new();
    inline.insert("path", toml_edit::Value::from(dep_path));

    doc["dependencies"][dep_name] = toml_edit::value(inline);

    std::fs::write(manifest_path, doc.to_string())?;
    Ok(())
}

/// Add a git dependency to the manifest TOML file.
/// H-1: Requires a commit SHA (rev) for security.
pub fn add_git_dependency(
    manifest_path: &Path,
    dep_name: &str,
    url: &str,
    rev: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(manifest_path)?;
    let mut doc = contents.parse::<toml_edit::DocumentMut>()?;

    // Ensure [dependencies] table exists
    if doc.get("dependencies").is_none() {
        doc["dependencies"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    // Build the inline table for { git = "...", rev = "..." }
    let mut inline = toml_edit::InlineTable::new();
    inline.insert("git", toml_edit::Value::from(url));
    inline.insert("rev", toml_edit::Value::from(rev));

    doc["dependencies"][dep_name] = toml_edit::value(inline);

    std::fs::write(manifest_path, doc.to_string())?;
    Ok(())
}

/// Add a registry dependency to the manifest TOML file.
pub fn add_registry_dependency(
    manifest_path: &Path,
    dep_name: &str,
    version: &str,
    registry: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(manifest_path)?;
    let mut doc = contents.parse::<toml_edit::DocumentMut>()?;

    // Ensure [dependencies] table exists
    if doc.get("dependencies").is_none() {
        doc["dependencies"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    // Build the inline table for { version = "...", registry = "..." }
    let mut inline = toml_edit::InlineTable::new();
    inline.insert("version", toml_edit::Value::from(version));
    inline.insert("registry", toml_edit::Value::from(registry));

    doc["dependencies"][dep_name] = toml_edit::value(inline);

    std::fs::write(manifest_path, doc.to_string())?;
    Ok(())
}

/// Legacy alias for add_path_dependency (for backward compatibility).
#[deprecated(since = "0.2.0", note = "Use add_path_dependency instead")]
pub fn add_dependency(
    manifest_path: &Path,
    dep_name: &str,
    dep_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    add_path_dependency(manifest_path, dep_name, dep_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_manifest_with_dependencies() {
        let toml = r#"
[package]
name = "my-app"
version = "0.1.0"

[dependencies]
math-utils = { path = "../math-utils" }
logging = { path = "../logging" }
"#;
        let manifest = parse(toml).unwrap();
        assert_eq!(manifest.package.name, "my-app");
        assert_eq!(manifest.package.version, "0.1.0");
        assert_eq!(manifest.dependencies.len(), 2);

        let math = &manifest.dependencies["math-utils"];
        assert_eq!(math.path(), Some("../math-utils"));

        let logging = &manifest.dependencies["logging"];
        assert_eq!(logging.path(), Some("../logging"));
    }

    #[test]
    fn parse_manifest_empty_dependencies() {
        let toml = r#"
[package]
name = "empty-project"
version = "1.0.0"
"#;
        let manifest = parse(toml).unwrap();
        assert_eq!(manifest.package.name, "empty-project");
        assert!(manifest.dependencies.is_empty());
    }

    #[test]
    fn parse_manifest_with_version_dep() {
        let toml = r#"
[package]
name = "my-app"
version = "0.1.0"

[dependencies]
json = "1.0"
"#;
        let manifest = parse(toml).unwrap();
        let json = &manifest.dependencies["json"];
        assert_eq!(json.version(), Some("1.0"));
        assert_eq!(json.path(), None);
    }

    #[test]
    fn parse_manifest_with_registry_dep() {
        let toml = r#"
[package]
name = "my-app"
version = "0.1.0"

[dependencies]
math = { version = "1.2.0", registry = "github" }
"#;
        let manifest = parse(toml).unwrap();
        let math = &manifest.dependencies["math"];
        assert_eq!(math.version(), Some("1.2.0"));
        assert_eq!(math.registry(), Some("github"));
        assert_eq!(math.path(), None);
        assert_eq!(math.git(), None);
    }

    #[test]
    fn parse_manifest_with_git_dep() {
        let toml = r#"
[package]
name = "my-app"
version = "0.1.0"

[dependencies]
utils = { git = "https://github.com/example/utils.git", rev = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0" }
"#;
        let manifest = parse(toml).unwrap();
        let utils = &manifest.dependencies["utils"];
        assert_eq!(utils.git(), Some("https://github.com/example/utils.git"));
        assert_eq!(utils.path(), None);
        assert_eq!(utils.version(), None);
        assert_eq!(utils.rev(), Some("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0"));
    }

    #[test]
    fn create_default_manifest() {
        let content = create_default("test-proj");
        let manifest = parse(&content).unwrap();
        assert_eq!(manifest.package.name, "test-proj");
        assert_eq!(manifest.package.version, "0.1.0");
        assert!(manifest.dependencies.is_empty());
    }

    #[test]
    fn dependency_from_path() {
        let dep = Dependency::from_path("../libs/math");
        assert_eq!(dep.path(), Some("../libs/math"));
        assert_eq!(dep.version(), None);
        assert_eq!(dep.registry(), None);
    }

    #[test]
    fn dependency_from_git() {
        let dep = Dependency::from_git("https://github.com/example/repo.git");
        assert_eq!(dep.git(), Some("https://github.com/example/repo.git"));
        assert_eq!(dep.path(), None);
        assert_eq!(dep.registry(), None);
    }

    #[test]
    fn dependency_from_registry() {
        let dep = Dependency::from_registry("1.2.0", "github");
        assert_eq!(dep.version(), Some("1.2.0"));
        assert_eq!(dep.registry(), Some("github"));
        assert_eq!(dep.path(), None);
        assert_eq!(dep.git(), None);
    }

    #[test]
    fn add_path_dependency_modifies_toml() {
        let tmp = std::env::temp_dir().join("gradient_test_add_path_dep");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Write an initial manifest
        let manifest_path = tmp.join("gradient.toml");
        let initial = create_default("my-project");
        std::fs::write(&manifest_path, &initial).unwrap();

        // Add a dependency
        add_path_dependency(&manifest_path, "math-utils", "../math-utils").unwrap();

        // Re-read and parse
        let contents = std::fs::read_to_string(&manifest_path).unwrap();
        let manifest = parse(&contents).unwrap();

        assert_eq!(manifest.package.name, "my-project");
        assert_eq!(manifest.dependencies.len(), 1);
        assert_eq!(
            manifest.dependencies["math-utils"].path(),
            Some("../math-utils")
        );

        // Add another dependency
        add_path_dependency(&manifest_path, "logging", "../logging").unwrap();

        let contents = std::fs::read_to_string(&manifest_path).unwrap();
        let manifest = parse(&contents).unwrap();
        assert_eq!(manifest.dependencies.len(), 2);
        assert_eq!(manifest.dependencies["logging"].path(), Some("../logging"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn add_registry_dependency_modifies_toml() {
        let tmp = std::env::temp_dir().join("gradient_test_add_registry_dep");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Write an initial manifest
        let manifest_path = tmp.join("gradient.toml");
        let initial = create_default("my-project");
        std::fs::write(&manifest_path, &initial).unwrap();

        // Add a registry dependency
        add_registry_dependency(&manifest_path, "math", "1.2.0", "github").unwrap();

        // Re-read and parse
        let contents = std::fs::read_to_string(&manifest_path).unwrap();
        let manifest = parse(&contents).unwrap();

        assert_eq!(manifest.dependencies.len(), 1);
        let math = &manifest.dependencies["math"];
        assert_eq!(math.version(), Some("1.2.0"));
        assert_eq!(math.registry(), Some("github"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn add_git_dependency_modifies_toml() {
        let tmp = std::env::temp_dir().join("gradient_test_add_git_dep");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Write an initial manifest
        let manifest_path = tmp.join("gradient.toml");
        let initial = create_default("my-project");
        std::fs::write(&manifest_path, &initial).unwrap();

        // Add a git dependency
        add_git_dependency(
            &manifest_path,
            "utils",
            "https://github.com/example/utils.git",
            "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0",
        )
        .unwrap();

        // Re-read and parse
        let contents = std::fs::read_to_string(&manifest_path).unwrap();
        let manifest = parse(&contents).unwrap();

        assert_eq!(manifest.dependencies.len(), 1);
        let utils = &manifest.dependencies["utils"];
        assert_eq!(utils.git(), Some("https://github.com/example/utils.git"));
        assert_eq!(utils.rev(), Some("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0"));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
