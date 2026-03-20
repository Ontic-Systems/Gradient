// manifest.rs — Parsing and generation of `gradient.toml` project manifests

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// The top-level manifest read from `gradient.toml`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub package: Package,
    #[serde(default)]
    pub dependencies: HashMap<String, Dependency>,
}

/// The `[package]` section of a Gradient manifest.
#[derive(Debug, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub edition: Option<String>,
}

/// A single dependency entry in the `[dependencies]` table.
#[derive(Debug, Serialize, Deserialize)]
pub struct Dependency {
    pub version: String,
    pub path: Option<String>,
    pub git: Option<String>,
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
        dependencies: HashMap::new(),
    };
    toml::to_string_pretty(&manifest).expect("failed to serialize default manifest")
}
