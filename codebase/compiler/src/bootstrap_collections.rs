//! Runtime-backed bootstrap collection handles for self-hosted compiler phases.
//!
//! This module is intentionally narrow: it gives bootstrap-stage Gradient code a
//! stable host boundary for list-like storage before the full self-hosted runtime
//! owns compiler data structures. Handles are non-zero, typed, and preserve item
//! order for append/get/len operations.

use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroU32;

/// Bootstrap collection categories used by self-hosted compiler wrappers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BootstrapCollectionKind {
    TokenList,
    ExprList,
    StmtList,
    ParamList,
    FunctionList,
    ModuleItemList,
    DiagnosticList,
    SymbolList,
    IrValueList,
    IrBlockList,
    IrFunctionList,
    IrModuleList,
    StringList,
    IntList,
}

/// Non-zero typed handle for a bootstrap collection.
#[derive(Eq, PartialEq, Hash)]
pub struct BootstrapHandle<T> {
    raw: NonZeroU32,
    kind: BootstrapCollectionKind,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for BootstrapHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for BootstrapHandle<T> {}

impl<T> BootstrapHandle<T> {
    fn new(raw: NonZeroU32, kind: BootstrapCollectionKind) -> Self {
        Self {
            raw,
            kind,
            _marker: PhantomData,
        }
    }

    pub fn raw(self) -> u32 {
        self.raw.get()
    }

    pub fn kind(self) -> BootstrapCollectionKind {
        self.kind
    }
}

impl<T> fmt::Debug for BootstrapHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BootstrapHandle")
            .field("raw", &self.raw.get())
            .field("kind", &self.kind)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BootstrapCollectionError {
    UnknownHandle {
        handle: u32,
    },
    KindMismatch {
        handle: u32,
        expected: BootstrapCollectionKind,
        actual: BootstrapCollectionKind,
    },
    IndexOutOfBounds {
        handle: u32,
        index: usize,
        len: usize,
    },
}

#[derive(Clone, Debug)]
struct Collection<T> {
    kind: BootstrapCollectionKind,
    items: Vec<T>,
}

/// In-memory host store for bootstrap compiler collections.
#[derive(Clone, Debug)]
pub struct BootstrapCollectionStore<T> {
    next_handle: u32,
    collections: HashMap<u32, Collection<T>>,
}

impl<T> Default for BootstrapCollectionStore<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> BootstrapCollectionStore<T> {
    pub fn new() -> Self {
        Self {
            next_handle: 1,
            collections: HashMap::new(),
        }
    }

    pub fn alloc(&mut self, kind: BootstrapCollectionKind) -> BootstrapHandle<T> {
        let raw = NonZeroU32::new(self.next_handle).expect("next_handle starts non-zero");
        self.next_handle = self
            .next_handle
            .checked_add(1)
            .expect("bootstrap collection handle space exhausted");
        self.collections.insert(
            raw.get(),
            Collection {
                kind,
                items: Vec::new(),
            },
        );
        BootstrapHandle::new(raw, kind)
    }

    pub fn append(
        &mut self,
        handle: BootstrapHandle<T>,
        item: T,
    ) -> Result<(), BootstrapCollectionError> {
        let collection = self.collection_mut(handle)?;
        collection.items.push(item);
        Ok(())
    }

    pub fn len(&self, handle: BootstrapHandle<T>) -> Result<usize, BootstrapCollectionError> {
        Ok(self.collection(handle)?.items.len())
    }

    pub fn is_empty(&self, handle: BootstrapHandle<T>) -> Result<bool, BootstrapCollectionError> {
        Ok(self.len(handle)? == 0)
    }

    pub fn get(
        &self,
        handle: BootstrapHandle<T>,
        index: usize,
    ) -> Result<&T, BootstrapCollectionError> {
        let collection = self.collection(handle)?;
        collection
            .items
            .get(index)
            .ok_or(BootstrapCollectionError::IndexOutOfBounds {
                handle: handle.raw(),
                index,
                len: collection.items.len(),
            })
    }

    fn collection(
        &self,
        handle: BootstrapHandle<T>,
    ) -> Result<&Collection<T>, BootstrapCollectionError> {
        let collection =
            self.collections
                .get(&handle.raw())
                .ok_or(BootstrapCollectionError::UnknownHandle {
                    handle: handle.raw(),
                })?;
        if collection.kind != handle.kind() {
            return Err(BootstrapCollectionError::KindMismatch {
                handle: handle.raw(),
                expected: handle.kind(),
                actual: collection.kind,
            });
        }
        Ok(collection)
    }

    fn collection_mut(
        &mut self,
        handle: BootstrapHandle<T>,
    ) -> Result<&mut Collection<T>, BootstrapCollectionError> {
        let collection = self.collections.get_mut(&handle.raw()).ok_or(
            BootstrapCollectionError::UnknownHandle {
                handle: handle.raw(),
            },
        )?;
        if collection.kind != handle.kind() {
            return Err(BootstrapCollectionError::KindMismatch {
                handle: handle.raw(),
                expected: handle.kind(),
                actual: collection.kind,
            });
        }
        Ok(collection)
    }
}

#[cfg(test)]
mod tests {
    use super::{BootstrapCollectionKind, BootstrapCollectionStore};

    #[test]
    fn handles_are_non_zero_and_kind_tagged() {
        let mut store = BootstrapCollectionStore::<i64>::new();
        let tokens = store.alloc(BootstrapCollectionKind::TokenList);
        let symbols = store.alloc(BootstrapCollectionKind::SymbolList);

        assert_ne!(tokens.raw(), 0);
        assert_ne!(symbols.raw(), 0);
        assert_ne!(tokens.raw(), symbols.raw());
        assert_eq!(tokens.kind(), BootstrapCollectionKind::TokenList);
        assert_eq!(symbols.kind(), BootstrapCollectionKind::SymbolList);
    }

    #[test]
    fn append_get_len_round_trip_preserves_order() {
        let mut store = BootstrapCollectionStore::<String>::new();
        let diagnostics = store.alloc(BootstrapCollectionKind::DiagnosticList);

        store.append(diagnostics, "first".to_string()).unwrap();
        store.append(diagnostics, "second".to_string()).unwrap();

        assert_eq!(store.len(diagnostics).unwrap(), 2);
        assert_eq!(store.get(diagnostics, 0).unwrap(), "first");
        assert_eq!(store.get(diagnostics, 1).unwrap(), "second");
    }

    #[test]
    fn get_reports_out_of_bounds() {
        let mut store = BootstrapCollectionStore::<i64>::new();
        let values = store.alloc(BootstrapCollectionKind::IrValueList);
        store.append(values, 42).unwrap();

        let err = store.get(values, 1).unwrap_err();
        assert!(format!("{err:?}").contains("IndexOutOfBounds"));
    }
}
