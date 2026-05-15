//! Plugin discovery and dispatch (E11 #377).
//!
//! Plugins are external binaries on `PATH` whose names start with
//! `gradient-`. Invoking `gradient <name> [args]` looks up
//! `gradient-<name>` on `PATH`, sets a small set of well-known
//! environment variables, and re-execs the plugin binary with the
//! remaining arguments.
//!
//! The protocol is documented under `docs/plugins/protocol.md` and
//! pinned at version 1 (`PLUGIN_PROTOCOL_VERSION`).
//!
//! ## Reserved subcommand names
//!
//! Any name in [`BUILTIN_SUBCOMMANDS`] is dispatched to the in-tree
//! handler and never reaches the plugin loader. Plugins that pick a
//! reserved name are silently shadowed by the built-in.
//!
//! ## Discovery
//!
//! [`find_plugin`] walks `PATH` (split by the platform separator),
//! returning the first executable whose filename matches the plugin
//! name (with platform-specific extension on Windows). The walk is
//! deterministic: first match wins, in PATH order.
//!
//! ## Dispatch
//!
//! [`dispatch_plugin`] performs the lookup and, on hit, calls
//! [`std::process::Command::status`] so the plugin inherits stdin /
//! stdout / stderr. The host process exits with the plugin's exit
//! code.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Plugin protocol version. Pinned at 1; bumps require an ADR.
pub const PLUGIN_PROTOCOL_VERSION: u32 = 1;

/// Built-in subcommand names that always take precedence over a
/// same-named plugin on PATH.
///
/// Keep this in sync with `Commands` in `main.rs`. Order doesn't
/// matter for correctness; alphabetical for readability.
pub const BUILTIN_SUBCOMMANDS: &[&str] = &[
    "add", "bench", "bindgen", "build", "check", "doc", "fetch", "fmt", "init", "new", "repl",
    "run", "test", "update",
];

/// Standard prefix every plugin binary's filename must start with.
pub const PLUGIN_PREFIX: &str = "gradient-";

/// True iff the given subcommand name is a built-in (not eligible for
/// plugin dispatch).
pub fn is_builtin(name: &str) -> bool {
    BUILTIN_SUBCOMMANDS.contains(&name)
}

/// True iff the candidate string looks like a syntactically valid
/// plugin name. Names must:
///
/// - be non-empty
/// - start with an ASCII alphanumeric character
/// - contain only ASCII alphanumerics, `-`, or `_`
///
/// This excludes things like `--help`, `-v`, `..`, paths, etc., so the
/// pre-clap dispatcher can safely fall through to clap for those.
pub fn is_valid_plugin_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Return the platform-specific executable filename for a plugin name.
///
/// Unix: `gradient-<name>`. Windows: `gradient-<name>.exe` (the
/// `.exe` is appended unconditionally — most discovery callers also
/// try the bare form, see [`platform_candidates`]).
#[allow(dead_code)]
pub fn plugin_filename(name: &str) -> String {
    if cfg!(windows) {
        format!("{PLUGIN_PREFIX}{name}.exe")
    } else {
        format!("{PLUGIN_PREFIX}{name}")
    }
}

/// All filenames to probe for a plugin name on the current platform,
/// in order. On Unix this is just `[gradient-<name>]`; on Windows it
/// is `[gradient-<name>.exe, gradient-<name>]`.
pub fn platform_candidates(name: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![
            format!("{PLUGIN_PREFIX}{name}.exe"),
            format!("{PLUGIN_PREFIX}{name}"),
        ]
    } else {
        vec![format!("{PLUGIN_PREFIX}{name}")]
    }
}

/// Locate a plugin binary by name on `PATH`.
///
/// Returns the absolute path to the first matching executable in
/// `PATH` order, or `None` if no match was found. Built-in names are
/// rejected before lookup.
pub fn find_plugin(name: &str) -> Option<PathBuf> {
    if is_builtin(name) || !is_valid_plugin_name(name) {
        return None;
    }
    let path_var = env::var_os("PATH")?;
    find_plugin_in_path(name, &path_var)
}

/// Locate a plugin binary by name in an explicit PATH-like value.
///
/// Test-friendly: callers can pass a synthetic PATH built from
/// `tempfile::TempDir`s without mutating the process environment.
pub fn find_plugin_in_path(name: &str, path_var: &OsString) -> Option<PathBuf> {
    if !is_valid_plugin_name(name) || is_builtin(name) {
        return None;
    }
    let candidates = platform_candidates(name);
    for dir in env::split_paths(path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for cand in &candidates {
            let full = dir.join(cand);
            if is_executable(&full) {
                return Some(full);
            }
        }
    }
    None
}

/// True iff the given path exists and (on Unix) has the executable
/// bit set for some user.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

/// Build the plugin environment, layered on top of the current
/// process environment. Pure / inspectable for tests.
///
/// The plugin sees:
///
/// | Variable | Meaning |
/// |---|---|
/// | `GRADIENT_PLUGIN_PROTOCOL_VERSION` | Protocol version (currently `1`) |
/// | `GRADIENT_VERSION` | The host CLI version (`env!("CARGO_PKG_VERSION")`) |
/// | `GRADIENT_BIN` | Absolute path to the `gradient` binary that invoked the plugin (best-effort; may be missing) |
/// | `GRADIENT_PROJECT_ROOT` | Absolute path to the nearest enclosing project root, when one is found |
///
/// Existing plugin-specified env vars in the parent are inherited.
pub fn plugin_env(host_bin: Option<&Path>, project_root: Option<&Path>) -> Vec<(String, OsString)> {
    let mut out: Vec<(String, OsString)> = Vec::new();
    out.push((
        "GRADIENT_PLUGIN_PROTOCOL_VERSION".to_string(),
        OsString::from(PLUGIN_PROTOCOL_VERSION.to_string()),
    ));
    out.push((
        "GRADIENT_VERSION".to_string(),
        OsString::from(env!("CARGO_PKG_VERSION")),
    ));
    if let Some(bin) = host_bin {
        out.push(("GRADIENT_BIN".to_string(), bin.as_os_str().to_owned()));
    }
    if let Some(root) = project_root {
        out.push((
            "GRADIENT_PROJECT_ROOT".to_string(),
            root.as_os_str().to_owned(),
        ));
    }
    out
}

/// The result of a plugin dispatch attempt.
#[derive(Debug)]
pub enum DispatchOutcome {
    /// Plugin found and executed; the host should exit with this code.
    Ran { exit_code: i32 },
    /// No plugin matched the given name on PATH.
    NotFound,
}

/// Dispatch a plugin by name, forwarding the given args.
///
/// On hit, runs the plugin to completion (inheriting stdio) and
/// returns `Ran { exit_code }`. The host caller is responsible for
/// `process::exit(exit_code)`.
///
/// Splitting find / run / exit lets unit tests cover the wiring
/// without spawning real subprocesses.
pub fn dispatch_plugin(name: &str, args: &[String]) -> DispatchOutcome {
    let Some(plugin_path) = find_plugin(name) else {
        return DispatchOutcome::NotFound;
    };
    let host_bin = env::current_exe().ok();
    let project_root = find_project_root();
    let envs = plugin_env(host_bin.as_deref(), project_root.as_deref());

    let mut cmd = Command::new(&plugin_path);
    cmd.args(args);
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    match cmd.status() {
        Ok(status) => DispatchOutcome::Ran {
            exit_code: status.code().unwrap_or(1),
        },
        Err(e) => {
            eprintln!(
                "error: failed to execute plugin `{}` at {}: {e}",
                name,
                plugin_path.display()
            );
            DispatchOutcome::Ran { exit_code: 1 }
        }
    }
}

/// Walk up from `cwd` looking for `gradient.toml`. Returns the
/// directory holding it, or `None` if not inside a project.
fn find_project_root() -> Option<PathBuf> {
    let mut cur = env::current_dir().ok()?;
    loop {
        if cur.join("gradient.toml").is_file() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// Format a friendly "command not found" message that lists the
/// plugin lookup paths and the name probed.
#[allow(dead_code)]
pub fn format_not_found_error(name: &str) -> String {
    let candidate = plugin_filename(name);
    format!(
        "error: unrecognized subcommand `{name}`\n\
         note: tried plugin lookup for `{candidate}` on PATH (none found)\n\
         note: see `gradient --help` for built-in subcommands and \
         docs/plugins/protocol.md for the plugin protocol"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    #[test]
    fn schema_version_is_one() {
        assert_eq!(PLUGIN_PROTOCOL_VERSION, 1);
    }

    #[test]
    fn builtins_listed_alphabetically_and_unique() {
        let mut sorted = BUILTIN_SUBCOMMANDS.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.as_slice(), BUILTIN_SUBCOMMANDS);
    }

    #[test]
    fn is_builtin_recognizes_each() {
        for name in BUILTIN_SUBCOMMANDS {
            assert!(is_builtin(name), "{name} should be a builtin");
        }
        assert!(!is_builtin("hello"));
        assert!(!is_builtin(""));
    }

    #[test]
    fn is_valid_plugin_name_examples() {
        assert!(is_valid_plugin_name("hello"));
        assert!(is_valid_plugin_name("hello-world"));
        assert!(is_valid_plugin_name("hello_world"));
        assert!(is_valid_plugin_name("a"));
        assert!(is_valid_plugin_name("h2o"));
        assert!(!is_valid_plugin_name(""));
        assert!(!is_valid_plugin_name("-flag"));
        assert!(!is_valid_plugin_name("--help"));
        assert!(!is_valid_plugin_name("a b"));
        assert!(!is_valid_plugin_name("a/b"));
        assert!(!is_valid_plugin_name(".."));
        assert!(!is_valid_plugin_name("a.b"));
    }

    #[test]
    fn plugin_filename_unix_form() {
        if !cfg!(windows) {
            assert_eq!(plugin_filename("hello"), "gradient-hello");
        }
    }

    #[test]
    fn platform_candidates_unix_form() {
        if !cfg!(windows) {
            assert_eq!(platform_candidates("hello"), vec!["gradient-hello"]);
        }
    }

    #[test]
    fn find_plugin_in_path_returns_none_for_builtin() {
        let path = OsString::from("/nonexistent");
        assert!(find_plugin_in_path("build", &path).is_none());
        assert!(find_plugin_in_path("test", &path).is_none());
    }

    #[test]
    fn find_plugin_in_path_returns_none_for_invalid_name() {
        let path = OsString::from("/nonexistent");
        assert!(find_plugin_in_path("--help", &path).is_none());
        assert!(find_plugin_in_path("", &path).is_none());
        assert!(find_plugin_in_path("a/b", &path).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn find_plugin_in_path_locates_executable() {
        let dir = TempDir::new().expect("tempdir");
        let plugin_path = dir.path().join("gradient-hello");
        fs::write(&plugin_path, "#!/bin/sh\necho hi\n").expect("write plugin");
        let mut perms = fs::metadata(&plugin_path).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&plugin_path, perms).expect("chmod");

        let path = OsString::from(dir.path().as_os_str());
        let found = find_plugin_in_path("hello", &path).expect("plugin found");
        assert_eq!(found, plugin_path);
    }

    #[cfg(unix)]
    #[test]
    fn find_plugin_in_path_skips_non_executable() {
        let dir = TempDir::new().expect("tempdir");
        let plugin_path = dir.path().join("gradient-hello");
        fs::write(&plugin_path, "data").expect("write file");
        // Default permissions: not executable.
        let mut perms = fs::metadata(&plugin_path).expect("meta").permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&plugin_path, perms).expect("chmod");

        let path = OsString::from(dir.path().as_os_str());
        assert!(find_plugin_in_path("hello", &path).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn find_plugin_in_path_takes_first_match_in_path_order() {
        let first = TempDir::new().expect("first tempdir");
        let second = TempDir::new().expect("second tempdir");
        for dir in [&first, &second] {
            let plugin_path = dir.path().join("gradient-hello");
            fs::write(&plugin_path, "#!/bin/sh\nexit 0\n").expect("write plugin");
            let mut perms = fs::metadata(&plugin_path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&plugin_path, perms).expect("chmod");
        }

        let mut path_str = first.path().as_os_str().to_owned();
        path_str.push(":");
        path_str.push(second.path().as_os_str());
        let found = find_plugin_in_path("hello", &path_str).expect("plugin found");
        assert_eq!(found, first.path().join("gradient-hello"));
    }

    #[cfg(unix)]
    #[test]
    fn find_plugin_in_path_skips_empty_path_segments() {
        let dir = TempDir::new().expect("tempdir");
        let plugin_path = dir.path().join("gradient-hello");
        fs::write(&plugin_path, "#!/bin/sh\nexit 0\n").expect("write plugin");
        let mut perms = fs::metadata(&plugin_path).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&plugin_path, perms).expect("chmod");

        // Empty leading segment should be ignored, plugin still found.
        let mut path_str = OsString::from(":");
        path_str.push(dir.path().as_os_str());
        let found = find_plugin_in_path("hello", &path_str).expect("plugin found");
        assert_eq!(found, plugin_path);
    }

    #[test]
    fn plugin_env_includes_protocol_version_and_gradient_version() {
        let envs = plugin_env(None, None);
        let map: std::collections::HashMap<&str, &OsString> =
            envs.iter().map(|(k, v)| (k.as_str(), v)).collect();
        assert_eq!(
            map.get("GRADIENT_PLUGIN_PROTOCOL_VERSION")
                .expect("protocol version set")
                .to_string_lossy(),
            "1"
        );
        assert!(map.contains_key("GRADIENT_VERSION"));
        assert!(!map.contains_key("GRADIENT_BIN"));
        assert!(!map.contains_key("GRADIENT_PROJECT_ROOT"));
    }

    #[test]
    fn plugin_env_threads_bin_and_project_root() {
        let bin = PathBuf::from("/usr/local/bin/gradient");
        let root = PathBuf::from("/work/myapp");
        let envs = plugin_env(Some(&bin), Some(&root));
        let map: std::collections::HashMap<&str, &OsString> =
            envs.iter().map(|(k, v)| (k.as_str(), v)).collect();
        assert_eq!(
            map.get("GRADIENT_BIN").expect("bin set").as_os_str(),
            bin.as_os_str()
        );
        assert_eq!(
            map.get("GRADIENT_PROJECT_ROOT")
                .expect("project root set")
                .as_os_str(),
            root.as_os_str()
        );
    }

    #[test]
    fn format_not_found_error_mentions_protocol_doc() {
        let msg = format_not_found_error("doesnotexist");
        assert!(msg.contains("doesnotexist"));
        assert!(msg.contains("docs/plugins/protocol.md"));
        if !cfg!(windows) {
            assert!(msg.contains("gradient-doesnotexist"));
        }
    }
}
