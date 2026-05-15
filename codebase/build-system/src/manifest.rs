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
///
/// **#365**: in addition to the legacy `name`/`version`/`edition` fields,
/// a package may declare its maximum effect set and its requested
/// capabilities. These two fields lock in the agent-readable surface area
/// of the package: the registry can use them to reject installation when
/// a project's tier ceiling forbids one of them, and `gradient install`
/// can warn before a download happens (see `docs/registry/manifest.md`).
///
/// `effects` and `capabilities` are both stored as plain `Vec<String>`s
/// today because the typechecker uses the same canonical effect-name
/// vocabulary for both — see `KNOWN_EFFECTS` in
/// `gradient_compiler::typechecker::effects`. When E3 (#296) adds true
/// capability tokens distinct from effects, the `capabilities` field
/// will gain its own validator; until then both fields validate against
/// the same set.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Package {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edition: Option<String>,
    /// **#365**: declared maximum effect set. Optional — when absent, the
    /// package places no manifest-level ceiling on effects (per-function
    /// effects still apply during typecheck).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<String>>,
    /// **#365**: declared capability requests. Optional — when absent,
    /// the package requests no extra capabilities at install time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
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
        return Err(format!(
            "Package name '{}' cannot start with a hyphen",
            name
        ));
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

/// **#365**: Validate that every effect name in `[package].effects` (and in
/// `[package].capabilities`) is recognized by the Gradient typechecker.
///
/// This delegates to `gradient_compiler::typechecker::effects::is_valid_effect_name`
/// so the manifest accepts both bare known effects (`Heap`, `IO`, `Net`, ...)
/// and parameterized ones (`Throws(MyError)`, `FFI(C)`, `Arena(scratch)`, ...).
///
/// `capabilities` is validated against the same vocabulary today because
/// Gradient currently models capabilities as effect-name lists (see `@cap`
/// in `ast/item.rs::CapDecl`). When E3 (#296 / #321) adds typestate
/// capability tokens distinct from effects, swap the `capabilities` arm
/// for its own validator.
fn validate_effect_list(field_label: &str, names: &[String]) -> Result<(), String> {
    use gradient_compiler::typechecker::effects::is_valid_effect_name;
    for n in names {
        let trimmed = n.trim();
        if trimmed.is_empty() {
            return Err(format!(
                "[package].{} contains an empty entry — remove the empty string",
                field_label
            ));
        }
        if !is_valid_effect_name(trimmed) {
            return Err(format!(
                "[package].{} contains unknown effect '{}'. Allowed: {:?} plus \
parameterized forms like Throws(<Type>), FFI(<abi>), Arena(<name>).",
                field_label,
                trimmed,
                gradient_compiler::typechecker::effects::KNOWN_EFFECTS,
            ));
        }
    }
    Ok(())
}

/// Validate a manifest according to M-1, H-1, and #365 rules.
pub fn validate(manifest: &Manifest) -> Result<(), Box<dyn std::error::Error>> {
    // M-1: Validate package name
    validate_package_name(&manifest.package.name)
        .map_err(|e| format!("Invalid [package].name: {}", e))?;

    // H-1: Validate all dependencies
    for (name, dep) in &manifest.dependencies {
        validate_git_dependency(name, dep)
            .map_err(|e| format!("Invalid dependency '{}': {}", name, e))?;
    }

    // #365: validate declared effect set + capability requests.
    if let Some(effects) = &manifest.package.effects {
        validate_effect_list("effects", effects)?;
    }
    if let Some(capabilities) = &manifest.package.capabilities {
        validate_effect_list("capabilities", capabilities)?;
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
            effects: None,
            capabilities: None,
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
        assert_eq!(
            utils.rev(),
            Some("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0")
        );
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
        assert_eq!(
            utils.rev(),
            Some("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0")
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─────────────────────────────────────────────────────────────────────
    // #365 — declared max effect set + capability requests
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn manifest_with_effects_and_capabilities_parses() {
        let toml = r#"
[package]
name = "agent-runner"
version = "0.1.0"
effects = ["Heap", "IO", "Net"]
capabilities = ["FS", "Time"]
"#;
        let manifest = parse(toml).unwrap();
        assert_eq!(
            manifest.package.effects.as_deref(),
            Some(&["Heap".to_string(), "IO".to_string(), "Net".to_string()][..])
        );
        assert_eq!(
            manifest.package.capabilities.as_deref(),
            Some(&["FS".to_string(), "Time".to_string()][..])
        );
    }

    #[test]
    fn manifest_without_effects_or_capabilities_parses() {
        // Absent fields stay None; existing manifests continue to parse.
        let toml = r#"
[package]
name = "legacy"
version = "0.1.0"
"#;
        let manifest = parse(toml).unwrap();
        assert!(manifest.package.effects.is_none());
        assert!(manifest.package.capabilities.is_none());
    }

    #[test]
    fn manifest_round_trip_preserves_effects_and_capabilities() {
        let original = Manifest {
            package: Package {
                name: "round-trip".to_string(),
                version: "1.0.0".to_string(),
                edition: Some("2026".to_string()),
                effects: Some(vec!["Heap".to_string(), "Async".to_string()]),
                capabilities: Some(vec!["Net".to_string()]),
            },
            dependencies: BTreeMap::new(),
        };

        let serialized = toml::to_string(&original).unwrap();
        let parsed = parse(&serialized).unwrap();
        assert_eq!(parsed.package.effects, original.package.effects);
        assert_eq!(parsed.package.capabilities, original.package.capabilities);
        // Edition + name + version also survive.
        assert_eq!(parsed.package.name, original.package.name);
        assert_eq!(parsed.package.version, original.package.version);
        assert_eq!(parsed.package.edition, original.package.edition);
    }

    #[test]
    fn manifest_accepts_parameterized_effects() {
        // Throws(E), FFI(C), Arena(<name>) — all valid effect-name shapes.
        let toml = r#"
[package]
name = "params"
version = "0.1.0"
effects = ["Throws(MyError)", "FFI(C)", "Arena(scratch)"]
"#;
        let manifest = parse(toml).unwrap();
        let effects = manifest.package.effects.unwrap();
        assert_eq!(effects.len(), 3);
        assert_eq!(effects[0], "Throws(MyError)");
        assert_eq!(effects[1], "FFI(C)");
        assert_eq!(effects[2], "Arena(scratch)");
    }

    #[test]
    fn manifest_rejects_unknown_effect_name() {
        let toml = r#"
[package]
name = "bogus"
version = "0.1.0"
effects = ["NotAnEffect"]
"#;
        let err = parse(toml).unwrap_err().to_string();
        assert!(err.contains("[package].effects"), "got: {}", err);
        assert!(err.contains("NotAnEffect"), "got: {}", err);
    }

    #[test]
    fn manifest_rejects_unknown_capability_name() {
        let toml = r#"
[package]
name = "bogus-cap"
version = "0.1.0"
capabilities = ["MadeUpThing"]
"#;
        let err = parse(toml).unwrap_err().to_string();
        assert!(err.contains("[package].capabilities"), "got: {}", err);
        assert!(err.contains("MadeUpThing"), "got: {}", err);
    }

    #[test]
    fn manifest_rejects_empty_effect_entry() {
        let toml = r#"
[package]
name = "empty-entry"
version = "0.1.0"
effects = ["Heap", ""]
"#;
        let err = parse(toml).unwrap_err().to_string();
        assert!(err.contains("empty entry"), "got: {}", err);
    }

    #[test]
    fn manifest_empty_effects_list_is_valid() {
        // An empty list is the "declares zero allowed effects" case —
        // valid TOML, valid semantics (pure package).
        let toml = r#"
[package]
name = "pure"
version = "0.1.0"
effects = []
"#;
        let manifest = parse(toml).unwrap();
        assert_eq!(manifest.package.effects.as_deref(), Some(&[][..]));
    }

    #[test]
    fn manifest_with_dependencies_and_effects_round_trips() {
        let toml = r#"
[package]
name = "mixed"
version = "0.1.0"
effects = ["Heap"]

[dependencies]
math = "1.0"
utils = { path = "../utils" }
"#;
        let manifest = parse(toml).unwrap();
        assert_eq!(
            manifest.package.effects.as_deref(),
            Some(&["Heap".to_string()][..])
        );
        assert_eq!(manifest.dependencies.len(), 2);
        assert_eq!(manifest.dependencies["math"].version(), Some("1.0"));
        assert_eq!(manifest.dependencies["utils"].path(), Some("../utils"));
    }
}
