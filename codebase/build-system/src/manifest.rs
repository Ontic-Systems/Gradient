// manifest.rs — Parsing and generation of `gradient.toml` project manifests
//
// Supports the enhanced manifest format with path-based dependencies:
//
// ```toml
// [package]
// name = "my-project"
// version = "0.1.0"
//
// [dependencies]
// math-utils = { path = "../math-utils" }
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
/// Supports two forms:
/// - Path dependency: `dep-name = { path = "../dep" }`
/// - Version dependency (future): `dep-name = "1.0"` or `dep-name = { version = "1.0" }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    /// A simple version string: `dep = "1.0"` (future use)
    Simple(String),
    /// A detailed dependency specification with optional fields.
    Detailed(DetailedDependency),
}

/// Detailed dependency with optional path, version, and git fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailedDependency {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
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

    /// Create a new path dependency.
    pub fn from_path(path: &str) -> Self {
        Dependency::Detailed(DetailedDependency {
            path: Some(path.to_string()),
            version: None,
            git: None,
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
    Ok(manifest)
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

/// Add a dependency to the manifest TOML file, preserving formatting.
/// Uses `toml_edit` for minimal-disruption editing.
pub fn add_dependency(
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
    }

    #[test]
    fn add_dependency_modifies_toml() {
        let tmp = std::env::temp_dir().join("gradient_test_add_dep");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Write an initial manifest
        let manifest_path = tmp.join("gradient.toml");
        let initial = create_default("my-project");
        std::fs::write(&manifest_path, &initial).unwrap();

        // Add a dependency
        add_dependency(&manifest_path, "math-utils", "../math-utils").unwrap();

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
        add_dependency(&manifest_path, "logging", "../logging").unwrap();

        let contents = std::fs::read_to_string(&manifest_path).unwrap();
        let manifest = parse(&contents).unwrap();
        assert_eq!(manifest.dependencies.len(), 2);
        assert_eq!(
            manifest.dependencies["logging"].path(),
            Some("../logging")
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
