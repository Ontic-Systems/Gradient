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
// ```

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

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

    /// Validate that all checksums in the lockfile still match the actual
    /// source files on disk. Returns a list of package names with mismatched checksums.
    pub fn validate_checksums(
        &self,
        project_dir: &Path,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut mismatches = Vec::new();

        for pkg in &self.packages {
            if let Some(rel_path) = pkg.source.strip_prefix("path:") {
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
        assert!(mismatches.is_empty());

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
}
