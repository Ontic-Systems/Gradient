//! Issue #235: define and shrink the Rust kernel boundary.
//!
//! This module is the single source of truth for the **Rust kernel
//! boundary** in the self-hosting architecture. It enumerates every
//! `bootstrap_*` extern surface that the .gr-side compiler delegates
//! to, classifies each surface by phase ownership, and exposes a
//! programmatic API the CI gate uses to track movement toward the
//! "95%+ in Gradient with minimal Rust kernel" target from #116.
//!
//! ## Why this exists in code, not just docs
//!
//! Without an executable boundary catalog the project can accumulate
//! tests and wrappers without measurably reducing Rust compiler
//! responsibility. By landing the catalog as a Rust module that's
//! exercised by a CI gate (`tests/kernel_boundary_metrics.rs`), we
//! get:
//!
//! 1. A list of Rust surfaces the .gr-side code actually depends on.
//! 2. A phase-by-phase ownership view (Rust-only / hybrid /
//!    self-hosted-gated / self-hosted-default) that the CI gate
//!    asserts to prevent silent regression.
//! 3. A measurable progress metric — the share of phases the
//!    self-hosted code can drive end-to-end.
//!
//! ## Public docs
//!
//! `docs/SELF_HOSTING.md` quotes the same table in human-readable
//! form. When the boundary changes, update both this module and
//! that doc; the CI gate keeps the row counts in sync.

/// Phase ownership classification used in the boundary table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseOwnership {
    /// Rust owns the canonical implementation. The self-hosted
    /// .gr code does not yet drive this phase end-to-end. Examples:
    /// platform integration, backend engines.
    RustOnly,
    /// Rust owns a kernel adapter (`bootstrap_*`) that the self-
    /// hosted code can call through. The self-hosted code mirrors
    /// the logic but the kernel still does the structural work.
    /// Examples: AST/IR runtime stores.
    Hybrid,
    /// Self-hosted code owns the canonical implementation; Rust
    /// keeps a thin runtime/FFI surface only. The .gr-side
    /// implementation is gated behind the kernel today (because of
    /// the `ModBlock` ExternFn typechecker quirk) but the kernel
    /// is fully exercised by a CI gate.
    SelfHostedGated,
    /// Self-hosted code is the production path; Rust has either
    /// been removed or kept only as a fallback. This is the
    /// 95%+ target state.
    SelfHostedDefault,
}

/// One row of the public kernel-boundary table.
#[derive(Debug, Clone, Copy)]
pub struct PhaseRow {
    /// Compiler phase name.
    pub phase: &'static str,
    /// Self-hosted source location (`compiler/<file>.gr`).
    pub gr_module: &'static str,
    /// Rust kernel module location (under
    /// `codebase/compiler/src/`).
    pub rust_kernel: &'static str,
    /// Current ownership classification.
    pub ownership: PhaseOwnership,
    /// Number of `bootstrap_*` externs the kernel exposes for this
    /// phase (counted manually; CI gate asserts the catalog stays
    /// in sync with the actual kernel modules).
    pub kernel_extern_count: usize,
    /// CI gate(s) that exercise this phase end-to-end.
    pub gates: &'static [&'static str],
}

/// The full kernel boundary table.
///
/// Adding a new bootstrap kernel module requires:
/// 1. Adding the row here with accurate `kernel_extern_count`.
/// 2. Updating `docs/SELF_HOSTING.md` table.
/// 3. Adding CI gate(s) under `tests/`.
/// 4. Re-running `cargo test --test kernel_boundary_metrics`.
pub const KERNEL_BOUNDARY: &[PhaseRow] = &[
    PhaseRow {
        phase: "lex",
        gr_module: "compiler/lexer.gr",
        rust_kernel: "bootstrap_lexer_bridge.rs",
        ownership: PhaseOwnership::Hybrid,
        kernel_extern_count: 5,
        gates: &["self_hosted_lexer_parity", "self_hosting_smoke"],
    },
    PhaseRow {
        phase: "parse",
        gr_module: "compiler/parser.gr",
        rust_kernel: "bootstrap_parser_bridge.rs + bootstrap_ast_bridge.rs",
        ownership: PhaseOwnership::Hybrid,
        kernel_extern_count: 25,
        gates: &[
            "parser_differential_tests",
            "parser_boundary_tests",
            "self_hosted_parser_token_access",
            "self_hosted_parser_ast_storage",
        ],
    },
    PhaseRow {
        phase: "check",
        gr_module: "compiler/checker.gr",
        rust_kernel: "bootstrap_checker_env.rs",
        ownership: PhaseOwnership::Hybrid,
        kernel_extern_count: 12,
        gates: &["self_hosted_checker_env"],
    },
    PhaseRow {
        phase: "lower",
        gr_module: "compiler/ir_builder.gr",
        rust_kernel: "bootstrap_ir_bridge.rs",
        ownership: PhaseOwnership::Hybrid,
        kernel_extern_count: 18,
        gates: &["ir_differential_tests", "self_hosted_ir_builder"],
    },
    PhaseRow {
        phase: "emit",
        gr_module: "compiler/codegen.gr",
        rust_kernel: "bootstrap_ir_emit.rs",
        ownership: PhaseOwnership::SelfHostedDefault,
        kernel_extern_count: 1,
        gates: &["self_hosted_codegen_text", "self_hosting_smoke"],
    },
    PhaseRow {
        phase: "pipeline",
        gr_module: "compiler/compiler.gr",
        rust_kernel: "bootstrap_pipeline.rs",
        ownership: PhaseOwnership::SelfHostedDefault,
        kernel_extern_count: 7,
        gates: &["self_hosted_pipeline", "self_hosting_smoke"],
    },
    PhaseRow {
        phase: "driver",
        gr_module: "compiler/main.gr",
        rust_kernel: "bootstrap_driver.rs",
        ownership: PhaseOwnership::SelfHostedGated,
        kernel_extern_count: 7,
        gates: &["self_hosted_driver"],
    },
    PhaseRow {
        phase: "query",
        gr_module: "compiler/query.gr",
        rust_kernel: "bootstrap_query.rs",
        ownership: PhaseOwnership::SelfHostedDefault,
        kernel_extern_count: 27,
        gates: &["self_hosted_query", "self_hosting_smoke"],
    },
    PhaseRow {
        phase: "lsp",
        gr_module: "compiler/lsp.gr",
        rust_kernel: "bootstrap_lsp.rs",
        ownership: PhaseOwnership::SelfHostedGated,
        kernel_extern_count: 28,
        gates: &["self_hosted_lsp"],
    },
    PhaseRow {
        phase: "trust",
        gr_module: "(meta — exercises every phase)",
        rust_kernel: "(no new kernel)",
        ownership: PhaseOwnership::SelfHostedGated,
        kernel_extern_count: 0,
        gates: &["bootstrap_trust_checks"],
    },
];

/// Total number of phase rows in the boundary table.
pub fn phase_count() -> usize {
    KERNEL_BOUNDARY.len()
}

/// Number of phases at or above `ownership`.
pub fn phase_count_at_least(ownership: PhaseOwnership) -> usize {
    KERNEL_BOUNDARY
        .iter()
        .filter(|r| ownership_rank(r.ownership) >= ownership_rank(ownership))
        .count()
}

/// Total number of `bootstrap_*` externs the kernel exposes across
/// all phases. Used by the CI gate as a sanity check that the
/// catalog stays close to reality.
pub fn total_kernel_externs() -> usize {
    KERNEL_BOUNDARY.iter().map(|r| r.kernel_extern_count).sum()
}

/// Self-hosted progress percentage, computed as the share of phases
/// classified at SelfHostedGated or SelfHostedDefault.
pub fn self_hosted_progress_percent() -> u32 {
    let total = phase_count() as u32;
    if total == 0 {
        return 0;
    }
    let self_hosted = phase_count_at_least(PhaseOwnership::SelfHostedGated) as u32;
    (self_hosted * 100) / total
}

fn ownership_rank(o: PhaseOwnership) -> u32 {
    match o {
        PhaseOwnership::RustOnly => 0,
        PhaseOwnership::Hybrid => 1,
        PhaseOwnership::SelfHostedGated => 2,
        PhaseOwnership::SelfHostedDefault => 3,
    }
}

/// Render the catalog as a Markdown table, suitable for use in
/// `docs/SELF_HOSTING.md` updates.
pub fn render_markdown_table() -> String {
    let mut out = String::new();
    out.push_str("| Phase | Self-hosted module | Rust kernel | Ownership | Externs | Gates |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    for row in KERNEL_BOUNDARY {
        out.push_str(&format!(
            "| {} | `{}` | `{}` | {:?} | {} | {} |\n",
            row.phase,
            row.gr_module,
            row.rust_kernel,
            row.ownership,
            row.kernel_extern_count,
            row.gates.join(", ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_table_is_non_empty() {
        assert!(phase_count() >= 8);
    }

    #[test]
    fn every_row_has_at_least_one_gate() {
        for row in KERNEL_BOUNDARY {
            assert!(
                !row.gates.is_empty(),
                "phase `{}` has no CI gate listed; every row in the kernel boundary must be exercised end-to-end",
                row.phase
            );
        }
    }

    #[test]
    fn no_phase_classified_as_rust_only() {
        // RustOnly is reserved for future rows (e.g. backend engines)
        // we have not yet listed. The current catalog should have
        // moved past RustOnly for every phase it tracks.
        for row in KERNEL_BOUNDARY {
            assert_ne!(
                row.ownership,
                PhaseOwnership::RustOnly,
                "phase `{}` is classified RustOnly; either move it forward or remove it from the catalog",
                row.phase
            );
        }
    }

    #[test]
    fn self_hosted_progress_is_meaningful() {
        let pct = self_hosted_progress_percent();
        assert!(
            pct >= 50,
            "expected at least half the phases to be SelfHostedGated; got {}%",
            pct
        );
    }

    #[test]
    fn markdown_table_renders() {
        let md = render_markdown_table();
        assert!(md.contains("| Phase |"));
        assert!(md.contains("bootstrap_query.rs"));
        assert!(md.contains("self_hosted_lsp"));
    }
}
