//! Parser error types and diagnostic output.
//!
//! [`ParseError`] records everything needed to produce a human-readable
//! diagnostic or a machine-readable JSON diagnostic that conforms to the
//! Gradient diagnostic schema used by editor integrations and CI tooling.

use std::fmt;

// We use the lexer's Span type since that is what Token carries.
use crate::lexer::token::Span;

/// A single syntax error discovered during parsing.
///
/// The parser is designed for error recovery: it collects every error it
/// encounters rather than aborting on the first one. After parsing, the
/// caller receives both a (partial) AST and a `Vec<ParseError>`.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    /// A human-readable description of the error.
    pub message: String,
    /// The source span where the error was detected.
    pub span: Span,
    /// What the parser expected to see at this position.
    pub expected: Vec<String>,
    /// A description of the token that was actually found.
    pub found: String,
}

impl ParseError {
    /// Create a new parse error.
    pub fn new(
        message: impl Into<String>,
        span: Span,
        expected: Vec<String>,
        found: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            span,
            expected,
            found: found.into(),
        }
    }

    /// Produce a JSON diagnostic string conforming to the Gradient diagnostic
    /// schema.
    ///
    /// Example output:
    /// ```json
    /// {
    ///   "source_phase": "parser",
    ///   "severity": "error",
    ///   "message": "expected ':'",
    ///   "span": { "file_id": 0, "start": { "line": 3, "col": 5, "offset": 42 }, "end": { "line": 3, "col": 6, "offset": 43 } },
    ///   "expected": [":"],
    ///   "found": "="
    /// }
    /// ```
    pub fn to_json(&self) -> String {
        let expected_json: Vec<String> = self
            .expected
            .iter()
            .map(|e| format!("\"{}\"", escape_json_string(e)))
            .collect();

        format!(
            concat!(
                "{{",
                "\"source_phase\":\"parser\",",
                "\"severity\":\"error\",",
                "\"message\":\"{}\",",
                "\"span\":{{",
                "\"file_id\":{},",
                "\"start\":{{\"line\":{},\"col\":{},\"offset\":{}}},",
                "\"end\":{{\"line\":{},\"col\":{},\"offset\":{}}}",
                "}},",
                "\"expected\":[{}],",
                "\"found\":\"{}\"",
                "}}"
            ),
            escape_json_string(&self.message),
            self.span.file_id,
            self.span.start.line,
            self.span.start.col,
            self.span.start.offset,
            self.span.end.line,
            self.span.end.col,
            self.span.end.offset,
            expected_json.join(","),
            escape_json_string(&self.found),
        )
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "error at {}:{}: {}",
            self.span.start.line, self.span.start.col, self.message,
        )?;

        if !self.expected.is_empty() {
            write!(f, " (expected ")?;
            for (i, exp) in self.expected.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "`{}`", exp)?;
            }
            write!(f, "; found `{}`)", self.found)?;
        }

        Ok(())
    }
}

/// Minimally escape a string for embedding in a JSON string literal.
fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::token::Position;

    #[test]
    fn display_with_expected() {
        let err = ParseError::new(
            "expected ':'",
            Span::new(
                0,
                Position::new(3, 5, 42),
                Position::new(3, 6, 43),
            ),
            vec![":".into()],
            "=",
        );
        let s = format!("{}", err);
        assert!(s.contains("error at 3:5"));
        assert!(s.contains("expected `:`"));
        assert!(s.contains("found `=`"));
    }

    #[test]
    fn display_without_expected() {
        let err = ParseError::new(
            "syntax error",
            Span::new(
                0,
                Position::new(1, 1, 0),
                Position::new(1, 2, 1),
            ),
            vec![],
            "+",
        );
        let s = format!("{}", err);
        assert!(s.contains("syntax error"));
        // When the expected list is empty, the display should NOT include
        // the "(expected ...; found ...)" suffix.
        assert!(!s.contains("(expected"));
    }

    #[test]
    fn json_output() {
        let err = ParseError::new(
            "expected ':'",
            Span::new(
                0,
                Position::new(3, 5, 42),
                Position::new(3, 6, 43),
            ),
            vec![":".into()],
            "=",
        );
        let json = err.to_json();
        assert!(json.contains("\"source_phase\":\"parser\""));
        assert!(json.contains("\"severity\":\"error\""));
        assert!(json.contains("\"message\":\"expected ':'\""));
        assert!(json.contains("\"file_id\":0"));
        assert!(json.contains("\"expected\":[\":\"]"));
        assert!(json.contains("\"found\":\"=\""));
    }
}
