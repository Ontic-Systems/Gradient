//! Interactive REPL (Read-Eval-Print Loop) for the Gradient language.
//!
//! The REPL provides an interactive environment for evaluating Gradient
//! expressions and statements. It operates in **check mode**: each input
//! line is type-checked (not compiled) and the inferred type or any errors
//! are reported immediately.
//!
//! This is useful for both humans exploring the language and for AI agents
//! that want to test expressions and get type information interactively.
//!
//! # Modes
//!
//! - **Interactive** (TTY): displays a `gradient> ` prompt and welcome banner.
//! - **Non-interactive** (piped stdin): reads lines without prompts, suitable
//!   for agent scripting.
//!
//! # Input handling
//!
//! The REPL classifies each input line and wraps it appropriately:
//!
//! - **Expressions** (e.g. `2 + 3`): wrapped in a synthetic `main` function
//!   and the inferred type is reported.
//! - **`let` bindings** (e.g. `let x = 5`): accumulated as state and
//!   type-checked within a synthetic `main`.
//! - **Function definitions** (e.g. `fn foo(): ...`): accumulated as
//!   top-level definitions.
//! - **Statements with effects** (e.g. `print("hello")`): wrapped in a
//!   synthetic `main` with `!{IO}` effects and type-checked.
//!
//! # Example
//!
//! ```text
//! gradient> 2 + 3
//! : Int
//! gradient> let x = 10
//! (bound x)
//! gradient> x * 2
//! : Int
//! gradient> fn double(n: Int) -> Int:
//! ...         n * 2
//! (defined fn double)
//! gradient> double(5)
//! : Int
//! ```

use crate::query::Session;

use std::io::{self, BufRead, Write};

// =========================================================================
// REPL state
// =========================================================================

/// Accumulated state across REPL iterations.
///
/// The REPL remembers previous function definitions and let bindings so
/// that later inputs can reference them.
pub struct ReplState {
    /// Top-level function definitions accumulated across iterations.
    /// Each entry is the full source text of a `fn` definition.
    fn_definitions: Vec<String>,
    /// Let bindings accumulated across iterations (inside main).
    /// Each entry is a single `let ...` line.
    let_bindings: Vec<String>,
}

impl ReplState {
    /// Create a new, empty REPL state.
    pub fn new() -> Self {
        Self {
            fn_definitions: Vec::new(),
            let_bindings: Vec::new(),
        }
    }

    /// Build a complete Gradient program from accumulated state plus new input.
    ///
    /// The program has:
    /// 1. All accumulated `fn` definitions at the top level.
    /// 2. A synthetic `main` function containing all accumulated `let` bindings
    ///    followed by the new input.
    ///
    /// If `as_expr` is true, the input is treated as an expression (the last
    /// line of the main body). If false, it is treated as a statement.
    pub fn build_program(&self, input: &str, effects: &[&str]) -> String {
        let mut program = String::new();

        // Add accumulated function definitions.
        for def in &self.fn_definitions {
            program.push_str(def);
            program.push('\n');
        }

        // Build the synthetic main function.
        let effect_str = if effects.is_empty() {
            String::new()
        } else {
            format!(" -> !{{{}}} ()", effects.join(", "))
        };
        program.push_str(&format!("fn main(){effect_str}:\n"));

        // Add accumulated let bindings.
        for binding in &self.let_bindings {
            program.push_str("    ");
            program.push_str(binding);
            program.push('\n');
        }

        // Add the new input as the last line of main.
        program.push_str("    ");
        program.push_str(input);
        program.push('\n');

        program
    }

    /// Add a function definition to the accumulated state.
    pub fn add_fn_definition(&mut self, def: String) {
        self.fn_definitions.push(def);
    }

    /// Add a let binding to the accumulated state.
    pub fn add_let_binding(&mut self, binding: String) {
        self.let_bindings.push(binding);
    }
}

impl Default for ReplState {
    fn default() -> Self {
        Self::new()
    }
}

// =========================================================================
// Input classification
// =========================================================================

/// The kind of input the user entered.
#[derive(Debug, PartialEq)]
pub enum InputKind {
    /// A bare expression (e.g. `2 + 3`, `x`, `foo(1)`).
    Expression,
    /// A `let` binding (e.g. `let x = 5`).
    LetBinding,
    /// A function definition (e.g. `fn foo(x: Int) -> Int:`).
    FnDefinition,
    /// An empty line or whitespace-only input.
    Empty,
    /// A REPL meta-command (e.g. `:quit`, `:state`, `:reset`, `:help`).
    MetaCommand(String),
}

/// Classify a line of REPL input.
pub fn classify_input(input: &str) -> InputKind {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return InputKind::Empty;
    }

    if trimmed.starts_with(':') {
        return InputKind::MetaCommand(trimmed.to_string());
    }

    if trimmed.starts_with("let ") || trimmed.starts_with("let\t") {
        return InputKind::LetBinding;
    }

    if trimmed.starts_with("fn ") || trimmed.starts_with("fn\t") {
        return InputKind::FnDefinition;
    }

    InputKind::Expression
}

// =========================================================================
// Type inference for expressions
// =========================================================================

/// Attempt to infer the type of an expression by type-checking a synthetic
/// program.
///
/// Strategy: build a probe function `__repl_probe__` that returns the
/// expression as its body, with a deliberately wrong return type annotation.
/// The type checker will produce a mismatch error whose `found` field
/// contains the actual inferred type of the expression.
///
/// Returns `Ok(type_string)` or `Err(error_messages)`.
pub fn infer_expression_type(state: &ReplState, expr: &str) -> Result<String, Vec<String>> {
    // Phase 1: Check that the expression is valid (no type errors of its own).
    let validity_program = state.build_program(expr, &[]);
    let validity_session = Session::from_source(&validity_program);
    let validity_result = validity_session.check();

    // Try with IO effects if plain check fails.
    let (is_valid, needs_io) = if validity_result.is_ok() {
        (true, false)
    } else {
        let io_program = state.build_program(expr, &["IO"]);
        let io_session = Session::from_source(&io_program);
        let io_result = io_session.check();
        if io_result.is_ok() {
            (true, true)
        } else {
            (false, false)
        }
    };

    if !is_valid {
        // Expression itself has errors. Return them.
        let errors: Vec<String> = validity_result
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect();
        return Err(errors);
    }

    // Phase 2: Probe for the expression's type using a return-type mismatch.
    // We build a probe function with a deliberately wrong return type.
    // The type checker will report "body has type X, expected Y" and we
    // extract X from the error's `found` field.
    //
    // We try two sentinel return types to handle the case where the
    // expression happens to match one of them.
    let sentinel_types = ["String", "Int"];
    let effects_list: &[&str] = if needs_io { &["IO"] } else { &[] };

    for sentinel in &sentinel_types {
        let probe_program = build_type_probe_program(state, expr, sentinel, effects_list);
        let probe_session = Session::from_source(&probe_program);
        let probe_result = probe_session.check();

        if probe_result.is_ok() {
            // The expression's type matches the sentinel -- we know the type.
            return Ok(sentinel.to_string());
        }

        // Look for the mismatch error on the probe function.
        for err in probe_session.type_errors() {
            if err.message.contains("__repl_probe__") && err.message.contains("body has type") {
                if let Some(ref found_ty) = err.found {
                    return Ok(found_ty.to_string());
                }
            }
        }
    }

    // Fallback: the expression is valid but we could not determine its type.
    // This can happen for Unit-typed expressions (the checker skips mismatch
    // for Unit bodies). Report as Unit.
    Ok("()".to_string())
}

/// Build a probe program that wraps the expression as the body of a function
/// with a deliberately mismatched return type, so the type checker reports
/// the expression's actual type in the error.
fn build_type_probe_program(
    state: &ReplState,
    expr: &str,
    sentinel_return_type: &str,
    effects: &[&str],
) -> String {
    let mut program = String::new();

    // Add accumulated function definitions.
    for def in &state.fn_definitions {
        program.push_str(def);
        program.push('\n');
    }

    // Build the probe function. It contains the accumulated let bindings
    // followed by the expression as the last line (return value).
    let effect_str = if effects.is_empty() {
        String::new()
    } else {
        format!(" -> !{{{}}} {}", effects.join(", "), sentinel_return_type)
    };

    if effects.is_empty() {
        program.push_str(&format!(
            "fn __repl_probe__() -> {}:\n",
            sentinel_return_type
        ));
    } else {
        program.push_str(&format!("fn __repl_probe__(){}:\n", effect_str));
    }

    for binding in &state.let_bindings {
        program.push_str("    ");
        program.push_str(binding);
        program.push('\n');
    }

    program.push_str("    ");
    program.push_str(expr);
    program.push('\n');

    // Add a minimal main so the program is complete.
    program.push_str("fn main():\n");
    program.push_str("    ()\n");

    program
}

/// Check whether a let binding is valid by type-checking it in context.
/// Returns `Ok(())` on success or `Err(messages)` on failure.
pub fn check_let_binding(state: &ReplState, binding: &str) -> Result<(), Vec<String>> {
    // Try without effects first.
    let program = state.build_program(binding, &[]);
    let session = Session::from_source(&program);
    let result = session.check();

    if result.is_ok() {
        return Ok(());
    }

    // Try with IO effects.
    let io_program = state.build_program(binding, &["IO"]);
    let io_session = Session::from_source(&io_program);
    let io_result = io_session.check();

    if io_result.is_ok() {
        return Ok(());
    }

    let errors: Vec<String> = result
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();
    Err(errors)
}

/// Check whether a function definition is valid by type-checking it in context.
/// Returns `Ok(())` on success or `Err(messages)` on failure.
pub fn check_fn_definition(state: &ReplState, fn_source: &str) -> Result<(), Vec<String>> {
    let mut program = String::new();

    // Add previously accumulated function definitions.
    for def in &state.fn_definitions {
        program.push_str(def);
        program.push('\n');
    }

    // Add the new function definition.
    program.push_str(fn_source);
    program.push('\n');

    // Add a minimal main so the program is complete.
    // Include accumulated let bindings.
    program.push_str("fn main():\n");
    for binding in &state.let_bindings {
        program.push_str("    ");
        program.push_str(binding);
        program.push('\n');
    }
    program.push_str("    ()\n");

    let session = Session::from_source(&program);
    let result = session.check();

    if result.is_ok() {
        return Ok(());
    }

    let errors: Vec<String> = result
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();
    Err(errors)
}

// =========================================================================
// The REPL loop
// =========================================================================

/// Run the interactive REPL.
///
/// If `interactive` is true, a prompt and welcome banner are displayed.
/// If false (piped input), lines are read silently.
pub fn run_repl(interactive: bool) {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut state = ReplState::new();

    if interactive {
        let _ = writeln!(stdout, "Gradient REPL v0.1 (type-check mode)");
        let _ = writeln!(stdout, "Type expressions to see their types, or :help for commands.");
        let _ = writeln!(stdout);
    }

    let mut multiline_buffer: Option<String> = None;
    let mut multiline_indent_expected = false;

    for line_result in stdin.lock().lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => break,
        };

        // Handle multiline input (function definitions).
        if let Some(ref mut buffer) = multiline_buffer {
            if multiline_indent_expected {
                // We expect indented continuation lines.
                if line.starts_with("    ") || line.starts_with('\t') {
                    buffer.push('\n');
                    buffer.push_str(&line);
                    // Continue reading more lines.
                    if interactive {
                        let _ = write!(stdout, "...     ");
                        let _ = stdout.flush();
                    }
                    continue;
                } else {
                    // Non-indented line: the function definition is complete.
                    let fn_source = buffer.clone();
                    multiline_buffer = None;
                    multiline_indent_expected = false;
                    handle_fn_definition(&mut state, &fn_source, &mut stdout);

                    // Now process the current line normally (if non-empty).
                    if line.trim().is_empty() {
                        if interactive {
                            let _ = write!(stdout, "gradient> ");
                            let _ = stdout.flush();
                        }
                        continue;
                    }
                    // Fall through to process this line.
                }
            }
        }

        let input = line.as_str();
        let kind = classify_input(input);

        match kind {
            InputKind::Empty => {
                // If we were in multiline mode and got an empty line, finish.
                if let Some(ref buffer) = multiline_buffer {
                    let fn_source = buffer.clone();
                    multiline_buffer = None;
                    multiline_indent_expected = false;
                    handle_fn_definition(&mut state, &fn_source, &mut stdout);
                }

                if interactive {
                    let _ = write!(stdout, "gradient> ");
                    let _ = stdout.flush();
                }
            }

            InputKind::MetaCommand(cmd) => {
                handle_meta_command(&cmd, &mut state, &mut stdout, interactive);
                if interactive {
                    let _ = write!(stdout, "gradient> ");
                    let _ = stdout.flush();
                }
            }

            InputKind::LetBinding => {
                let trimmed = input.trim();
                match check_let_binding(&state, trimmed) {
                    Ok(()) => {
                        // Extract the binding name for user feedback.
                        let name = extract_let_name(trimmed).unwrap_or("?");
                        state.add_let_binding(trimmed.to_string());
                        let _ = writeln!(stdout, "(bound {})", name);
                    }
                    Err(errors) => {
                        for err in &errors {
                            let _ = writeln!(stdout, "error: {}", err);
                        }
                    }
                }
                if interactive {
                    let _ = write!(stdout, "gradient> ");
                    let _ = stdout.flush();
                }
            }

            InputKind::FnDefinition => {
                let trimmed = input.trim();
                // Check if this is a single-line fn definition or needs continuation.
                if trimmed.ends_with(':') {
                    // Start multiline mode.
                    multiline_buffer = Some(trimmed.to_string());
                    multiline_indent_expected = true;
                    if interactive {
                        let _ = write!(stdout, "...     ");
                        let _ = stdout.flush();
                    }
                } else {
                    // Single-line fn definition (unusual but possible for the parser).
                    handle_fn_definition(&mut state, trimmed, &mut stdout);
                    if interactive {
                        let _ = write!(stdout, "gradient> ");
                        let _ = stdout.flush();
                    }
                }
            }

            InputKind::Expression => {
                let trimmed = input.trim();
                match infer_expression_type(&state, trimmed) {
                    Ok(ty) => {
                        let _ = writeln!(stdout, ": {}", ty);
                    }
                    Err(errors) => {
                        for err in &errors {
                            let _ = writeln!(stdout, "error: {}", err);
                        }
                    }
                }
                if interactive {
                    let _ = write!(stdout, "gradient> ");
                    let _ = stdout.flush();
                }
            }
        }
    }

    if interactive {
        let _ = writeln!(stdout);
        let _ = writeln!(stdout, "Goodbye!");
    }
}

/// Handle a completed function definition.
fn handle_fn_definition(state: &mut ReplState, fn_source: &str, stdout: &mut impl Write) {
    match check_fn_definition(state, fn_source) {
        Ok(()) => {
            let name = extract_fn_name(fn_source).unwrap_or("?");
            state.add_fn_definition(fn_source.to_string());
            let _ = writeln!(stdout, "(defined fn {})", name);
        }
        Err(errors) => {
            for err in &errors {
                let _ = writeln!(stdout, "error: {}", err);
            }
        }
    }
}

/// Handle a REPL meta-command.
fn handle_meta_command(
    cmd: &str,
    state: &mut ReplState,
    stdout: &mut impl Write,
    _interactive: bool,
) {
    match cmd {
        ":quit" | ":q" | ":exit" => {
            let _ = writeln!(stdout, "Goodbye!");
            std::process::exit(0);
        }
        ":help" | ":h" => {
            let _ = writeln!(stdout, "Gradient REPL commands:");
            let _ = writeln!(stdout, "  :help, :h       Show this help message");
            let _ = writeln!(stdout, "  :quit, :q       Exit the REPL");
            let _ = writeln!(stdout, "  :state, :s      Show accumulated definitions");
            let _ = writeln!(stdout, "  :reset, :r      Clear all accumulated state");
            let _ = writeln!(stdout, "  :type <expr>    Show the type of an expression");
            let _ = writeln!(stdout);
            let _ = writeln!(stdout, "Enter expressions to see their types.");
            let _ = writeln!(stdout, "Enter `let x = ...` to define bindings.");
            let _ = writeln!(stdout, "Enter `fn name(...):` to start a function definition.");
        }
        ":state" | ":s" => {
            if state.fn_definitions.is_empty() && state.let_bindings.is_empty() {
                let _ = writeln!(stdout, "(no accumulated state)");
            } else {
                if !state.fn_definitions.is_empty() {
                    let _ = writeln!(stdout, "Functions:");
                    for def in &state.fn_definitions {
                        // Show just the first line (signature).
                        let first_line = def.lines().next().unwrap_or(def);
                        let _ = writeln!(stdout, "  {}", first_line);
                    }
                }
                if !state.let_bindings.is_empty() {
                    let _ = writeln!(stdout, "Bindings:");
                    for binding in &state.let_bindings {
                        let _ = writeln!(stdout, "  {}", binding);
                    }
                }
            }
        }
        ":reset" | ":r" => {
            *state = ReplState::new();
            let _ = writeln!(stdout, "(state cleared)");
        }
        _ if cmd.starts_with(":type ") || cmd.starts_with(":t ") => {
            let expr = if let Some(rest) = cmd.strip_prefix(":type ") {
                rest
            } else if let Some(rest) = cmd.strip_prefix(":t ") {
                rest
            } else {
                cmd
            };
            let trimmed = expr.trim();
            match infer_expression_type(state, trimmed) {
                Ok(ty) => {
                    let _ = writeln!(stdout, ": {}", ty);
                }
                Err(errors) => {
                    for err in &errors {
                        let _ = writeln!(stdout, "error: {}", err);
                    }
                }
            }
        }
        _ => {
            let _ = writeln!(stdout, "Unknown command: {}", cmd);
            let _ = writeln!(stdout, "Type :help for available commands.");
        }
    }
}

// =========================================================================
// Helpers
// =========================================================================

/// Extract the binding name from a `let` statement.
///
/// Given `let x = 5` or `let mut x: Int = 5`, returns `"x"`.
fn extract_let_name(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix("let ")?;
    let rest = rest.trim_start();
    // Skip `mut` if present.
    let rest = rest.strip_prefix("mut ").unwrap_or(rest);
    let rest = rest.trim_start();
    // The name is the next identifier.
    let end = rest
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(&rest[..end])
}

/// Extract the function name from a `fn` definition.
///
/// Given `fn foo(x: Int) -> Int:`, returns `"foo"`.
fn extract_fn_name(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix("fn ")?;
    let rest = rest.trim_start();
    let end = rest
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(&rest[..end])
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ----- Input classification -----

    #[test]
    fn classify_empty_input() {
        assert_eq!(classify_input(""), InputKind::Empty);
        assert_eq!(classify_input("   "), InputKind::Empty);
        assert_eq!(classify_input("\t"), InputKind::Empty);
    }

    #[test]
    fn classify_expression() {
        assert_eq!(classify_input("2 + 3"), InputKind::Expression);
        assert_eq!(classify_input("x"), InputKind::Expression);
        assert_eq!(classify_input("foo(1, 2)"), InputKind::Expression);
        assert_eq!(classify_input("true"), InputKind::Expression);
        assert_eq!(classify_input("\"hello\""), InputKind::Expression);
    }

    #[test]
    fn classify_let_binding() {
        assert_eq!(classify_input("let x = 5"), InputKind::LetBinding);
        assert_eq!(classify_input("let mut y = 10"), InputKind::LetBinding);
        assert_eq!(classify_input("  let x = 5"), InputKind::LetBinding);
    }

    #[test]
    fn classify_fn_definition() {
        assert_eq!(
            classify_input("fn foo(x: Int) -> Int:"),
            InputKind::FnDefinition
        );
        assert_eq!(classify_input("fn main():"), InputKind::FnDefinition);
    }

    #[test]
    fn classify_meta_command() {
        assert_eq!(
            classify_input(":quit"),
            InputKind::MetaCommand(":quit".to_string())
        );
        assert_eq!(
            classify_input(":help"),
            InputKind::MetaCommand(":help".to_string())
        );
        assert_eq!(
            classify_input(":type 2 + 3"),
            InputKind::MetaCommand(":type 2 + 3".to_string())
        );
    }

    // ----- Name extraction -----

    #[test]
    fn extract_let_names() {
        assert_eq!(extract_let_name("let x = 5"), Some("x"));
        assert_eq!(extract_let_name("let mut y = 10"), Some("y"));
        assert_eq!(extract_let_name("let foo_bar: Int = 42"), Some("foo_bar"));
        assert_eq!(extract_let_name("let x123 = 0"), Some("x123"));
    }

    #[test]
    fn extract_fn_names() {
        assert_eq!(extract_fn_name("fn foo(x: Int) -> Int:"), Some("foo"));
        assert_eq!(extract_fn_name("fn main():"), Some("main"));
        assert_eq!(
            extract_fn_name("fn my_func(a: Int, b: Int) -> Int:"),
            Some("my_func")
        );
    }

    // ----- State accumulation -----

    #[test]
    fn state_accumulates_let_bindings() {
        let mut state = ReplState::new();
        state.add_let_binding("let x = 5".to_string());
        state.add_let_binding("let y = 10".to_string());

        let program = state.build_program("x + y", &[]);
        assert!(program.contains("fn main():"));
        assert!(program.contains("    let x = 5"));
        assert!(program.contains("    let y = 10"));
        assert!(program.contains("    x + y"));
    }

    #[test]
    fn state_accumulates_fn_definitions() {
        let mut state = ReplState::new();
        state.add_fn_definition("fn double(n: Int) -> Int:\n    n * 2".to_string());

        let program = state.build_program("double(5)", &[]);
        assert!(program.contains("fn double(n: Int) -> Int:"));
        assert!(program.contains("    n * 2"));
        assert!(program.contains("fn main():"));
        assert!(program.contains("    double(5)"));
    }

    #[test]
    fn state_build_program_with_effects() {
        let state = ReplState::new();
        let program = state.build_program("print(42)", &["IO"]);
        assert!(program.contains("fn main() -> !{IO} ():"));
    }

    #[test]
    fn state_build_program_no_effects() {
        let state = ReplState::new();
        let program = state.build_program("2 + 3", &[]);
        assert!(program.contains("fn main():"));
        // Should NOT have effect annotation.
        assert!(!program.contains("!{"));
    }

    // ----- Type inference -----

    #[test]
    fn infer_int_literal() {
        let state = ReplState::new();
        let result = infer_expression_type(&state, "42");
        assert_eq!(result, Ok("Int".to_string()));
    }

    #[test]
    fn infer_string_literal() {
        let state = ReplState::new();
        let result = infer_expression_type(&state, "\"hello\"");
        assert_eq!(result, Ok("String".to_string()));
    }

    #[test]
    fn infer_bool_literal() {
        let state = ReplState::new();
        let result = infer_expression_type(&state, "true");
        assert_eq!(result, Ok("Bool".to_string()));
    }

    #[test]
    fn infer_arithmetic_expression() {
        let state = ReplState::new();
        let result = infer_expression_type(&state, "2 + 3");
        assert_eq!(result, Ok("Int".to_string()));
    }

    #[test]
    fn infer_comparison_expression() {
        let state = ReplState::new();
        let result = infer_expression_type(&state, "2 > 3");
        assert_eq!(result, Ok("Bool".to_string()));
    }

    #[test]
    fn infer_expression_with_accumulated_fn() {
        let mut state = ReplState::new();
        state.add_fn_definition("fn double(n: Int) -> Int:\n    n * 2".to_string());

        let result = infer_expression_type(&state, "double(5)");
        assert_eq!(result, Ok("Int".to_string()));
    }

    #[test]
    fn infer_undefined_variable_reports_error() {
        let state = ReplState::new();
        let result = infer_expression_type(&state, "nonexistent_var");
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(!errors.is_empty());
        assert!(errors[0].contains("undefined"));
    }

    // ----- Let binding checks -----

    #[test]
    fn check_valid_let_binding() {
        let state = ReplState::new();
        let result = check_let_binding(&state, "let x = 42");
        assert!(result.is_ok());
    }

    #[test]
    fn check_let_binding_with_type_error() {
        let state = ReplState::new();
        // Assigning to an undefined variable in the initializer.
        let result = check_let_binding(&state, "let x = nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn check_let_binding_references_previous() {
        let mut state = ReplState::new();
        state.add_let_binding("let x = 5".to_string());
        let result = check_let_binding(&state, "let y = x + 1");
        assert!(result.is_ok());
    }

    // ----- Function definition checks -----

    #[test]
    fn check_valid_fn_definition() {
        let state = ReplState::new();
        let result = check_fn_definition(&state, "fn add(a: Int, b: Int) -> Int:\n    a + b");
        assert!(result.is_ok());
    }

    #[test]
    fn check_fn_definition_with_type_error() {
        let state = ReplState::new();
        let result = check_fn_definition(
            &state,
            "fn bad(a: Int) -> String:\n    a + 1",
        );
        assert!(result.is_err());
    }

    // ----- Meta command handling -----

    #[test]
    fn meta_command_help() {
        let mut state = ReplState::new();
        let mut output = Vec::new();
        handle_meta_command(":help", &mut state, &mut output, false);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("REPL commands"));
        assert!(text.contains(":quit"));
    }

    #[test]
    fn meta_command_state_empty() {
        let mut state = ReplState::new();
        let mut output = Vec::new();
        handle_meta_command(":state", &mut state, &mut output, false);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("no accumulated state"));
    }

    #[test]
    fn meta_command_state_with_definitions() {
        let mut state = ReplState::new();
        state.add_fn_definition("fn foo():\n    ()".to_string());
        state.add_let_binding("let x = 5".to_string());
        let mut output = Vec::new();
        handle_meta_command(":state", &mut state, &mut output, false);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Functions:"));
        assert!(text.contains("fn foo():"));
        assert!(text.contains("Bindings:"));
        assert!(text.contains("let x = 5"));
    }

    #[test]
    fn meta_command_reset() {
        let mut state = ReplState::new();
        state.add_let_binding("let x = 5".to_string());
        let mut output = Vec::new();
        handle_meta_command(":reset", &mut state, &mut output, false);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("state cleared"));
        assert!(state.let_bindings.is_empty());
        assert!(state.fn_definitions.is_empty());
    }

    #[test]
    fn meta_command_type_expr() {
        let mut state = ReplState::new();
        let mut output = Vec::new();
        handle_meta_command(":type 42", &mut state, &mut output, false);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains(": Int"));
    }

    #[test]
    fn meta_command_unknown() {
        let mut state = ReplState::new();
        let mut output = Vec::new();
        handle_meta_command(":foobar", &mut state, &mut output, false);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Unknown command"));
    }

    // ----- Default trait -----

    #[test]
    fn repl_state_default() {
        let state = ReplState::default();
        assert!(state.fn_definitions.is_empty());
        assert!(state.let_bindings.is_empty());
    }
}
