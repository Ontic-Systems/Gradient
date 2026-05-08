// gradient bench — Run benchmarks for the current project (E11 #371)
//
// Discovers all @bench functions in .gr source files, compiles a benchmark
// harness for each one, runs it with auto-tuned iteration counts, and
// reports per-iteration nanoseconds in a stable JSON format suitable for CI
// regression detection.
//
// Output format (acceptance criterion #2 — stable for CI baseline):
// {
//   "schema_version": 1,
//   "benches": [
//     { "name": "bench_add", "file": "src/lib.gr",
//       "iters": 1024, "total_ns": 12_345_000, "ns_per_iter": 12_055 }
//   ]
// }
//
// Compare-to-baseline mode (acceptance criterion #3):
//   gradient bench --baseline path/to/baseline.json
// Loads the prior JSON, compares ns_per_iter for each bench by name, and
// exits 1 if any bench regresses by more than the threshold (default 10%).

use crate::project::Project;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

/// Stable JSON schema version for `gradient bench` output. Bumping this is
/// a breaking change for downstream consumers (CI baselines, regression
/// dashboards). Keep schema additions backward-compatible when possible.
pub const BENCH_SCHEMA_VERSION: u32 = 1;

/// A single benchmark result emitted to JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchResult {
    /// The function name.
    pub name: String,
    /// Source file the bench was found in (relative to project root).
    pub file: String,
    /// Number of iterations the harness ran.
    pub iters: u64,
    /// Total wall-clock nanoseconds across all iterations.
    pub total_ns: u64,
    /// Mean nanoseconds per iteration (`total_ns / iters`, rounded down).
    pub ns_per_iter: u64,
}

/// Top-level bench report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchReport {
    /// JSON schema version.
    pub schema_version: u32,
    /// Per-bench results, in deterministic discovery order.
    pub benches: Vec<BenchResult>,
}

/// A discovered bench function.
#[derive(Debug, Clone)]
pub struct BenchCase {
    /// The file the bench was found in (absolute).
    pub file: PathBuf,
    /// Path relative to project root (for the JSON `file` field).
    pub rel_file: String,
    /// The function name.
    pub name: String,
    /// Whether the bench returns `Int` (true) or `()` (false).
    pub returns_int: bool,
}

/// Discover all `@bench` functions in `.gr` files under a directory.
pub fn discover_benches(src_dir: &Path, project_root: &Path) -> Vec<BenchCase> {
    let mut benches = Vec::new();
    let mut gr_files = find_gr_files(src_dir);
    // Deterministic order so JSON output is stable across runs.
    gr_files.sort();

    for file in &gr_files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let session = gradient_compiler::query::Session::from_source(&source);
        let mut found_in_file: Vec<BenchCase> = Vec::new();
        for sym in session.symbols() {
            if sym.is_bench {
                let returns_int = sym.ty.contains("-> Int");
                let rel_file = file
                    .strip_prefix(project_root)
                    .unwrap_or(file)
                    .display()
                    .to_string();
                found_in_file.push(BenchCase {
                    file: file.clone(),
                    rel_file,
                    name: sym.name.clone(),
                    returns_int,
                });
            }
        }
        // Sort within-file by name for deterministic output even when the
        // parser reports symbols in non-source order.
        found_in_file.sort_by(|a, b| a.name.cmp(&b.name));
        benches.extend(found_in_file);
    }

    benches
}

/// Recursively find all `.gr` files in a directory.
fn find_gr_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !dir.is_dir() {
        return files;
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_gr_files(&path));
            } else if path.extension().and_then(|e| e.to_str()) == Some("gr") {
                files.push(path);
            }
        }
    }
    files
}

/// Locate the C runtime helper relative to the compiler binary, mirroring
/// the search order used by `commands::build`. Returns `None` if no runtime
/// source can be found — the link step will then fail with an unresolved
/// `__gradient_*` symbol, which is the correct user-visible signal.
fn find_runtime_c(compiler: &Path) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = vec![
        compiler
            .parent()
            .map(|d| d.join("../../compiler/runtime/gradient_runtime.c"))
            .unwrap_or_default(),
        compiler
            .parent()
            .map(|d| d.join("runtime").join("gradient_runtime.c"))
            .unwrap_or_default(),
        PathBuf::from("../compiler/runtime/gradient_runtime.c"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// Compile `gradient_runtime.c` to an object file under `bench_dir`. Returns
/// `None` if the runtime source isn't present (the `gradient bench` use case
/// requires `now_ms()` so the bench will fail at link time, but the function
/// itself is non-fatal).
fn build_runtime_object(compiler: &Path, bench_dir: &Path) -> Option<PathBuf> {
    let rc = find_runtime_c(compiler)?;
    let ro = bench_dir.join("gradient_runtime.o");
    let status = Command::new("cc")
        .arg("-c")
        .arg(rc.to_str().unwrap_or(""))
        .arg("-o")
        .arg(ro.to_str().unwrap_or(""))
        .status()
        .ok()?;
    if status.success() {
        Some(ro)
    } else {
        None
    }
}

/// Strip any top-level `fn main` so we can append our own without a
/// duplicate-definition error. Mirrors `commands::test::strip_main_fn`.
fn strip_main_fn(source: &str) -> String {
    let mut out = Vec::new();
    let mut in_main = false;

    for line in source.lines() {
        if !in_main {
            let trimmed = line.trim_start();
            if trimmed.starts_with("fn main(") || trimmed.starts_with("fn main ") {
                in_main = true;
                continue;
            }
            out.push(line);
        } else if line.is_empty() || line.starts_with(' ') || line.starts_with('\t') {
            continue;
        } else {
            in_main = false;
            out.push(line);
        }
    }

    out.join("\n")
}

/// Generate a synthetic harness `.gr` that runs `bench.name` in a timed loop.
///
/// Auto-tuning protocol:
/// - Start at 1 iteration; double until elapsed_ms >= MIN_TARGET_MS or we hit
///   MAX_ITERS (whichever first).
/// - Print `BENCH_RESULT name=<n> iters=<n> total_ms=<n>` so the runner can
///   parse a single line. ms-precision because the runtime exposes
///   `now_ms()`; the host crate converts to ns by multiplying by 1_000_000.
///
/// Constants are inlined into the generated source so the harness has no
/// external knobs at the launch surface (acceptance criterion #2 — stable).
fn generate_harness(bench: &BenchCase, source_content: &str) -> String {
    let source_without_main = strip_main_fn(source_content);

    // The bench fn returns either Int (we discard the value) or ().
    // Until we have @hint(black_box), Int returns are not used to defeat DCE.
    // Cranelift -O0 is conservative enough that pure-fn DCE rarely happens.
    let call = if bench.returns_int {
        format!("        let _ = {}()\n", bench.name)
    } else {
        format!("        {}()\n", bench.name)
    };

    // The body uses a `while`-equivalent (recursive helper) plus the
    // already-available `now_ms()` builtin (env.rs registers it). We keep
    // the harness intentionally simple — auto-tuning is performed in the
    // harness loop itself by doubling iters across multiple compiled
    // runs would be expensive, so we use a single fixed-target inner loop.
    //
    // The strategy: run a calibration pass at 64 iters, project the
    // iter count needed to hit ~200ms, clamp to [128, 10_000_000], then
    // do the measured run.
    format!(
        r#"{src}

fn main() -> !{{IO,Time}} ():
    // Calibration pass at 64 iters.
    let calib_iters: Int = 64
    let calib_start: Int = now_ms()
    let mut i: Int = 0
    while i < calib_iters:
{call}        i = i + 1
    let calib_end: Int = now_ms()
    let calib_ms: Int = calib_end - calib_start

    // Target ~200ms of measured time. Project the iter count.
    // iters = max(128, min(10_000_000, calib_iters * 200 / max(calib_ms, 1)))
    let target_ms: Int = 200
    let safe_calib_ms: Int = max(calib_ms, 1)
    let projected: Int = calib_iters * target_ms / safe_calib_ms
    let iters: Int = max(128, min(projected, 10000000))

    // Measured pass.
    let start: Int = now_ms()
    let mut j: Int = 0
    while j < iters:
{call}        j = j + 1
    let end: Int = now_ms()
    let total_ms: Int = end - start

    print("BENCH_RESULT name={name} iters=")
    print_int(iters)
    print(" total_ms=")
    print_int(total_ms)
    println("")
"#,
        src = source_without_main,
        call = call,
        name = bench.name,
    )
}

/// Parse `BENCH_RESULT name=<n> iters=<i> total_ms=<m>` from harness stdout.
fn parse_harness_output(stdout: &str) -> Option<(u64, u64)> {
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("BENCH_RESULT ") {
            let mut iters: Option<u64> = None;
            let mut total_ms: Option<u64> = None;
            for part in rest.split_whitespace() {
                if let Some(v) = part.strip_prefix("iters=") {
                    iters = v.parse().ok();
                } else if let Some(v) = part.strip_prefix("total_ms=") {
                    total_ms = v.parse().ok();
                }
            }
            if let (Some(i), Some(m)) = (iters, total_ms) {
                return Some((i, m));
            }
        }
    }
    None
}

/// Default regression threshold (10%). A bench is flagged regressing if its
/// `ns_per_iter` exceeds `baseline.ns_per_iter * (1 + threshold)`.
pub const DEFAULT_REGRESSION_THRESHOLD: f64 = 0.10;

/// Compare a fresh report against a baseline. Returns the list of regressions
/// (each: name, baseline_ns_per_iter, current_ns_per_iter, ratio).
pub fn compare_to_baseline(
    fresh: &BenchReport,
    baseline: &BenchReport,
    threshold: f64,
) -> Vec<(String, u64, u64, f64)> {
    let mut regressions = Vec::new();
    for f in &fresh.benches {
        if let Some(b) = baseline.benches.iter().find(|b| b.name == f.name) {
            // Avoid div-by-zero on baselines that hit the resolution floor.
            if b.ns_per_iter == 0 {
                continue;
            }
            let ratio = f.ns_per_iter as f64 / b.ns_per_iter as f64;
            if ratio > 1.0 + threshold {
                regressions.push((f.name.clone(), b.ns_per_iter, f.ns_per_iter, ratio));
            }
        }
    }
    regressions
}

/// Execute the `gradient bench` subcommand.
pub fn execute(filter: Option<String>, baseline_path: Option<String>, json: bool) {
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

    let src_dir = project.root.join("src");
    if !src_dir.is_dir() {
        eprintln!("Error: No `src/` directory found in project root.");
        process::exit(1);
    }

    let mut benches = discover_benches(&src_dir, &project.root);
    if let Some(ref pattern) = filter {
        benches.retain(|b| b.name.contains(pattern.as_str()));
    }

    if benches.is_empty() {
        if json {
            let report = BenchReport {
                schema_version: BENCH_SCHEMA_VERSION,
                benches: Vec::new(),
            };
            let s = serde_json::to_string_pretty(&report).unwrap_or_default();
            println!("{}", s);
        } else {
            println!("No benchmarks found.");
        }
        return;
    }

    if !json {
        println!("Running {} benchmark(s)...\n", benches.len());
    }

    let bench_dir = project.root.join("target").join("bench");
    if let Err(e) = fs::create_dir_all(&bench_dir) {
        eprintln!("Error: Could not create bench directory: {}", e);
        process::exit(1);
    }

    // Compile the runtime helper once so every linked benchmark binary can
    // resolve `__gradient_now_ms` and friends.
    let runtime_o = build_runtime_object(&compiler, &bench_dir);
    if runtime_o.is_none() && !json {
        eprintln!("Warning: gradient_runtime.c not found next to compiler — link errors likely.");
    }

    let mut results: Vec<BenchResult> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for bench in &benches {
        let source_content = match fs::read_to_string(&bench.file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  Error reading {}: {}", bench.file.display(), e);
                failures.push(bench.name.clone());
                continue;
            }
        };

        let harness_source = generate_harness(bench, &source_content);
        let harness_path = bench_dir.join(format!("bench_{}.gr", bench.name));
        let object_path = bench_dir.join(format!("bench_{}.o", bench.name));
        let binary_path = bench_dir.join(format!("bench_{}", bench.name));

        if let Err(e) = fs::write(&harness_path, &harness_source) {
            eprintln!("  Error writing harness for {}: {}", bench.name, e);
            failures.push(bench.name.clone());
            continue;
        }

        // Compile.
        let compile_result = Command::new(&compiler)
            .arg(harness_path.to_str().unwrap())
            .arg(object_path.to_str().unwrap())
            .output();

        match compile_result {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !json {
                    eprintln!("  FAIL  {} (compile error)", bench.name);
                    if !stderr.is_empty() {
                        eprintln!("        {}", stderr.trim());
                    }
                }
                failures.push(bench.name.clone());
                continue;
            }
            Err(e) => {
                if !json {
                    eprintln!("  FAIL  {} (compiler not found: {})", bench.name, e);
                }
                failures.push(bench.name.clone());
                continue;
            }
        }

        // Link.
        let mut link_cmd = Command::new("cc");
        link_cmd.arg(object_path.to_str().unwrap());
        if let Some(ref ro) = runtime_o {
            link_cmd.arg(ro.to_str().unwrap());
        }
        link_cmd
            .arg("-o")
            .arg(binary_path.to_str().unwrap())
            .arg("-lcurl");
        let link_result = link_cmd.output();

        match link_result {
            Ok(out) if out.status.success() => {}
            Ok(_) => {
                if !json {
                    eprintln!("  FAIL  {} (link error)", bench.name);
                }
                failures.push(bench.name.clone());
                continue;
            }
            Err(e) => {
                if !json {
                    eprintln!("  FAIL  {} (linker not found: {})", bench.name, e);
                }
                failures.push(bench.name.clone());
                continue;
            }
        }

        // Run.
        let run_result = Command::new(&binary_path).output();
        let (iters, total_ms) = match run_result {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                match parse_harness_output(&stdout) {
                    Some(v) => v,
                    None => {
                        if !json {
                            eprintln!(
                                "  FAIL  {} (could not parse BENCH_RESULT line; stdout was: {:?})",
                                bench.name, stdout
                            );
                        }
                        failures.push(bench.name.clone());
                        continue;
                    }
                }
            }
            Ok(_) => {
                if !json {
                    eprintln!("  FAIL  {} (run error)", bench.name);
                }
                failures.push(bench.name.clone());
                continue;
            }
            Err(e) => {
                if !json {
                    eprintln!("  FAIL  {} (execution error: {})", bench.name, e);
                }
                failures.push(bench.name.clone());
                continue;
            }
        };

        let total_ns = total_ms.saturating_mul(1_000_000);
        let ns_per_iter = if iters == 0 {
            0
        } else {
            total_ns.checked_div(iters).unwrap_or(0)
        };
        let result = BenchResult {
            name: bench.name.clone(),
            file: bench.rel_file.clone(),
            iters,
            total_ns,
            ns_per_iter,
        };

        if !json {
            println!(
                "  {:<32} {:>12} ns/iter ({} iters, {} ms total)",
                bench.name, ns_per_iter, iters, total_ms,
            );
        }
        results.push(result);
    }

    let report = BenchReport {
        schema_version: BENCH_SCHEMA_VERSION,
        benches: results,
    };

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{}", s),
            Err(e) => {
                eprintln!("Error serializing report: {}", e);
                process::exit(1);
            }
        }
    } else {
        println!();
        println!(
            "bench result: {}. {} ran; {} failed",
            if failures.is_empty() { "ok" } else { "FAILED" },
            report.benches.len(),
            failures.len(),
        );
        if !failures.is_empty() {
            println!("failures:");
            for name in &failures {
                println!("    {}", name);
            }
        }
    }

    // Compare to baseline if requested.
    if let Some(path) = baseline_path {
        match fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<BenchReport>(&s) {
                Ok(baseline) => {
                    if baseline.schema_version != BENCH_SCHEMA_VERSION {
                        eprintln!(
                            "Warning: baseline schema_version {} does not match current {} — comparison may be unreliable.",
                            baseline.schema_version, BENCH_SCHEMA_VERSION
                        );
                    }
                    let regs =
                        compare_to_baseline(&report, &baseline, DEFAULT_REGRESSION_THRESHOLD);
                    if regs.is_empty() {
                        if !json {
                            println!("\nBaseline check: no regressions vs {}", path);
                        }
                    } else {
                        eprintln!("\nBaseline regressions vs {}:", path);
                        for (name, base, cur, ratio) in &regs {
                            eprintln!(
                                "    {:<32} {:>12} -> {:>12} ns/iter ({:.2}x slower)",
                                name, base, cur, ratio,
                            );
                        }
                        process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error: failed to parse baseline JSON at {}: {}", path, e);
                    process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("Error: could not read baseline file {}: {}", path, e);
                process::exit(1);
            }
        }
    }

    if !failures.is_empty() {
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_harness_output_extracts_iters_and_total_ms() {
        let stdout = "noise\nBENCH_RESULT name=foo iters=1024 total_ms=42\nmore noise\n";
        assert_eq!(parse_harness_output(stdout), Some((1024, 42)));
    }

    #[test]
    fn parse_harness_output_returns_none_when_missing() {
        assert!(parse_harness_output("nothing here\n").is_none());
    }

    #[test]
    fn parse_harness_output_returns_none_when_partial() {
        // Missing total_ms.
        assert!(parse_harness_output("BENCH_RESULT name=x iters=10\n").is_none());
    }

    #[test]
    fn compare_to_baseline_flags_regression_above_threshold() {
        let baseline = BenchReport {
            schema_version: BENCH_SCHEMA_VERSION,
            benches: vec![BenchResult {
                name: "b1".into(),
                file: "src/lib.gr".into(),
                iters: 1000,
                total_ns: 1_000_000,
                ns_per_iter: 1000,
            }],
        };
        let fresh = BenchReport {
            schema_version: BENCH_SCHEMA_VERSION,
            benches: vec![BenchResult {
                name: "b1".into(),
                file: "src/lib.gr".into(),
                iters: 1000,
                total_ns: 1_500_000,
                ns_per_iter: 1500,
            }],
        };
        let regs = compare_to_baseline(&fresh, &baseline, 0.10);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].0, "b1");
        assert_eq!(regs[0].1, 1000);
        assert_eq!(regs[0].2, 1500);
    }

    #[test]
    fn compare_to_baseline_ignores_within_threshold() {
        let baseline = BenchReport {
            schema_version: BENCH_SCHEMA_VERSION,
            benches: vec![BenchResult {
                name: "b1".into(),
                file: "src/lib.gr".into(),
                iters: 1000,
                total_ns: 1_000_000,
                ns_per_iter: 1000,
            }],
        };
        // 5% slower — within default 10% threshold.
        let fresh = BenchReport {
            schema_version: BENCH_SCHEMA_VERSION,
            benches: vec![BenchResult {
                name: "b1".into(),
                file: "src/lib.gr".into(),
                iters: 1000,
                total_ns: 1_050_000,
                ns_per_iter: 1050,
            }],
        };
        let regs = compare_to_baseline(&fresh, &baseline, 0.10);
        assert!(regs.is_empty());
    }

    #[test]
    fn compare_to_baseline_skips_zero_baseline() {
        let baseline = BenchReport {
            schema_version: BENCH_SCHEMA_VERSION,
            benches: vec![BenchResult {
                name: "b1".into(),
                file: "src/lib.gr".into(),
                iters: 1,
                total_ns: 0,
                ns_per_iter: 0,
            }],
        };
        let fresh = BenchReport {
            schema_version: BENCH_SCHEMA_VERSION,
            benches: vec![BenchResult {
                name: "b1".into(),
                file: "src/lib.gr".into(),
                iters: 1000,
                total_ns: 1_000_000,
                ns_per_iter: 1000,
            }],
        };
        let regs = compare_to_baseline(&fresh, &baseline, 0.10);
        assert!(
            regs.is_empty(),
            "Should not flag regression when baseline is 0"
        );
    }

    #[test]
    fn report_round_trips_through_json() {
        let report = BenchReport {
            schema_version: BENCH_SCHEMA_VERSION,
            benches: vec![BenchResult {
                name: "b1".into(),
                file: "src/lib.gr".into(),
                iters: 1024,
                total_ns: 12_345_000,
                ns_per_iter: 12055,
            }],
        };
        let s = serde_json::to_string(&report).unwrap();
        let parsed: BenchReport = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.schema_version, report.schema_version);
        assert_eq!(parsed.benches.len(), 1);
        assert_eq!(parsed.benches[0], report.benches[0]);
    }

    #[test]
    fn schema_version_is_stable() {
        // Bumping this constant is a breaking change. If you are intentionally
        // breaking the JSON wire format, update both the constant and this
        // assertion in the same PR, and document the migration in the PR body.
        assert_eq!(BENCH_SCHEMA_VERSION, 1);
    }

    #[test]
    fn strip_main_fn_removes_existing_main() {
        let src = "fn add(a: Int, b: Int) -> Int:\n    a + b\n\nfn main():\n    let x: Int = 1\n    print_int(x)\n";
        let stripped = strip_main_fn(src);
        assert!(!stripped.contains("fn main"));
        assert!(stripped.contains("fn add"));
    }
}
