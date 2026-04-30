//! Integration gate for #235: kernel boundary metrics.
//!
//! Verifies the `kernel_boundary` catalog is internally consistent
//! and that `docs/SELF_HOSTING.md` references the same boundary
//! table this Rust catalog enumerates.

use std::fs;
use std::path::PathBuf;

use gradient_compiler::kernel_boundary::{
    self_hosted_progress_percent, total_kernel_externs, KERNEL_BOUNDARY,
};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn boundary_catalog_has_expected_phase_set() {
    let phases: Vec<&'static str> = KERNEL_BOUNDARY.iter().map(|r| r.phase).collect();
    let must_include = [
        "lex", "parse", "check", "lower", "emit", "pipeline", "driver", "query", "lsp", "trust",
    ];
    for p in &must_include {
        assert!(
            phases.contains(p),
            "kernel boundary catalog missing required phase `{}`",
            p
        );
    }
}

#[test]
fn rust_kernel_files_referenced_in_catalog_exist() {
    // Each `rust_kernel` field should point at file(s) under
    // codebase/compiler/src/. Some rows reference multiple files
    // separated by ` + ` (e.g. parse uses both
    // bootstrap_parser_bridge.rs and bootstrap_ast_bridge.rs).
    let src = workspace_root().join("codebase/compiler/src");
    for row in KERNEL_BOUNDARY {
        if row.rust_kernel == "(no new kernel)" {
            continue;
        }
        for file in row.rust_kernel.split(" + ").map(str::trim) {
            let path = src.join(file);
            assert!(
                path.exists(),
                "kernel boundary phase `{}` references missing file: {}",
                row.phase,
                path.display()
            );
        }
    }
}

#[test]
fn ci_gates_listed_in_catalog_have_test_files() {
    let tests = workspace_root().join("codebase/compiler/tests");
    for row in KERNEL_BOUNDARY {
        for gate in row.gates {
            // CI gates are integration test files, named
            // `<gate>.rs` under the tests directory. (Some rows list
            // unit-test names that live alongside their kernel
            // module — for now we just check at least one
            // candidate.)
            let integration = tests.join(format!("{}.rs", gate));
            // Some gates live as inline unit tests, not in tests/.
            // Allow either.
            let inline = workspace_root()
                .join("codebase/compiler/src")
                .join(format!("{}.rs", gate));
            assert!(
                integration.exists() || inline.exists() || gate.starts_with("self_hosting_smoke"),
                "kernel boundary phase `{}` references missing CI gate `{}`",
                row.phase,
                gate
            );
        }
    }
}

#[test]
fn self_hosting_doc_references_kernel_boundary() {
    let doc = workspace_root().join("docs/SELF_HOSTING.md");
    let body = fs::read_to_string(&doc).expect("read SELF_HOSTING.md");
    // The doc must mention the kernel-boundary metrics catalog
    // landed by #235 so future readers can find both halves.
    assert!(
        body.contains("kernel_boundary") || body.contains("Kernel Boundary"),
        "SELF_HOSTING.md must reference the kernel_boundary catalog (issue #235)"
    );
    // The doc should explicitly mention the phase progress metric
    // so the percent stays publicly tracked.
    assert!(
        body.contains("progress") || body.contains("Progress"),
        "SELF_HOSTING.md must mention progress tracking"
    );
}

#[test]
fn progress_percent_stays_meaningful() {
    let pct = self_hosted_progress_percent();
    assert!(
        pct >= 50,
        "self-hosted progress should remain at >=50% (was {}%)",
        pct
    );
    assert!(pct <= 100);
}

#[test]
fn total_extern_count_is_in_expected_range() {
    let total = total_kernel_externs();
    // The catalog tracks ~120-150 externs across all kernel
    // modules today. If this drops dramatically a phase row was
    // probably removed without updating the catalog.
    assert!(
        total >= 100,
        "total kernel extern count dropped below 100 (got {}); did a phase row get deleted without updating the catalog?",
        total
    );
}
