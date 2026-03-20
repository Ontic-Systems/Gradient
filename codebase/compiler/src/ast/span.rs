//! Source location tracking for all AST nodes.
//!
//! Every node in the Gradient AST carries a [`Span`] that records where it
//! appeared in the original source text. The generic [`Spanned<T>`] wrapper
//! pairs an arbitrary node payload with its span, and is the primary
//! mechanism used throughout the tree.

use serde::Serialize;

/// A position within a single source file.
///
/// All fields use 1-based indexing for lines and columns, matching
/// the convention used by editors and diagnostic messages. The `offset`
/// field is a 0-based byte offset from the start of the file, useful for
/// slicing into the raw source string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Position {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number (in bytes, not characters).
    pub col: u32,
    /// 0-based byte offset from the start of the file.
    pub offset: u32,
}

/// A contiguous region of source text within a single file.
///
/// Spans are half-open: they cover bytes from `start.offset` up to (but not
/// including) `end.offset`. A zero-width span (`start == end`) is used for
/// synthetic nodes that have no direct textual representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    /// Index into the compiler's file table, identifying which source file
    /// this span belongs to.
    pub file_id: u32,
    /// The position of the first byte covered by this span.
    pub start: Position,
    /// The position one past the last byte covered by this span.
    pub end: Position,
}

/// An AST node `T` paired with the source [`Span`] it was parsed from.
///
/// This is the standard wrapper used for nodes whose "kind" enum does not
/// already contain a span field. For example, `Expr` is defined as
/// `Spanned<ExprKind>` and `Stmt` as `Spanned<StmtKind>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    /// The semantic payload of this node.
    pub node: T,
    /// The source region this node was parsed from.
    pub span: Span,
}

impl Span {
    /// Create a new span covering the region between two positions in the
    /// given file.
    pub fn new(file_id: u32, start: Position, end: Position) -> Self {
        Self { file_id, start, end }
    }

    /// Create a zero-width span at the given position, useful for synthetic
    /// or compiler-generated nodes.
    pub fn point(file_id: u32, pos: Position) -> Self {
        Self {
            file_id,
            start: pos,
            end: pos,
        }
    }

    /// Produce a new span that covers everything from the start of `self` to
    /// the end of `other`. Both spans must belong to the same file.
    pub fn merge(&self, other: &Span) -> Span {
        debug_assert_eq!(self.file_id, other.file_id, "cannot merge spans from different files");
        Span {
            file_id: self.file_id,
            start: self.start,
            end: other.end,
        }
    }
}

impl Position {
    /// Create a new source position.
    pub fn new(line: u32, col: u32, offset: u32) -> Self {
        Self { line, col, offset }
    }
}

impl<T> Spanned<T> {
    /// Wrap a node with a span.
    pub fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }

    /// Apply a function to the inner node, preserving the span.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            node: f(self.node),
            span: self.span,
        }
    }

    /// Borrow the inner node.
    pub fn inner(&self) -> &T {
        &self.node
    }

    /// Mutably borrow the inner node.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.node
    }
}
