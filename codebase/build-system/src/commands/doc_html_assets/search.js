// gradient doc client-side search.
// Loads search-index.json and filters as the user types.
// Schema version: 1 (must match SEARCH_INDEX_SCHEMA_VERSION in
// build-system/src/commands/doc_html.rs).

(function () {
  "use strict";

  var EXPECTED_SCHEMA = 1;
  var input = document.getElementById("gd-search");
  var results = document.getElementById("gd-search-results");
  if (!input || !results) {
    return;
  }

  var entries = [];
  var loaded = false;

  function loadIndex() {
    return fetch("search-index.json", { cache: "no-store" })
      .then(function (resp) {
        if (!resp.ok) {
          throw new Error("HTTP " + resp.status);
        }
        return resp.json();
      })
      .then(function (data) {
        if (!data || data.schema_version !== EXPECTED_SCHEMA) {
          console.warn(
            "gradient doc: search-index.json schema version mismatch " +
              "(expected " +
              EXPECTED_SCHEMA +
              ", got " +
              (data && data.schema_version) +
              "). Search disabled."
          );
          input.disabled = true;
          input.placeholder = "Search disabled (schema mismatch)";
          return;
        }
        entries = Array.isArray(data.entries) ? data.entries : [];
        loaded = true;
      })
      .catch(function (err) {
        // Most likely cause: file:// protocol blocks fetch in some
        // browsers. Fail soft — the static page still works.
        console.warn("gradient doc: failed to load search index:", err);
        input.disabled = true;
        input.placeholder = "Search disabled (failed to load index)";
      });
  }

  function escapeHtml(s) {
    if (s === null || s === undefined) return "";
    return String(s)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  function score(entry, query) {
    if (!query) return 0;
    var name = entry.name.toLowerCase();
    var sig = (entry.signature || "").toLowerCase();
    var summary = (entry.summary || "").toLowerCase();
    if (name === query) return 1000;
    if (name.indexOf(query) === 0) return 500;
    if (name.indexOf(query) >= 0) return 200;
    if (sig.indexOf(query) >= 0) return 50;
    if (summary.indexOf(query) >= 0) return 25;
    // Sub-token match (camelCase / snake_case)
    var tokens = name.split(/[_\-]/);
    for (var i = 0; i < tokens.length; i++) {
      if (tokens[i].indexOf(query) === 0) return 100;
    }
    return 0;
  }

  function render(matches) {
    if (matches.length === 0) {
      results.innerHTML = "";
      return;
    }
    var html = "";
    var max = Math.min(matches.length, 25);
    for (var i = 0; i < max; i++) {
      var e = matches[i];
      html +=
        '<li><a href="#' +
        escapeHtml(e.anchor) +
        '">' +
        '<span class="gd-result-name">' +
        escapeHtml(e.name) +
        "</span>" +
        '<span class="gd-result-kind">' +
        escapeHtml(e.kind) +
        "</span>" +
        (e.summary
          ? '<div class="gd-result-summary">' + escapeHtml(e.summary) + "</div>"
          : "") +
        "</a></li>";
    }
    results.innerHTML = html;
  }

  function update() {
    if (!loaded) return;
    var query = input.value.trim().toLowerCase();
    if (!query) {
      results.innerHTML = "";
      return;
    }
    var scored = entries
      .map(function (e) {
        return { entry: e, s: score(e, query) };
      })
      .filter(function (x) {
        return x.s > 0;
      })
      .sort(function (a, b) {
        if (b.s !== a.s) return b.s - a.s;
        return a.entry.name.localeCompare(b.entry.name);
      })
      .map(function (x) {
        return x.entry;
      });
    render(scored);
  }

  input.addEventListener("input", update);
  input.addEventListener("keydown", function (e) {
    if (e.key === "Escape") {
      input.value = "";
      results.innerHTML = "";
      input.blur();
    }
  });

  loadIndex().then(update);
})();
