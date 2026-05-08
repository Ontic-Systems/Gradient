// gradient doc — Generate API documentation from source
//
// Two modes:
// 1. Default: invokes the compiler with --doc and prints text/JSON to stdout
//    (existing MVP shipped via #424).
// 2. --html: renders a self-contained static HTML site to <out_dir>
//    (default: target/doc/) with per-function effect badges, capability
//    ceiling display, contracts/budget rendering, and a client-side
//    search box. This is the full surface tracked by E11 #372.
//
// The HTML mode bypasses the subprocess and links the compiler library
// directly (mirrors `commands::bench` which calls
// `gradient_compiler::query::Session::from_source`). This avoids
// re-parsing JSON and gives us typed access to ModuleDocumentation.

use crate::commands::doc_html;
use crate::project::Project;
use std::process::{self, Command};

/// Default output directory for `gradient doc --html`.
pub const DEFAULT_HTML_OUT_DIR: &str = "target/doc";

/// Execute the `gradient doc` subcommand.
///
/// `html` and `out_dir` are mutually informative: when `html` is true,
/// the renderer writes to `out_dir` (or `DEFAULT_HTML_OUT_DIR` when
/// `out_dir` is `None`). When `html` is false, the original `--json` /
/// `--pretty` text-and-JSON path runs and `out_dir` is ignored.
pub fn execute(verbose: bool, json: bool, pretty: bool, html: bool, out_dir: Option<String>) {
    if html {
        execute_html(verbose, out_dir);
        return;
    }

    execute_text_or_json(verbose, json, pretty);
}

/// Original MVP path: shells out to the compiler with `--doc` and lets it
/// print text or JSON to stdout. Preserved for backwards compatibility.
fn execute_text_or_json(verbose: bool, json: bool, pretty: bool) {
    let project = match Project::find() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    let compiler = match Project::find_compiler() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    let main_source = project.main_source();
    if !main_source.is_file() {
        eprintln!(
            "Error: Main source file not found at `{}`.\n\
             Every Gradient project needs a `src/main.gr`.",
            main_source.display()
        );
        process::exit(1);
    }

    if verbose {
        println!(
            "  Documenting: {} {} --doc{}{}",
            compiler.display(),
            main_source.display(),
            if json { " --json" } else { "" },
            if pretty { " --pretty" } else { "" }
        );
    }

    let mut cmd = Command::new(&compiler);
    cmd.arg(main_source.to_str().unwrap_or("src/main.gr"));
    cmd.arg("--doc");
    if json {
        cmd.arg("--json");
    }
    if pretty {
        cmd.arg("--pretty");
    }

    let status = cmd.status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!(
                "Documentation generation failed with exit code {}.",
                s.code().unwrap_or(1)
            );
            process::exit(s.code().unwrap_or(1));
        }
        Err(e) => {
            eprintln!(
                "Error: Failed to invoke compiler at `{}`: {}",
                compiler.display(),
                e
            );
            process::exit(1);
        }
    }
}

/// HTML path: link the compiler library, run `Session::documentation()`,
/// and render a static site.
fn execute_html(verbose: bool, out_dir: Option<String>) {
    let project = match Project::find() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    let main_source = project.main_source();
    if !main_source.is_file() {
        eprintln!(
            "Error: Main source file not found at `{}`.\n\
             Every Gradient project needs a `src/main.gr`.",
            main_source.display()
        );
        process::exit(1);
    }

    let out_dir_str = out_dir.unwrap_or_else(|| DEFAULT_HTML_OUT_DIR.to_string());
    let out_path = project.root.join(&out_dir_str);

    if verbose {
        println!(
            "  Rendering HTML docs from {} → {}",
            main_source.display(),
            out_path.display()
        );
    }

    // Read source. We could call Session::from_file but that triggers the
    // module resolver, which is overkill for single-file projects and adds
    // I/O complexity to error paths. The build-system already enforces
    // single-entry projects.
    let source = match std::fs::read_to_string(&main_source) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: Failed to read {}: {}", main_source.display(), e);
            process::exit(1);
        }
    };

    let session = gradient_compiler::query::Session::from_source(&source);
    let doc = session.documentation();

    if let Err(e) = doc_html::render_to_dir(&doc, &out_path) {
        eprintln!("Error: HTML rendering failed: {}", e);
        process::exit(1);
    }

    println!(
        "Documentation written to {}",
        out_path.join("index.html").display()
    );
}

#[cfg(test)]
mod tests {
    /// Verify the compiler is invoked with --doc.
    #[test]
    fn doc_command_uses_doc_flag() {
        let source = std::include_str!("doc.rs");
        assert!(
            source.contains(r#"cmd.arg("--doc")"#),
            "doc.rs must pass --doc to the compiler"
        );
    }

    /// Verify --json is forwarded when requested.
    #[test]
    fn doc_json_flag_forwarded() {
        let source = std::include_str!("doc.rs");
        assert!(
            source.contains(r#"cmd.arg("--json")"#),
            "doc.rs must forward --json to the compiler when requested"
        );
    }

    /// Verify --pretty is forwarded when requested.
    #[test]
    fn doc_pretty_flag_forwarded() {
        let source = std::include_str!("doc.rs");
        assert!(
            source.contains(r#"cmd.arg("--pretty")"#),
            "doc.rs must forward --pretty to the compiler when requested"
        );
    }

    /// Verify default HTML out-dir is target/doc (so CI / IDEs can
    /// auto-open it without extra flags).
    #[test]
    fn html_default_out_dir_is_target_doc() {
        assert_eq!(super::DEFAULT_HTML_OUT_DIR, "target/doc");
    }
}
