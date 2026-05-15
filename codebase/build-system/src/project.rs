// project.rs — Shared project discovery and configuration logic
//
// Provides the `Project` struct that encapsulates finding the project root,
// loading the manifest, locating the compiler binary, and computing paths
// for source files and build artifacts.

use crate::manifest::{self, Manifest};
use std::env;
use std::path::PathBuf;

/// A resolved Gradient project with its root directory and parsed manifest.
#[allow(dead_code)]
pub struct Project {
    /// The project name from `gradient.toml`.
    pub name: String,
    /// The absolute path to the project root (the directory containing `gradient.toml`).
    pub root: PathBuf,
    /// The parsed manifest.
    pub manifest: Manifest,
}

impl Project {
    /// Find the project root by walking up from the current directory looking
    /// for `gradient.toml`. Returns an error if no manifest is found.
    pub fn find() -> Result<Self, String> {
        let cwd = env::current_dir()
            .map_err(|e| format!("Failed to determine current directory: {}", e))?;

        let mut dir = cwd.as_path();
        loop {
            let candidate = dir.join("gradient.toml");
            if candidate.is_file() {
                let manifest = manifest::load(dir)
                    .map_err(|e| format!("Failed to parse {}: {}", candidate.display(), e))?;
                let name = manifest.package.name.clone();
                return Ok(Project {
                    name,
                    root: dir.to_path_buf(),
                    manifest,
                });
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => break,
            }
        }

        Err(format!(
            "Could not find `gradient.toml` in `{}` or any parent directory.\n\
             Is this a Gradient project? Try `gradient new <name>` or `gradient init`.",
            cwd.display()
        ))
    }

    /// Find the compiler binary. Search order:
    /// 1. `GRADIENT_COMPILER` environment variable
    /// 2. Sibling binary in the same directory as the running build-system
    ///    binary (dev-tree signal — `current_exe().parent()/gradient-compiler`).
    ///    This prevents a stale `~/.cargo/bin/gradient-compiler` from shadowing
    ///    a fresh dev-tree build when running `./target/debug/gradient ...`.
    /// 3. `gradient-compiler` on PATH
    /// 4. Relative path `../compiler/target/debug/gradient-compiler` from the
    ///    build-system crate (development fallback for older layouts)
    pub fn find_compiler() -> Result<PathBuf, String> {
        // 1. Explicit env var
        if let Ok(path) = env::var("GRADIENT_COMPILER") {
            let p = PathBuf::from(&path);
            if p.is_file() {
                return Ok(p);
            }
            return Err(format!(
                "GRADIENT_COMPILER is set to '{}' but that file does not exist.",
                path
            ));
        }

        // 2. Dev-tree sibling: if the running build-system binary has a
        //    `gradient-compiler` sibling in the same directory, prefer it.
        //    This catches the canonical dev layout (both binaries built into
        //    `codebase/target/debug/`) and prevents a stale cargo-installed
        //    `~/.cargo/bin/gradient-compiler` from being preferred over the
        //    fresh dev-tree build.
        if let Ok(exe) = env::current_exe() {
            if let Some(sibling) = dev_tree_sibling(&exe) {
                return Ok(sibling);
            }
        }

        // 3. Search PATH
        if let Ok(path) = which("gradient-compiler") {
            return Ok(path);
        }

        // 4. Development fallback: relative to the build-system crate.
        //    We try the path relative to the executable's location first,
        //    then relative to the current working directory.
        let fallback_candidates = vec![
            // relative to the running binary
            env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
                .map(|p| p.join("../../../compiler/target/debug/gradient-compiler")),
            // relative to cwd (for when running from the build-system directory)
            Some(PathBuf::from("../compiler/target/debug/gradient-compiler")),
        ];

        for candidate in fallback_candidates.into_iter().flatten() {
            let candidate = candidate.canonicalize().unwrap_or(candidate);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }

        Err("Could not find the Gradient compiler.\n\
             Set the GRADIENT_COMPILER environment variable to the path of `gradient-compiler`,\n\
             or ensure `gradient-compiler` is on your PATH."
            .to_string())
    }

    /// Get the target directory for build artifacts.
    pub fn target_dir(&self, release: bool) -> PathBuf {
        let profile = if release { "release" } else { "debug" };
        self.root.join("target").join(profile)
    }

    /// Get the path to the main source file.
    pub fn main_source(&self) -> PathBuf {
        self.root.join("src").join("main.gr")
    }

    /// Get the path to the output binary.
    pub fn output_binary(&self, release: bool) -> PathBuf {
        self.target_dir(release).join(&self.name)
    }

    /// Get the path to the intermediate object file.
    pub fn output_object(&self, release: bool) -> PathBuf {
        self.target_dir(release).join(format!("{}.o", self.name))
    }
}

/// Simple `which`-like lookup: search PATH for an executable.
fn which(name: &str) -> Result<PathBuf, String> {
    let path_var = env::var("PATH").unwrap_or_default();
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!("'{}' not found on PATH", name))
}

/// Given the path to a running build-system binary, return the path to a
/// `gradient-compiler` sibling in the same directory if such a file exists.
/// This is the dev-tree detection signal used by [`Project::find_compiler`]
/// to prefer a freshly-built dev-tree compiler over any stale binary on PATH
/// (e.g. an outdated `~/.cargo/bin/gradient-compiler`).
fn dev_tree_sibling(exe: &std::path::Path) -> Option<PathBuf> {
    let parent = exe.parent()?;
    let sibling = parent.join("gradient-compiler");
    if sibling.is_file() {
        Some(sibling)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Unique-temp-dir helper: cargo runs tests in parallel, so each test gets
    /// its own directory under `std::env::temp_dir()` to avoid interference.
    fn unique_tmpdir(label: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = env::temp_dir().join(format!("gradient_project_test_{}_{}_{}", label, pid, n));
        fs::create_dir_all(&dir).expect("create tmp dir");
        dir
    }

    #[test]
    fn dev_tree_sibling_returns_path_when_sibling_exists() {
        let dir = unique_tmpdir("sibling_exists");
        let exe = dir.join("gradient");
        let compiler = dir.join("gradient-compiler");
        fs::write(&exe, b"#!/bin/sh\n").unwrap();
        fs::write(&compiler, b"#!/bin/sh\n").unwrap();

        let found = dev_tree_sibling(&exe).expect("should find sibling");
        assert_eq!(found, compiler);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dev_tree_sibling_returns_none_when_sibling_absent() {
        let dir = unique_tmpdir("sibling_absent");
        let exe = dir.join("gradient");
        fs::write(&exe, b"#!/bin/sh\n").unwrap();
        // intentionally no gradient-compiler sibling

        assert!(dev_tree_sibling(&exe).is_none());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dev_tree_sibling_returns_none_when_sibling_is_directory() {
        let dir = unique_tmpdir("sibling_is_dir");
        let exe = dir.join("gradient");
        let compiler_dir = dir.join("gradient-compiler");
        fs::write(&exe, b"#!/bin/sh\n").unwrap();
        fs::create_dir(&compiler_dir).unwrap();

        // A directory is not a file; should not be picked up.
        assert!(dev_tree_sibling(&exe).is_none());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dev_tree_sibling_handles_root_exe() {
        // An exe at the filesystem root has no parent in the same sense; we
        // accept whatever `Path::parent` returns and probe accordingly. The
        // key invariant: the function does not panic on edge-case paths.
        let exe = PathBuf::from("/");
        // Should not panic and should return None (no `/gradient-compiler`
        // file expected in CI).
        let _ = dev_tree_sibling(&exe);
    }
}
