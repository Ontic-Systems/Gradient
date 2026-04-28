//! Multi-file module resolution for the Gradient compiler.
//!
//! This module handles resolving `use` declarations to source files on disk,
//! parsing dependent modules, and building a combined type environment that
//! spans all modules in a compilation unit.
//!
//! # Resolution rules
//!
//! - `use math` looks for `math.gr` in the same directory as the importing file.
//! - `use math.utils` looks for `math/utils.gr` relative to the source root.
//!   (For now, only the simple case `use <name>` is supported.)
//! - Circular imports are detected and reported as errors.
//!
//! # Usage
//!
//! ```ignore
//! use gradient_compiler::resolve::ModuleResolver;
//!
//! let resolver = ModuleResolver::new("/path/to/src/main.gr");
//! let result = resolver.resolve_all();
//! // result.modules contains all parsed modules
//! // result.errors contains any resolution errors
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::module::{ImportKind, Module};
use crate::lexer::Lexer;
use crate::parser::error::ParseError;
use crate::parser::Parser;

/// Information about a single resolved module.
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    /// The module name (from the `mod` declaration, or derived from the filename).
    pub name: String,
    /// The path to the source file.
    pub path: PathBuf,
    /// The parsed AST.
    pub module: Module,
    /// Parse errors from this module.
    pub parse_errors: Vec<ParseError>,
    /// The file id assigned to this module (used in spans).
    pub file_id: u32,
    /// The raw source text (kept for diagnostics and query API).
    pub source: String,
}

/// The result of multi-file module resolution.
#[derive(Debug)]
pub struct ResolveResult {
    /// All resolved modules, keyed by module name.
    /// The entry module is always present.
    pub modules: HashMap<String, ResolvedModule>,
    /// The name of the entry module (the one the user passed to the compiler).
    pub entry_module: String,
    /// Resolution errors (e.g., file not found, circular imports).
    pub errors: Vec<String>,
}

impl ResolveResult {
    /// Returns true if resolution succeeded without errors.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty() && self.modules.values().all(|m| m.parse_errors.is_empty())
    }

    /// Collect all parse errors across all modules.
    pub fn all_parse_errors(&self) -> Vec<(String, &ParseError)> {
        let mut all = Vec::new();
        for (name, m) in &self.modules {
            for err in &m.parse_errors {
                all.push((name.clone(), err));
            }
        }
        all
    }
}

/// Resolves `use` declarations to source files and parses all dependencies.
///
/// # Import sandboxing (issue #181)
///
/// The resolver enforces a *source root* sandbox: every successfully-resolved
/// import path must canonicalize to a path that lies inside the source root
/// (or inside one of the allowlisted stdlib roots, if any are configured).
/// Imports whose canonicalized path escapes the sandbox — via `..`, an
/// absolute path, or a symlink that points outside the root — are rejected
/// with a resolution error and never read from disk.
///
/// By default the source root is the canonicalized parent directory of the
/// entry file. Callers that need a different root (for example a project
/// root distinct from the entry file's directory) can use
/// [`ModuleResolver::with_source_root`].
pub struct ModuleResolver {
    /// The base directory for resolving imports (directory of the entry file).
    /// This is *not* the security boundary — it is just the search start point.
    base_dir: PathBuf,
    /// Canonicalized source root that every resolved import must stay under.
    /// `None` means the resolver could not canonicalize a root (e.g. the
    /// entry file's parent does not exist on disk); in that case all
    /// filesystem-touching imports are rejected.
    source_root: Option<PathBuf>,
    /// Canonicalized roots outside `source_root` that imports are *also*
    /// allowed to resolve into (typically the standard library install dir).
    /// Empty by default — absolute imports are rejected unless they fall
    /// under one of these roots.
    stdlib_roots: Vec<PathBuf>,
    /// Already-loaded modules, keyed by module name.
    loaded: HashMap<String, ResolvedModule>,
    /// Modules currently being loaded (for cycle detection).
    loading: HashSet<String>,
    /// Resolution errors.
    errors: Vec<String>,
    /// Counter for assigning file IDs.
    next_file_id: u32,
}

impl ModuleResolver {
    /// Create a new resolver rooted at the directory containing the entry file.
    ///
    /// The source root is set to the canonicalized parent directory of the
    /// entry file; all imports are sandboxed to that directory.
    pub fn new(entry_file: &Path) -> Self {
        let base_dir = entry_file
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        // Canonicalize the source root for the security check. If the parent
        // directory does not exist or cannot be canonicalized, leave the
        // source root as `None` and downstream sandbox checks will reject
        // every filesystem import.
        let source_root = std::fs::canonicalize(&base_dir).ok();

        Self {
            base_dir,
            source_root,
            stdlib_roots: Vec::new(),
            loaded: HashMap::new(),
            loading: HashSet::new(),
            errors: Vec::new(),
            next_file_id: 0,
        }
    }

    /// Create a resolver with an explicit source root.
    ///
    /// The given `source_root` is canonicalized and becomes the security
    /// boundary for all imports. `base_dir` (the search start) defaults to
    /// the entry file's parent directory, just like [`ModuleResolver::new`].
    pub fn with_source_root(entry_file: &Path, source_root: &Path) -> Self {
        let base_dir = entry_file
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        let canonical_root = std::fs::canonicalize(source_root).ok();

        Self {
            base_dir,
            source_root: canonical_root,
            stdlib_roots: Vec::new(),
            loaded: HashMap::new(),
            loading: HashSet::new(),
            errors: Vec::new(),
            next_file_id: 0,
        }
    }

    /// Add an allowlisted root directory that imports are allowed to resolve
    /// into in addition to the source root. Intended for stdlib install
    /// directories. Non-existent paths are silently ignored.
    pub fn allow_stdlib_root(mut self, root: &Path) -> Self {
        if let Ok(canonical) = std::fs::canonicalize(root) {
            self.stdlib_roots.push(canonical);
        }
        self
    }

    /// Returns the canonicalized source root, if any.
    pub fn source_root(&self) -> Option<&Path> {
        self.source_root.as_deref()
    }

    /// Check that `candidate` (which must already exist on disk) canonicalizes
    /// to a path inside the source root or one of the allowlisted stdlib
    /// roots. Returns the canonicalized path on success, or `None` if the
    /// path escapes the sandbox.
    ///
    /// This is the single security gate for filesystem imports: every
    /// successfully-resolved candidate from `resolve_file_path` /
    /// `resolve_module_path` is run through here before the file is read.
    fn enforce_sandbox(&self, candidate: &Path) -> Option<PathBuf> {
        // Canonicalize follows symlinks, so a symlink that points out of the
        // source root will be caught by the `starts_with` check below.
        let canonical = std::fs::canonicalize(candidate).ok()?;

        if let Some(root) = &self.source_root {
            if canonical.starts_with(root) {
                return Some(canonical);
            }
        }

        for stdlib in &self.stdlib_roots {
            if canonical.starts_with(stdlib) {
                return Some(canonical);
            }
        }

        None
    }

    /// Resolve all modules starting from the given entry file.
    ///
    /// This reads and parses the entry file, discovers its `use` declarations,
    /// recursively resolves them, and returns a `ResolveResult` containing
    /// all parsed modules.
    pub fn resolve_all(mut self, entry_file: &Path) -> ResolveResult {
        let entry_source = match std::fs::read_to_string(entry_file) {
            Ok(s) => s,
            Err(e) => {
                self.errors.push(format!(
                    "cannot read entry file `{}`: {}",
                    entry_file.display(),
                    e
                ));
                return ResolveResult {
                    modules: HashMap::new(),
                    entry_module: String::new(),
                    errors: self.errors,
                };
            }
        };

        let entry_name = self.resolve_module_from_source(&entry_source, entry_file.to_path_buf());

        ResolveResult {
            modules: self.loaded,
            entry_module: entry_name,
            errors: self.errors,
        }
    }

    /// Resolve from a source string directly (without reading a file).
    /// The path is used for resolving relative imports.
    pub fn resolve_from_source(mut self, source: &str, virtual_path: &Path) -> ResolveResult {
        let entry_name = self.resolve_module_from_source(source, virtual_path.to_path_buf());

        ResolveResult {
            modules: self.loaded,
            entry_module: entry_name,
            errors: self.errors,
        }
    }

    /// Internal: parse a source string and recursively resolve its dependencies.
    /// Returns the module name.
    fn resolve_module_from_source(&mut self, source: &str, path: PathBuf) -> String {
        let file_id = self.next_file_id;
        self.next_file_id += 1;

        // Parse the source.
        let mut lexer = Lexer::new(source, file_id);
        let tokens = lexer.tokenize();
        let (module, parse_errors) = Parser::parse(tokens, file_id);

        // Determine the module name.
        let module_name = module
            .module_decl
            .as_ref()
            .map(|md| md.path.join("."))
            .unwrap_or_else(|| {
                // Derive from the filename.
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("main")
                    .to_string()
            });

        // Check for circular imports.
        if self.loading.contains(&module_name) {
            self.errors.push(format!(
                "circular import detected: module `{}` is already being loaded",
                module_name
            ));
            return module_name;
        }

        // Check if already loaded.
        if self.loaded.contains_key(&module_name) {
            return module_name;
        }

        // Mark as loading (cycle detection).
        self.loading.insert(module_name.clone());

        // Collect use declarations and import items before storing the module.
        let uses: Vec<_> = module.uses.clone();
        let imports: Vec<_> = module
            .items
            .iter()
            .filter_map(|item| match &item.node {
                crate::ast::item::ItemKind::Import { path, alias } => {
                    Some((path.clone(), alias.clone()))
                }
                _ => None,
            })
            .collect();

        // Store the resolved module.
        self.loaded.insert(
            module_name.clone(),
            ResolvedModule {
                name: module_name.clone(),
                path: path.clone(),
                module,
                parse_errors,
                file_id,
                source: source.to_string(),
            },
        );

        // Resolve each `use` declaration.
        for use_decl in &uses {
            let dep_name = use_decl.module_name();

            // Check for circular imports: if the dependency is currently
            // being loaded (in the call stack), that's a cycle.
            if self.loading.contains(&dep_name) {
                self.errors.push(format!(
                    "circular import detected: `{}` and `{}` import each other",
                    module_name, dep_name
                ));
                continue;
            }

            if self.loaded.contains_key(&dep_name) {
                continue; // Already loaded.
            }

            // Resolve the file path for this import.
            let dep_path = match &use_decl.import {
                ImportKind::FilePath(file_path) => {
                    // For file paths, resolve relative to the importing file
                    self.resolve_file_path(file_path, &path)
                }
                ImportKind::ModulePath(path_segments) => {
                    // For module paths, use the standard resolution logic
                    self.resolve_module_path(path_segments, &path)
                }
            };

            match dep_path {
                Some(dep_file) => match std::fs::read_to_string(&dep_file) {
                    Ok(dep_source) => {
                        self.resolve_module_from_source(&dep_source, dep_file);
                    }
                    Err(e) => {
                        self.errors.push(format!(
                            "cannot read module `{}` at `{}`: {}",
                            dep_name,
                            dep_file.display(),
                            e
                        ));
                    }
                },
                None => {
                    self.errors.push(format!(
                        "cannot resolve import `{}`: file not found (searched in `{}`)",
                        use_decl.import_path_string(),
                        self.base_dir.display()
                    ));
                }
            }
        }

        // Resolve each `import` statement (from ItemKind::Import).
        for (import_path, alias) in &imports {
            let dep_name = alias
                .as_ref()
                .cloned()
                .unwrap_or_else(|| {
                    // Derive module name from path (e.g., "./lexer.gr" -> "lexer")
                    Path::new(import_path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(import_path)
                        .to_string()
                });

            // Check for circular imports.
            if self.loading.contains(&dep_name) {
                self.errors.push(format!(
                    "circular import detected: `{}` and `{}` import each other",
                    module_name, dep_name
                ));
                continue;
            }

            if self.loaded.contains_key(&dep_name) {
                continue; // Already loaded.
            }

            // Resolve the file path for this import.
            let dep_file = self.resolve_file_path(import_path, &path);

            match dep_file {
                Some(dep_file) => match std::fs::read_to_string(&dep_file) {
                    Ok(dep_source) => {
                        self.resolve_module_from_source(&dep_source, dep_file);
                    }
                    Err(e) => {
                        self.errors.push(format!(
                            "cannot read imported module `{}` at `{}`: {}",
                            dep_name,
                            dep_file.display(),
                            e
                        ));
                    }
                },
                None => {
                    self.errors.push(format!(
                        "cannot resolve import `{}`: file not found (searched in `{}`)",
                        import_path,
                        self.base_dir.display()
                    ));
                }
            }
        }

        // Unmark loading.
        self.loading.remove(&module_name);

        module_name
    }

    /// Resolve the file path for a file path import (e.g. `use "./token.gr"`).
    ///
    /// Resolves relative to the importing file's directory, then enforces the
    /// import sandbox: every successful candidate must canonicalize to a path
    /// inside the source root (or an allowlisted stdlib root). Absolute paths
    /// are rejected unless they fall under one of those roots — which makes
    /// `..` escapes and absolute escapes both fail closed.
    fn resolve_file_path(&self, file_path: &str, from_file: &Path) -> Option<PathBuf> {
        let from_dir = from_file.parent().unwrap_or_else(|| Path::new("."));

        // If the path starts with ./ or ../, resolve relative to from_dir
        if file_path.starts_with("./") || file_path.starts_with("../") {
            let candidate = from_dir.join(file_path);
            if candidate.exists() {
                if let Some(canonical) = self.enforce_sandbox(&candidate) {
                    return Some(canonical);
                }
            }
        } else {
            // For absolute paths or bare filenames, try as-is first.
            // The sandbox check below will reject absolute paths that are not
            // under the source root or an allowlisted stdlib root, so absolute
            // imports are denied by default.
            let candidate = PathBuf::from(file_path);
            if candidate.is_absolute() && candidate.exists() {
                if let Some(canonical) = self.enforce_sandbox(&candidate) {
                    return Some(canonical);
                }
                // Fall through: an absolute path that escapes the sandbox is
                // not silently re-resolved against base_dir. It is rejected.
                return None;
            }
            // Then try relative to from_dir
            let candidate = from_dir.join(file_path);
            if candidate.exists() {
                if let Some(canonical) = self.enforce_sandbox(&candidate) {
                    return Some(canonical);
                }
            }
            // Finally try relative to base directory
            let candidate = self.base_dir.join(file_path);
            if candidate.exists() {
                if let Some(canonical) = self.enforce_sandbox(&candidate) {
                    return Some(canonical);
                }
            }
        }

        None
    }

    /// Resolve the file path for a module path import (e.g. `use math` or `use math.utils`).
    ///
    /// For `use math`: looks for `math.gr` in the same directory as `from_file`.
    /// For `use math.utils`: looks for `math/utils.gr` relative to the base dir.
    ///
    /// Like [`resolve_file_path`], every successful candidate is run through
    /// the sandbox check and rejected if it escapes the source root.
    fn resolve_module_path(&self, path_segments: &[String], from_file: &Path) -> Option<PathBuf> {
        let from_dir = from_file.parent().unwrap_or_else(|| Path::new("."));

        if path_segments.is_empty() {
            return None;
        }

        if path_segments.len() == 1 {
            // Simple case: `use math` -> `math.gr` in the same directory
            let candidate = from_dir.join(format!("{}.gr", path_segments[0]));
            if candidate.exists() {
                if let Some(canonical) = self.enforce_sandbox(&candidate) {
                    return Some(canonical);
                }
            }
            // Also try from the base directory
            let candidate = self.base_dir.join(format!("{}.gr", path_segments[0]));
            if candidate.exists() {
                if let Some(canonical) = self.enforce_sandbox(&candidate) {
                    return Some(canonical);
                }
            }
        } else {
            // Multi-segment: `use math.utils` -> `math/utils.gr`
            let mut rel_path = PathBuf::new();
            for seg in &path_segments[..path_segments.len() - 1] {
                rel_path.push(seg);
            }
            let last_segment = path_segments.last()?;
            rel_path.push(format!("{}.gr", last_segment));

            // Try from the importing file's directory
            let candidate = from_dir.join(&rel_path);
            if candidate.exists() {
                if let Some(canonical) = self.enforce_sandbox(&candidate) {
                    return Some(canonical);
                }
            }
            // Try from the base directory
            let candidate = self.base_dir.join(&rel_path);
            if candidate.exists() {
                if let Some(canonical) = self.enforce_sandbox(&candidate) {
                    return Some(canonical);
                }
            }
        }

        None
    }
}

/// Trait to clean up path normalization (resolve . and .. components).
///
/// Kept for backwards-compatibility / potential reuse, but no longer used by
/// the resolver itself: the import sandbox check (see
/// [`ModuleResolver::enforce_sandbox`]) uses `std::fs::canonicalize` so that
/// symlinks are followed and `..` cannot escape the source root.
#[allow(dead_code)]
trait PathClean {
    fn clean(&self) -> PathBuf;
}

#[allow(dead_code)]
impl PathClean for PathBuf {
    fn clean(&self) -> PathBuf {
        let mut result = PathBuf::new();
        for component in self.components() {
            match component {
                std::path::Component::ParentDir => {
                    // Pop the last component if it's not a parent dir
                    if let Some(last) = result.file_name() {
                        if last != ".." {
                            result.pop();
                        } else {
                            result.push("..");
                        }
                    }
                }
                std::path::Component::CurDir => {
                    // Skip current dir components
                }
                _ => {
                    result.push(component);
                }
            }
        }
        result
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a temporary directory with test files and return its path.
    fn create_test_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            let file_path = dir.path().join(name);
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&file_path, content).unwrap();
        }
        dir
    }

    #[test]
    fn resolve_single_file_no_imports() {
        let dir = create_test_dir(&[(
            "main.gr",
            "mod main\n\nfn main() -> !{IO} ():\n    print(\"hello\")\n",
        )]);
        let entry = dir.path().join("main.gr");
        let resolver = ModuleResolver::new(&entry);
        let result = resolver.resolve_all(&entry);

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.entry_module, "main");
        assert_eq!(result.modules.len(), 1);
        assert!(result.modules.contains_key("main"));
    }

    #[test]
    fn resolve_two_files() {
        let dir = create_test_dir(&[
            (
                "main.gr",
                "mod main\n\nuse helper\n\nfn main() -> !{IO} ():\n    print(\"hello\")\n",
            ),
            (
                "helper.gr",
                "mod helper\n\nfn add(a: Int, b: Int) -> Int:\n    a + b\n",
            ),
        ]);
        let entry = dir.path().join("main.gr");
        let resolver = ModuleResolver::new(&entry);
        let result = resolver.resolve_all(&entry);

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.entry_module, "main");
        assert_eq!(result.modules.len(), 2);
        assert!(result.modules.contains_key("main"));
        assert!(result.modules.contains_key("helper"));
    }

    #[test]
    fn resolve_missing_import() {
        let dir = create_test_dir(&[(
            "main.gr",
            "mod main\n\nuse nonexistent\n\nfn main():\n    ()\n",
        )]);
        let entry = dir.path().join("main.gr");
        let resolver = ModuleResolver::new(&entry);
        let result = resolver.resolve_all(&entry);

        assert!(!result.errors.is_empty());
        assert!(
            result.errors[0].contains("cannot resolve import `nonexistent`"),
            "expected 'cannot resolve import' error, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn detect_circular_imports() {
        let dir = create_test_dir(&[
            ("a.gr", "mod a\n\nuse b\n\nfn from_a() -> Int:\n    1\n"),
            ("b.gr", "mod b\n\nuse a\n\nfn from_b() -> Int:\n    2\n"),
        ]);
        let entry = dir.path().join("a.gr");
        let resolver = ModuleResolver::new(&entry);
        let result = resolver.resolve_all(&entry);

        assert!(
            result.errors.iter().any(|e| e.contains("circular import")),
            "expected circular import error, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn resolve_transitive_dependency() {
        let dir = create_test_dir(&[
            ("main.gr", "mod main\n\nuse helper\n\nfn main():\n    ()\n"),
            (
                "helper.gr",
                "mod helper\n\nuse utils\n\nfn help() -> Int:\n    1\n",
            ),
            ("utils.gr", "mod utils\n\nfn util() -> Int:\n    42\n"),
        ]);
        let entry = dir.path().join("main.gr");
        let resolver = ModuleResolver::new(&entry);
        let result = resolver.resolve_all(&entry);

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.modules.len(), 3);
        assert!(result.modules.contains_key("main"));
        assert!(result.modules.contains_key("helper"));
        assert!(result.modules.contains_key("utils"));
    }

    #[test]
    fn module_name_from_mod_decl() {
        let dir = create_test_dir(&[(
            "math.gr",
            "mod math\n\nfn add(a: Int, b: Int) -> Int:\n    a + b\n",
        )]);
        let entry = dir.path().join("math.gr");
        let resolver = ModuleResolver::new(&entry);
        let result = resolver.resolve_all(&entry);

        assert!(result.errors.is_empty());
        let m = result.modules.get("math").unwrap();
        assert_eq!(m.name, "math");
    }

    #[test]
    fn module_name_from_filename() {
        let dir = create_test_dir(&[("math.gr", "fn add(a: Int, b: Int) -> Int:\n    a + b\n")]);
        let entry = dir.path().join("math.gr");
        let resolver = ModuleResolver::new(&entry);
        let result = resolver.resolve_all(&entry);

        assert!(result.errors.is_empty());
        // Without a mod declaration, the name should be derived from the filename.
        let m = result.modules.get("math").unwrap();
        assert_eq!(m.name, "math");
    }

    #[test]
    fn duplicate_import_loaded_once() {
        let dir = create_test_dir(&[
            (
                "main.gr",
                "mod main\n\nuse helper\nuse helper\n\nfn main():\n    ()\n",
            ),
            ("helper.gr", "mod helper\n\nfn help() -> Int:\n    1\n"),
        ]);
        let entry = dir.path().join("main.gr");
        let resolver = ModuleResolver::new(&entry);
        let result = resolver.resolve_all(&entry);

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.modules.len(), 2);
    }

    #[test]
    fn resolve_import_statement() {
        // Test the new `import "./path.gr"` syntax
        let dir = create_test_dir(&[
            (
                "main.gr",
                "mod main\n\nimport \"./helper.gr\"\n\nfn main():\n    ()\n",
            ),
            ("helper.gr", "mod helper\n\nfn help() -> Int:\n    1\n"),
        ]);
        let entry = dir.path().join("main.gr");
        let resolver = ModuleResolver::new(&entry);
        let result = resolver.resolve_all(&entry);

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.modules.len(), 2);
        assert!(result.modules.contains_key("main"));
        assert!(result.modules.contains_key("helper")); // Derived from "helper.gr"
    }

    #[test]
    fn resolve_import_statement_with_alias() {
        // Test `import "./path.gr" as alias` syntax
        let dir = create_test_dir(&[
            (
                "main.gr",
                "mod main\n\nimport \"./utils.gr\" as utilities\n\nfn main():\n    ()\n",
            ),
            ("utils.gr", "mod utils\n\nfn util() -> Int:\n    42\n"),
        ]);
        let entry = dir.path().join("main.gr");
        let resolver = ModuleResolver::new(&entry);
        let result = resolver.resolve_all(&entry);

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.modules.len(), 2);
        assert!(result.modules.contains_key("main"));
        // The module is loaded with its declared name "utils" from the mod declaration
        // The alias "utilities" is for referencing the module, not its stored name
        assert!(result.modules.contains_key("utils"), "expected 'utils' module, got: {:?}", result.modules.keys());
    }
}
