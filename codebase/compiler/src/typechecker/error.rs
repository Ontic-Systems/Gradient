//! Type error representation and reporting for the Gradient type checker.
//!
//! Each [`TypeError`] records a structured diagnostic with a human-readable
//! message, the source span where the error occurred, and optional expected /
//! found type information. The `to_json` method produces machine-readable
//! diagnostics suitable for editor integrations and CI tooling.

use std::fmt;

use crate::ast::span::Span;
use super::types::Ty;

/// A type error or warning detected during type checking.
#[derive(Debug, Clone)]
pub struct TypeError {
    /// A human-readable description of the error.
    pub message: String,
    /// The source span where the error was detected.
    pub span: Span,
    /// The type that was expected, if applicable.
    pub expected: Option<Ty>,
    /// The type that was actually found, if applicable.
    pub found: Option<Ty>,
    /// Additional notes providing context or suggestions.
    pub notes: Vec<String>,
    /// Whether this diagnostic is a warning rather than an error.
    pub is_warning: bool,
}

impl TypeError {
    /// Create a new type error with just a message and span.
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
            expected: None,
            found: None,
            notes: Vec::new(),
            is_warning: false,
        }
    }

    /// Create a type mismatch error.
    pub fn mismatch(
        message: impl Into<String>,
        span: Span,
        expected: Ty,
        found: Ty,
    ) -> Self {
        Self {
            message: message.into(),
            span,
            expected: Some(expected),
            found: Some(found),
            notes: Vec::new(),
            is_warning: false,
        }
    }

    /// Create a warning (non-fatal diagnostic).
    pub fn warning(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
            expected: None,
            found: None,
            notes: Vec::new(),
            is_warning: true,
        }
    }

    /// Add a note to this error.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Produce a JSON string for machine-readable diagnostics.
    ///
    /// The output format is:
    /// ```json
    /// {
    ///   "source_phase": "typechecker",
    ///   "severity": "error",
    ///   "message": "...",
    ///   "span": {
    ///     "file_id": 0,
    ///     "start": { "line": 1, "col": 1, "offset": 0 },
    ///     "end": { "line": 1, "col": 10, "offset": 9 }
    ///   },
    ///   "expected": "Int",
    ///   "found": "String",
    ///   "notes": ["..."]
    /// }
    /// ```
    pub fn to_json(&self) -> String {
        let mut parts = Vec::new();

        parts.push(r#""source_phase": "typechecker""#.to_string());
        let severity = if self.is_warning { "warning" } else { "error" };
        parts.push(format!(r#""severity": "{}""#, severity));
        parts.push(format!(
            r#""message": "{}""#,
            self.message.replace('\\', "\\\\").replace('"', "\\\"")
        ));

        let span_json = format!(
            r#""span": {{"file_id": {}, "start": {{"line": {}, "col": {}, "offset": {}}}, "end": {{"line": {}, "col": {}, "offset": {}}}}}"#,
            self.span.file_id,
            self.span.start.line, self.span.start.col, self.span.start.offset,
            self.span.end.line, self.span.end.col, self.span.end.offset,
        );
        parts.push(span_json);

        if let Some(ref expected) = self.expected {
            parts.push(format!(r#""expected": "{}""#, expected));
        }
        if let Some(ref found) = self.found {
            parts.push(format!(r#""found": "{}""#, found));
        }
        if !self.notes.is_empty() {
            let notes_json: Vec<String> = self
                .notes
                .iter()
                .map(|n| format!(r#""{}""#, n.replace('\\', "\\\\").replace('"', "\\\"")))
                .collect();
            parts.push(format!(r#""notes": [{}]"#, notes_json.join(", ")));
        }

        format!("{{{}}}", parts.join(", "))
    }
}

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = if self.is_warning { "warning" } else { "error" };
        write!(
            f,
            "{}[{}:{}]: {}",
            label, self.span.start.line, self.span.start.col, self.message
        )?;
        if let (Some(ref expected), Some(ref found)) = (&self.expected, &self.found) {
            write!(f, " (expected `{}`, found `{}`)", expected, found)?;
        }
        for note in &self.notes {
            write!(f, "\n  note: {}", note)?;
        }
        Ok(())
    }
}
