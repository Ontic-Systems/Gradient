//! Issue #225: runtime-backed type-environment storage for the self-hosted
//! checker.
//!
//! The self-hosted checker (`compiler/checker.gr`) maintains lexical
//! variable / function environments while it walks parser-owned AST nodes
//! (#222 / #239). Until #225 these were placeholders — `lookup_var` always
//! reported "not found", `insert_var` was the identity, and `check_stmt`
//! dispatched on `IntLitKind(0)` regardless of input.
//!
//! This module mirrors `bootstrap_ast_bridge.rs`: a process-wide store
//! reached through FFI-shaped `bootstrap_checker_env_*` free functions
//! that the .gr source declares as Phase 0 externs and the Rust host
//! drives directly from parity tests.
//!
//! Records (vars and fns) are *immutable*: each `insert` allocates a new
//! environment frame whose sole entry is the new record and whose
//! `parent` points at the previous frame. Lookups walk the parent chain.
//! This shape lets the .gr code keep its `TypeEnv` value-semantics while
//! actually moving real bindings through the runtime store.

use std::sync::{Mutex, MutexGuard, OnceLock};

/// A variable record stored in the checker's runtime environment.
#[derive(Debug, Clone, Default)]
pub struct VarRecord {
    pub name: String,
    pub type_tag: i64,
    pub type_name: String,
    pub is_mut: i64,
    pub scope_level: i64,
}

/// A function record stored in the checker's runtime environment.
#[derive(Debug, Clone, Default)]
pub struct FnRecord {
    pub name: String,
    pub params_handle: i64,
    pub ret_type_tag: i64,
    pub ret_type_name: String,
    pub effects_handle: i64,
    pub is_extern: i64,
}

/// A single environment frame: at most one var binding plus at most one
/// fn binding plus a parent pointer back to the enclosing frame.
///
/// The 0 frame is an always-empty sentinel used as the root parent.
#[derive(Debug, Clone, Default)]
struct EnvFrame {
    parent: i64,
    scope_level: i64,
    /// Index into `vars`, or 0 if this frame doesn't introduce a var.
    var_id: i64,
    /// Index into `fns`, or 0 if this frame doesn't introduce a fn.
    fn_id: i64,
}

/// Process-wide runtime backing for the self-hosted checker's
/// environments and records.
#[derive(Debug, Default)]
pub struct BootstrapCheckerEnvStore {
    /// Index 0 is reserved as a sentinel root frame so that `parent: 0`
    /// always means "no parent". Real frames start at index 1.
    frames: Vec<EnvFrame>,
    /// Index 0 is reserved as the sentinel "no var" record.
    vars: Vec<VarRecord>,
    /// Index 0 is reserved as the sentinel "no fn" record.
    fns: Vec<FnRecord>,
}

impl BootstrapCheckerEnvStore {
    fn new() -> Self {
        Self {
            frames: vec![EnvFrame::default()],
            vars: vec![VarRecord::default()],
            fns: vec![FnRecord::default()],
        }
    }

    fn alloc_env(&mut self, parent: i64, scope_level: i64) -> i64 {
        let id = self.frames.len() as i64;
        self.frames.push(EnvFrame {
            parent,
            scope_level,
            var_id: 0,
            fn_id: 0,
        });
        id
    }

    fn alloc_var(&mut self, rec: VarRecord) -> i64 {
        let id = self.vars.len() as i64;
        self.vars.push(rec);
        id
    }

    fn alloc_fn(&mut self, rec: FnRecord) -> i64 {
        let id = self.fns.len() as i64;
        self.fns.push(rec);
        id
    }

    fn frame(&self, id: i64) -> Option<&EnvFrame> {
        let idx = usize::try_from(id).ok()?;
        self.frames.get(idx)
    }

    fn var(&self, id: i64) -> Option<&VarRecord> {
        let idx = usize::try_from(id).ok()?;
        if idx == 0 {
            return None;
        }
        self.vars.get(idx)
    }

    fn fn_rec(&self, id: i64) -> Option<&FnRecord> {
        let idx = usize::try_from(id).ok()?;
        if idx == 0 {
            return None;
        }
        self.fns.get(idx)
    }

    fn lookup_var(&self, env_id: i64, name: &str) -> i64 {
        let mut cur = env_id;
        // Walk parent chain. 0 is the sentinel root; abort there.
        while cur > 0 {
            let Some(frame) = self.frame(cur) else {
                return 0;
            };
            if frame.var_id != 0 {
                if let Some(v) = self.var(frame.var_id) {
                    if v.name == name {
                        return frame.var_id;
                    }
                }
            }
            cur = frame.parent;
        }
        0
    }

    fn lookup_fn(&self, env_id: i64, name: &str) -> i64 {
        let mut cur = env_id;
        while cur > 0 {
            let Some(frame) = self.frame(cur) else {
                return 0;
            };
            if frame.fn_id != 0 {
                if let Some(f) = self.fn_rec(frame.fn_id) {
                    if f.name == name {
                        return frame.fn_id;
                    }
                }
            }
            cur = frame.parent;
        }
        0
    }
}

fn store() -> &'static Mutex<BootstrapCheckerEnvStore> {
    static STORE: OnceLock<Mutex<BootstrapCheckerEnvStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(BootstrapCheckerEnvStore::new()))
}

fn lock() -> MutexGuard<'static, BootstrapCheckerEnvStore> {
    store().lock().unwrap_or_else(|p| p.into_inner())
}

/// Reset the ambient checker-env store. Tests that drive the bridge
/// must call this before running so they see a clean slate.
pub fn reset_checker_env_store() {
    let mut s = lock();
    *s = BootstrapCheckerEnvStore::new();
}

/// Run a closure with mutable access to the ambient store.
pub fn with_checker_env_store<R>(f: impl FnOnce(&mut BootstrapCheckerEnvStore) -> R) -> R {
    let mut s = lock();
    f(&mut s)
}

/// Run a closure with shared access to the ambient store.
pub fn with_checker_env_store_ref<R>(f: impl FnOnce(&BootstrapCheckerEnvStore) -> R) -> R {
    let s = lock();
    f(&s)
}

// ── FFI-shaped free functions ────────────────────────────────────────────
// These are what the self-hosted checker.gr declares as Phase 0 externs.
// All of them swallow out-of-range / sentinel ids and return safe
// defaults (0 / empty) so that checker walks never panic on bad input.

/// Allocate a fresh empty environment frame whose parent is `parent`
/// and whose recorded scope level is `scope_level`. Returns the new
/// frame id (always > 0).
pub fn bootstrap_checker_env_alloc(parent: i64, scope_level: i64) -> i64 {
    with_checker_env_store(|s| s.alloc_env(parent, scope_level))
}

/// Allocate a new environment frame that introduces variable
/// `(name, type_tag, type_name, is_mut, scope_level)` on top of
/// `env_id`. Returns the new frame id.
pub fn bootstrap_checker_env_insert_var(
    env_id: i64,
    name: &str,
    type_tag: i64,
    type_name: &str,
    is_mut: i64,
    scope_level: i64,
) -> i64 {
    with_checker_env_store(|s| {
        let var_id = s.alloc_var(VarRecord {
            name: name.to_string(),
            type_tag,
            type_name: type_name.to_string(),
            is_mut,
            scope_level,
        });
        let frame_id = s.alloc_env(env_id, scope_level);
        if let Ok(idx) = usize::try_from(frame_id) {
            if let Some(frame) = s.frames.get_mut(idx) {
                frame.var_id = var_id;
            }
        }
        frame_id
    })
}

/// Allocate a new environment frame that introduces function
/// `(name, params_handle, ret_type_tag, ret_type_name, effects_handle, is_extern)`
/// on top of `env_id`. Returns the new frame id.
pub fn bootstrap_checker_env_insert_fn(
    env_id: i64,
    name: &str,
    params_handle: i64,
    ret_type_tag: i64,
    ret_type_name: &str,
    effects_handle: i64,
    is_extern: i64,
) -> i64 {
    with_checker_env_store(|s| {
        let fn_id = s.alloc_fn(FnRecord {
            name: name.to_string(),
            params_handle,
            ret_type_tag,
            ret_type_name: ret_type_name.to_string(),
            effects_handle,
            is_extern,
        });
        let scope_level = s.frame(env_id).map(|f| f.scope_level).unwrap_or(0);
        let frame_id = s.alloc_env(env_id, scope_level);
        if let Ok(idx) = usize::try_from(frame_id) {
            if let Some(frame) = s.frames.get_mut(idx) {
                frame.fn_id = fn_id;
            }
        }
        frame_id
    })
}

/// Look up `name` in `env_id` walking the parent chain. Returns the
/// var record id, or 0 if not found.
pub fn bootstrap_checker_env_lookup_var(env_id: i64, name: &str) -> i64 {
    with_checker_env_store_ref(|s| s.lookup_var(env_id, name))
}

/// Look up `name` in `env_id` walking the parent chain. Returns the
/// fn record id, or 0 if not found.
pub fn bootstrap_checker_env_lookup_fn(env_id: i64, name: &str) -> i64 {
    with_checker_env_store_ref(|s| s.lookup_fn(env_id, name))
}

/// Returns the parent frame id of `env_id` (0 if root).
pub fn bootstrap_checker_env_get_parent(env_id: i64) -> i64 {
    with_checker_env_store_ref(|s| s.frame(env_id).map(|f| f.parent).unwrap_or(0))
}

/// Returns the recorded scope level of `env_id` (0 if root).
pub fn bootstrap_checker_env_get_scope_level(env_id: i64) -> i64 {
    with_checker_env_store_ref(|s| s.frame(env_id).map(|f| f.scope_level).unwrap_or(0))
}

// Var record accessors — return safe defaults for id 0 / unknown ids.
pub fn bootstrap_checker_var_get_name(var_id: i64) -> String {
    with_checker_env_store_ref(|s| s.var(var_id).map(|v| v.name.clone()).unwrap_or_default())
}
pub fn bootstrap_checker_var_get_type_tag(var_id: i64) -> i64 {
    with_checker_env_store_ref(|s| s.var(var_id).map(|v| v.type_tag).unwrap_or(0))
}
pub fn bootstrap_checker_var_get_type_name(var_id: i64) -> String {
    with_checker_env_store_ref(|s| {
        s.var(var_id)
            .map(|v| v.type_name.clone())
            .unwrap_or_default()
    })
}
pub fn bootstrap_checker_var_get_is_mut(var_id: i64) -> i64 {
    with_checker_env_store_ref(|s| s.var(var_id).map(|v| v.is_mut).unwrap_or(0))
}
pub fn bootstrap_checker_var_get_scope_level(var_id: i64) -> i64 {
    with_checker_env_store_ref(|s| s.var(var_id).map(|v| v.scope_level).unwrap_or(0))
}

// Fn record accessors.
pub fn bootstrap_checker_fn_get_name(fn_id: i64) -> String {
    with_checker_env_store_ref(|s| s.fn_rec(fn_id).map(|f| f.name.clone()).unwrap_or_default())
}
pub fn bootstrap_checker_fn_get_params_handle(fn_id: i64) -> i64 {
    with_checker_env_store_ref(|s| s.fn_rec(fn_id).map(|f| f.params_handle).unwrap_or(0))
}
pub fn bootstrap_checker_fn_get_ret_type_tag(fn_id: i64) -> i64 {
    with_checker_env_store_ref(|s| s.fn_rec(fn_id).map(|f| f.ret_type_tag).unwrap_or(0))
}
pub fn bootstrap_checker_fn_get_ret_type_name(fn_id: i64) -> String {
    with_checker_env_store_ref(|s| {
        s.fn_rec(fn_id)
            .map(|f| f.ret_type_name.clone())
            .unwrap_or_default()
    })
}
pub fn bootstrap_checker_fn_get_effects_handle(fn_id: i64) -> i64 {
    with_checker_env_store_ref(|s| s.fn_rec(fn_id).map(|f| f.effects_handle).unwrap_or(0))
}
pub fn bootstrap_checker_fn_get_is_extern(fn_id: i64) -> i64 {
    with_checker_env_store_ref(|s| s.fn_rec(fn_id).map(|f| f.is_extern).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Serialize bridge tests on a shared lock so the singleton store
    /// can't race under `cargo test` parallelism.
    fn t_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn empty_root_env_lookups_return_zero() {
        let _g = t_lock();
        reset_checker_env_store();

        let root = bootstrap_checker_env_alloc(0, 0);
        assert!(root > 0);
        assert_eq!(bootstrap_checker_env_lookup_var(root, "x"), 0);
        assert_eq!(bootstrap_checker_env_lookup_fn(root, "f"), 0);
    }

    #[test]
    fn insert_var_then_lookup_returns_record() {
        let _g = t_lock();
        reset_checker_env_store();

        let root = bootstrap_checker_env_alloc(0, 0);
        let env1 = bootstrap_checker_env_insert_var(root, "x", 1, "", 0, 0);
        let var_id = bootstrap_checker_env_lookup_var(env1, "x");
        assert!(var_id > 0);
        assert_eq!(bootstrap_checker_var_get_name(var_id), "x");
        assert_eq!(bootstrap_checker_var_get_type_tag(var_id), 1);
        assert_eq!(bootstrap_checker_var_get_is_mut(var_id), 0);
    }

    #[test]
    fn shadowing_returns_innermost_binding() {
        let _g = t_lock();
        reset_checker_env_store();

        let root = bootstrap_checker_env_alloc(0, 0);
        let outer = bootstrap_checker_env_insert_var(root, "x", 1, "", 0, 0);
        let inner = bootstrap_checker_env_insert_var(outer, "x", 3, "", 0, 1);
        let var_id = bootstrap_checker_env_lookup_var(inner, "x");
        assert_eq!(bootstrap_checker_var_get_type_tag(var_id), 3);
        // outer still resolves to the original binding.
        let outer_id = bootstrap_checker_env_lookup_var(outer, "x");
        assert_eq!(bootstrap_checker_var_get_type_tag(outer_id), 1);
    }

    #[test]
    fn lookup_walks_parent_chain_for_unrelated_names() {
        let _g = t_lock();
        reset_checker_env_store();

        let root = bootstrap_checker_env_alloc(0, 0);
        let e1 = bootstrap_checker_env_insert_var(root, "x", 1, "", 0, 0);
        let e2 = bootstrap_checker_env_insert_var(e1, "y", 2, "", 0, 0);
        let e3 = bootstrap_checker_env_insert_var(e2, "z", 3, "", 0, 0);

        assert!(bootstrap_checker_env_lookup_var(e3, "x") > 0);
        assert!(bootstrap_checker_env_lookup_var(e3, "y") > 0);
        assert!(bootstrap_checker_env_lookup_var(e3, "z") > 0);
        assert_eq!(bootstrap_checker_env_lookup_var(e3, "absent"), 0);
    }

    #[test]
    fn fn_insert_then_lookup() {
        let _g = t_lock();
        reset_checker_env_store();

        let root = bootstrap_checker_env_alloc(0, 0);
        let env1 = bootstrap_checker_env_insert_fn(root, "add", 7, 1, "", 0, 0);
        let fn_id = bootstrap_checker_env_lookup_fn(env1, "add");
        assert!(fn_id > 0);
        assert_eq!(bootstrap_checker_fn_get_name(fn_id), "add");
        assert_eq!(bootstrap_checker_fn_get_params_handle(fn_id), 7);
        assert_eq!(bootstrap_checker_fn_get_ret_type_tag(fn_id), 1);
    }

    #[test]
    fn unknown_ids_return_safe_defaults() {
        let _g = t_lock();
        reset_checker_env_store();

        assert_eq!(bootstrap_checker_var_get_name(99999), "");
        assert_eq!(bootstrap_checker_var_get_type_tag(99999), 0);
        assert_eq!(bootstrap_checker_fn_get_name(99999), "");
        assert_eq!(bootstrap_checker_env_get_parent(99999), 0);
        assert_eq!(bootstrap_checker_env_get_scope_level(99999), 0);
    }

    #[test]
    fn parent_and_scope_level_round_trip() {
        let _g = t_lock();
        reset_checker_env_store();

        let root = bootstrap_checker_env_alloc(0, 0);
        let mid = bootstrap_checker_env_alloc(root, 1);
        let leaf = bootstrap_checker_env_alloc(mid, 2);
        assert_eq!(bootstrap_checker_env_get_parent(leaf), mid);
        assert_eq!(bootstrap_checker_env_get_scope_level(leaf), 2);
        assert_eq!(bootstrap_checker_env_get_parent(mid), root);
        assert_eq!(bootstrap_checker_env_get_scope_level(root), 0);
    }
}
