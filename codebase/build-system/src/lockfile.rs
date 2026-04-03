// lockfile.rs — Parsing and generation of `gradient.lock` lockfiles
//
// The lockfile records resolved dependency versions and integrity hashes
// so that builds are reproducible across machines and time.
//
// Format:
// ```toml
// [[package]]
// name = "math-utils"
// version = "0.1.0"
// source = "path:../math-utils"
// checksum = "sha256:abc123..."
//
// [[package]]
// name = "registry-pkg"
// version = "1.2.0"
// source = "github:namespace/name#v1.2.0"
// checksum = "sha256:def456..."
// ```

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::Path;

/// The source type for a locked package.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceType {
    /// A local path dependency: "path:../relative"
    Path(String),
    /// A git repository dependency: "git:https://...#rev"
    Git { url: String, rev: Option<String> },
    /// A registry package: "github:namespace/name#v1.2.0"
    Registry {
        registry: String,
        name: String,
        version: String,
    },
}

impl SourceType {
    /// Parse a source string into a SourceType.
    ///
    /// Supported formats:
    /// - `path:../relative` - Path dependency
    /// - `git:https://...#rev` - Git dependency with optional revision
    /// - `github:namespace/name#v1.2.0` - Registry package
    pub fn parse(source: &str) -> Result<Self, String> {
        if let Some(rest) = source.strip_prefix("path:") {
            Ok(SourceType::Path(rest.to_string()))
        } else if let Some(rest) = source.strip_prefix("git:") {
            // Parse git URL with optional #rev suffix
            if let Some((url, rev)) = rest.split_once('#') {
                Ok(SourceType::Git {
                    url: url.to_string(),
                    rev: Some(rev.to_string()),
                })
            } else {
                Ok(SourceType::Git {
                    url: rest.to_string(),
                    rev: None,
                })
            }
        } else if let Some(rest) = source.strip_prefix("github:") {
            // Parse github:namespace/name#version
            if let Some((name_part, version)) = rest.split_once('#') {
                Ok(SourceType::Registry {
                    registry: "github".to_string(),
                    name: name_part.to_string(),
                    version: version.to_string(),
                })
            } else {
                // No version specified - use the package name as-is
                // This shouldn't happen in practice but we handle it
                Ok(SourceType::Registry {
                    registry: "github".to_string(),
                    name: rest.to_string(),
                    version: String::new(),
                })
            }
        } else {
            // Legacy format - assume it's a path without prefix
            // This maintains backward compatibility
            Ok(SourceType::Path(source.to_string()))
        }
    }

    /// Returns true if this is a path dependency.
    pub fn is_path(&self) -> bool {
        matches!(self, SourceType::Path(_))
    }

    /// Returns true if this is a git dependency.
    pub fn is_git(&self) -> bool {
        matches!(self, SourceType::Git { .. })
    }

    /// Returns true if this is a registry dependency.
    pub fn is_registry(&self) -> bool {
        matches!(self, SourceType::Registry { .. })
    }

    /// Get the path if this is a path dependency.
    pub fn as_path(&self) -> Option<&str> {
        match self {
            SourceType::Path(p) => Some(p),
            _ => None,
        }
    }

    /// Get the registry info if this is a registry dependency.
    pub fn as_registry(&self) -> Option<(&str, &str, &str)> {
        match self {
            SourceType::Registry {
                registry,
                name,
                version,
            } => Some((registry.as_str(), name.as_str(), version.as_str())),
            _ => None,
        }
    }
}

impl fmt::Display for SourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceType::Path(path) => write!(f, "path:{}", path),
            SourceType::Git { url, rev: None } => write!(f, "git:{}", url),
            SourceType::Git {
                url,
                rev: Some(rev),
            } => write!(f, "git:{}#{}", url, rev),
            SourceType::Registry {
                registry,
                name,
                version,
            } => {
                write!(f, "{}:{}#{}", registry, name, version)
            }
        }
    }
}

/// A complete lockfile with all resolved packages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Lockfile {
    #[serde(default, rename = "package")]
    pub packages: Vec<LockedPackage>,
}

/// A single resolved and locked package entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub source: String,
    pub checksum: String,
}

impl LockedPackage {
    /// Parse the source string into a SourceType.
    pub fn source_type(&self) -> Result<SourceType, String> {
        SourceType::parse(&self.source)
    }

    /// Create a new locked package with a path source.
    pub fn with_path(name: &str, version: &str, path: &str, checksum: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            source: format!("path:{}", path),
            checksum: checksum.to_string(),
        }
    }

    /// Create a new locked package with a git source.
    pub fn with_git(
        name: &str,
        version: &str,
        url: &str,
        rev: Option<&str>,
        checksum: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            source: match rev {
                Some(r) => format!("git:{}#{}", url, r),
                None => format!("git:{}", url),
            },
            checksum: checksum.to_string(),
        }
    }

    /// Create a new locked package with a registry source.
    pub fn with_registry(
        name: &str,
        version: &str,
        registry: &str,
        full_name: &str,
        checksum: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            source: format!("{}:{}#{}", registry, full_name, version),
            checksum: checksum.to_string(),
        }
    }
}

impl Lockfile {
    /// Create a new empty lockfile.
    pub fn new() -> Self {
        Lockfile {
            packages: Vec::new(),
        }
    }

    /// Add a resolved package to the lockfile.
    pub fn add_package(&mut self, pkg: LockedPackage) {
        // Replace existing entry for same name, or append
        if let Some(existing) = self.packages.iter_mut().find(|p| p.name == pkg.name) {
            *existing = pkg;
        } else {
            self.packages.push(pkg);
        }
    }

    /// Sort packages by name for deterministic output.
    pub fn sort(&mut self) {
        self.packages.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Load a lockfile from the given directory.
    pub fn load(project_dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let lock_path = project_dir.join("gradient.lock");
        let contents = std::fs::read_to_string(&lock_path)?;
        let lockfile: Lockfile = toml::from_str(&contents)?;
        Ok(lockfile)
    }

    /// Parse a lockfile from a TOML string.
    pub fn parse(contents: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let lockfile: Lockfile = toml::from_str(contents)?;
        Ok(lockfile)
    }

    /// Write the lockfile to `gradient.lock` in the given directory.
    pub fn save(&self, project_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let lock_path = project_dir.join("gradient.lock");
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(&lock_path, contents)?;
        Ok(())
    }

    /// Serialize the lockfile to a TOML string.
    pub fn to_toml(&self) -> Result<String, Box<dyn std::error::Error>> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Look up a locked package by name.
    pub fn find_package(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// Look up a locked package by name (mutable).
    pub fn find_package_mut(&mut self, name: &str) -> Option<&mut LockedPackage> {
        self.packages.iter_mut().find(|p| p.name == name)
    }

    /// Get all registry packages in the lockfile.
    pub fn registry_packages(&self) -> Vec<&LockedPackage> {
        self.packages
            .iter()
            .filter(|p| p.source_type().map(|s| s.is_registry()).unwrap_or(false))
            .collect()
    }

    /// Validate that all checksums in the lockfile still match the actual
    /// source files on disk. Returns a list of package names with mismatched checksums.
    pub fn validate_checksums(
        &self,
        project_dir: &Path,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut mismatches = Vec::new();

        for pkg in &self.packages {
            if let Ok(source_type) = pkg.source_type() {
                if let Some(rel_path) = source_type.as_path() {
                    let dep_dir = project_dir.join(rel_path);
                    if dep_dir.is_dir() {
                        let actual = compute_directory_checksum(&dep_dir)?;
                        if actual != pkg.checksum {
                            mismatches.push(pkg.name.clone());
                        }
                    } else {
                        mismatches.push(pkg.name.clone());
                    }
                }
                // TODO: Add validation for git and registry sources
            }
        }

        Ok(mismatches)
    }
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute a SHA-256 checksum of all `.gr` source files in a directory.
///
/// Files are sorted by their relative path to ensure deterministic output.
/// The checksum covers both file paths and contents.
pub fn compute_directory_checksum(dir: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut hasher = Sha256::new();
    let mut files: Vec<_> = Vec::new();

    collect_gr_files(dir, dir, &mut files)?;
    files.sort();

    for (rel_path, contents) in &files {
        // Hash the relative path and file contents
        hasher.update(rel_path.as_bytes());
        hasher.update(b"\0");
        hasher.update(contents.as_bytes());
        hasher.update(b"\0");
    }

    let hash = hasher.finalize();
    Ok(format!("sha256:{}", hex::encode(hash)))
}

/// Recursively collect all `.gr` files in a directory, returning
/// (relative_path, contents) pairs.
fn collect_gr_files(
    base: &Path,
    dir: &Path,
    out: &mut Vec<(String, String)>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !dir.is_dir() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Skip hidden directories and target/
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with('.') && name_str != "target" {
                collect_gr_files(base, &path, out)?;
            }
        } else if path.extension().is_some_and(|ext| ext == "gr") {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let contents = std::fs::read_to_string(&path)?;
            out.push((rel, contents));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn generate_and_parse_lockfile() {
        let mut lockfile = Lockfile::new();
        lockfile.add_package(LockedPackage {
            name: "math-utils".to_string(),
            version: "0.1.0".to_string(),
            source: "path:../math-utils".to_string(),
            checksum: "sha256:abc123".to_string(),
        });
        lockfile.add_package(LockedPackage {
            name: "logging".to_string(),
            version: "0.2.0".to_string(),
            source: "path:../logging".to_string(),
            checksum: "sha256:def456".to_string(),
        });

        let toml_str = lockfile.to_toml().unwrap();
        let parsed = Lockfile::parse(&toml_str).unwrap();

        assert_eq!(parsed.packages.len(), 2);
        assert_eq!(parsed, lockfile);
    }

    #[test]
    fn parse_lockfile_format() {
        let toml = r#"
[[package]]
name = "math-utils"
version = "0.1.0"
source = "path:../math-utils"
checksum = "sha256:abc123"

[[package]]
name = "logging"
version = "0.2.0"
source = "path:../logging"
checksum = "sha256:def456"
"#;
        let lockfile = Lockfile::parse(toml).unwrap();
        assert_eq!(lockfile.packages.len(), 2);

        let math = lockfile.find_package("math-utils").unwrap();
        assert_eq!(math.version, "0.1.0");
        assert_eq!(math.source, "path:../math-utils");
        assert_eq!(math.checksum, "sha256:abc123");
    }

    #[test]
    fn parse_lockfile_with_github_source() {
        let toml = r#"
[[package]]
name = "math"
version = "1.2.0"
source = "github:gradient-lang/math#v1.2.0"
checksum = "sha256:abc123"
"#;
        let lockfile = Lockfile::parse(toml).unwrap();
        let math = lockfile.find_package("math").unwrap();
        assert_eq!(math.version, "1.2.0");
        assert_eq!(math.source, "github:gradient-lang/math#v1.2.0");

        // Test source type parsing
        let source_type = math.source_type().unwrap();
        assert!(source_type.is_registry());
        let (registry, name, version) = source_type.as_registry().unwrap();
        assert_eq!(registry, "github");
        assert_eq!(name, "gradient-lang/math");
        assert_eq!(version, "v1.2.0");
    }

    #[test]
    fn parse_lockfile_with_git_source() {
        let toml = r#"
[[package]]
name = "utils"
version = "0.5.0"
source = "git:https://github.com/example/utils.git#v0.5.0"
checksum = "sha256:def789"
"#;
        let lockfile = Lockfile::parse(toml).unwrap();
        let utils = lockfile.find_package("utils").unwrap();
        assert_eq!(utils.version, "0.5.0");
        assert_eq!(
            utils.source,
            "git:https://github.com/example/utils.git#v0.5.0"
        );

        // Test source type parsing
        let source_type = utils.source_type().unwrap();
        assert!(source_type.is_git());
    }

    #[test]
    fn empty_lockfile() {
        let lockfile = Lockfile::new();
        let toml_str = lockfile.to_toml().unwrap();
        let parsed = Lockfile::parse(&toml_str).unwrap();
        assert!(parsed.packages.is_empty());
    }

    #[test]
    fn add_package_replaces_existing() {
        let mut lockfile = Lockfile::new();
        lockfile.add_package(LockedPackage {
            name: "dep".to_string(),
            version: "0.1.0".to_string(),
            source: "path:../dep".to_string(),
            checksum: "sha256:old".to_string(),
        });
        lockfile.add_package(LockedPackage {
            name: "dep".to_string(),
            version: "0.2.0".to_string(),
            source: "path:../dep".to_string(),
            checksum: "sha256:new".to_string(),
        });

        assert_eq!(lockfile.packages.len(), 1);
        assert_eq!(lockfile.packages[0].version, "0.2.0");
        assert_eq!(lockfile.packages[0].checksum, "sha256:new");
    }

    #[test]
    fn source_type_display_roundtrip() {
        // Test path source
        let path = SourceType::Path("../relative/path".to_string());
        assert_eq!(path.to_string(), "path:../relative/path");
        let parsed = SourceType::parse(&path.to_string()).unwrap();
        assert_eq!(path, parsed);

        // Test git source without rev
        let git = SourceType::Git {
            url: "https://github.com/user/repo.git".to_string(),
            rev: None,
        };
        assert_eq!(git.to_string(), "git:https://github.com/user/repo.git");
        let parsed = SourceType::parse(&git.to_string()).unwrap();
        assert_eq!(git, parsed);

        // Test git source with rev
        let git_rev = SourceType::Git {
            url: "https://github.com/user/repo.git".to_string(),
            rev: Some("abc123".to_string()),
        };
        assert_eq!(
            git_rev.to_string(),
            "git:https://github.com/user/repo.git#abc123"
        );
        let parsed = SourceType::parse(&git_rev.to_string()).unwrap();
        assert_eq!(git_rev, parsed);

        // Test registry source
        let reg = SourceType::Registry {
            registry: "github".to_string(),
            name: "user/package".to_string(),
            version: "v1.2.0".to_string(),
        };
        assert_eq!(reg.to_string(), "github:user/package#v1.2.0");
        let parsed = SourceType::parse(&reg.to_string()).unwrap();
        assert_eq!(reg, parsed);
    }

    #[test]
    fn locked_package_constructors() {
        let path_pkg = LockedPackage::with_path("my-dep", "1.0.0", "../my-dep", "sha256:abc");
        assert_eq!(path_pkg.name, "my-dep");
        assert_eq!(path_pkg.version, "1.0.0");
        assert_eq!(path_pkg.source, "path:../my-dep");
        assert!(path_pkg.source_type().unwrap().is_path());

        let git_pkg = LockedPackage::with_git(
            "git-dep",
            "2.0.0",
            "https://github.com/user/repo.git",
            Some("abc123"),
            "sha256:def",
        );
        assert_eq!(
            git_pkg.source,
            "git:https://github.com/user/repo.git#abc123"
        );
        assert!(git_pkg.source_type().unwrap().is_git());

        let reg_pkg = LockedPackage::with_registry(
            "reg-dep",
            "3.0.0",
            "github",
            "user/package",
            "sha256:ghi",
        );
        assert_eq!(reg_pkg.source, "github:user/package#3.0.0");
        assert!(reg_pkg.source_type().unwrap().is_registry());
    }

    #[test]
    fn checksum_deterministic() {
        let tmp = std::env::temp_dir().join("gradient_test_checksum");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();
        fs::write(
            tmp.join("src/main.gr"),
            "mod main\n\nfn main() -> !{IO} ():\n    print(\"hello\")\n",
        )
        .unwrap();

        let hash1 = compute_directory_checksum(&tmp).unwrap();
        let hash2 = compute_directory_checksum(&tmp).unwrap();
        assert_eq!(hash1, hash2);
        assert!(hash1.starts_with("sha256:"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn lockfile_checksum_validation() {
        let tmp = std::env::temp_dir().join("gradient_test_lockfile_validate");
        let _ = fs::remove_dir_all(&tmp);

        // Set up a project dir with a dependency dir
        let dep_dir = tmp.join("dep-lib");
        fs::create_dir_all(dep_dir.join("src")).unwrap();
        fs::write(
            dep_dir.join("src/lib.gr"),
            "mod lib\n\nfn add(a: Int, b: Int) -> Int:\n    a + b\n",
        )
        .unwrap();
        fs::write(
            dep_dir.join("gradient.toml"),
            "[package]\nname = \"dep-lib\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let checksum = compute_directory_checksum(&dep_dir).unwrap();

        let mut lockfile = Lockfile::new();
        lockfile.add_package(LockedPackage {
            name: "dep-lib".to_string(),
            version: "0.1.0".to_string(),
            source: "path:dep-lib".to_string(),
            checksum: checksum.clone(),
        });

        // Validate: should pass
        let mismatches = lockfile.validate_checksums(&tmp).unwrap();
        assert!(
            mismatches.is_empty(),
            "Checksums should match immediately after generation"
        );

        // Modify the source file
        fs::write(
            dep_dir.join("src/lib.gr"),
            "mod lib\n\nfn add(a: Int, b: Int) -> Int:\n    a + b + 1\n",
        )
        .unwrap();

        // Validate again: should detect mismatch
        let mismatches = lockfile.validate_checksums(&tmp).unwrap();
        assert_eq!(mismatches, vec!["dep-lib"]);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn registry_packages_filtering() {
        let mut lockfile = Lockfile::new();
        lockfile.add_package(LockedPackage::with_path(
            "path-dep",
            "1.0.0",
            "../path-dep",
            "sha256:abc",
        ));
        lockfile.add_package(LockedPackage::with_registry(
            "reg-dep",
            "2.0.0",
            "github",
            "user/reg-dep",
            "sha256:def",
        ));
        lockfile.add_package(LockedPackage::with_git(
            "git-dep",
            "3.0.0",
            "https://github.com/user/git-dep.git",
            None,
            "sha256:ghi",
        ));
        lockfile.add_package(LockedPackage::with_registry(
            "another-reg",
            "1.5.0",
            "github",
            "user/another",
            "sha256:jkl",
        ));

        let registry_packages = lockfile.registry_packages();
        assert_eq!(registry_packages.len(), 2);
        assert!(registry_packages.iter().any(|p| p.name == "reg-dep"));
        assert!(registry_packages.iter().any(|p| p.name == "another-reg"));
    }
}
