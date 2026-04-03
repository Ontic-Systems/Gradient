// gradient test — Run tests for the current project
//
// Discovers all @test functions in .gr source files, compiles a test harness
// for each one, executes it, and reports results.

use crate::project::Project;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

/// A discovered test function.
#[derive(Debug, Clone)]
pub struct TestCase {
    /// The file the test was found in (relative to project root).
    pub file: PathBuf,
    /// The function name.
    pub name: String,
    /// Whether the test returns Bool (true) or () (false).
    pub returns_bool: bool,
}

/// Discover all @test functions in .gr files under a directory.
///
/// Uses the compiler's `Session` API to parse each file and inspect its
/// symbols for the `is_test` flag.
pub fn discover_tests(src_dir: &Path) -> Vec<TestCase> {
    let mut tests = Vec::new();
    let gr_files = find_gr_files(src_dir);

    for file in &gr_files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let session = gradient_compiler::query::Session::from_source(&source);
        for sym in session.symbols() {
            if sym.is_test {
                let returns_bool = sym.ty.contains("-> Bool");
                tests.push(TestCase {
                    file: file.clone(),
                    name: sym.name.clone(),
                    returns_bool,
                });
            }
        }
    }

    tests
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

/// Generate a synthetic test harness `.gr` file for a single test function.
///
/// The harness imports the test function's source module and calls the test.
/// For Bool-returning tests: if it returns false, exit with code 1.
/// For ()-returning tests: just call it (panics will cause non-zero exit).
fn generate_harness(test: &TestCase, source_content: &str) -> String {
    // We inline the test source and add a main function that calls the test.
    // This avoids needing an import system — just concatenate the source
    // with a synthetic main that calls the test function.
    if test.returns_bool {
        format!(
            "{}\n\nfn main() -> !{{IO}} ():\n    if {}():\n        print(\"\")\n    else:\n        exit(1)\n",
            source_content, test.name
        )
    } else {
        format!(
            "{}\n\nfn main() -> !{{IO}} ():\n    {}()\n",
            source_content, test.name
        )
    }
}

/// Execute the `gradient test` subcommand.
pub fn execute(filter: Option<String>) {
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

    // 1. Discover tests
    let mut tests = discover_tests(&src_dir);

    // 2. Apply filter
    if let Some(ref pattern) = filter {
        tests.retain(|t| t.name.contains(pattern.as_str()));
    }

    if tests.is_empty() {
        println!("No tests found.");
        return;
    }

    println!("Running {} test(s)...\n", tests.len());

    // 3. Create temp directory for test harness files
    let test_dir = project.root.join("target").join("test");
    if let Err(e) = fs::create_dir_all(&test_dir) {
        eprintln!("Error: Could not create test directory: {}", e);
        process::exit(1);
    }

    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut failures: Vec<String> = Vec::new();

    // 4. Run each test
    for test in &tests {
        let source_content = match fs::read_to_string(&test.file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  Error reading {}: {}", test.file.display(), e);
                failed += 1;
                failures.push(test.name.clone());
                continue;
            }
        };

        let harness_source = generate_harness(test, &source_content);
        let harness_path = test_dir.join(format!("test_{}.gr", test.name));
        let object_path = test_dir.join(format!("test_{}.o", test.name));
        let binary_path = test_dir.join(format!("test_{}", test.name));

        // Write the harness file
        if let Err(e) = fs::write(&harness_path, &harness_source) {
            eprintln!("  Error writing harness for {}: {}", test.name, e);
            failed += 1;
            failures.push(test.name.clone());
            continue;
        }

        // Compile
        let compile_result = Command::new(&compiler)
            .arg(harness_path.to_str().unwrap())
            .arg(object_path.to_str().unwrap())
            .output();

        match compile_result {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("  FAIL  {} (compile error)", test.name);
                if !stderr.is_empty() {
                    eprintln!("        {}", stderr.trim());
                }
                failed += 1;
                failures.push(test.name.clone());
                continue;
            }
            Err(e) => {
                eprintln!("  FAIL  {} (compiler not found: {})", test.name, e);
                failed += 1;
                failures.push(test.name.clone());
                continue;
            }
        }

        // Link
        let link_result = Command::new("cc")
            .arg(object_path.to_str().unwrap())
            .arg("-o")
            .arg(binary_path.to_str().unwrap())
            .output();

        match link_result {
            Ok(output) if output.status.success() => {}
            Ok(_) => {
                eprintln!("  FAIL  {} (link error)", test.name);
                failed += 1;
                failures.push(test.name.clone());
                continue;
            }
            Err(e) => {
                eprintln!("  FAIL  {} (linker not found: {})", test.name, e);
                failed += 1;
                failures.push(test.name.clone());
                continue;
            }
        }

        // Run
        let run_result = Command::new(&binary_path).output();

        match run_result {
            Ok(output) if output.status.success() => {
                println!("  PASS  {}", test.name);
                passed += 1;
            }
            Ok(_) => {
                eprintln!("  FAIL  {}", test.name);
                failed += 1;
                failures.push(test.name.clone());
            }
            Err(e) => {
                eprintln!("  FAIL  {} (execution error: {})", test.name, e);
                failed += 1;
                failures.push(test.name.clone());
            }
        }
    }

    // 5. Summary
    let total = passed + failed;
    println!();
    if !failures.is_empty() {
        println!("failures:");
        for name in &failures {
            println!("    {}", name);
        }
        println!();
    }
    println!(
        "test result: {}. {} passed; {} failed; {} total",
        if failed == 0 { "ok" } else { "FAILED" },
        passed,
        failed,
        total,
    );

    // 6. Exit with code 1 if any test failed
    if failed > 0 {
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper to create a temp directory with test source files.
    fn create_test_project(dir: &Path, files: &[(&str, &str)]) {
        let src_dir = dir.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        for (name, content) in files {
            fs::write(src_dir.join(name), content).unwrap();
        }
    }

    #[test]
    fn discover_tests_finds_test_functions() {
        let dir = std::env::temp_dir().join("gradient_test_discover_1");
        let _ = fs::remove_dir_all(&dir);
        create_test_project(
            &dir,
            &[(
                "math_test.gr",
                "@test\nfn test_add() -> Bool:\n    1 + 1 == 2\n",
            )],
        );

        let tests = discover_tests(&dir.join("src"));
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].name, "test_add");
        assert!(tests[0].returns_bool);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_tests_ignores_non_test_functions() {
        let dir = std::env::temp_dir().join("gradient_test_discover_2");
        let _ = fs::remove_dir_all(&dir);
        create_test_project(
            &dir,
            &[("lib.gr", "fn add(a: Int, b: Int) -> Int:\n    a + b\n")],
        );

        let tests = discover_tests(&dir.join("src"));
        assert_eq!(tests.len(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_tests_multiple_files() {
        let dir = std::env::temp_dir().join("gradient_test_discover_3");
        let _ = fs::remove_dir_all(&dir);
        let src_dir = dir.join("src");
        fs::create_dir_all(&src_dir).unwrap();

        fs::write(
            src_dir.join("a.gr"),
            "@test\nfn test_a() -> Bool:\n    true\n",
        )
        .unwrap();
        fs::write(
            src_dir.join("b.gr"),
            "@test\nfn test_b():\n    let x: Int = 1\n",
        )
        .unwrap();

        let tests = discover_tests(&src_dir);
        assert_eq!(tests.len(), 2);

        let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"test_a"));
        assert!(names.contains(&"test_b"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_tests_respects_filter() {
        let dir = std::env::temp_dir().join("gradient_test_discover_4");
        let _ = fs::remove_dir_all(&dir);
        let src_dir = dir.join("src");
        fs::create_dir_all(&src_dir).unwrap();

        fs::write(
            src_dir.join("tests.gr"),
            "@test\nfn test_add() -> Bool:\n    true\n\n@test\nfn test_sub() -> Bool:\n    true\n",
        )
        .unwrap();

        let mut tests = discover_tests(&src_dir);
        tests.retain(|t| t.name.contains("add"));
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].name, "test_add");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_tests_unit_return() {
        let dir = std::env::temp_dir().join("gradient_test_discover_5");
        let _ = fs::remove_dir_all(&dir);
        create_test_project(
            &dir,
            &[(
                "unit_test.gr",
                "@test\nfn test_unit():\n    let x: Int = 1\n",
            )],
        );

        let tests = discover_tests(&dir.join("src"));
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].name, "test_unit");
        assert!(!tests[0].returns_bool);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generate_harness_bool_test() {
        let test = TestCase {
            file: PathBuf::from("src/test.gr"),
            name: "test_add".to_string(),
            returns_bool: true,
        };
        let harness = generate_harness(&test, "fn test_add() -> Bool:\n    true\n");
        assert!(harness.contains("fn main()"));
        assert!(harness.contains("test_add()"));
        assert!(harness.contains("exit(1)"));
    }

    #[test]
    fn generate_harness_unit_test() {
        let test = TestCase {
            file: PathBuf::from("src/test.gr"),
            name: "test_unit".to_string(),
            returns_bool: false,
        };
        let harness = generate_harness(&test, "fn test_unit():\n    let x: Int = 1\n");
        assert!(harness.contains("fn main()"));
        assert!(harness.contains("test_unit()"));
        assert!(!harness.contains("exit(1)"));
    }

    #[test]
    fn find_gr_files_empty_dir() {
        let dir = std::env::temp_dir().join("gradient_test_find_gr_1");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let files = find_gr_files(&dir);
        assert!(files.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_gr_files_mixed() {
        let dir = std::env::temp_dir().join("gradient_test_find_gr_2");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        fs::write(dir.join("main.gr"), "fn main():\n    ()").unwrap();
        fs::write(dir.join("readme.txt"), "not a gr file").unwrap();
        fs::write(
            dir.join("lib.gr"),
            "fn add(a: Int, b: Int) -> Int:\n    a + b",
        )
        .unwrap();

        let files = find_gr_files(&dir);
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| f.extension().unwrap() == "gr"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_gr_files_nested() {
        let dir = std::env::temp_dir().join("gradient_test_find_gr_3");
        let _ = fs::remove_dir_all(&dir);
        let sub = dir.join("sub");
        fs::create_dir_all(&sub).unwrap();

        fs::write(dir.join("top.gr"), "fn f():\n    ()").unwrap();
        fs::write(sub.join("nested.gr"), "fn g():\n    ()").unwrap();

        let files = find_gr_files(&dir);
        assert_eq!(files.len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }
}
