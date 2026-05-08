//! Integration tests for the Gradient LSP server.
//!
//! These tests spawn the `gradient-lsp` binary as a child process, send
//! JSON-RPC messages over stdin, and read responses from stdout. This
//! validates the full server lifecycle without needing a real editor.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Build and return the path to the `gradient-lsp` binary.
fn lsp_binary() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    // Build from the workspace root (parent of devtools/lsp)
    let workspace_root = std::path::Path::new(manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .expect("failed to find workspace root");

    let output = Command::new("cargo")
        .args(["build", "--bin", "gradient-lsp"])
        .current_dir(workspace_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to build gradient-lsp");

    assert!(
        output.status.success(),
        "cargo build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Workspace builds put binaries in workspace_root/target/debug/
    let binary = workspace_root.join("target/debug/gradient-lsp");

    assert!(binary.exists(), "binary not found at {:?}", binary);
    binary.to_str().unwrap().to_string()
}

/// Encode a JSON-RPC message with the Content-Length header required by LSP.
fn encode_lsp_message(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

/// Read a single LSP response from the given reader with a timeout.
///
/// Returns `None` if no complete message is available within the timeout.
fn read_lsp_response_timeout(
    reader: &mut BufReader<impl Read>,
    timeout: Duration,
) -> Option<String> {
    let start = Instant::now();

    let mut content_length: usize = 0;

    // Read headers.
    loop {
        if start.elapsed() > timeout {
            return None;
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return None,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    break;
                }
                if let Some(len_str) = trimmed.strip_prefix("Content-Length: ") {
                    content_length = len_str.parse().unwrap_or(0);
                }
            }
            Err(_) => return None,
        }
    }

    if content_length == 0 {
        return None;
    }

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

/// Read LSP responses until we find one matching the given predicate,
/// or until the timeout expires. Returns the matching JSON value.
fn read_until<F>(
    reader: &mut BufReader<impl Read>,
    timeout: Duration,
    predicate: F,
) -> Option<serde_json::Value>
where
    F: Fn(&serde_json::Value) -> bool,
{
    let start = Instant::now();
    loop {
        let remaining = timeout.checked_sub(start.elapsed())?;
        if let Some(response) = read_lsp_response_timeout(reader, remaining) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&response) {
                if predicate(&json) {
                    return Some(json);
                }
                // Otherwise skip this message (it's a notification or a different response).
            }
        } else {
            return None;
        }
    }
}

/// Helper: send a message to stdin.
fn send(stdin: &mut impl Write, msg: &serde_json::Value) {
    let body = msg.to_string();
    let encoded = encode_lsp_message(&body);
    stdin.write_all(encoded.as_bytes()).unwrap();
    stdin.flush().unwrap();
}

/// Helper: perform the full initialize handshake and return the initialize result.
fn initialize(stdin: &mut impl Write, reader: &mut BufReader<impl Read>) -> serde_json::Value {
    initialize_with_workspace(stdin, reader, None)
}

/// Initialize variant that advertises a workspace root via
/// `workspace_folders`, used by the `@untrusted`-default workspace
/// tests for #359.
fn initialize_with_workspace(
    stdin: &mut impl Write,
    reader: &mut BufReader<impl Read>,
    workspace: Option<&std::path::Path>,
) -> serde_json::Value {
    let mut params = serde_json::json!({ "capabilities": {} });
    if let Some(root) = workspace {
        let uri = format!("file://{}", root.display());
        params["workspaceFolders"] = serde_json::json!([{
            "uri": uri,
            "name": "test-workspace"
        }]);
    }
    let init = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": params,
    });
    send(stdin, &init);

    let response = read_until(reader, Duration::from_secs(10), |json| {
        json.get("id") == Some(&serde_json::json!(1))
    })
    .expect("no initialize response");

    // Send initialized notification.
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    send(stdin, &initialized);

    response
}

/// Helper: send shutdown + exit and kill the child.
fn shutdown_and_exit(stdin: &mut impl Write, child: &mut std::process::Child) {
    let shutdown = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 9999,
        "method": "shutdown",
        "params": null
    });
    let _ = send(stdin, &shutdown);

    // Small delay to allow shutdown response to be sent.
    std::thread::sleep(Duration::from_millis(100));

    let exit = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "exit",
        "params": null
    });
    let body = exit.to_string();
    let encoded = encode_lsp_message(&body);
    let _ = stdin.write_all(encoded.as_bytes());
    let _ = stdin.flush();

    // Give it a moment, then kill if still running.
    std::thread::sleep(Duration::from_millis(200));
    let _ = child.kill();
    let _ = child.wait();
}

/// Spawn the LSP server process.
fn spawn_server() -> (
    std::process::Child,
    Box<dyn Write + Send>,
    BufReader<std::process::ChildStdout>,
) {
    let binary = lsp_binary();
    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start gradient-lsp");

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);

    (child, Box::new(stdin), reader)
}

// ── Tests ────────────────────────────────────────────────────────────────

#[test]
fn test_initialize_capabilities() {
    let (mut child, mut stdin, mut reader) = spawn_server();

    let response = initialize(&mut stdin, &mut reader);

    // Verify capabilities.
    let caps = &response["result"]["capabilities"];
    assert!(caps.is_object(), "capabilities should be an object");
    assert!(
        response["result"]["serverInfo"]["name"] == "gradient-lsp",
        "server name should be gradient-lsp"
    );

    // Check hover is enabled.
    assert!(
        caps.get("hoverProvider").is_some(),
        "hover provider should be present"
    );

    // Check completion is enabled.
    assert!(
        caps.get("completionProvider").is_some(),
        "completion provider should be present"
    );

    // Check text document sync.
    assert!(
        caps.get("textDocumentSync").is_some(),
        "text document sync should be present"
    );

    shutdown_and_exit(&mut stdin, &mut child);
}

#[test]
fn test_completion_returns_keywords_and_builtins() {
    let (mut child, mut stdin, mut reader) = spawn_server();
    initialize(&mut stdin, &mut reader);

    // Request completions.
    let completion = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "textDocument/completion",
        "params": {
            "textDocument": { "uri": "file:///test.gr" },
            "position": { "line": 0, "character": 0 }
        }
    });
    send(&mut stdin, &completion);

    // Find the completion response (id=10).
    let response = read_until(&mut reader, Duration::from_secs(10), |json| {
        json.get("id") == Some(&serde_json::json!(10))
    })
    .expect("no completion response");

    let result = &response["result"];
    assert!(result.is_array(), "completion result should be an array");
    let items = result.as_array().unwrap();

    let labels: Vec<&str> = items
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();

    // Check keywords.
    assert!(labels.contains(&"fn"), "should contain 'fn' keyword");
    assert!(labels.contains(&"let"), "should contain 'let' keyword");
    assert!(labels.contains(&"if"), "should contain 'if' keyword");
    assert!(labels.contains(&"ret"), "should contain 'ret' keyword");
    assert!(labels.contains(&"for"), "should contain 'for' keyword");
    assert!(labels.contains(&"match"), "should contain 'match' keyword");
    assert!(labels.contains(&"true"), "should contain 'true' keyword");
    assert!(labels.contains(&"false"), "should contain 'false' keyword");

    // Check builtins.
    assert!(labels.contains(&"print"), "should contain 'print' builtin");
    assert!(labels.contains(&"abs"), "should contain 'abs' builtin");
    assert!(labels.contains(&"range"), "should contain 'range' builtin");
    assert!(labels.contains(&"min"), "should contain 'min' builtin");
    assert!(labels.contains(&"max"), "should contain 'max' builtin");

    // Verify that builtin items have function kind and detail.
    let print_item = items.iter().find(|i| i["label"] == "print").unwrap();
    assert_eq!(
        print_item["kind"],
        serde_json::json!(3), // CompletionItemKind::FUNCTION = 3
        "print should have function kind"
    );
    assert!(
        print_item["detail"].as_str().unwrap().contains("String"),
        "print detail should mention String"
    );

    shutdown_and_exit(&mut stdin, &mut child);
}

#[test]
fn test_diagnostics_on_broken_file() {
    let (mut child, mut stdin, mut reader) = spawn_server();
    initialize(&mut stdin, &mut reader);

    // Open a file with a syntax error (missing close paren in function params).
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///broken.gr",
                "languageId": "gradient",
                "version": 1,
                "text": "fn foo(x: Int -> Int:\n    ret x\n"
            }
        }
    });
    send(&mut stdin, &did_open);

    // Wait for publishDiagnostics notification.
    let response = read_until(&mut reader, Duration::from_secs(10), |json| {
        json.get("method") == Some(&serde_json::json!("textDocument/publishDiagnostics"))
            && json.pointer("/params/uri") == Some(&serde_json::json!("file:///broken.gr"))
    })
    .expect("no publishDiagnostics notification");

    let diags = response
        .pointer("/params/diagnostics")
        .and_then(|d| d.as_array())
        .expect("diagnostics should be an array");

    assert!(
        !diags.is_empty(),
        "broken file should produce at least one diagnostic"
    );

    shutdown_and_exit(&mut stdin, &mut child);
}

#[test]
fn test_diagnostics_on_clean_file() {
    let (mut child, mut stdin, mut reader) = spawn_server();
    initialize(&mut stdin, &mut reader);

    // Open a well-formed file. Note: the LSP defaults to `@untrusted`
    // mode (see #359), which requires explicit effects on every fn.
    // We use `@trusted` here to opt back in to inferred effects so the
    // test still exercises the typechecker happy-path. A separate test
    // (`test_lsp_defaults_to_untrusted_mode`) covers the default-on path.
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///clean.gr",
                "languageId": "gradient",
                "version": 1,
                "text": "@trusted\nfn add(a: Int, b: Int) -> Int:\n    ret a + b\n"
            }
        }
    });
    send(&mut stdin, &did_open);

    // Wait for publishDiagnostics notification for this specific file.
    let response = read_until(&mut reader, Duration::from_secs(10), |json| {
        json.get("method") == Some(&serde_json::json!("textDocument/publishDiagnostics"))
            && json.pointer("/params/uri") == Some(&serde_json::json!("file:///clean.gr"))
    })
    .expect("no publishDiagnostics notification");

    let diags = response
        .pointer("/params/diagnostics")
        .and_then(|d| d.as_array())
        .expect("diagnostics should be an array");

    assert!(
        diags.is_empty(),
        "clean file should produce no diagnostics, got: {:?}",
        diags
    );

    shutdown_and_exit(&mut stdin, &mut child);
}

#[test]
fn test_type_error_diagnostics() {
    let (mut child, mut stdin, mut reader) = spawn_server();
    initialize(&mut stdin, &mut reader);

    // Open a file with a type error: adding Int and String.
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///type_error.gr",
                "languageId": "gradient",
                "version": 1,
                "text": "fn main() -> !{IO} ():\n    let x: Int = \"hello\"\n    print(x)\n"
            }
        }
    });
    send(&mut stdin, &did_open);

    // Wait for publishDiagnostics.
    let response = read_until(&mut reader, Duration::from_secs(10), |json| {
        json.get("method") == Some(&serde_json::json!("textDocument/publishDiagnostics"))
            && json.pointer("/params/uri") == Some(&serde_json::json!("file:///type_error.gr"))
    })
    .expect("no publishDiagnostics notification");

    let diags = response
        .pointer("/params/diagnostics")
        .and_then(|d| d.as_array())
        .expect("diagnostics should be an array");

    assert!(
        !diags.is_empty(),
        "file with type error should produce diagnostics"
    );

    // At least one diagnostic should mention type mismatch.
    let has_type_error = diags
        .iter()
        .any(|d| d["source"].as_str() == Some("gradient-typechecker"));
    assert!(
        has_type_error,
        "should have a typechecker diagnostic, got: {:?}",
        diags
    );

    shutdown_and_exit(&mut stdin, &mut child);
}

#[test]
fn test_batch_diagnostics_notification() {
    let (mut child, mut stdin, mut reader) = spawn_server();
    initialize(&mut stdin, &mut reader);

    // Open a file with a lex error (easy to produce, doesn't hang).
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///batch.gr",
                "languageId": "gradient",
                "version": 1,
                "text": "let x = 42\n"
            }
        }
    });
    send(&mut stdin, &did_open);

    // Wait for the custom gradient/batchDiagnostics notification.
    let response = read_until(&mut reader, Duration::from_secs(10), |json| {
        json.get("method") == Some(&serde_json::json!("gradient/batchDiagnostics"))
    })
    .expect("no gradient/batchDiagnostics notification");

    let params = &response["params"];
    assert!(
        params.get("uri").is_some(),
        "batch diagnostics should contain a uri"
    );
    assert!(
        params.get("diagnostics").is_some(),
        "batch diagnostics should contain diagnostics array"
    );
    assert!(
        params.get("parse_errors").is_some(),
        "batch diagnostics should contain parse_errors count"
    );
    assert!(
        params.get("type_errors").is_some(),
        "batch diagnostics should contain type_errors count"
    );

    shutdown_and_exit(&mut stdin, &mut child);
}

// ── #359 — LSP defaults to `@untrusted` mode ─────────────────────────────

/// Create a unique temp directory for a single test. Cleanup is
/// best-effort — the same directory is removed on entry so re-runs are
/// idempotent.
fn workspace_tmp_dir(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("gradient-lsp-359-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Write a `.gradient/lsp.toml` config under `root` with the given
/// `untrusted` value.
fn write_lsp_config(root: &std::path::Path, body: &str) {
    let cfg_dir = root.join(".gradient");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("lsp.toml"), body).unwrap();
}

/// Acceptance criterion 1: with no `.gradient/lsp.toml`, the LSP must
/// treat unannotated documents as `@untrusted` — surfacing the same
/// diagnostics that `gradient check --untrusted` would produce. This
/// closes adversarial finding F4 (input-surface workspace default).
#[test]
fn test_lsp_defaults_to_untrusted_mode() {
    let workspace = workspace_tmp_dir("default");
    let (mut child, mut stdin, mut reader) = spawn_server();
    initialize_with_workspace(&mut stdin, &mut reader, Some(&workspace));

    // A program with FFI passes under default `@trusted` but should be
    // rejected under the new LSP `@untrusted` default.
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///359-default.gr",
                "languageId": "gradient",
                "version": 1,
                "text": "@extern(\"libc\")\nfn puts(s: String) -> Int\n\nfn main() -> Int:\n    ret 0\n"
            }
        }
    });
    send(&mut stdin, &did_open);

    let response = read_until(&mut reader, Duration::from_secs(10), |json| {
        json.get("method") == Some(&serde_json::json!("gradient/batchDiagnostics"))
            && json.pointer("/params/uri") == Some(&serde_json::json!("file:///359-default.gr"))
    })
    .expect("no batchDiagnostics for 359-default.gr");

    let trust_mode = response
        .pointer("/params/trust_mode")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    assert_eq!(
        trust_mode, "untrusted",
        "LSP must default to untrusted mode (#359 acceptance #1); got `{}`",
        trust_mode
    );

    let diags = response
        .pointer("/params/diagnostics")
        .and_then(|d| d.as_array())
        .expect("diagnostics array");
    let has_untrusted_rejection = diags.iter().any(|d| {
        d["message"]
            .as_str()
            .map(|m| m.contains("untrusted"))
            .unwrap_or(false)
    });
    assert!(
        has_untrusted_rejection,
        "expected an `@untrusted` rejection diagnostic, got: {:?}",
        diags
    );

    shutdown_and_exit(&mut stdin, &mut child);
}

/// Acceptance criterion 2: `.gradient/lsp.toml` lets a workspace opt
/// out by setting `untrusted = false`. The same FFI program that's
/// rejected under the default must pass once the config opts back in
/// to the trusted posture.
#[test]
fn test_lsp_workspace_can_opt_in_to_trusted_mode() {
    let workspace = workspace_tmp_dir("opt-in");
    write_lsp_config(&workspace, "untrusted = false\n");

    let (mut child, mut stdin, mut reader) = spawn_server();
    initialize_with_workspace(&mut stdin, &mut reader, Some(&workspace));

    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///359-optin.gr",
                "languageId": "gradient",
                "version": 1,
                "text": "@extern(\"libc\")\nfn puts(s: String) -> Int\n\nfn main() -> Int:\n    ret 0\n"
            }
        }
    });
    send(&mut stdin, &did_open);

    let response = read_until(&mut reader, Duration::from_secs(10), |json| {
        json.get("method") == Some(&serde_json::json!("gradient/batchDiagnostics"))
            && json.pointer("/params/uri") == Some(&serde_json::json!("file:///359-optin.gr"))
    })
    .expect("no batchDiagnostics for 359-optin.gr");

    let trust_mode = response
        .pointer("/params/trust_mode")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    assert_eq!(
        trust_mode, "trusted",
        "with `untrusted = false`, LSP must keep modules trusted; got `{}`",
        trust_mode
    );

    let diags = response
        .pointer("/params/diagnostics")
        .and_then(|d| d.as_array())
        .expect("diagnostics array");
    let untrusted_diags: Vec<_> = diags
        .iter()
        .filter(|d| {
            d["message"]
                .as_str()
                .map(|m| m.contains("untrusted"))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        untrusted_diags.is_empty(),
        "no untrusted diagnostics expected when opted out, got: {:?}",
        untrusted_diags
    );

    shutdown_and_exit(&mut stdin, &mut child);
}

/// Source-level `@trusted` always wins over the workspace default,
/// matching the behavior documented in
/// `docs/security/untrusted-source-mode.md` and the source-mode flag
/// in PR #508. This is the per-file escape hatch.
#[test]
fn test_explicit_trusted_attribute_overrides_lsp_default() {
    let workspace = workspace_tmp_dir("explicit-trusted");
    // Workspace default is `untrusted = true` (no config file).
    let (mut child, mut stdin, mut reader) = spawn_server();
    initialize_with_workspace(&mut stdin, &mut reader, Some(&workspace));

    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///359-explicit-trusted.gr",
                "languageId": "gradient",
                "version": 1,
                "text": "@trusted\n@extern(\"libc\")\nfn puts(s: String) -> Int\n\nfn main() -> Int:\n    ret 0\n"
            }
        }
    });
    send(&mut stdin, &did_open);

    let response = read_until(&mut reader, Duration::from_secs(10), |json| {
        json.get("method") == Some(&serde_json::json!("gradient/batchDiagnostics"))
            && json.pointer("/params/uri")
                == Some(&serde_json::json!("file:///359-explicit-trusted.gr"))
    })
    .expect("no batchDiagnostics for 359-explicit-trusted.gr");

    let trust_mode = response
        .pointer("/params/trust_mode")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    assert_eq!(
        trust_mode, "trusted",
        "explicit @trusted must beat the workspace default; got `{}`",
        trust_mode
    );

    shutdown_and_exit(&mut stdin, &mut child);
}
