// gradient bindgen — Generate Gradient `extern fn` declarations and
// `@repr(C)` type aliases from a C header file (E3 #324 MVP).
//
// Usage:
//   gradient bindgen path/to/foo.h                   # write to stdout
//   gradient bindgen path/to/foo.h --out libc.gr     # write to file
//   gradient bindgen path/to/foo.h --module libc     # set module name in header
//
// MVP scope (curated C subset — issue #324):
//   * Single-line `//` and block `/* ... */` comments are stripped.
//   * Preprocessor lines (`#include`, `#define`, `#ifdef`, ...) are skipped.
//   * `typedef <scalar> <name>;`      (scalar type aliases)
//   * `typedef struct { ... } Name;`  (anonymous struct + alias)
//   * `typedef struct Name { ... } Name;`
//   * `struct Name { ... };`          (named struct decl)
//   * `<ret> name(<params>);`         (function declarations)
//   * `typedef <ret> (*Name)(<params>);` (function pointer typedefs)
//
// Deferred (out-of-scope for the MVP; documented in the generated banner
// and in the parent issue):
//   * Macros, conditional compilation, preprocessor expansion.
//   * Bitfields, unions, enums.
//   * Variadic functions (`...`).
//   * Multi-dimensional arrays.
//   * `const`/`volatile`/`restrict` qualifiers (silently dropped).
//   * Anonymous nested structs.
//
// Acceptance (issue #324):
//   * Generated output parses and type-checks as Gradient.
//   * Generated struct aliases carry `@repr(C)`.
//   * Generated `extern fn` declarations carry the `!{FFI(C)}` effect
//     (made explicit in source — extern fns are also auto-tagged by the
//     checker, but the explicit form is clearer for the agent and for
//     review).
//   * Round-trip: bindgen output is re-parsed by `Session::from_source`
//     in the unit-test suite and asserted to type-check cleanly.

use crate::project::Project;
use std::fs;
use std::path::PathBuf;
use std::process;

/// Top-level entry point dispatched from `main.rs`.
pub fn execute(header: String, out: Option<String>, module: Option<String>) {
    let header_path = PathBuf::from(&header);
    let source = match fs::read_to_string(&header_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "Error: failed to read header `{}`: {}",
                header_path.display(),
                e
            );
            process::exit(1);
        }
    };

    let module_name = module.unwrap_or_else(|| derive_module_name(&header_path));

    let generated = match generate_bindings(&source, &header, &module_name) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Error: bindgen failed: {}", e);
            process::exit(1);
        }
    };

    match out {
        Some(path) => {
            // If we're in a project, treat relative paths as project-relative.
            let target = if let Ok(project) = Project::find() {
                if PathBuf::from(&path).is_absolute() {
                    PathBuf::from(&path)
                } else {
                    project.root.join(&path)
                }
            } else {
                PathBuf::from(&path)
            };
            if let Some(parent) = target.parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = fs::create_dir_all(parent);
                }
            }
            if let Err(e) = fs::write(&target, &generated) {
                eprintln!("Error: failed to write `{}`: {}", target.display(), e);
                process::exit(1);
            }
        }
        None => {
            print!("{}", generated);
        }
    }
}

/// Derive a default module name from a header filename, e.g. `libc.h` -> `libc`.
fn derive_module_name(header: &std::path::Path) -> String {
    header
        .file_stem()
        .and_then(|s| s.to_str())
        .map(sanitize_ident)
        .unwrap_or_else(|| "bindings".to_string())
}

/// Replace any character that isn't a valid Gradient identifier character
/// with `_`. Leading digit gets prefixed with `_`.
fn sanitize_ident(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    if let Some(first) = chars.next() {
        if first.is_ascii_alphabetic() || first == '_' {
            out.push(first);
        } else {
            out.push('_');
            if first.is_ascii_alphanumeric() {
                out.push(first);
            }
        }
    }
    for c in chars {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "bindings".to_string()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// Public API for tests
// ---------------------------------------------------------------------------

/// Strip C comments and preprocessor lines, returning a cleaned source.
pub fn preprocess(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut at_line_start = true;
    while i < bytes.len() {
        let b = bytes[i];
        // Block comment.
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                if bytes[i] == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        // Line comment.
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Preprocessor line.
        if at_line_start && b == b'#' {
            // Consume entire line (with continuation `\\\n` support).
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'\n' {
                    out.push('\n');
                    i += 1;
                    break;
                }
                i += 1;
            }
            at_line_start = true;
            continue;
        }
        if b == b'\n' {
            at_line_start = true;
        } else if !b.is_ascii_whitespace() {
            at_line_start = false;
        }
        out.push(b as char);
        i += 1;
    }
    out
}

/// A C declaration extracted from the header.
#[derive(Debug, Clone, PartialEq)]
pub enum CDecl {
    /// `typedef <scalar> Name;` — Name aliases a primitive Gradient type.
    TypedefScalar { name: String, gradient_ty: String },
    /// A record-typed C type: `typedef struct { ... } Name;` or
    /// `struct Name { ... };`. Both produce a Gradient `@repr(C)` type
    /// declaration.
    Struct {
        name: String,
        fields: Vec<(String, String)>, // (field name, Gradient type)
    },
    /// `typedef <ret> (*Name)(<params>);` — a callback type. Currently
    /// emitted as a comment-only stub plus an opaque `Int` alias because
    /// Gradient's surface lacks a first-class function pointer alias.
    FnPointerTypedef { name: String, summary: String },
    /// `<ret> name(<params>);` — a free function declaration.
    FnDecl {
        name: String,
        params: Vec<(String, String)>, // (param name, Gradient type)
        ret: String,                   // Gradient return type or "()"
    },
    /// Something the MVP recognized but cannot represent. Emitted as a
    /// commented-out line so the user sees the gap.
    Skipped { source: String, reason: String },
}

/// Resolve a C type token string to a Gradient type, consulting both the
/// builtin scalar map and the user-defined type set discovered in the
/// first pass.
///
/// Resolution order:
///   1. Strip qualifiers and collapse whitespace.
///   2. If the cleaned text is a known user-defined type, return it
///      verbatim (the bindgen output will have emitted a corresponding
///      `type Name = ...` or `@repr(C) type Name:` declaration earlier).
///   3. `T*` where `T` is a known user type — opaque `Int` handle.
///   4. Fall back to `map_c_type` for builtin C scalars.
fn resolve_type(raw: &str, user_types: &std::collections::HashSet<String>) -> Option<String> {
    let cleaned = strip_qualifiers(raw);
    let cleaned = cleaned.trim();

    // Pointer to user type — opaque Int handle.
    if let Some(without_star) = cleaned.strip_suffix('*') {
        let inner = strip_qualifiers(without_star.trim()).trim().to_string();
        if user_types.contains(&inner) {
            return Some("Int".to_string());
        }
        // Fall through to map_c_type so char*/void*/T* handling stays uniform.
    }

    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    // Direct user-type match (e.g. `pid_t`, `Vec3`).
    if user_types.contains(&collapsed) {
        return Some(collapsed);
    }
    // `struct Name` form referencing a known user struct.
    if let Some(name) = collapsed.strip_prefix("struct ") {
        if user_types.contains(name.trim()) {
            return Some(name.trim().to_string());
        }
    }

    map_c_type(raw)
}

/// Map a C scalar type (after qualifier stripping) onto a Gradient type
/// name. Returns `None` if the type is not in the MVP-supported set.
///
/// MVP mapping:
///   * `void`                                                -> `()` (return only)
///   * `bool` / `_Bool`                                      -> `Bool`
///   * `char` / `signed char` / `unsigned char`              -> `Int`
///   * `short` / `int` / `long` / `long long` (+ unsigned/signed) -> `Int`
///   * `int8_t` / `uint8_t` / ... / `int64_t` / `uint64_t`   -> `Int`
///   * `size_t` / `ssize_t` / `intptr_t` / `uintptr_t`       -> `Int`
///   * `float` / `double` / `long double`                    -> `Float`
///   * `char *` / `const char *`                             -> `String`
///   * Other `T *`                                            -> `Int` (opaque handle)
pub fn map_c_type(raw: &str) -> Option<String> {
    let cleaned = strip_qualifiers(raw);
    let cleaned = cleaned.trim();

    // Pointer types: char* -> String, anything else* -> Int handle.
    if let Some(without_star) = cleaned.strip_suffix('*') {
        let inner = strip_qualifiers(without_star.trim()).trim().to_string();
        if inner == "char" || inner == "void" {
            // `void*` and `char*` both map to opaque/handle-style;
            // `char*` is conventionally a NUL-terminated string in C.
            if inner == "char" {
                return Some("String".to_string());
            } else {
                return Some("Int".to_string());
            }
        }
        // `T*` for any other T is an opaque pointer represented as Int.
        return Some("Int".to_string());
    }

    // Collapse internal whitespace runs.
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed.as_str() {
        "void" => Some("()".to_string()),
        "bool" | "_Bool" => Some("Bool".to_string()),
        // Floating-point.
        "float" | "double" | "long double" => Some("Float".to_string()),
        // Character & integer types collapsed to `Int`. Gradient's MVP
        // primitive set only has a 64-bit signed `Int`; finer-grained
        // widths are tracked in a follow-up issue.
        "char" | "signed char" | "unsigned char" => Some("Int".to_string()),
        "short" | "signed short" | "unsigned short" | "short int" | "unsigned short int" => {
            Some("Int".to_string())
        }
        "int" | "signed" | "signed int" | "unsigned" | "unsigned int" => Some("Int".to_string()),
        "long" | "signed long" | "unsigned long" | "long int" | "unsigned long int" => {
            Some("Int".to_string())
        }
        "long long"
        | "signed long long"
        | "unsigned long long"
        | "long long int"
        | "unsigned long long int" => Some("Int".to_string()),
        // Fixed-width stdint.h types.
        "int8_t" | "int16_t" | "int32_t" | "int64_t" | "uint8_t" | "uint16_t" | "uint32_t"
        | "uint64_t" => Some("Int".to_string()),
        "size_t" | "ssize_t" | "intptr_t" | "uintptr_t" | "ptrdiff_t" => Some("Int".to_string()),
        _ => None,
    }
}

/// Drop `const`, `volatile`, `restrict`, `__restrict`, `__restrict__`,
/// `register`, `static`, `inline`, `__inline`, `extern` qualifiers from a
/// C type token string. Returns a new string with the qualifiers gone.
pub fn strip_qualifiers(raw: &str) -> String {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let filtered: Vec<&str> = tokens
        .into_iter()
        .filter(|t| {
            !matches!(
                *t,
                "const"
                    | "volatile"
                    | "restrict"
                    | "__restrict"
                    | "__restrict__"
                    | "register"
                    | "static"
                    | "inline"
                    | "__inline"
                    | "extern"
            )
        })
        .collect();
    filtered.join(" ")
}

/// Parse the preprocessed header into a flat sequence of `CDecl`s.
///
/// The parser is intentionally simple — it walks top-level `;`-terminated
/// statements. Nested braces (struct bodies) are tracked so a statement
/// that contains a brace block is captured atomically.
///
/// Performs two passes:
///   1. Discover user-defined type names (typedef'd or struct'd) so
///      later references resolve.
///   2. Parse each statement into a `CDecl`, threading the known-type
///      set in for resolution.
pub fn parse_header(source: &str) -> Vec<CDecl> {
    let cleaned = preprocess(source);
    let stmts: Vec<String> = split_top_level_statements(&cleaned);

    // Pass 1: collect user-defined type names that the MVP can resolve.
    let mut user_types: std::collections::HashSet<String> = std::collections::HashSet::new();
    for stmt in &stmts {
        let trimmed = stmt.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = collapse_whitespace(trimmed);
        // Scalar typedef: typedef <ty> <name>
        if let Some(rest) = normalized.strip_prefix("typedef ") {
            // Function pointer typedef: typedef <ret> (*Name)(...)
            if rest.contains("(*") {
                if let Some(name) = peek_fn_pointer_name(rest) {
                    user_types.insert(name);
                    continue;
                }
            }
            // Struct typedef: typedef struct ... Name
            if rest.starts_with("struct ") || rest.starts_with("struct{") {
                if let Some(name) = peek_struct_typedef_name(rest) {
                    user_types.insert(name);
                    continue;
                }
            }
            // Scalar typedef: trailing identifier is the alias.
            if let Some(idx) = rest.rfind(|c: char| c.is_ascii_whitespace()) {
                let name = rest[idx..].trim();
                if is_valid_c_ident(name) {
                    user_types.insert(name.to_string());
                }
            }
        } else if normalized.starts_with("struct ") && normalized.contains('{') {
            // Named struct decl: struct Name { ... }
            let after = normalized.trim_start_matches("struct ").trim_start();
            if let Some(open) = after.find('{') {
                let name = after[..open].trim();
                if is_valid_c_ident(name) {
                    user_types.insert(name.to_string());
                }
            }
        }
    }

    let mut decls = Vec::new();
    for stmt in stmts {
        let trimmed = stmt.trim();
        if trimmed.is_empty() {
            continue;
        }
        decls.push(parse_one(trimmed, &user_types));
    }
    decls
}

fn peek_fn_pointer_name(rest: &str) -> Option<String> {
    let lparen = rest.find('(')?;
    let after = &rest[lparen + 1..];
    let after_trim = after.trim_start();
    let after_star = after_trim.strip_prefix('*')?.trim_start();
    let close_name = after_star.find(')')?;
    let name = after_star[..close_name].trim();
    if is_valid_c_ident(name) {
        Some(name.to_string())
    } else {
        None
    }
}

fn peek_struct_typedef_name(rest: &str) -> Option<String> {
    let open = rest.find('{')?;
    let close = find_matching_brace(rest, open)?;
    let tail = rest[close + 1..].trim();
    let name = tail.split_whitespace().next()?;
    let name = name.trim_end_matches(';').trim();
    if is_valid_c_ident(name) {
        Some(name.to_string())
    } else {
        None
    }
}

/// Split a preprocessed C source into top-level `;`-terminated statements,
/// honoring `{ ... }` nesting (so a struct body that spans many lines is
/// one statement).
fn split_top_level_statements(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut depth: i32 = 0;
    let mut paren: i32 = 0;
    for c in source.chars() {
        match c {
            '{' => {
                depth += 1;
                buf.push(c);
            }
            '}' => {
                depth -= 1;
                buf.push(c);
            }
            '(' => {
                paren += 1;
                buf.push(c);
            }
            ')' => {
                paren -= 1;
                buf.push(c);
            }
            ';' if depth == 0 && paren == 0 => {
                out.push(std::mem::take(&mut buf));
            }
            _ => buf.push(c),
        }
    }
    if !buf.trim().is_empty() {
        out.push(buf);
    }
    out
}

/// Parse one top-level C statement. Dispatches by leading keyword.
fn parse_one(stmt: &str, user_types: &std::collections::HashSet<String>) -> CDecl {
    let normalized = collapse_whitespace(stmt);
    if let Some(rest) = normalized.strip_prefix("typedef ") {
        return parse_typedef(rest, stmt, user_types);
    }
    if normalized.starts_with("struct ") && normalized.contains('{') {
        return parse_struct_decl(&normalized, stmt, user_types);
    }
    // Function declaration: <ret> <name>(<params>)
    if normalized.contains('(') && normalized.contains(')') {
        if let Some(fn_decl) = parse_fn_decl(&normalized, stmt, user_types) {
            return fn_decl;
        }
    }
    CDecl::Skipped {
        source: stmt.to_string(),
        reason: "unrecognized top-level construct".to_string(),
    }
}

/// Collapse runs of whitespace to single spaces.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_typedef(
    rest: &str,
    original: &str,
    user_types: &std::collections::HashSet<String>,
) -> CDecl {
    // Function pointer typedef: <ret> (*Name)(<params>)
    if let Some(fp) = parse_fn_pointer_typedef(rest) {
        return fp;
    }
    // Struct typedef: `struct [Tag] { fields } Name`
    if rest.starts_with("struct ") || rest.starts_with("struct{") {
        if let Some(s) = parse_struct_typedef(rest, original, user_types) {
            return s;
        }
    }
    // Scalar typedef: `<type tokens> Name`
    if let Some(idx) = rest.rfind(|c: char| c.is_ascii_whitespace()) {
        let (ty, name) = rest.split_at(idx);
        let name = name.trim();
        if is_valid_c_ident(name) {
            if let Some(gradient_ty) = resolve_type(ty.trim(), user_types) {
                return CDecl::TypedefScalar {
                    name: name.to_string(),
                    gradient_ty,
                };
            }
            return CDecl::Skipped {
                source: original.to_string(),
                reason: format!("unsupported typedef base type `{}`", ty.trim()),
            };
        }
    }
    CDecl::Skipped {
        source: original.to_string(),
        reason: "could not parse typedef shape".to_string(),
    }
}

fn parse_fn_pointer_typedef(rest: &str) -> Option<CDecl> {
    // `<ret> (*Name)(<params>)`
    let lparen = rest.find('(')?;
    let after = &rest[lparen + 1..];
    let after_trim = after.trim_start();
    let after_star = after_trim.strip_prefix('*')?.trim_start();
    let close_name = after_star.find(')')?;
    let name = after_star[..close_name].trim();
    if !is_valid_c_ident(name) {
        return None;
    }
    let ret = rest[..lparen].trim();
    Some(CDecl::FnPointerTypedef {
        name: name.to_string(),
        summary: format!(
            "C function pointer `{} (*{}) (...)`; opaque `Int` handle until function-type aliases land",
            ret, name
        ),
    })
}

fn parse_struct_typedef(
    rest: &str,
    original: &str,
    user_types: &std::collections::HashSet<String>,
) -> Option<CDecl> {
    let open = rest.find('{')?;
    let close = find_matching_brace(rest, open)?;
    let body = &rest[open + 1..close];
    let tail = rest[close + 1..].trim();
    let name = tail.split_whitespace().next()?;
    let name = name.trim_end_matches(';').trim();
    if !is_valid_c_ident(name) {
        return None;
    }
    let fields = parse_struct_fields(body, original, user_types);
    match fields {
        Ok(fields) => Some(CDecl::Struct {
            name: name.to_string(),
            fields,
        }),
        Err(reason) => Some(CDecl::Skipped {
            source: original.to_string(),
            reason,
        }),
    }
}

fn parse_struct_decl(
    normalized: &str,
    original: &str,
    user_types: &std::collections::HashSet<String>,
) -> CDecl {
    // `struct Name { ... }`
    let after_struct = normalized.trim_start_matches("struct ").trim_start();
    let open = match after_struct.find('{') {
        Some(o) => o,
        None => {
            return CDecl::Skipped {
                source: original.to_string(),
                reason: "expected `{` in struct declaration".to_string(),
            };
        }
    };
    let name = after_struct[..open].trim();
    if !is_valid_c_ident(name) {
        return CDecl::Skipped {
            source: original.to_string(),
            reason: format!("invalid struct name `{}`", name),
        };
    }
    let close = match find_matching_brace(after_struct, open) {
        Some(c) => c,
        None => {
            return CDecl::Skipped {
                source: original.to_string(),
                reason: "unmatched `{` in struct declaration".to_string(),
            };
        }
    };
    let body = &after_struct[open + 1..close];
    match parse_struct_fields(body, original, user_types) {
        Ok(fields) => CDecl::Struct {
            name: name.to_string(),
            fields,
        },
        Err(reason) => CDecl::Skipped {
            source: original.to_string(),
            reason,
        },
    }
}

fn parse_struct_fields(
    body: &str,
    _original: &str,
    user_types: &std::collections::HashSet<String>,
) -> Result<Vec<(String, String)>, String> {
    let mut fields = Vec::new();
    for raw in body.split(';') {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Skip nested structs/unions in MVP.
        if trimmed.contains('{') {
            return Err(format!(
                "nested struct/union not supported in MVP: `{}`",
                trimmed
            ));
        }
        // `<type tokens> <field_name>` — optionally with arrays/bitfields.
        // Reject bitfields (contain ':').
        if trimmed.contains(':') {
            return Err(format!("bitfield not supported in MVP: `{}`", trimmed));
        }
        // Drop array dimensions in the MVP and report opaque.
        let (decl_text, _was_array) = match trimmed.find('[') {
            Some(idx) => (trimmed[..idx].trim_end(), true),
            None => (trimmed, false),
        };
        let last_space = match decl_text.rfind(|c: char| c.is_ascii_whitespace() || c == '*') {
            Some(i) => i,
            None => return Err(format!("could not parse field `{}`", trimmed)),
        };
        let (ty_text, name_text) = decl_text.split_at(last_space + 1);
        let name = name_text.trim();
        if !is_valid_c_ident(name) {
            return Err(format!("invalid field name `{}`", name));
        }
        let ty = ty_text.trim();
        let gradient_ty = match resolve_type(ty, user_types) {
            Some(g) => g,
            None => {
                return Err(format!("unsupported field type `{}` for `{}`", ty, name));
            }
        };
        fields.push((name.to_string(), gradient_ty));
    }
    if fields.is_empty() {
        return Err("empty struct body".to_string());
    }
    Ok(fields)
}

fn parse_fn_decl(
    normalized: &str,
    original: &str,
    user_types: &std::collections::HashSet<String>,
) -> Option<CDecl> {
    // The function name is the identifier just before the first `(`.
    let lparen = normalized.find('(')?;
    let head = &normalized[..lparen];
    let rparen = find_matching_paren(normalized, lparen)?;
    if rparen + 1 < normalized.len() && !normalized[rparen + 1..].trim().is_empty() {
        // Trailing content after `)` (e.g. attribute) is not handled.
        return None;
    }
    let head_trim = head.trim_end();
    // Identify name as the trailing identifier in `head`.
    let head_chars: Vec<char> = head_trim.chars().collect();
    let mut end = head_chars.len();
    while end > 0 && (head_chars[end - 1].is_ascii_alphanumeric() || head_chars[end - 1] == '_') {
        end -= 1;
    }
    let name: String = head_chars[end..].iter().collect();
    let name = name.trim().to_string();
    if !is_valid_c_ident(&name) {
        return None;
    }
    let ret_text = head_chars[..end].iter().collect::<String>();
    let ret_text = ret_text.trim();
    let ret_gradient = resolve_type(ret_text, user_types)?;

    let params_text = &normalized[lparen + 1..rparen];
    let params = match parse_params(params_text, user_types) {
        Ok(p) => p,
        Err(reason) => {
            return Some(CDecl::Skipped {
                source: original.to_string(),
                reason,
            });
        }
    };

    Some(CDecl::FnDecl {
        name,
        params,
        ret: ret_gradient,
    })
}

fn parse_params(
    params_text: &str,
    user_types: &std::collections::HashSet<String>,
) -> Result<Vec<(String, String)>, String> {
    let trimmed = params_text.trim();
    if trimmed.is_empty() || trimmed == "void" {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut anon_counter = 0;
    for raw in trimmed.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        if raw == "..." {
            return Err("variadic functions are not supported in the MVP".to_string());
        }
        // Find param name as trailing identifier (or synthesize one).
        let chars: Vec<char> = raw.chars().collect();
        let mut end = chars.len();
        while end > 0 && (chars[end - 1].is_ascii_alphanumeric() || chars[end - 1] == '_') {
            end -= 1;
        }
        let name: String = chars[end..].iter().collect();
        let (param_name, ty_text) = if name.is_empty() || !is_valid_c_ident(&name) {
            anon_counter += 1;
            (format!("arg{}", anon_counter), raw.to_string())
        } else {
            (
                ensure_safe_param_name(name.trim(), anon_counter),
                chars[..end].iter().collect::<String>(),
            )
        };
        let ty_text = ty_text.trim();
        if ty_text.is_empty() {
            return Err(format!("missing type for parameter `{}`", param_name));
        }
        let gradient_ty = resolve_type(ty_text, user_types)
            .ok_or_else(|| format!("unsupported parameter type `{}`", ty_text))?;
        out.push((param_name, gradient_ty));
    }
    Ok(out)
}

/// Rename C parameter names that collide with Gradient reserved keywords.
/// The list mirrors Pitfall 12 of `gradient-project-development`.
fn ensure_safe_param_name(name: &str, anon_counter: usize) -> String {
    const RESERVED: &[&str] = &[
        "tag", "state", "actor", "on", "spawn", "send", "ask", "as", "break", "continue", "defer",
        "extern", "export", "pub", "comptime", "trait", "val", "type", "fn", "let", "mut", "if",
        "else", "while", "for", "ret", "return", "match", "case", "true", "false", "and", "or",
        "not", "mod", "in",
    ];
    if RESERVED.contains(&name) {
        format!("{}_{}", name, anon_counter.max(1))
    } else {
        name.to_string()
    }
}

fn is_valid_c_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn find_matching_brace(s: &str, open: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn find_matching_paren(s: &str, open: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 0;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Gradient emitter
// ---------------------------------------------------------------------------

/// Top-level: parse `source`, produce a Gradient source string.
/// `header_path` and `module_name` only appear in the generated banner.
pub fn generate_bindings(
    source: &str,
    header_path: &str,
    module_name: &str,
) -> Result<String, String> {
    let decls = parse_header(source);
    Ok(emit_gradient(&decls, header_path, module_name))
}

fn emit_gradient(decls: &[CDecl], header_path: &str, module_name: &str) -> String {
    let mut out = String::new();
    out.push_str("// Auto-generated by `gradient bindgen` (E3 #324 MVP).\n");
    out.push_str(&format!("// Source header: {}\n", header_path));
    out.push_str(&format!("// Module: {}\n", module_name));
    out.push_str("// MVP coverage: scalar typedefs, @repr(C) structs, function decls,\n");
    out.push_str("// function-pointer typedefs. Bitfields, unions, macros, and variadic\n");
    out.push_str("// functions are out of scope; see issue #324 and follow-ups.\n");
    out.push_str("//\n");
    out.push_str("// Integer widths: the MVP collapses every C integer type onto Gradient's\n");
    out.push_str("// 64-bit `Int`. Floating-point types collapse onto `Float`. `char*` maps\n");
    out.push_str("// to `String`; any other `T*` maps to an opaque `Int` handle. `void` is\n");
    out.push_str("// `()` only in return position.\n\n");

    let mut emitted_any = false;
    for decl in decls {
        match decl {
            CDecl::TypedefScalar { name, gradient_ty } => {
                out.push_str(&format!("type {} = {}\n\n", name, gradient_ty));
                emitted_any = true;
            }
            CDecl::Struct { name, fields } => {
                out.push_str("@repr(C)\n");
                out.push_str(&format!("type {}:\n", name));
                for (field, ty) in fields {
                    out.push_str(&format!("    {}: {}\n", field, ty));
                }
                out.push('\n');
                emitted_any = true;
            }
            CDecl::FnPointerTypedef { name, summary } => {
                out.push_str(&format!("// {}\n", summary));
                out.push_str(&format!("type {} = Int\n\n", name));
                emitted_any = true;
            }
            CDecl::FnDecl { name, params, ret } => {
                out.push_str("@extern\n");
                let params_rendered = params
                    .iter()
                    .map(|(n, t)| format!("{}: {}", n, t))
                    .collect::<Vec<_>>()
                    .join(", ");
                if ret == "()" {
                    out.push_str(&format!(
                        "fn {}({}) -> !{{FFI(C)}} ()\n\n",
                        name, params_rendered
                    ));
                } else {
                    out.push_str(&format!(
                        "fn {}({}) -> !{{FFI(C)}} {}\n\n",
                        name, params_rendered, ret
                    ));
                }
                emitted_any = true;
            }
            CDecl::Skipped { source, reason } => {
                out.push_str(&format!("// SKIPPED ({}):\n", reason));
                for line in source.lines() {
                    out.push_str(&format!("//   {}\n", line));
                }
                out.push('\n');
            }
        }
    }

    if !emitted_any {
        out.push_str("// (no recognized declarations in this header)\n");
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_strips_line_comments() {
        let src = "int x; // a comment\nint y;";
        let out = preprocess(src);
        assert!(!out.contains("a comment"));
        assert!(out.contains("int x;"));
        assert!(out.contains("int y;"));
    }

    #[test]
    fn preprocess_strips_block_comments() {
        let src = "int x; /* block\n comment */ int y;";
        let out = preprocess(src);
        assert!(!out.contains("block"));
        assert!(!out.contains("comment"));
    }

    #[test]
    fn preprocess_skips_preprocessor_lines() {
        let src = "#include <stdio.h>\n#define FOO 1\nint x;";
        let out = preprocess(src);
        assert!(!out.contains("#include"));
        assert!(!out.contains("#define"));
        assert!(out.contains("int x;"));
    }

    #[test]
    fn preprocess_handles_line_continuation_in_preprocessor() {
        let src = "#define FOO \\\n   1\nint x;";
        let out = preprocess(src);
        assert!(!out.contains("#define"));
        assert!(out.contains("int x;"));
    }

    #[test]
    fn map_scalar_types() {
        assert_eq!(map_c_type("int"), Some("Int".to_string()));
        assert_eq!(map_c_type("unsigned int"), Some("Int".to_string()));
        assert_eq!(map_c_type("long long"), Some("Int".to_string()));
        assert_eq!(map_c_type("float"), Some("Float".to_string()));
        assert_eq!(map_c_type("double"), Some("Float".to_string()));
        assert_eq!(map_c_type("_Bool"), Some("Bool".to_string()));
        assert_eq!(map_c_type("bool"), Some("Bool".to_string()));
        assert_eq!(map_c_type("void"), Some("()".to_string()));
        assert_eq!(map_c_type("size_t"), Some("Int".to_string()));
        assert_eq!(map_c_type("int32_t"), Some("Int".to_string()));
        assert_eq!(map_c_type("uint64_t"), Some("Int".to_string()));
    }

    #[test]
    fn map_pointer_types() {
        assert_eq!(map_c_type("char *"), Some("String".to_string()));
        assert_eq!(map_c_type("const char *"), Some("String".to_string()));
        assert_eq!(map_c_type("void *"), Some("Int".to_string()));
        assert_eq!(map_c_type("FILE *"), Some("Int".to_string()));
        assert_eq!(map_c_type("int *"), Some("Int".to_string()));
    }

    #[test]
    fn map_unsupported_type_returns_none() {
        // No support for arbitrary user types as values yet.
        assert_eq!(map_c_type("struct foo"), None);
    }

    #[test]
    fn strip_qualifiers_drops_common_modifiers() {
        assert_eq!(strip_qualifiers("const int"), "int");
        assert_eq!(strip_qualifiers("volatile unsigned int"), "unsigned int");
        assert_eq!(strip_qualifiers("static inline int"), "int");
    }

    #[test]
    fn parse_simple_scalar_typedef() {
        let src = "typedef int my_int_t;";
        let decls = parse_header(src);
        assert_eq!(decls.len(), 1);
        match &decls[0] {
            CDecl::TypedefScalar { name, gradient_ty } => {
                assert_eq!(name, "my_int_t");
                assert_eq!(gradient_ty, "Int");
            }
            other => panic!("expected TypedefScalar, got {:?}", other),
        }
    }

    #[test]
    fn parse_function_decl_no_params() {
        let src = "int getpid(void);";
        let decls = parse_header(src);
        assert_eq!(decls.len(), 1);
        match &decls[0] {
            CDecl::FnDecl { name, params, ret } => {
                assert_eq!(name, "getpid");
                assert!(params.is_empty());
                assert_eq!(ret, "Int");
            }
            other => panic!("expected FnDecl, got {:?}", other),
        }
    }

    #[test]
    fn parse_function_decl_with_params() {
        let src = "int add(int a, int b);";
        let decls = parse_header(src);
        assert_eq!(decls.len(), 1);
        match &decls[0] {
            CDecl::FnDecl { name, params, ret } => {
                assert_eq!(name, "add");
                assert_eq!(params.len(), 2);
                assert_eq!(params[0], ("a".to_string(), "Int".to_string()));
                assert_eq!(params[1], ("b".to_string(), "Int".to_string()));
                assert_eq!(ret, "Int");
            }
            other => panic!("expected FnDecl, got {:?}", other),
        }
    }

    #[test]
    fn parse_function_decl_string_arg() {
        let src = "int puts(const char *s);";
        let decls = parse_header(src);
        match &decls[0] {
            CDecl::FnDecl { name, params, ret } => {
                assert_eq!(name, "puts");
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].0, "s");
                assert_eq!(params[0].1, "String");
                assert_eq!(ret, "Int");
            }
            other => panic!("expected FnDecl, got {:?}", other),
        }
    }

    #[test]
    fn parse_function_decl_void_return() {
        let src = "void exit(int code);";
        let decls = parse_header(src);
        match &decls[0] {
            CDecl::FnDecl { name, params, ret } => {
                assert_eq!(name, "exit");
                assert_eq!(ret, "()");
                assert_eq!(params[0], ("code".to_string(), "Int".to_string()));
            }
            other => panic!("expected FnDecl, got {:?}", other),
        }
    }

    #[test]
    fn parse_struct_typedef() {
        let src = "typedef struct { int x; double y; } Point;";
        let decls = parse_header(src);
        assert_eq!(decls.len(), 1);
        match &decls[0] {
            CDecl::Struct { name, fields } => {
                assert_eq!(name, "Point");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0], ("x".to_string(), "Int".to_string()));
                assert_eq!(fields[1], ("y".to_string(), "Float".to_string()));
            }
            other => panic!("expected Struct, got {:?}", other),
        }
    }

    #[test]
    fn parse_named_struct_decl() {
        let src = "struct Vec3 { float x; float y; float z; };";
        let decls = parse_header(src);
        assert_eq!(decls.len(), 1);
        match &decls[0] {
            CDecl::Struct { name, fields } => {
                assert_eq!(name, "Vec3");
                assert_eq!(fields.len(), 3);
                for (i, field_name) in ["x", "y", "z"].iter().enumerate() {
                    assert_eq!(fields[i].0, *field_name);
                    assert_eq!(fields[i].1, "Float");
                }
            }
            other => panic!("expected Struct, got {:?}", other),
        }
    }

    #[test]
    fn parse_fn_pointer_typedef() {
        let src = "typedef int (*Callback)(int, int);";
        let decls = parse_header(src);
        assert_eq!(decls.len(), 1);
        match &decls[0] {
            CDecl::FnPointerTypedef { name, .. } => {
                assert_eq!(name, "Callback");
            }
            other => panic!("expected FnPointerTypedef, got {:?}", other),
        }
    }

    #[test]
    fn variadic_fn_is_skipped() {
        let src = "int printf(const char *fmt, ...);";
        let decls = parse_header(src);
        assert_eq!(decls.len(), 1);
        match &decls[0] {
            CDecl::Skipped { reason, .. } => {
                assert!(reason.contains("variadic"));
            }
            other => panic!("expected Skipped, got {:?}", other),
        }
    }

    #[test]
    fn reserved_param_name_is_renamed() {
        let src = "int f(int type);";
        let decls = parse_header(src);
        match &decls[0] {
            CDecl::FnDecl { params, .. } => {
                assert_ne!(params[0].0, "type", "reserved param name should be renamed");
            }
            other => panic!("expected FnDecl, got {:?}", other),
        }
    }

    #[test]
    fn emit_struct_carries_repr_c() {
        let src = "typedef struct { int x; int y; } Pair;";
        let out = generate_bindings(src, "test.h", "test").unwrap();
        assert!(out.contains("@repr(C)"), "output:\n{}", out);
        assert!(out.contains("type Pair:"));
        assert!(out.contains("x: Int"));
        assert!(out.contains("y: Int"));
    }

    #[test]
    fn emit_fn_carries_ffi_c_effect() {
        let src = "int add(int a, int b);";
        let out = generate_bindings(src, "test.h", "test").unwrap();
        assert!(out.contains("@extern"));
        assert!(out.contains("!{FFI(C)}"), "output:\n{}", out);
        assert!(out.contains("fn add(a: Int, b: Int)"));
    }

    #[test]
    fn emit_void_return_renders_as_unit() {
        let src = "void noop(void);";
        let out = generate_bindings(src, "test.h", "test").unwrap();
        assert!(
            out.contains("fn noop() -> !{FFI(C)} ()"),
            "output:\n{}",
            out
        );
    }

    #[test]
    fn emit_header_banner_present() {
        let src = "int x(void);";
        let out = generate_bindings(src, "libc.h", "libc").unwrap();
        assert!(out.contains("Auto-generated by `gradient bindgen`"));
        assert!(out.contains("Source header: libc.h"));
        assert!(out.contains("Module: libc"));
    }

    // -----------------------------------------------------------------
    // Round-trip acceptance — issue #324 acceptance item 4.
    // -----------------------------------------------------------------

    /// Parse + type-check the generated Gradient source via the
    /// compiler's `Session::from_source`. This is the canonical
    /// acceptance check: the output of `gradient bindgen` must produce
    /// valid Gradient.
    fn assert_roundtrip_typechecks(c_source: &str) {
        let gradient = generate_bindings(c_source, "test.h", "test").unwrap();
        let session = gradient_compiler::query::Session::from_source(&gradient);
        let result = session.check();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.severity == gradient_compiler::query::Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "generated source has errors:\n--- source ---\n{}\n--- errors ---\n{:#?}",
            gradient,
            errors
        );
    }

    #[test]
    fn roundtrip_scalar_typedef() {
        assert_roundtrip_typechecks("typedef int my_int_t;");
    }

    #[test]
    fn roundtrip_fn_decl() {
        assert_roundtrip_typechecks("int add(int a, int b);");
    }

    #[test]
    fn roundtrip_struct_with_repr_c() {
        assert_roundtrip_typechecks("typedef struct { int x; double y; } Point;");
    }

    #[test]
    fn roundtrip_fn_with_string_param() {
        assert_roundtrip_typechecks("int puts(const char *s);");
    }

    #[test]
    fn roundtrip_void_return() {
        assert_roundtrip_typechecks("void exit(int code);");
    }

    #[test]
    fn roundtrip_mixed_libc_subset() {
        let src = r#"
// A small libc-like header.
#include <stddef.h>

typedef int pid_t;
typedef unsigned long size_t;

struct timespec {
    long tv_sec;
    long tv_nsec;
};

pid_t getpid(void);
int puts(const char *s);
size_t strlen(const char *s);
void exit(int code);
"#;
        assert_roundtrip_typechecks(src);
    }
}
