// gradient doc HTML renderer (E11 #372)
//
// Renders a `ModuleDocumentation` (produced by
// `gradient_compiler::query::Session::documentation`) into a
// self-contained static HTML site. Output layout under `<out_dir>/`:
//
//   index.html      — module overview + per-fn cards + per-type cards
//   style.css       — punk-zine black/red/gray theme, no external fonts
//   search.js       — vanilla JS client-side fuzzy search over fns + types
//   search-index.json — JSON index that `search.js` consumes
//
// The renderer is intentionally dependency-free at the browser tier
// (no React, no Tailwind, no Google Fonts). The whole site works
// offline and can be tarball'd or served via `python -m http.server`.
//
// ## Schema stability
//
// The HTML structure pins the following CSS classes for downstream
// theming:
//
//   .gd-module           — wraps the whole site
//   .gd-fn-card          — wraps a single function entry
//   .gd-type-card        — wraps a single type entry
//   .gd-effect-badge     — single effect chip
//   .gd-cap-ceiling      — capability ceiling banner
//   .gd-contract         — contract row (requires/ensures)
//   .gd-budget           — budget row (cpu/mem)
//   .gd-doc-comment      — `///` doc comment block
//   .gd-search           — search input
//   .gd-search-results   — search results panel
//   .gd-pure             — pure marker pill
//
// The `search-index.json` schema is also pinned at version 1 (see
// `SEARCH_INDEX_SCHEMA_VERSION`). Bumping it requires updating
// `search.js` in the same PR.

use gradient_compiler::query::{ContractInfo, FunctionDoc, ModuleDocumentation, TypeDoc};
use std::path::Path;

/// Pinned schema version for the on-disk `search-index.json`.
///
/// `search.js` checks this version at load time and refuses to render
/// indexes from a newer schema. Bump in lockstep with the JS file.
pub const SEARCH_INDEX_SCHEMA_VERSION: u32 = 1;

/// HTML escape using the standard XML/HTML 5-character set.
///
/// Everything written into the rendered HTML must pass through this —
/// doc comments, function names, contract conditions, and signature
/// strings can all contain user-supplied content. Effects/capabilities
/// come from the compiler so they're trusted, but escaping them costs
/// nothing.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Render the `ModuleDocumentation` into `out_dir`.
///
/// Creates `out_dir` if needed. Overwrites `index.html`, `style.css`,
/// `search.js`, and `search-index.json` if they already exist.
pub fn render_to_dir(doc: &ModuleDocumentation, out_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("create_dir_all({}): {}", out_dir.display(), e))?;

    let html = render_index_html(doc);
    let css = STYLE_CSS.to_string();
    let js = SEARCH_JS.to_string();
    let index = render_search_index_json(doc);

    write_file(out_dir.join("index.html"), &html)?;
    write_file(out_dir.join("style.css"), &css)?;
    write_file(out_dir.join("search.js"), &js)?;
    write_file(out_dir.join("search-index.json"), &index)?;

    Ok(())
}

fn write_file(path: std::path::PathBuf, contents: &str) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| format!("write({}): {}", path.display(), e))
}

/// Render the full `index.html` for the module.
pub fn render_index_html(doc: &ModuleDocumentation) -> String {
    let mut out = String::new();
    out.push_str("<!DOCTYPE html>\n");
    out.push_str("<html lang=\"en\">\n<head>\n");
    out.push_str("<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str(&format!(
        "<title>{} — Gradient docs</title>\n",
        escape_html(&doc.module)
    ));
    out.push_str("<link rel=\"stylesheet\" href=\"style.css\">\n");
    out.push_str("</head>\n<body>\n");
    out.push_str("<main class=\"gd-module\">\n");

    // Header
    out.push_str(&format!(
        "<header class=\"gd-header\"><h1>module <code>{}</code></h1>\n",
        escape_html(&doc.module)
    ));
    if let Some(ceiling) = &doc.capability_ceiling {
        out.push_str(
            "<div class=\"gd-cap-ceiling\"><span class=\"gd-label\">capabilities:</span> ",
        );
        if ceiling.is_empty() {
            out.push_str("<em>none</em>");
        } else {
            for (i, cap) in ceiling.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!(
                    "<code class=\"gd-cap\">{}</code>",
                    escape_html(cap)
                ));
            }
        }
        out.push_str("</div>\n");
    } else {
        out.push_str(
            "<div class=\"gd-cap-ceiling gd-cap-unrestricted\">\
             <span class=\"gd-label\">capabilities:</span> <em>unrestricted</em></div>\n",
        );
    }
    out.push_str("</header>\n");

    // Search box
    out.push_str(
        "<section class=\"gd-search-box\">\n\
         <input type=\"search\" id=\"gd-search\" class=\"gd-search\" \
         placeholder=\"Search functions and types…\" autocomplete=\"off\">\n\
         <ul id=\"gd-search-results\" class=\"gd-search-results\"></ul>\n\
         </section>\n",
    );

    // Functions section
    if !doc.functions.is_empty() {
        out.push_str("<section class=\"gd-section gd-fns\">\n");
        out.push_str(&format!(
            "<h2>Functions <span class=\"gd-count\">({})</span></h2>\n",
            doc.functions.len()
        ));
        for func in &doc.functions {
            out.push_str(&render_function_card(func));
        }
        out.push_str("</section>\n");
    }

    // Types section
    if !doc.types.is_empty() {
        out.push_str("<section class=\"gd-section gd-types\">\n");
        out.push_str(&format!(
            "<h2>Types <span class=\"gd-count\">({})</span></h2>\n",
            doc.types.len()
        ));
        for ty in &doc.types {
            out.push_str(&render_type_card(ty));
        }
        out.push_str("</section>\n");
    }

    // Footer
    out.push_str("<footer class=\"gd-footer\">");
    out.push_str("Generated by <code>gradient doc --html</code>");
    out.push_str("</footer>\n");

    out.push_str("</main>\n");
    out.push_str("<script src=\"search.js\"></script>\n");
    out.push_str("</body>\n</html>\n");
    out
}

fn render_function_card(func: &FunctionDoc) -> String {
    let mut out = String::new();
    let anchor = format!("fn-{}", slugify(&func.name));
    out.push_str(&format!(
        "<article class=\"gd-fn-card\" id=\"{}\">\n",
        escape_html(&anchor)
    ));
    out.push_str(&format!(
        "<h3 class=\"gd-fn-name\"><a href=\"#{anchor}\">{name}</a>",
        anchor = escape_html(&anchor),
        name = escape_html(&func.name)
    ));
    if func.is_pure {
        out.push_str(" <span class=\"gd-pure\" title=\"provably pure\">pure</span>");
    }
    out.push_str("</h3>\n");

    out.push_str(&format!(
        "<pre class=\"gd-signature\"><code>{}</code></pre>\n",
        escape_html(&func.signature)
    ));

    if !func.effects.is_empty() {
        out.push_str("<div class=\"gd-effects\"><span class=\"gd-label\">effects:</span> ");
        for eff in &func.effects {
            out.push_str(&format!(
                "<span class=\"gd-effect-badge\">{}</span>",
                escape_html(eff)
            ));
        }
        out.push_str("</div>\n");
    }

    if let Some(comment) = &func.doc_comment {
        out.push_str(&format!(
            "<div class=\"gd-doc-comment\">{}</div>\n",
            render_doc_comment(comment)
        ));
    }

    if !func.contracts.is_empty() {
        out.push_str("<div class=\"gd-contracts\">\n");
        for c in &func.contracts {
            out.push_str(&render_contract(c));
        }
        out.push_str("</div>\n");
    }

    if let Some(budget) = &func.budget {
        out.push_str("<div class=\"gd-budget\">\n");
        out.push_str("<span class=\"gd-label\">budget:</span> ");
        let mut parts = Vec::new();
        if let Some(cpu) = &budget.cpu {
            parts.push(format!("cpu: <code>{}</code>", escape_html(cpu)));
        }
        if let Some(mem) = &budget.mem {
            parts.push(format!("mem: <code>{}</code>", escape_html(mem)));
        }
        out.push_str(&parts.join(", "));
        out.push_str("</div>\n");
    }

    if !func.calls.is_empty() {
        out.push_str("<details class=\"gd-calls\"><summary>calls</summary><ul>");
        for callee in &func.calls {
            out.push_str(&format!("<li><code>{}</code></li>", escape_html(callee)));
        }
        out.push_str("</ul></details>\n");
    }

    out.push_str("</article>\n");
    out
}

fn render_type_card(ty: &TypeDoc) -> String {
    let mut out = String::new();
    let anchor = format!("type-{}", slugify(&ty.name));
    out.push_str(&format!(
        "<article class=\"gd-type-card\" id=\"{anchor}\">\n",
        anchor = escape_html(&anchor)
    ));
    out.push_str(&format!(
        "<h3 class=\"gd-type-name\"><a href=\"#{anchor}\">{name}</a></h3>\n",
        anchor = escape_html(&anchor),
        name = escape_html(&ty.name)
    ));
    out.push_str(&format!(
        "<pre class=\"gd-signature\"><code>{}</code></pre>\n",
        escape_html(&ty.definition)
    ));
    if let Some(comment) = &ty.doc_comment {
        out.push_str(&format!(
            "<div class=\"gd-doc-comment\">{}</div>\n",
            render_doc_comment(comment)
        ));
    }
    out.push_str("</article>\n");
    out
}

fn render_contract(c: &ContractInfo) -> String {
    let release_marker = if c.runtime_only_off_in_release {
        " <span class=\"gd-contract-rt-only\" title=\"runtime-only — stripped in release\">runtime-only</span>"
    } else {
        ""
    };
    format!(
        "<div class=\"gd-contract gd-contract-{kind}\">\
         <span class=\"gd-label\">@{kind}</span>(<code>{cond}</code>){marker}</div>\n",
        kind = escape_html(&c.kind),
        cond = escape_html(&c.condition),
        marker = release_marker,
    )
}

/// Render a doc comment (`///`-stripped lines) as a paragraph block.
/// Treats blank lines as paragraph separators; everything else is
/// preserved verbatim within `<p>` tags.
fn render_doc_comment(comment: &str) -> String {
    let mut out = String::new();
    let mut current = String::new();
    for line in comment.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push_str(&format!("<p>{}</p>", escape_html(current.trim_end())));
                current.clear();
            }
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(line.trim());
        }
    }
    if !current.is_empty() {
        out.push_str(&format!("<p>{}</p>", escape_html(current.trim_end())));
    }
    out
}

/// Convert an arbitrary identifier into a URL-safe anchor slug.
/// We keep ASCII alphanumerics + `_` and `-`; everything else becomes
/// `-`. Idempotent.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    // Trim trailing dash for cleanliness.
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Build the JSON index consumed by `search.js`. Contains:
/// - `schema_version`: pinned constant.
/// - `module`: the module name.
/// - `entries`: sorted list of `{kind, name, anchor, signature,
///   effects, summary}` records.
///
/// `summary` is the first paragraph of the doc comment (max ~120 chars)
/// so the search results panel can show a one-line preview.
pub fn render_search_index_json(doc: &ModuleDocumentation) -> String {
    let mut entries: Vec<serde_json::Value> = Vec::new();

    for func in &doc.functions {
        let summary = func
            .doc_comment
            .as_ref()
            .map(|c| first_summary_line(c))
            .unwrap_or_default();
        entries.push(serde_json::json!({
            "kind": "function",
            "name": func.name,
            "anchor": format!("fn-{}", slugify(&func.name)),
            "signature": func.signature,
            "effects": func.effects,
            "is_pure": func.is_pure,
            "summary": summary,
        }));
    }

    for ty in &doc.types {
        let summary = ty
            .doc_comment
            .as_ref()
            .map(|c| first_summary_line(c))
            .unwrap_or_default();
        entries.push(serde_json::json!({
            "kind": "type",
            "name": ty.name,
            "anchor": format!("type-{}", slugify(&ty.name)),
            "signature": ty.definition,
            "effects": Vec::<String>::new(),
            "is_pure": false,
            "summary": summary,
        }));
    }

    // Already deterministic since `documentation()` returns symbols in
    // source order; nonetheless sort by `name` to make the search UI
    // alphabetical.
    entries.sort_by(|a, b| {
        let an = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let bn = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        an.cmp(bn)
    });

    let payload = serde_json::json!({
        "schema_version": SEARCH_INDEX_SCHEMA_VERSION,
        "module": doc.module,
        "entries": entries,
    });
    serde_json::to_string_pretty(&payload).unwrap_or_else(|e| {
        format!(
            "{{\"schema_version\":{},\"error\":\"serialization failed: {}\"}}",
            SEARCH_INDEX_SCHEMA_VERSION, e
        )
    })
}

fn first_summary_line(comment: &str) -> String {
    for line in comment.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            // Truncate at ~120 chars on a UTF-8 char boundary.
            if trimmed.len() <= 120 {
                return trimmed.to_string();
            }
            let mut end = 120;
            while end > 0 && !trimmed.is_char_boundary(end) {
                end -= 1;
            }
            let mut clipped = trimmed[..end].to_string();
            clipped.push('…');
            return clipped;
        }
    }
    String::new()
}

const STYLE_CSS: &str = include_str!("doc_html_assets/style.css");
const SEARCH_JS: &str = include_str!("doc_html_assets/search.js");

#[cfg(test)]
mod tests {
    use super::*;
    use gradient_compiler::query::{
        BudgetInfo, CallGraphEntry, ContractInfo, FunctionDoc, ModuleDocumentation, TypeDoc,
    };

    fn fixture_doc() -> ModuleDocumentation {
        ModuleDocumentation {
            module: "math".to_string(),
            capability_ceiling: Some(vec!["Pure".to_string()]),
            functions: vec![
                FunctionDoc {
                    name: "factorial".to_string(),
                    signature: "fn factorial(n: Int) -> Int".to_string(),
                    type_params: vec![],
                    effects: vec![],
                    is_pure: true,
                    contracts: vec![ContractInfo {
                        kind: "requires".to_string(),
                        condition: "n >= 0".to_string(),
                        runtime_only_off_in_release: false,
                    }],
                    budget: None,
                    calls: vec![],
                    doc_comment: Some("Compute n!".to_string()),
                },
                FunctionDoc {
                    name: "read_file".to_string(),
                    signature: "fn read_file(path: String) -> String".to_string(),
                    type_params: vec![],
                    effects: vec!["IO".to_string(), "Throws".to_string()],
                    is_pure: false,
                    contracts: vec![],
                    budget: Some(BudgetInfo {
                        cpu: Some("100ms".to_string()),
                        mem: Some("10mb".to_string()),
                    }),
                    calls: vec!["fopen".to_string()],
                    doc_comment: None,
                },
            ],
            types: vec![TypeDoc {
                name: "Result".to_string(),
                definition: "type Result = Ok(Int) | Err(String)".to_string(),
                variants: vec!["Ok(Int)".to_string(), "Err(String)".to_string()],
                doc_comment: Some("A simple result type.".to_string()),
            }],
            call_graph: vec![CallGraphEntry {
                function: "read_file".to_string(),
                calls: vec!["fopen".to_string()],
            }],
        }
    }

    #[test]
    fn schema_version_is_pinned_at_1() {
        assert_eq!(SEARCH_INDEX_SCHEMA_VERSION, 1);
    }

    #[test]
    fn escape_html_handles_five_special_chars() {
        assert_eq!(
            escape_html("<a href=\"x\">'&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn slugify_strips_non_alnum_and_lowercases() {
        assert_eq!(slugify("Foo::Bar Baz!"), "foo-bar-baz");
        assert_eq!(slugify("with_under_score"), "with_under_score");
        assert_eq!(slugify("trailing!!"), "trailing");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn render_index_html_contains_module_name_and_fn_signatures() {
        let doc = fixture_doc();
        let html = render_index_html(&doc);
        assert!(html.contains("module <code>math</code>"));
        assert!(html.contains("fn factorial(n: Int) -&gt; Int"));
        assert!(html.contains("fn read_file(path: String) -&gt; String"));
    }

    #[test]
    fn render_index_html_renders_effect_badges_per_function() {
        let doc = fixture_doc();
        let html = render_index_html(&doc);
        assert!(html.contains("<span class=\"gd-effect-badge\">IO</span>"));
        assert!(html.contains("<span class=\"gd-effect-badge\">Throws</span>"));
    }

    #[test]
    fn render_index_html_marks_pure_functions() {
        let doc = fixture_doc();
        let html = render_index_html(&doc);
        // factorial is pure
        assert!(html.contains("<span class=\"gd-pure\""));
    }

    #[test]
    fn render_index_html_renders_contracts() {
        let doc = fixture_doc();
        let html = render_index_html(&doc);
        assert!(html.contains("@requires"));
        assert!(html.contains("n &gt;= 0"));
    }

    #[test]
    fn render_index_html_renders_budget() {
        let doc = fixture_doc();
        let html = render_index_html(&doc);
        assert!(html.contains("cpu: <code>100ms</code>"));
        assert!(html.contains("mem: <code>10mb</code>"));
    }

    #[test]
    fn render_index_html_renders_capability_ceiling() {
        let doc = fixture_doc();
        let html = render_index_html(&doc);
        assert!(html.contains("<code class=\"gd-cap\">Pure</code>"));
    }

    #[test]
    fn render_index_html_renders_types_section() {
        let doc = fixture_doc();
        let html = render_index_html(&doc);
        assert!(html.contains("type Result = Ok(Int) | Err(String)"));
        assert!(html.contains("A simple result type."));
    }

    #[test]
    fn render_index_html_links_search_assets() {
        let doc = fixture_doc();
        let html = render_index_html(&doc);
        assert!(html.contains("<link rel=\"stylesheet\" href=\"style.css\">"));
        assert!(html.contains("<script src=\"search.js\"></script>"));
    }

    #[test]
    fn search_index_json_includes_schema_version() {
        let doc = fixture_doc();
        let json = render_search_index_json(&doc);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["schema_version"], SEARCH_INDEX_SCHEMA_VERSION);
        assert_eq!(parsed["module"], "math");
        let entries = parsed["entries"].as_array().unwrap();
        // 2 fns + 1 type
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn search_index_json_entries_are_sorted_alphabetically() {
        let doc = fixture_doc();
        let json = render_search_index_json(&doc);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = parsed["entries"].as_array().unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        // Expected: factorial, read_file, Result. Sorted: Result, factorial, read_file.
        // ASCII: 'R' (82) < 'f' (102) < 'r' (114).
        assert_eq!(names, vec!["Result", "factorial", "read_file"]);
    }

    #[test]
    fn search_index_json_records_function_kind_and_anchor() {
        let doc = fixture_doc();
        let json = render_search_index_json(&doc);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = parsed["entries"].as_array().unwrap();
        let factorial = entries
            .iter()
            .find(|e| e["name"] == "factorial")
            .expect("factorial in index");
        assert_eq!(factorial["kind"], "function");
        assert_eq!(factorial["anchor"], "fn-factorial");
        assert_eq!(factorial["is_pure"], true);
    }

    #[test]
    fn render_doc_comment_treats_blank_lines_as_paragraph_breaks() {
        let html = render_doc_comment("First para.\n\nSecond para spans\nmultiple lines.");
        assert!(html.contains("<p>First para.</p>"));
        assert!(html.contains("<p>Second para spans multiple lines.</p>"));
    }

    #[test]
    fn first_summary_line_truncates_long_lines() {
        let long = "a".repeat(300);
        let summary = first_summary_line(&long);
        assert!(summary.ends_with('…'));
        // 120 ASCII bytes + '…' (3 bytes UTF-8) = 123.
        assert!(summary.len() <= 123);
    }

    #[test]
    fn render_to_dir_writes_four_files() {
        let doc = fixture_doc();
        let tmp = tempfile::tempdir().unwrap();
        render_to_dir(&doc, tmp.path()).unwrap();
        assert!(tmp.path().join("index.html").is_file());
        assert!(tmp.path().join("style.css").is_file());
        assert!(tmp.path().join("search.js").is_file());
        assert!(tmp.path().join("search-index.json").is_file());
    }

    #[test]
    fn render_to_dir_overwrites_existing_files() {
        let doc = fixture_doc();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("index.html"), "STALE").unwrap();
        render_to_dir(&doc, tmp.path()).unwrap();
        let html = std::fs::read_to_string(tmp.path().join("index.html")).unwrap();
        assert!(!html.contains("STALE"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn unrestricted_capability_renders_when_ceiling_is_none() {
        let mut doc = fixture_doc();
        doc.capability_ceiling = None;
        let html = render_index_html(&doc);
        assert!(html.contains("gd-cap-unrestricted"));
        assert!(html.contains("unrestricted"));
    }

    #[test]
    fn empty_module_still_renders_valid_html() {
        let doc = ModuleDocumentation {
            module: "empty".to_string(),
            capability_ceiling: None,
            functions: vec![],
            types: vec![],
            call_graph: vec![],
        };
        let html = render_index_html(&doc);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("module <code>empty</code>"));
        // No functions/types sections when both are empty.
        assert!(!html.contains("Functions <span"));
        assert!(!html.contains("Types <span"));
    }
}
