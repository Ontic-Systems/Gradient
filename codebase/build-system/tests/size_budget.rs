//! End-to-end hello-world size-budget integration tests (E5 #338).
//!
//! Locks the post-strip binary size of representative `gradient build`
//! outputs against absolute byte budgets so future PRs that bloat the
//! runtime get caught at CI time. Sibling of `tests/{panic,alloc,actor,
//! async}_runtime.rs` — same project-tempdir + verbose-build pattern,
//! but the assertion is on `fs::metadata(...).len()` after `strip`.
//!
//! Two budgets, two programs:
//!   - **embedded**: pure arithmetic + IO printing only. Selects every
//!     `none`/`minimal` runtime variant (alloc=minimal, actor=none,
//!     async=none, panic=unwind by default). Represents the "no_std-ish"
//!     end of the dial that Epic E5's modular runtime is supposed to
//!     keep small.
//!   - **full**: declares `Heap` via `String + String` to flip the
//!     alloc strategy to `full`. Represents an app-mode build that
//!     drags in rc/COW machinery.
//!
//! The acceptance criteria in the parent issue (`no_std ≤ 4KB,
//! full ≤ 200KB`) are aspirational — they assume the variant runtimes
//! actually shrink the canonical runtime, which today they don't (the
//! variant crates are tag-only; the canonical `gradient_runtime.c`
//! still drags in libcurl + IO helpers). The budgets below reflect
//! today's measured size-on-Linux-CI plus a small headroom and are
//! intended to TIGHTEN as the follow-on extractions land:
//!
//!   - #538 follow-on: extract rc/COW machinery into `runtime_alloc_full.c`.
//!   - #539 follow-on: extract actor-scheduler into `runtime_actor_full.c`.
//!   - #540 follow-on: extract async-executor into `runtime_async_full.c`.
//!
//! When any of those land and produce a measurable delta, retighten
//! the relevant budget here in the same PR. The whole point of this
//! gate is "binary size only goes down".
//!
//! Marked Unix-only because the build subprocess shells out to `cc`
//! and the tests shell out to `strip`.

#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Stripped-binary budget for the embedded program (no Heap, no Actor,
/// no Async). Today's measured Linux-CI size is ~76 KB; budget set to
/// 100 KB to absorb minor cc/glibc variation across runners. Tighten
/// as the alloc/actor/async runtime extractions land.
const BUDGET_EMBEDDED_STRIPPED_BYTES: u64 = 100_000;

/// Stripped-binary budget for the full app-mode program (declares Heap
/// via `String + String`). Today's measured Linux-CI size is ~76 KB
/// (essentially the same as embedded — variant runtimes are tag-only).
/// Budget set to 250 KB to leave room for the rc/COW machinery
/// extraction follow-on; tighten once that lands.
const BUDGET_FULL_STRIPPED_BYTES: u64 = 250_000;

fn gradient_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gradient"))
}

/// Locate the `gradient-compiler` sibling binary so the build subprocess
/// can invoke it via the `GRADIENT_COMPILER` env var. Cargo doesn't
/// automatically expose other-crate bins via `CARGO_BIN_EXE_*`, so we
/// walk up from the current binary's parent dir (= `target/debug/deps`)
/// to `target/debug` and look for `gradient-compiler` there.
fn compiler_bin() -> Option<PathBuf> {
    let me = PathBuf::from(env!("CARGO_BIN_EXE_gradient"));
    let target_debug = me.parent()?;
    let candidate = target_debug.join("gradient-compiler");
    if candidate.is_file() {
        return Some(candidate);
    }
    let mut cur = target_debug.parent()?;
    for _ in 0..6 {
        let alt = cur.join("target").join("debug").join("gradient-compiler");
        if alt.is_file() {
            return Some(alt);
        }
        cur = cur.parent()?;
    }
    None
}

fn write_project(dir: &Path, name: &str, source: &str) {
    fs::write(
        dir.join("gradient.toml"),
        format!("[package]\nname = \"{}\"\nversion = \"0.1.0\"\n", name),
    )
    .expect("write manifest");
    let src_dir = dir.join("src");
    fs::create_dir_all(&src_dir).expect("mkdir src");
    fs::write(src_dir.join("main.gr"), source).expect("write main.gr");
}

fn run_build(project_root: &Path) -> std::process::Output {
    let compiler = compiler_bin().expect(
        "gradient-compiler binary not found — build the workspace first \
         (cargo build -p gradient-compiler) or run via cargo test --workspace",
    );
    let env_path = match std::env::var_os("PATH") {
        Some(p) => p,
        None => OsString::from(""),
    };
    Command::new(gradient_bin())
        .arg("build")
        .arg("--verbose")
        .current_dir(project_root)
        .env("GRADIENT_COMPILER", &compiler)
        .env("PATH", env_path)
        .output()
        .expect("invoke gradient build")
}

/// Strip a binary in-place using the system `strip` utility. Used to
/// match the production size users will actually ship; debug symbols
/// blow up unstripped builds by ~50% and are not part of the runtime
/// contract.
fn strip_in_place(path: &Path) {
    let status = Command::new("strip")
        .arg("--strip-all")
        .arg(path)
        .status()
        .expect("invoke strip");
    assert!(
        status.success(),
        "strip --strip-all failed for {}",
        path.display()
    );
}

/// Pick out the `Binary size: N bytes (alloc=A, panic=P, actor=AC,
/// async=AS)` line from verbose stdout so size-regression failures can
/// say which variant runtimes were linked. Returns the verbatim line
/// or `<binary-size line not found>` for the assertion message.
fn extract_size_line(stdout: &str) -> String {
    stdout
        .lines()
        .find(|l| l.trim_start().starts_with("Binary size:"))
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| "<binary-size line not found>".to_string())
}

#[test]
fn embedded_hello_world_size_under_budget() {
    // Pure arithmetic + print_int — no Heap, no Actor, no Async,
    // default panic=unwind. Selects alloc=minimal + actor=none +
    // async=none variants of every modular runtime axis E5 has shipped.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "size_embedded",
        r#"fn main() -> !{IO} ():
    print_int(0)
"#,
    );

    let out = run_build(dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "build failed; stdout={stdout}\nstderr={stderr}"
    );

    // Sanity-pin the variant selection so a future change that
    // accidentally promotes alloc=full or actor=full doesn't silently
    // pass the size budget by linking heavier runtimes that happen to
    // strip below the cap.
    let size_line = extract_size_line(&stdout);
    assert!(
        stdout.contains("alloc=minimal")
            && stdout.contains("actor=none")
            && stdout.contains("async=none"),
        "expected embedded program to select alloc=minimal/actor=none/async=none; \
         verbose size line was: {size_line}"
    );

    let bin = dir.path().join("target/debug/size_embedded");
    assert!(bin.is_file(), "expected output binary at {}", bin.display());
    strip_in_place(&bin);
    let stripped = fs::metadata(&bin).expect("stat stripped binary").len();
    assert!(
        stripped <= BUDGET_EMBEDDED_STRIPPED_BYTES,
        "embedded hello-world stripped binary size {} bytes exceeds budget {} bytes\n\
         verbose size line: {size_line}\n\
         (if a runtime extraction PR INCREASED the size, that's a regression; \
          if it DECREASED size below budget, tighten BUDGET_EMBEDDED_STRIPPED_BYTES \
          in the same PR)",
        stripped,
        BUDGET_EMBEDDED_STRIPPED_BYTES,
    );
}

#[test]
fn full_app_mode_hello_world_size_under_budget() {
    // String concatenation propagates Heap (#532) so this flips the
    // alloc strategy to `full`. Print + IO are still selected by the
    // canonical runtime path. This represents an app-mode build that
    // drags in the rc/COW machinery once that's extracted into
    // `runtime_alloc_full.c` (#538 follow-on).
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "size_full",
        r#"fn main() -> !{IO, Heap} ():
    let g: String = "Hello" + ", " + "Gradient!"
    print(g)
"#,
    );

    let out = run_build(dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "build failed; stdout={stdout}\nstderr={stderr}"
    );

    // Pin the variant selection so a future regression that flips the
    // program back to alloc=minimal isn't silently rewarded with a
    // tighter passing size.
    let size_line = extract_size_line(&stdout);
    assert!(
        stdout.contains("alloc=full"),
        "expected full app-mode program to select alloc=full; \
         verbose size line was: {size_line}"
    );

    let bin = dir.path().join("target/debug/size_full");
    assert!(bin.is_file(), "expected output binary at {}", bin.display());
    strip_in_place(&bin);
    let stripped = fs::metadata(&bin).expect("stat stripped binary").len();
    assert!(
        stripped <= BUDGET_FULL_STRIPPED_BYTES,
        "full app-mode hello-world stripped binary size {} bytes exceeds budget {} bytes\n\
         verbose size line: {size_line}\n\
         (if a runtime extraction PR INCREASED the size, that's a regression; \
          if it DECREASED size below budget, tighten BUDGET_FULL_STRIPPED_BYTES \
          in the same PR)",
        stripped,
        BUDGET_FULL_STRIPPED_BYTES,
    );
}

#[test]
fn embedded_smaller_than_full_or_equal_today() {
    // Defense-in-depth invariant: the embedded variant must NEVER be
    // larger than the full variant. Today the variant runtimes are
    // tag-only so the two binaries are essentially the same size
    // (variance comes from the tag string in the linked object).
    // After the rc/COW + actor + async extractions land, this gap will
    // grow to ~5-10 KB and this test continues to pass.
    //
    // If a future change inverts this invariant — embedded somehow
    // ends up bigger than full — that's a hint the variant-selection
    // logic regressed (linked the wrong object) or the canonical
    // runtime grew code paths only the minimal variant exercises.
    // Either is a bug worth catching here rather than at user-report
    // time.
    let embedded_dir = tempfile::tempdir().expect("tempdir");
    write_project(
        embedded_dir.path(),
        "size_emb",
        r#"fn main() -> !{IO} ():
    print_int(0)
"#,
    );
    let emb_out = run_build(embedded_dir.path());
    assert!(
        emb_out.status.success(),
        "embedded build failed: {}",
        String::from_utf8_lossy(&emb_out.stderr)
    );
    let emb_bin = embedded_dir.path().join("target/debug/size_emb");
    strip_in_place(&emb_bin);
    let emb_size = fs::metadata(&emb_bin).expect("stat emb").len();

    let full_dir = tempfile::tempdir().expect("tempdir");
    write_project(
        full_dir.path(),
        "size_full2",
        r#"fn main() -> !{IO, Heap} ():
    let g: String = "Hello" + ", " + "World"
    print(g)
"#,
    );
    let full_out = run_build(full_dir.path());
    assert!(
        full_out.status.success(),
        "full build failed: {}",
        String::from_utf8_lossy(&full_out.stderr)
    );
    let full_bin = full_dir.path().join("target/debug/size_full2");
    strip_in_place(&full_bin);
    let full_size = fs::metadata(&full_bin).expect("stat full").len();

    // Allow exact equality (today's reality with tag-only runtimes)
    // and embedded-strictly-smaller (post-extraction reality). Reject
    // embedded-strictly-larger.
    assert!(
        emb_size <= full_size,
        "embedded stripped size {emb_size} bytes is LARGER than full {full_size} bytes \
         — variant-selection regression or canonical-runtime growth in minimal-only path"
    );
}

#[test]
fn embedded_size_diagnostic_includes_runtime_axes() {
    // The failure-mode acceptance from #338 says: "Failure mode shows
    // which crate was unexpectedly linked". The verbose binary-size
    // line covers exactly that — alloc/panic/actor/async tags lock the
    // selected variants, so a budget-exceeded failure can blame the
    // right axis. This test pins the format so a future change that
    // drops a tag from the verbose line breaks loudly.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "size_diag",
        r#"fn main() -> !{IO} ():
    print_int(0)
"#,
    );
    let out = run_build(dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "build failed: {stdout}");
    let size_line = extract_size_line(&stdout);
    assert!(
        size_line.contains("alloc=")
            && size_line.contains("panic=")
            && size_line.contains("actor=")
            && size_line.contains("async="),
        "verbose Binary-size line MUST include alloc=/panic=/actor=/async= tags so \
         a budget-exceeded failure can attribute the regression to the right axis; \
         got: {size_line}"
    );
}
