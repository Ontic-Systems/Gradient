//! Module-level AST nodes for the Gradient language.
//!
//! A [`Module`] is the root node of every parsed Gradient source file. It
//! contains an optional module declaration, a list of use (import)
//! declarations, and the top-level items that make up the module's body.

use super::item::Item;
use super::span::Span;

/// The trust posture for a module (#360).
///
/// Each Gradient source file is implicitly `Trusted` unless it is
/// annotated with the module-level `@untrusted` attribute (or the
/// workspace default has been flipped — see #359). Untrusted modules
/// are produced by AI agents from external prompts and must not have
/// the same compile-time superpowers as human-authored code.
///
/// **Restrictions enforced on `@untrusted` modules** (addresses the related finding input
/// surface for #360):
///
/// 1. No comptime evaluation (`comptime { ... }` blocks rejected).
/// 2. No FFI (`@extern` rejected).
/// 3. Effects must be explicit on every function (no `effect_set: None`
///    inference at the function-signature level).
/// 4. No type / effect inference at module boundaries: every public
///    item must have an explicit type annotation.
///
/// See also: `docs/security/untrusted-source-mode.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustMode {
    /// Default. Full language available — comptime, FFI, inference, etc.
    #[default]
    Trusted,
    /// `@untrusted` module — restricted to a safe subset.
    Untrusted,
}

impl TrustMode {
    /// True iff this module is in `@untrusted` mode.
    pub fn is_untrusted(self) -> bool {
        matches!(self, TrustMode::Untrusted)
    }

    /// String form for diagnostics ("trusted" / "untrusted").
    pub fn as_str(self) -> &'static str {
        match self {
            TrustMode::Trusted => "trusted",
            TrustMode::Untrusted => "untrusted",
        }
    }
}

/// Module-level panic strategy (#318).
///
/// Per locked-design Q14, every Gradient module declares (or defaults) a
/// strategy for unrecoverable failures. The strategy is set by a top-of-file
/// `@panic(abort)` / `@panic(unwind)` / `@panic(none)` attribute. Default is
/// `Unwind`.
///
/// **Semantics:**
/// - `Abort` — on panic, terminate the process immediately. No landing pads,
///   no destructors. Smallest binaries; no recovery surface. Used for kernel
///   / `no_std` / `@system` agent code.
/// - `Unwind` — on panic, unwind the stack, running destructors and giving
///   `!{Throws(E)}` a place to land. Default for `@app` mode.
/// - `None` — the checker rejects any panic-able operation at compile time.
///   No runtime panic surface remains. Strictest tier for verified /
///   safety-critical code.
///
/// Operations the checker rejects under `@panic(none)`:
/// - Integer division (`/`) — panics on divide-by-zero.
/// - Integer modulo (`%`) — panics on modulo-by-zero.
/// - Array / list / string indexing — panics on out-of-bounds.
///
/// Codegen consequences of `Abort` vs `Unwind` are deferred to the ADR-0005
/// runtime-linker work (Epic #298); this attribute drives the checker pass
/// today and feeds the linker DCE later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanicStrategy {
    /// `@panic(abort)` — process terminates on panic. No landing pads.
    Abort,
    /// `@panic(unwind)` — stack unwinds on panic. Default.
    #[default]
    Unwind,
    /// `@panic(none)` — checker rejects any panic-able op at compile time.
    None,
}

impl PanicStrategy {
    /// String form for diagnostics ("abort" / "unwind" / "none").
    pub fn as_str(self) -> &'static str {
        match self {
            PanicStrategy::Abort => "abort",
            PanicStrategy::Unwind => "unwind",
            PanicStrategy::None => "none",
        }
    }

    /// True iff the checker should reject panic-able ops in this mode.
    pub fn forbids_panicking_ops(self) -> bool {
        matches!(self, PanicStrategy::None)
    }
}

/// Module-level allocator strategy (#336, ADR 0005).
///
/// Per locked-design Q12 + Q2 (effect-gated allocation), the runtime
/// closure is modular and the allocator is one of those modules. This
/// attribute selects WHICH allocator implementation gets linked into
/// the final binary. Default is `Default` (the system allocator wrapped
/// behind `__gradient_alloc` / `__gradient_free`).
///
/// **Semantics:**
/// - `Default` — the runtime ships a system-allocator-backed allocator
///   built on top of `malloc(3)` / `free(3)`. No vtable indirection;
///   the `__gradient_alloc(size)` and `__gradient_free(ptr)` symbols
///   are direct wrappers. Suitable for ordinary host programs.
/// - `Pluggable` — the runtime declares the `__gradient_alloc` /
///   `__gradient_free` symbols as `extern` references that the
///   embedder MUST resolve at link time. Suitable for `no_std` builds,
///   embedded targets, or apps that want to plug a bumpalo-style arena
///   or slab allocator under the same C ABI vtable. The `Allocator`
///   trait surface in C is the pair of those two symbols (size_t-in,
///   void*-out for alloc; void* for free) — see
///   `codebase/compiler/runtime/allocator/README.md`.
/// - `@allocator(arena)` — process-global bump-pointer arena allocator
///   wired up by the runtime crate itself (no embedder-supplied vtable).
///   Backed by `codebase/runtime/memory/arena.{c,h}` (~270 LOC of
///   bump-pointer chunks with checked-arithmetic helpers). The
///   allocator is a single global `Arena*` initialised on the first
///   `__gradient_alloc` call and freed by an `atexit` hook. Frees are
///   no-ops — bulk reclamation happens at process exit. This is the
///   first concrete `pluggable`-class implementation under the same
///   C ABI (#336 follow-on) and the runtime-side beachhead for the
///   capability + arena memory work tracked by E3 #320.
///
/// Selected by a top-of-file `@allocator(default | pluggable | arena)`
/// attribute. This is an attribute-driven axis (deployment decision),
/// NOT effect-driven — the effect surface alone can't tell us whether
/// the embedder is providing an allocator.
///
/// Codegen emits the same `__gradient_alloc` / `__gradient_free` calls
/// regardless of strategy; the runtime crate selection at link time
/// decides which body those calls resolve to. See
/// `codebase/compiler/runtime/allocator/README.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AllocatorStrategy {
    /// `@allocator(default)` — system allocator via `malloc`/`free`.
    /// Default.
    #[default]
    Default,
    /// `@allocator(pluggable)` — embedder supplies `__gradient_alloc` /
    /// `__gradient_free` at link time. Used by `no_std` and embedded
    /// targets that need a bumpalo arena, slab, or other custom
    /// allocator under the same C ABI.
    Pluggable,
    /// `@allocator(arena)` — process-global bump-pointer arena
    /// allocator supplied by `runtime_allocator_arena.c`. No embedder
    /// vtable required. Frees are no-ops; the entire arena is
    /// reclaimed at process exit via an `atexit` hook. First
    /// concrete `pluggable`-class implementation; closes the
    /// runtime-crate half of E3 #320.
    Arena,
    /// `@allocator(slab)` — fixed-size-class slab allocator supplied
    /// by `runtime_allocator_slab.c`. No embedder vtable required.
    /// Small allocations (≤128 B) are served from per-class free
    /// lists; larger allocations fall through to libc `malloc`.
    /// Frees on small-class pointers return them to the free list;
    /// frees on large-class pointers go to libc `free`. Sibling of
    /// `arena` under the same `@allocator(...)` axis (#545).
    Slab,
    /// `@allocator(bumpalo)` — multi-chunk bump-arena allocator
    /// supplied by `runtime_allocator_bumpalo.c`. Inspired by the
    /// bumpalo Rust crate. Allocations bump from the tail of the
    /// current chunk; when a chunk is exhausted, a new (larger)
    /// chunk is allocated and chained. Frees are no-ops; the
    /// entire chain is reclaimed at process exit via an `atexit`
    /// hook. Unlike `arena` (which logically grows a single
    /// region), `bumpalo` keeps every previously-returned pointer
    /// stable across allocations — chunks never relocate. No
    /// embedder vtable required. Sibling of `arena` and `slab`
    /// under the same `@allocator(...)` axis (#547).
    Bumpalo,
}

impl AllocatorStrategy {
    /// String form for diagnostics and Query API output
    /// (`"default"` / `"pluggable"` / `"arena"` / `"slab"` /
    /// `"bumpalo"`).
    pub fn as_str(self) -> &'static str {
        match self {
            AllocatorStrategy::Default => "default",
            AllocatorStrategy::Pluggable => "pluggable",
            AllocatorStrategy::Arena => "arena",
            AllocatorStrategy::Slab => "slab",
            AllocatorStrategy::Bumpalo => "bumpalo",
        }
    }
}

/// Module-level mode (#352, Epic #301 inference).
///
/// Per locked-design Q9 (ergonomics), every Gradient module is either in
/// `@app` mode (default — inference everywhere except where the public
/// surface forces explicit annotation) or `@system` mode (explicit
/// everywhere — every function must declare its return type AND its
/// effect set, even locals).
///
/// `@app` is the default for new modules and matches the inference-first
/// posture every existing test fixture has been written under.
///
/// `@system` is the explicit-everywhere mode chosen by kernel modules,
/// stdlib `core`/`alloc` modules, and any code where an effect/return-type
/// inference surprise would be a real bug. Self-hosted compiler modules
/// are slated to flip to `@system` in a follow-on dogfood PR (#352
/// acceptance bullet 4) once the launch tier is in place.
///
/// **Restrictions enforced on `@system` modules:**
///
/// 1. Every `FnDef` must declare an explicit return type (`fn foo(...) -> T:`,
///    not bare `fn foo(...):`).
/// 2. Every `FnDef` must declare an explicit effect set (`-> !{IO} T`,
///    not bare `-> T` or omitted).
///
/// These rules are checker-enforced at module check time; the parser
/// accepts both annotated and unannotated forms regardless of mode.
///
/// `@untrusted` (#360) is a stricter superset of `@system` — it also
/// bans comptime, FFI, etc. The two attributes compose: a module can be
/// both `@untrusted` and `@system` (or `@app`); the checker runs the
/// `@untrusted` pass on top of the `@system` pass and surfaces any
/// violations it finds.
///
/// See also: `docs/roadmap.md` § Epic #301, `docs/language-guide.md`
/// § "Module Attributes".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModuleMode {
    /// `@app` — inference everywhere; checker performs no
    /// per-fn explicit-annotation pass. Default.
    #[default]
    App,
    /// `@system` — explicit everywhere; checker rejects any
    /// `FnDef` that omits its return type or effect set.
    System,
}

impl ModuleMode {
    /// String form for diagnostics and Query API output (`"app"` /
    /// `"system"`).
    pub fn as_str(self) -> &'static str {
        match self {
            ModuleMode::App => "app",
            ModuleMode::System => "system",
        }
    }

    /// True iff the checker should reject `FnDef`s that omit their
    /// return type or effect set.
    pub fn requires_explicit_signatures(self) -> bool {
        matches!(self, ModuleMode::System)
    }
}

/// The **declared maximum stdlib tier** for a module (#348, ADR 0005).
///
/// Per ADR 0005 the runtime tier of any function is derived from its
/// effect closure (see [`crate::typechecker::stdlib_tier`]). A module
/// may *additionally* declare an upper bound on the tier of any function
/// it defines or transitively calls via a top-of-file attribute:
///
/// - `@no_std` — every function in this module must classify at
///   [`StdlibTier::Core`]. Calling any builtin whose tier is `Alloc`
///   or `Std` is a compile error. Maps to `Some(StdlibTier::Core)`.
/// - (future) `@no_alloc` — every function must classify at
///   `StdlibTier::Alloc` or below. Will map to
///   `Some(StdlibTier::Alloc)`.
///
/// `None` means no declared upper bound — the module accepts every
/// tier and the checker performs no tier-rejection pass.
///
/// Once #348 lands, `Some(StdlibTier::Core)` activates the rejection
/// rule:
///
/// ```text
/// error: this call to `int_to_string` requires !{Heap}; module is declared @no_std
///   --> src/parser.gr:42:5
///    |
/// 42 |     let s = int_to_string(value)
///    |             ^^^^^^^^^^^^^ tier `alloc` exceeds module-declared `core`
/// ```
///
/// Field lives on [`Module`] alongside [`TrustMode`] / [`PanicStrategy`].
pub type DeclaredTierCeiling = Option<crate::typechecker::stdlib_tier::StdlibTier>;

/// The root AST node for a single Gradient source file.
///
/// Corresponds to the grammar rule:
/// ```text
/// program → module_decl? use_decl* top_item*
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// An optional `mod` declaration that names this module, e.g.
    /// `mod std.io`.
    pub module_decl: Option<ModuleDecl>,
    /// Zero or more `use` declarations that import names from other modules.
    pub uses: Vec<UseDecl>,
    /// The top-level items (functions, type declarations, let bindings, etc.)
    /// defined in this module.
    pub items: Vec<Item>,
    /// The span covering the entire source file.
    pub span: Span,
    /// Trust posture (#360). `Trusted` by default; flipped to
    /// `Untrusted` by a top-of-file `@untrusted` attribute.
    pub trust: TrustMode,
    /// Panic strategy (#318). `Unwind` by default; flipped by a
    /// top-of-file `@panic(abort)` / `@panic(unwind)` / `@panic(none)`
    /// attribute.
    pub panic_strategy: PanicStrategy,
    /// Declared maximum stdlib tier for this module (#348, ADR 0005).
    ///
    /// `None` means no declaration — the module accepts every tier and
    /// the checker performs no tier-rejection pass.
    /// `Some(StdlibTier::Core)` is the `@no_std` mode and rejects any
    /// call whose classified tier is `Alloc` or `Std`.
    pub declared_tier_ceiling: DeclaredTierCeiling,
    /// Allocator strategy (#336, ADR 0005). `Default` by default;
    /// flipped by a top-of-file `@allocator(default | pluggable)`
    /// attribute. Selects which allocator runtime crate is linked at
    /// build time — `runtime_allocator_default.c` (system malloc) or
    /// `runtime_allocator_pluggable.c` (embedder-supplied vtable).
    pub allocator_strategy: AllocatorStrategy,
    /// Module mode (#352, Epic #301). `App` by default; flipped to
    /// `System` by a top-of-file `@system` attribute (or set explicitly
    /// to `App` by `@app`). Under `System`, every `FnDef` must declare
    /// an explicit return type AND an explicit effect set.
    pub mode: ModuleMode,
}

/// How a module was imported - by name or by file path.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportKind {
    /// Import by module path, e.g. `use std.io` or `use math.utils`
    ModulePath(Vec<String>),
    /// Import by file path, e.g. `use "./token.gr"` or `use "../lib/helper.gr"`
    FilePath(String),
}

/// A module declaration at the top of a source file.
///
/// Corresponds to the grammar rule:
/// ```text
/// module_decl → `mod` module_path
/// module_path → IDENT (`.` IDENT)*
/// ```
///
/// For example, `mod std.collections.list` would be represented with
/// `path: vec!["std", "collections", "list"]`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDecl {
    /// The segments of the module path, e.g. `["std", "io"]` for `mod std.io`.
    pub path: Vec<String>,
    /// The span covering the entire `mod` declaration.
    pub span: Span,
}

/// A use (import) declaration.
///
/// Corresponds to the grammar rule:
/// ```text
/// use_decl → `use` (module_path | file_path) (`.` `{` use_list `}`)?
/// file_path → STRING_LITERAL
/// ```
///
/// Examples:
/// - `use std.io` imports the module `std.io` as a whole
///   (`import: ImportKind::ModulePath(["std", "io"]), specific_imports: None`).
/// - `use std.io.{read, write}` imports specific names from `std.io`
///   (`import: ImportKind::ModulePath(["std", "io"]), specific_imports: Some(["read", "write"])`).
/// - `use "./token.gr"` imports from a file path
///   (`import: ImportKind::FilePath("./token.gr"), specific_imports: None`).
#[derive(Debug, Clone, PartialEq)]
pub struct UseDecl {
    /// How this module is being imported - by module path or file path.
    pub import: ImportKind,
    /// If the import uses the `{ name1, name2 }` syntax, this holds the
    /// list of specific names being imported. `None` means the entire
    /// module is imported.
    pub specific_imports: Option<Vec<String>>,
    /// The span covering the entire `use` declaration.
    pub span: Span,
}

impl UseDecl {
    /// Get the module name for this import (for lookups and identification).
    /// For module paths, returns the last segment. For file paths, returns
    /// the filename without extension.
    pub fn module_name(&self) -> String {
        match &self.import {
            ImportKind::ModulePath(path) => path.last().cloned().unwrap_or_default(),
            ImportKind::FilePath(path) => std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("module")
                .to_string(),
        }
    }

    /// Get the full import path as a string for error messages.
    pub fn import_path_string(&self) -> String {
        match &self.import {
            ImportKind::ModulePath(path) => path.join("."),
            ImportKind::FilePath(path) => path.clone(),
        }
    }
}
