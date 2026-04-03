# Gradient Package Registry Implementation Plan

## Overview
Implement crates.io-style package registry with GitHub integration for remote package management.

**Parallel Workstreams:** 3 concurrent

---

## Workstream 1: Registry Client & HTTP Infrastructure
**Goal:** Create the HTTP client and GitHub integration for fetching package metadata

**Files:**
- `codebase/build-system/src/registry/mod.rs` - Registry module root
- `codebase/build-system/src/registry/client.rs` - HTTP client with caching
- `codebase/build-system/src/registry/github.rs` - GitHub package resolution

**Tasks:**
1. Add `reqwest` and `semver` dependencies to `build-system/Cargo.toml`
2. Create `RegistryClient` struct:
   - `cache_dir: PathBuf` - ~/.gradient/cache
   - `github_token: Option<String>` - For private repos
   - `new() -> Self` - Initialize with cache dir
3. Implement `fetch_github_repo(repo: &str) -> Result<RepoInfo, Error>`:
   - GET https://api.github.com/repos/{repo}
   - Parse response for default branch, tags
4. Implement `fetch_package_manifest(repo: &str, ref: &str) -> Result<Manifest, Error>`:
   - GET https://raw.githubusercontent.com/{repo}/{ref}/gradient.toml
   - Parse into `Manifest`
5. Implement `download_tarball(repo: &str, tag: &str) -> Result<Vec<u8>, Error>`:
   - GET https://github.com/{repo}/archive/refs/tags/{tag}.tar.gz
   - Stream to cache
6. Add cache management:
   - `cache_package(name: &str, version: &str, data: &[u8]) -> PathBuf`
   - `get_cached_package(name: &str, version: &str) -> Option<PathBuf>`
   - Cache TTL (24 hours for metadata, permanent for versions)

**Dependencies:**
```toml
[dependencies]
reqwest = { version = "0.11", features = ["json", "stream"] }
semver = "1.0"
tokio = { version = "1.0", features = ["full"] }
flate2 = "1.0"  # For tar.gz decompression
tar = "0.4"     # For archive extraction
```

**Deliverable:** Can fetch package metadata and source from GitHub

---

## Workstream 2: Version Resolution & Semver
**Goal:** Implement semver-based version resolution for registry packages

**Files:**
- `codebase/build-system/src/registry/semver.rs` - Version requirement parsing
- `codebase/build-system/src/resolver.rs` - Update to support registry deps

**Tasks:**
1. Create `VersionReq` wrapper around `semver::VersionReq`:
   - Parse strings like "^1.2.0", ">=1.0.0 <2.0.0", "~1.2.3"
   - Display format for lockfile
2. Implement `resolve_version(
       available: &[Version],
       req: &VersionReq
   ) -> Option<Version>`:
   - Find highest version matching requirement
   - Handle pre-release versions correctly
3. Update `resolver.rs`:
   - Add `RegistryClient` field to resolver
   - Add async `resolve_registry_dep()` method:
     - Check if version is cached
     - If not, fetch available versions from GitHub
     - Resolve best matching version
     - Download and cache package
   - Update error handling for network failures
4. Add `ResolvedSource::Registry` variant:
   ```rust
   pub enum ResolvedSource {
       Path(PathBuf),
       Registry { repo: String, version: Version },
   }
   ```

**Deliverable:** Can resolve "math@^1.0.0" to specific version from GitHub

---

## Workstream 3: CLI Integration & Lockfile Updates
**Goal:** Update CLI commands to support registry package operations

**Files:**
- `codebase/build-system/src/commands/add.rs` - Support `gradient add <pkg>`
- `codebase/build-system/src/commands/update.rs` - Update to refetch registry packages
- `codebase/build-system/src/lockfile.rs` - Update format for registry sources
- `codebase/build-system/src/manifest.rs` - Update dependency format

**Tasks:**
1. Update `manifest.rs`:
   - Add `registry` field to `DetailedDependency`:
     ```rust
     pub registry: Option<String>,  // "github" or custom URL
     ```
   - Add `Dependency::registry(name, version, registry)` constructor
2. Update `add.rs`:
   - Parse arguments: `gradient add <package>[@<version>]`
   - Detect argument type:
     - If contains `/` or `\` → path dependency (existing)
     - If starts with `http` → git dependency
     - Otherwise → registry dependency (default to GitHub)
   - For registry packages:
     - Initialize `RegistryClient`
     - Resolve version if not specified (latest)
     - Add to manifest as `DetailedDependency`
     - Print: "Added 'math@1.2.0' from github.com/gradient-lang/math"
3. Update `lockfile.rs`:
   - Update `LockedPackage::source` format:
     - Path: `path:../relative/path`
     - Registry: `github:namespace/name#v1.2.0`
     - Git: `git:https://github.com/user/repo#abc123`
   - Update `parse_source()` to handle new formats
   - Update `validate_checksums()` for registry packages
4. Update `update.rs`:
   - For registry packages in lockfile:
     - Check if newer version available (respecting semver)
     - Update lockfile with new version
     - Re-download if version changed
   - Add `--force` flag to redownload all packages

**Deliverable:** `gradient add math@1.2.0` works end-to-end

---

## Integration Points

### Between Workstreams 1 & 2:
- Workstream 1 provides `RegistryClient::fetch_package_manifest()`
- Workstream 2 calls it in `resolve_registry_dep()`

### Between Workstreams 2 & 3:
- Workstream 2 provides `resolve_registry_dep()` for version resolution
- Workstream 3 calls it in `add.rs` when adding registry packages

### All Workstreams → Main:
- Must update `Cargo.toml` with new dependencies
- Must not break existing path-based dependency flow

---

## Testing Checklist

- [ ] Can fetch package metadata from GitHub API
- [ ] Can download and cache package tarballs
- [ ] Semver resolution works (^1.0.0 matches 1.2.0 not 2.0.0)
- [ ] `gradient add math` resolves latest version
- [ ] `gradient add math@1.2.0` pins version
- [ ] Lockfile updated with registry source format
- [ ] `gradient update` refreshes registry packages
- [ ] Cache is used on second fetch
- [ ] Network failures handled gracefully
- [ ] Existing path dependencies still work

---

## File Structure

```
codebase/build-system/src/
├── registry/
│   ├── mod.rs      # Public exports
│   ├── client.rs   # HTTP client + caching
│   ├── github.rs   # GitHub-specific fetching
│   └── semver.rs   # Version resolution
├── commands/
│   ├── add.rs      # Updated with registry support
│   └── update.rs   # Updated to refetch
├── manifest.rs     # Updated dependency format
├── lockfile.rs     # Updated source format
└── resolver.rs     # Updated with registry resolution
```

---

## Example Usage Flow

```bash
# Add a package from GitHub registry
$ gradient add math
Resolving 'math' from github.com/gradient-lang/math...
Found versions: 1.0.0, 1.1.0, 1.2.0
Adding 'math@1.2.0' to dependencies
Downloading math@1.2.0...
Cached to ~/.gradient/cache/github/gradient-lang/math/1.2.0
Updated gradient.toml

# View manifest
cat gradient.toml
[dependencies]
math = { version = "1.2.0", registry = "github" }

# Build (resolves from lockfile)
$ gradient build
[1/3] Resolving dependencies...
[2/3] Compiling math@1.2.0...
[3/3] Compiling my-project...

# Update to latest compatible version
$ gradient update
Checking for updates...
math: 1.2.0 → 1.3.1 (latest matching ^1.0.0)
Downloading math@1.3.1...
Updated gradient.lock
```

---

## Risk Mitigation

1. **Network failures:** Cache aggressively, provide offline mode
2. **GitHub rate limits:** Support GitHub token for higher limits
3. **Breaking changes:** Pin exact versions in lockfile, semver only in manifest
4. **Private repos:** Support via GITHUB_TOKEN env var
