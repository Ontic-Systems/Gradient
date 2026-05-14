// gradient publish — package, sigstore-sign, and upload a Gradient package.
//
// Launch-tier registry publication flow for E10 #367. The trust chain is
// external sigstore tooling (`cosign sign-blob --bundle ...`) plus a registry
// upload target. Tests use a fake cosign binary and a `file://` registry so the
// suite stays network-free.

use crate::project::Project;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use zip::write::FileOptions;

const PACKAGE_EXT: &str = "gradient-pkg";
const PUBLISH_METADATA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct PublishOptions<'a> {
    pub registry: Option<&'a str>,
    pub out_dir: Option<&'a str>,
    pub dry_run: bool,
    pub allow_dirty: bool,
    pub cosign_bin: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishResult {
    pub package_name: String,
    pub package_version: String,
    pub artifact_path: PathBuf,
    pub bundle_path: Option<PathBuf>,
    pub artifact_sha256: String,
    pub upload_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
struct PublishMetadata<'a> {
    schema_version: u32,
    package: &'a str,
    version: &'a str,
    artifact_sha256: &'a str,
    source_commit: Option<&'a str>,
    sigstore_bundle: Option<String>,
    registry: Option<&'a str>,
    dry_run: bool,
}

/// Execute the `gradient publish` subcommand.
pub fn execute(options: PublishOptions<'_>) {
    match publish(options) {
        Ok(result) => {
            println!(
                "Published {}@{}",
                result.package_name, result.package_version
            );
            println!("  artifact: {}", result.artifact_path.display());
            println!("  sha256: {}", result.artifact_sha256);
            if let Some(bundle) = result.bundle_path {
                println!("  sigstore bundle: {}", bundle.display());
            }
            if let Some(upload_dir) = result.upload_dir {
                println!("  uploaded: {}", upload_dir.display());
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub fn publish(options: PublishOptions<'_>) -> Result<PublishResult, String> {
    let project = Project::find()?;
    publish_project(&project, options)
}

pub fn publish_project(
    project: &Project,
    options: PublishOptions<'_>,
) -> Result<PublishResult, String> {
    let package_name = project.manifest.package.name.clone();
    let package_version = project.manifest.package.version.clone();

    if !options.allow_dirty {
        ensure_git_clean(&project.root)?;
    }

    let source_commit = git_commit(&project.root).ok();
    let out_dir = options
        .out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| project.root.join("target").join("package"));
    fs::create_dir_all(&out_dir)
        .map_err(|e| format!("Failed to create `{}`: {e}", out_dir.display()))?;

    let source_commit = source_commit.as_deref();
    let registry_manifest_path = out_dir.join(format!(
        "{}-{}.gradient-package.toml",
        package_name, package_version
    ));
    let registry_manifest = registry_manifest(project, source_commit)?;
    fs::write(&registry_manifest_path, registry_manifest).map_err(|e| {
        format!(
            "Failed to write `{}`: {e}",
            registry_manifest_path.display()
        )
    })?;

    let artifact_path = out_dir.join(format!(
        "{}-{}.{}",
        package_name, package_version, PACKAGE_EXT
    ));
    create_package_archive(project, &artifact_path, &registry_manifest_path)?;
    let artifact_sha256 = sha256_file(&artifact_path)?;

    let bundle_path = if options.dry_run {
        None
    } else {
        let cosign = options.cosign_bin.unwrap_or("cosign");
        let bundle_path = out_dir.join(format!(
            "{}-{}.sigstore.json",
            package_name, package_version
        ));
        sign_with_cosign(cosign, &artifact_path, &bundle_path)?;
        Some(bundle_path)
    };

    let metadata = PublishMetadata {
        schema_version: PUBLISH_METADATA_VERSION,
        package: &package_name,
        version: &package_version,
        artifact_sha256: &artifact_sha256,
        source_commit,
        sigstore_bundle: bundle_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string()),
        registry: options.registry,
        dry_run: options.dry_run,
    };
    let metadata_path = out_dir.join(format!("{}-{}.publish.json", package_name, package_version));
    write_json(&metadata_path, &metadata)?;

    let upload_dir = match options.registry {
        Some(registry) => Some(upload_to_registry(
            registry,
            &package_name,
            &package_version,
            &artifact_path,
            bundle_path.as_deref(),
            &metadata_path,
            &registry_manifest_path,
        )?),
        None if options.dry_run => None,
        None => {
            return Err(
                "gradient publish requires --registry <file://...> unless --dry-run is set"
                    .to_string(),
            )
        }
    };

    Ok(PublishResult {
        package_name,
        package_version,
        artifact_path,
        bundle_path,
        artifact_sha256,
        upload_dir,
    })
}

fn ensure_git_clean(root: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(root)
        .output()
        .map_err(|e| format!("Failed to run `git status --porcelain`: {e}"))?;
    if !output.status.success() {
        return Err("Failed to inspect git status before publish".to_string());
    }
    if !output.stdout.is_empty() {
        return Err(
            "Refusing to publish from a dirty working tree. Commit changes or pass --allow-dirty."
                .to_string(),
        );
    }
    Ok(())
}

fn git_commit(root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(root)
        .output()
        .map_err(|e| format!("Failed to run `git rev-parse HEAD`: {e}"))?;
    if !output.status.success() {
        return Err("git rev-parse HEAD failed".to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn create_package_archive(
    project: &Project,
    artifact_path: &Path,
    registry_manifest_path: &Path,
) -> Result<(), String> {
    let file = fs::File::create(artifact_path)
        .map_err(|e| format!("Failed to create `{}`: {e}", artifact_path.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    add_file_to_zip(
        &mut zip,
        &project.root.join("gradient.toml"),
        Path::new("gradient.toml"),
        options,
    )?;
    add_file_to_zip(
        &mut zip,
        registry_manifest_path,
        Path::new("gradient-package.toml"),
        options,
    )?;
    let lock = project.root.join("gradient.lock");
    if lock.is_file() {
        add_file_to_zip(&mut zip, &lock, Path::new("gradient.lock"), options)?;
    }
    let src = project.root.join("src");
    if src.is_dir() {
        add_dir_to_zip(&mut zip, &src, Path::new("src"), options)?;
    } else {
        return Err("Cannot publish: project has no `src/` directory".to_string());
    }

    zip.finish()
        .map_err(|e| format!("Failed to finalize package archive: {e}"))?;
    Ok(())
}

fn registry_manifest(project: &Project, source_commit: Option<&str>) -> Result<String, String> {
    let manifest = &project.manifest;
    let mut doc = String::new();
    doc.push_str("[package]\n");
    doc.push_str(&format!("name = {:?}\n", manifest.package.name));
    doc.push_str(&format!("version = {:?}\n", manifest.package.version));
    if let Some(edition) = &manifest.package.edition {
        doc.push_str(&format!("edition = {:?}\n", edition));
    }

    doc.push_str("\n[trust]\n");
    write_array(
        &mut doc,
        "capabilities",
        manifest.package.capabilities.as_deref(),
    );
    write_array(
        &mut doc,
        "public_effects",
        manifest.package.effects.as_deref(),
    );
    doc.push_str("contract_tier = \"runtime\"\n");
    doc.push_str("trust_label = \"untrusted\"\n");
    doc.push_str("unsafe_extern_count = 0\n");
    doc.push_str("allowed_origins = []\n");

    doc.push_str("\n[provenance]\n");
    if let Some(source_commit) = source_commit {
        doc.push_str(&format!("source_commit = {:?}\n", source_commit));
    }

    doc.push_str("\n[dependencies]\n");
    for (name, dep) in &manifest.dependencies {
        let dep_toml = toml::to_string(dep)
            .map_err(|e| format!("Failed to serialize dependency `{name}`: {e}"))?;
        doc.push_str(&format!("{:?} = {}\n", name, dep_toml.trim()));
    }

    toml::from_str::<toml::Value>(&doc)
        .map_err(|e| format!("Generated invalid registry manifest: {e}"))?;
    Ok(doc)
}

fn write_array(doc: &mut String, key: &str, values: Option<&[String]>) {
    let values = values.unwrap_or(&[]);
    doc.push_str(key);
    doc.push_str(" = [");
    for (idx, value) in values.iter().enumerate() {
        if idx > 0 {
            doc.push_str(", ");
        }
        doc.push_str(&format!("{:?}", value));
    }
    doc.push_str("]\n");
}

fn add_dir_to_zip<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    dir: &Path,
    archive_dir: &Path,
    options: FileOptions,
) -> Result<(), String> {
    let mut entries = fs::read_dir(dir)
        .map_err(|e| format!("Failed to read `{}`: {e}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read `{}`: {e}", dir.display()))?;
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        let archive_path = archive_dir.join(entry.file_name());
        if path.is_dir() {
            add_dir_to_zip(zip, &path, &archive_path, options)?;
        } else if path.is_file() {
            add_file_to_zip(zip, &path, &archive_path, options)?;
        }
    }
    Ok(())
}

fn add_file_to_zip<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    path: &Path,
    archive_path: &Path,
    options: FileOptions,
) -> Result<(), String> {
    let archive_name = archive_path.to_string_lossy().replace('\\', "/");
    zip.start_file(archive_name, options)
        .map_err(|e| format!("Failed to add `{}` to archive: {e}", path.display()))?;
    let mut file =
        fs::File::open(path).map_err(|e| format!("Failed to open `{}`: {e}", path.display()))?;
    std::io::copy(&mut file, zip)
        .map_err(|e| format!("Failed to copy `{}` into archive: {e}", path.display()))?;
    Ok(())
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

fn sign_with_cosign(cosign: &str, artifact_path: &Path, bundle_path: &Path) -> Result<(), String> {
    let status = Command::new(cosign)
        .arg("sign-blob")
        .arg("--yes")
        .arg("--bundle")
        .arg(bundle_path)
        .arg(artifact_path)
        .status()
        .map_err(|e| format!("Failed to invoke sigstore signer `{cosign}`: {e}"))?;
    if !status.success() {
        return Err(format!(
            "Sigstore signing failed with status {}",
            status.code().unwrap_or(-1)
        ));
    }
    if !bundle_path.is_file() {
        return Err(format!(
            "Sigstore signer did not create bundle `{}`",
            bundle_path.display()
        ));
    }
    Ok(())
}

fn upload_to_registry(
    registry: &str,
    package: &str,
    version: &str,
    artifact_path: &Path,
    bundle_path: Option<&Path>,
    metadata_path: &Path,
    registry_manifest_path: &Path,
) -> Result<PathBuf, String> {
    if registry.starts_with("http://") || registry.starts_with("https://") {
        return upload_to_http_registry(
            registry,
            package,
            version,
            artifact_path,
            bundle_path,
            metadata_path,
            registry_manifest_path,
        );
    }

    let root = registry.strip_prefix("file://").ok_or_else(|| {
        "Registry must be file://, http://, or https:// for the launch-tier publisher".to_string()
    })?;
    let upload_dir = PathBuf::from(root).join(package).join(version);
    fs::create_dir_all(&upload_dir).map_err(|e| {
        format!(
            "Failed to create registry dir `{}`: {e}",
            upload_dir.display()
        )
    })?;

    copy_into(artifact_path, &upload_dir)?;
    copy_into(metadata_path, &upload_dir)?;
    copy_named(
        registry_manifest_path,
        &upload_dir.join("gradient-package.toml"),
    )?;
    if let Some(bundle) = bundle_path {
        copy_into(bundle, &upload_dir)?;
    }
    Ok(upload_dir)
}

fn upload_to_http_registry(
    registry: &str,
    package: &str,
    version: &str,
    artifact_path: &Path,
    bundle_path: Option<&Path>,
    metadata_path: &Path,
    registry_manifest_path: &Path,
) -> Result<PathBuf, String> {
    let bundle_path = bundle_path.ok_or_else(|| {
        "HTTP registry upload requires a sigstore bundle; omit --dry-run and sign first".to_string()
    })?;
    let identity = read_sigstore_identity(bundle_path)?;
    let files = [
        artifact_path.to_path_buf(),
        metadata_path.to_path_buf(),
        registry_manifest_path.to_path_buf(),
        bundle_path.to_path_buf(),
    ];
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| format!("Failed to create registry upload runtime: {e}"))?;
    rt.block_on(async_upload_to_http_registry(
        registry, package, version, &identity, &files,
    ))?;
    Ok(PathBuf::from(format!(
        "{}/v1/packages/{}/{}",
        registry.trim_end_matches('/'),
        package,
        version
    )))
}

async fn async_upload_to_http_registry(
    registry: &str,
    package: &str,
    version: &str,
    identity: &str,
    files: &[PathBuf],
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent("gradient-build-system/0.1.0")
        .build()
        .map_err(|e| format!("Failed to create registry HTTP client: {e}"))?;
    for path in files {
        let file_name = path
            .file_name()
            .ok_or_else(|| format!("Path `{}` has no filename", path.display()))?
            .to_string_lossy()
            .to_string();
        let upload_name = if file_name.ends_with(".gradient-package.toml") {
            "gradient-package.toml".to_string()
        } else {
            file_name
        };
        let url = format!(
            "{}/v1/packages/{}/{}/{}",
            registry.trim_end_matches('/'),
            package,
            version,
            upload_name
        );
        let bytes = fs::read(path)
            .map_err(|e| format!("Failed to read `{}` for HTTP upload: {e}", path.display()))?;
        let response = client
            .put(&url)
            .header("X-Gradient-Sigstore-Identity", identity)
            .body(bytes)
            .send()
            .await
            .map_err(|e| format!("HTTP registry upload failed for `{url}`: {e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "HTTP registry upload failed for `{url}`: HTTP {status}: {body}"
            ));
        }
    }
    Ok(())
}

fn read_sigstore_identity(bundle_path: &Path) -> Result<String, String> {
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
    if let Some(log_id) = first.get("logID").and_then(|v| v.as_str()) {
        return Ok(log_id.to_string());
    }
    Err("Sigstore transparency log entry has no uuid, logIndex, or logID".to_string())
}

fn copy_into(path: &Path, dir: &Path) -> Result<(), String> {
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("Path `{}` has no filename", path.display()))?;
    copy_named(path, &dir.join(file_name))
}

fn copy_named(src: &Path, dst: &Path) -> Result<(), String> {
    fs::copy(src, dst).map(|_| ()).map_err(|e| {
        format!(
            "Failed to copy `{}` to `{}`: {e}",
            src.display(),
            dst.display()
        )
    })
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| format!("Failed to serialize publish metadata: {e}"))?;
    fs::write(path, bytes).map_err(|e| format!("Failed to write `{}`: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, Package};
    use std::collections::BTreeMap;

    fn project_at(root: PathBuf) -> Project {
        Project {
            root,
            manifest: Manifest {
                package: Package {
                    name: "demo_pkg".to_string(),
                    version: "1.2.3".to_string(),
                    edition: Some("2026".to_string()),
                    effects: Some(vec!["Heap".to_string()]),
                    capabilities: Some(vec!["IO".to_string()]),
                },
                dependencies: BTreeMap::new(),
            },
            name: "demo_pkg".to_string(),
        }
    }

    fn write_project(root: &Path) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("gradient.toml"),
            r#"[package]
name = "demo_pkg"
version = "1.2.3"
edition = "2026"
effects = ["Heap"]
capabilities = ["IO"]

[dependencies]
"#,
        )
        .unwrap();
        fs::write(root.join("src/main.gr"), "fn main() -> Int:\n    ret 0\n").unwrap();
    }

    #[test]
    fn dry_run_packages_project_without_signing_or_upload() {
        let tmp = tempfile::tempdir().unwrap();
        write_project(tmp.path());
        let project = project_at(tmp.path().to_path_buf());
        let out = tmp.path().join("out");

        let result = publish_project(
            &project,
            PublishOptions {
                registry: None,
                out_dir: Some(out.to_str().unwrap()),
                dry_run: true,
                allow_dirty: true,
                cosign_bin: None,
            },
        )
        .unwrap();

        assert!(result.artifact_path.is_file());
        assert!(result.bundle_path.is_none());
        assert!(result.artifact_sha256.starts_with("sha256:"));
        assert!(out.join("demo_pkg-1.2.3.publish.json").is_file());
    }

    fn fake_cosign_at(path: &Path) {
        fs::write(
            path,
            "#!/usr/bin/env bash\nset -euo pipefail\nbundle=\"\"\nwhile [[ $# -gt 0 ]]; do\n  case \"$1\" in\n    --bundle) bundle=\"$2\"; shift 2 ;;\n    *) shift ;;\n  esac\ndone\nprintf '{\"critical\":{\"identity\":{\"issuer\":\"https://token.actions.githubusercontent.com\"}},\"tlogEntries\":[{\"logIndex\":42,\"uuid\":\"abc\"}]}\n' > \"$bundle\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn publish_invokes_cosign_and_uploads_to_file_registry() {
        let tmp = tempfile::tempdir().unwrap();
        write_project(tmp.path());
        let fake_cosign = tmp.path().join("cosign");
        fake_cosign_at(&fake_cosign);

        let registry = tmp.path().join("registry");
        let out = tmp.path().join("out");
        let project = project_at(tmp.path().to_path_buf());
        let registry_arg = format!("file://{}", registry.display());
        let cosign_arg = fake_cosign.to_string_lossy().to_string();

        let result = publish_project(
            &project,
            PublishOptions {
                registry: Some(&registry_arg),
                out_dir: Some(out.to_str().unwrap()),
                dry_run: false,
                allow_dirty: true,
                cosign_bin: Some(&cosign_arg),
            },
        )
        .unwrap();

        let upload_dir = result.upload_dir.unwrap();
        assert!(upload_dir.join("demo_pkg-1.2.3.gradient-pkg").is_file());
        assert!(upload_dir.join("demo_pkg-1.2.3.sigstore.json").is_file());
        assert!(upload_dir.join("demo_pkg-1.2.3.publish.json").is_file());
        assert!(upload_dir.join("gradient-package.toml").is_file());
        let registry_manifest =
            fs::read_to_string(upload_dir.join("gradient-package.toml")).unwrap();
        assert!(registry_manifest.contains("[trust]"));
        assert!(registry_manifest.contains("public_effects"));
        let bundle = fs::read_to_string(upload_dir.join("demo_pkg-1.2.3.sigstore.json")).unwrap();
        assert!(bundle.contains("tlogEntries"));
    }

    #[test]
    fn publish_uploads_to_http_registry_with_sigstore_identity() {
        let tmp = tempfile::tempdir().unwrap();
        write_project(tmp.path());
        let fake_cosign = tmp.path().join("cosign");
        fake_cosign_at(&fake_cosign);

        let registry_root = tmp.path().join("http-registry");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        drop(listener);

        let server_root = registry_root.to_string_lossy().to_string();
        let server_addr = addr.clone();
        let server = std::thread::spawn(move || {
            crate::commands::registry::serve(crate::commands::registry::RegistryServeOptions {
                root: &server_root,
                addr: &server_addr,
                auth_identity: Some("abc"),
                max_requests: Some(5),
            })
            .unwrap();
        });
        wait_for_registry(&addr);

        let out = tmp.path().join("out-http");
        let project = project_at(tmp.path().to_path_buf());
        let registry_arg = format!("http://{addr}");
        let cosign_arg = fake_cosign.to_string_lossy().to_string();

        let result = publish_project(
            &project,
            PublishOptions {
                registry: Some(&registry_arg),
                out_dir: Some(out.to_str().unwrap()),
                dry_run: false,
                allow_dirty: true,
                cosign_bin: Some(&cosign_arg),
            },
        )
        .unwrap();
        server.join().unwrap();

        assert_eq!(
            result.upload_dir.unwrap(),
            PathBuf::from(format!("{registry_arg}/v1/packages/demo_pkg/1.2.3"))
        );
        let upload_dir = registry_root.join("demo_pkg/1.2.3");
        assert!(upload_dir.join("demo_pkg-1.2.3.gradient-pkg").is_file());
        assert!(upload_dir.join("demo_pkg-1.2.3.sigstore.json").is_file());
        assert!(upload_dir.join("demo_pkg-1.2.3.publish.json").is_file());
        assert!(upload_dir.join("gradient-package.toml").is_file());
    }

    fn wait_for_registry(addr: &str) {
        for _ in 0..50 {
            if let Ok(mut stream) = std::net::TcpStream::connect(addr) {
                use std::io::{Read, Write};
                stream
                    .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .unwrap();
                let mut response = String::new();
                stream.read_to_string(&mut response).unwrap();
                if response.contains("200 OK") {
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("registry backend did not start at {addr}");
    }
}
