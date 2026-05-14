// gradient install — verify and install signed file-registry packages.
//
// Launch-tier #368 implementation. File registries are intentionally
// filesystem-only: tests stay hermetic and the later #369 backend can swap in
// network transport without changing the trust checks.

use crate::lockfile::{LockedPackage, Lockfile};
use crate::manifest;
use crate::name_validation::safe_cache_path;
use crate::project::Project;
use crate::zip_safe::{safe_extract, ExtractLimits, ExtractOptions};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process;

const PACKAGE_EXT: &str = "gradient-pkg";
const PUBLISH_METADATA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct InstallOptions<'a> {
    pub package: &'a str,
    pub version: &'a str,
    pub registry: &'a str,
    pub cache_dir: Option<&'a str>,
    pub yes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallResult {
    pub package_name: String,
    pub package_version: String,
    pub artifact_sha256: String,
    pub signature_id: String,
    pub manifest_summary: String,
    pub cache_dir: PathBuf,
    pub lockfile_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct PublishMetadata {
    schema_version: u32,
    package: String,
    version: String,
    artifact_sha256: String,
    sigstore_bundle: Option<String>,
}

pub fn execute(options: InstallOptions<'_>) {
    match install(options) {
        Ok(result) => {
            println!(
                "Installed {}@{}",
                result.package_name, result.package_version
            );
            println!("  sha256: {}", result.artifact_sha256);
            println!("  signature: {}", result.signature_id);
            println!("  cache: {}", result.cache_dir.display());
            println!("  lockfile: {}", result.lockfile_path.display());
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub fn install(options: InstallOptions<'_>) -> Result<InstallResult, String> {
    let project = Project::find()?;
    install_project(&project, options)
}

pub fn install_project(
    project: &Project,
    options: InstallOptions<'_>,
) -> Result<InstallResult, String> {
    safe_cache_path(Path::new("/"), options.package, options.version)
        .map(|_| ())
        .map_err(|e| format!("Invalid package name or version: {e}"))?;

    let upload_dir = registry_package_dir(options.registry, options.package, options.version)?;
    let registry_manifest_path = upload_dir.join("gradient-package.toml");
    let registry_manifest = fs::read_to_string(&registry_manifest_path).map_err(|e| {
        format!(
            "Failed to read registry manifest `{}`: {e}",
            registry_manifest_path.display()
        )
    })?;
    let manifest_summary = summarize_registry_manifest(&registry_manifest)?;

    let first_install = !project.root.join("gradient.lock").is_file()
        || Lockfile::load(&project.root)
            .map(|lock| lock.find_package(options.package).is_none())
            .unwrap_or(true);
    if first_install && !options.yes {
        println!(
            "Registry manifest audit for {}@{}:",
            options.package, options.version
        );
        println!("{manifest_summary}");
        return Err("Review manifest above and rerun with --yes to install".to_string());
    }

    validate_registry_manifest(&registry_manifest, options.package, options.version)?;

    let artifact_path = upload_dir.join(format!(
        "{}-{}.{}",
        options.package, options.version, PACKAGE_EXT
    ));
    let metadata_path = upload_dir.join(format!(
        "{}-{}.publish.json",
        options.package, options.version
    ));
    let metadata = read_publish_metadata(&metadata_path)?;
    validate_publish_metadata(&metadata, options.package, options.version)?;

    let actual_sha = sha256_file(&artifact_path)?;
    if actual_sha != metadata.artifact_sha256 {
        return Err(format!(
            "artifact SHA-256 mismatch for {}@{}: metadata says {}, artifact hashes to {}",
            options.package, options.version, metadata.artifact_sha256, actual_sha
        ));
    }

    let bundle_name = metadata
        .sigstore_bundle
        .as_deref()
        .ok_or_else(|| "Publish metadata does not name a sigstore bundle".to_string())?;
    let bundle_rel = Path::new(bundle_name);
    if bundle_rel.components().count() != 1 || bundle_rel.file_name().is_none() {
        return Err("Publish metadata sigstore bundle must be a filename".to_string());
    }
    let bundle_path = upload_dir.join(bundle_name);
    let signature_id = read_signature_id(&bundle_path)?;

    let artifact_bytes = fs::read(&artifact_path)
        .map_err(|e| format!("Failed to read artifact `{}`: {e}", artifact_path.display()))?;
    let cache_root = cache_root(options.cache_dir)?;
    let cache_dir = safe_cache_path(&cache_root, options.package, options.version)
        .map_err(|e| format!("Invalid package name or version: {e}"))?;
    safe_extract(
        &artifact_bytes,
        &cache_dir,
        ExtractLimits::default(),
        ExtractOptions {
            strip_top_level: false,
        },
    )
    .map_err(|e| format!("Failed to extract package artifact: {e}"))?;

    let extracted_manifest = fs::read_to_string(cache_dir.join("gradient-package.toml"))
        .map_err(|e| format!("Extracted package is missing gradient-package.toml: {e}"))?;
    if extracted_manifest != registry_manifest {
        return Err("Extracted gradient-package.toml does not match registry manifest".to_string());
    }
    let extracted_gradient_toml = cache_dir.join("gradient.toml");
    if !extracted_gradient_toml.is_file() {
        return Err("Extracted package is missing gradient.toml".to_string());
    }
    let source_dir = cache_dir.join("src");
    if !source_dir.is_dir() {
        return Err("Extracted package is missing src/".to_string());
    }

    let mut lockfile = Lockfile::load(&project.root).unwrap_or_else(|_| Lockfile::new());
    lockfile.add_package(LockedPackage::with_registry_archive(
        options.package,
        options.version,
        "file",
        options.package,
        &actual_sha,
        &actual_sha,
    ));
    lockfile.sort();
    lockfile
        .save(&project.root)
        .map_err(|e| format!("Failed to write gradient.lock: {e}"))?;

    Ok(InstallResult {
        package_name: options.package.to_string(),
        package_version: options.version.to_string(),
        artifact_sha256: actual_sha,
        signature_id,
        manifest_summary,
        cache_dir,
        lockfile_path: project.root.join("gradient.lock"),
    })
}

fn registry_package_dir(registry: &str, package: &str, version: &str) -> Result<PathBuf, String> {
    let root = registry.strip_prefix("file://").ok_or_else(|| {
        "Only file:// registries are supported in the launch-tier installer".to_string()
    })?;
    Ok(PathBuf::from(root).join(package).join(version))
}

fn cache_root(cache_dir: Option<&str>) -> Result<PathBuf, String> {
    if let Some(cache_dir) = cache_dir {
        return Ok(PathBuf::from(cache_dir));
    }
    let home_dir = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| "Could not determine cache directory".to_string())?;
    Ok(PathBuf::from(home_dir).join(".gradient").join("cache"))
}

fn read_publish_metadata(path: &Path) -> Result<PublishMetadata, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read publish metadata `{}`: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("Invalid publish metadata `{}`: {e}", path.display()))
}

fn validate_publish_metadata(
    metadata: &PublishMetadata,
    package: &str,
    version: &str,
) -> Result<(), String> {
    if metadata.schema_version != PUBLISH_METADATA_VERSION {
        return Err(format!(
            "Unsupported publish metadata schema version {}",
            metadata.schema_version
        ));
    }
    if metadata.package != package || metadata.version != version {
        return Err(format!(
            "Publish metadata identifies {}@{}, expected {}@{}",
            metadata.package, metadata.version, package, version
        ));
    }
    Ok(())
}

fn validate_registry_manifest(text: &str, package: &str, version: &str) -> Result<(), String> {
    let manifest = manifest::parse(text)
        .map_err(|e| format!("Invalid registry manifest gradient-package.toml: {e}"))?;
    if manifest.package.name != package || manifest.package.version != version {
        return Err(format!(
            "Registry manifest identifies {}@{}, expected {}@{}",
            manifest.package.name, manifest.package.version, package, version
        ));
    }
    Ok(())
}

fn summarize_registry_manifest(text: &str) -> Result<String, String> {
    let value: toml::Value = toml::from_str(text)
        .map_err(|e| format!("Invalid registry manifest gradient-package.toml: {e}"))?;
    let package = value
        .get("package")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "Registry manifest missing [package]".to_string())?;
    let name = package
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Registry manifest missing package.name".to_string())?;
    let version = package
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Registry manifest missing package.version".to_string())?;
    let trust = value.get("trust").and_then(|v| v.as_table());
    let effects = trust
        .and_then(|t| t.get("public_effects"))
        .and_then(|v| v.as_array())
        .map(|values| format_toml_string_array(values))
        .unwrap_or_else(|| "<absent>".to_string());
    let capabilities = trust
        .and_then(|t| t.get("capabilities"))
        .and_then(|v| v.as_array())
        .map(|values| format_toml_string_array(values))
        .unwrap_or_else(|| "<absent>".to_string());
    let trust_label = trust
        .and_then(|t| t.get("trust_label"))
        .and_then(|v| v.as_str())
        .unwrap_or("<absent>");
    Ok(format!(
        "  package: {name}@{version}\n  effects: {effects}\n  capabilities: {capabilities}\n  trust: {trust_label}"
    ))
}

fn format_toml_string_array(values: &[toml::Value]) -> String {
    let strings: Vec<_> = values.iter().filter_map(|v| v.as_str()).collect();
    if strings.is_empty() {
        "<empty>".to_string()
    } else {
        strings.join(", ")
    }
}

fn read_signature_id(bundle_path: &Path) -> Result<String, String> {
    let text = fs::read_to_string(bundle_path).map_err(|e| {
        format!(
            "Failed to read sigstore bundle `{}`: {e}",
            bundle_path.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("Invalid sigstore bundle `{}`: {e}", bundle_path.display()))?;
    let entries = value
        .get("tlogEntries")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Sigstore bundle missing transparency log entries".to_string())?;
    let first = entries
        .first()
        .ok_or_else(|| "Sigstore bundle has no transparency log entries".to_string())?;
    if let Some(uuid) = first.get("uuid").and_then(|v| v.as_str()) {
        return Ok(uuid.to_string());
    }
    if let Some(log_index) = first.get("logIndex").and_then(|v| v.as_i64()) {
        return Ok(format!("tlog:{log_index}"));
    }
    Err("Sigstore transparency log entry has no uuid or logIndex".to_string())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|e| format!("Failed to open `{}` for hashing: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Failed to read `{}` for hashing: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, Package};
    use std::collections::BTreeMap;
    use std::io::Write;
    use zip::write::FileOptions;

    fn project_at(root: PathBuf) -> Project {
        Project {
            root,
            manifest: Manifest {
                package: Package {
                    name: "consumer".to_string(),
                    version: "0.1.0".to_string(),
                    edition: Some("2026".to_string()),
                    effects: None,
                    capabilities: None,
                },
                dependencies: BTreeMap::new(),
            },
            name: "consumer".to_string(),
        }
    }

    fn test_sha256_file(path: &Path) -> String {
        sha256_file(path).unwrap()
    }

    fn write_registry_package(registry: &Path) -> PathBuf {
        let upload_dir = registry.join("demo-pkg").join("1.2.3");
        fs::create_dir_all(&upload_dir).unwrap();
        let artifact = upload_dir.join("demo-pkg-1.2.3.gradient-pkg");
        let file = fs::File::create(&artifact).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("gradient.toml", options).unwrap();
        zip.write_all(b"[package]\nname = \"demo-pkg\"\nversion = \"1.2.3\"\neffects = [\"Heap\"]\ncapabilities = [\"FS\"]\n\n[dependencies]\n").unwrap();
        zip.start_file("gradient-package.toml", options).unwrap();
        zip.write_all(b"[package]\nname = \"demo-pkg\"\nversion = \"1.2.3\"\n\n[trust]\ncapabilities = [\"FS\"]\npublic_effects = [\"Heap\"]\ncontract_tier = \"runtime\"\ntrust_label = \"untrusted\"\nunsafe_extern_count = 0\nallowed_origins = []\n\n[provenance]\nsource_commit = \"abc123\"\n\n[dependencies]\n").unwrap();
        zip.start_file("src/main.gr", options).unwrap();
        zip.write_all(b"fn main() -> Int:\n    ret 0\n").unwrap();
        zip.finish().unwrap();

        let artifact_sha256 = test_sha256_file(&artifact);
        fs::write(
            upload_dir.join("gradient-package.toml"),
            "[package]\nname = \"demo-pkg\"\nversion = \"1.2.3\"\n\n[trust]\ncapabilities = [\"FS\"]\npublic_effects = [\"Heap\"]\ncontract_tier = \"runtime\"\ntrust_label = \"untrusted\"\nunsafe_extern_count = 0\nallowed_origins = []\n\n[provenance]\nsource_commit = \"abc123\"\n\n[dependencies]\n",
        )
        .unwrap();
        fs::write(
            upload_dir.join("demo-pkg-1.2.3.publish.json"),
            format!(
                "{{\n  \"schema_version\": 1,\n  \"package\": \"demo-pkg\",\n  \"version\": \"1.2.3\",\n  \"artifact_sha256\": \"{}\",\n  \"sigstore_bundle\": \"demo-pkg-1.2.3.sigstore.json\"\n}}\n",
                artifact_sha256
            ),
        )
        .unwrap();
        fs::write(
            upload_dir.join("demo-pkg-1.2.3.sigstore.json"),
            r##"{"tlogEntries":[{"logIndex":42,"uuid":"sig-abc"}]}"##,
        )
        .unwrap();
        upload_dir
    }

    #[test]
    fn install_verifies_file_registry_package_and_updates_lockfile() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join("consumer");
        fs::create_dir_all(&project_root).unwrap();
        fs::write(
            project_root.join("gradient.toml"),
            "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let registry = tmp.path().join("registry");
        write_registry_package(&registry);
        let cache = tmp.path().join("cache");
        let project = project_at(project_root.clone());
        let registry_arg = format!("file://{}", registry.display());
        let cache_arg = cache.to_string_lossy().to_string();

        let result = install_project(
            &project,
            InstallOptions {
                package: "demo-pkg",
                version: "1.2.3",
                registry: &registry_arg,
                cache_dir: Some(&cache_arg),
                yes: true,
            },
        )
        .unwrap();

        assert_eq!(result.package_name, "demo-pkg");
        assert_eq!(result.signature_id, "sig-abc");
        assert!(result.manifest_summary.contains("effects: Heap"));
        assert!(cache.join("demo-pkg/1.2.3/gradient.toml").is_file());
        let lock = fs::read_to_string(project_root.join("gradient.lock")).unwrap();
        assert!(lock.contains("name = \"demo-pkg\""));
        assert!(lock.contains("checksum = \"sha256:"));
        assert!(lock.contains("archive_sha256 = \"sha256:"));
    }

    #[test]
    fn install_rejects_tampered_artifact_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join("consumer");
        fs::create_dir_all(&project_root).unwrap();
        let registry = tmp.path().join("registry");
        let upload_dir = write_registry_package(&registry);
        fs::write(upload_dir.join("demo-pkg-1.2.3.gradient-pkg"), b"tampered").unwrap();
        let project = project_at(project_root);
        let registry_arg = format!("file://{}", registry.display());

        let err = install_project(
            &project,
            InstallOptions {
                package: "demo-pkg",
                version: "1.2.3",
                registry: &registry_arg,
                cache_dir: None,
                yes: true,
            },
        )
        .unwrap_err();

        assert!(err.contains("artifact SHA-256 mismatch"), "{err}");
    }

    #[test]
    fn install_rejects_bundle_without_transparency_log_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join("consumer");
        fs::create_dir_all(&project_root).unwrap();
        let registry = tmp.path().join("registry");
        let upload_dir = write_registry_package(&registry);
        fs::write(
            upload_dir.join("demo-pkg-1.2.3.sigstore.json"),
            r##"{"tlogEntries":[]}"##,
        )
        .unwrap();
        let project = project_at(project_root);
        let registry_arg = format!("file://{}", registry.display());

        let err = install_project(
            &project,
            InstallOptions {
                package: "demo-pkg",
                version: "1.2.3",
                registry: &registry_arg,
                cache_dir: None,
                yes: true,
            },
        )
        .unwrap_err();

        assert!(err.contains("transparency log"), "{err}");
    }
}
