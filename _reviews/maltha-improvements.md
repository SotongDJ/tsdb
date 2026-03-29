# MaLTHA Improvement Review

**Date:** 2026-03-29
**Context:** Observed while building the tsdb project website using MaLTHA (pip package, current version).

---

## Issues Observed

### 1. Unresolved placeholder error on code-block content

**Symptom:** `ERROR: need more formatting Docs` printed during generation when the Docs page contains single-brace tokens like `{C}`, `{G}`, `{YY}` inside fenced code blocks.

**Root cause:** MaLTHA's rendering pipeline applies `str.format(**base_dict)` to the full page content, including Markdown-rendered code blocks. Any `{word}` pattern inside a code block that is not a known key in `base_dict` raises a `KeyError` or produces a partial-format warning. Technical documentation pages routinely contain such patterns in code examples (format strings, UUID templates, shell variables, etc.).

**Workaround used:** None applied — the page rendered correctly because MaLTHA falls back gracefully, but the error is noisy and signals a real fragility.

**Suggested improvement:** Before the format pass, escape `{` and `}` inside rendered `<code>` and `<pre>` blocks to `{{` and `}}`, so code content is exempt from placeholder substitution. Alternatively, introduce a `format:safe-md` content type that wraps code blocks in a protected zone before the format pass.

---

### 2. Top-level directory scanning is too broad

**Symptom:** MaLTHA's `Convertor.post()` scans all top-level directories that don't start with `.` or `_`, don't contain `_files`, and aren't `docs` or `run`. This means project-specific directories such as `ref/`, `reviews/`, `src/` (a Rust source tree), and `target/` (Rust build output) were all scanned and caused `KeyError: 'header'` crashes because their `.md` files lack MaLTHA TOML front matter.

**Workaround applied:** Renamed `ref/` → `_ref/`, `reviews/` → `_reviews/`, `temp/` → `_temp/` to match MaLTHA's exclusion prefix convention.

**Suggested improvement:** Add an explicit `post_dirs` list to `config.toml` (e.g., `post_dirs = ["posts"]`) so MaLTHA only scans nominated directories rather than every top-level folder. This would make the tool safe to use in polyglot repos without requiring structural renaming.

---

### 3. No graceful skip for non-MaLTHA `.md` files

**Related to issue 2.** When a `.md` file is encountered without a `<!--break type:header content-->` block, MaLTHA raises `KeyError: 'header'` with no file path in the error message, making it hard to identify which file caused the crash.

**Suggested improvement:** Emit a warning with the offending file path and skip the file, rather than aborting the entire build. Or: treat files without a header block as skipped with a clear log message.

---

### 4. Atom feed page path must be a file extension path

**Observation (not a bug):** Pages whose `path` ends in a file extension (e.g., `/atom.xml`) are written as a flat file rather than `atom/index.html`. This is correct and documented, but it is easy to miss — if the path is accidentally written as `/atom/` instead of `/atom.xml`, the feed is generated as an HTML page inside a directory rather than as an XML file. A config-level `content_type` or `output_as_file` field would make this intent explicit and catch mistakes.

---

## Summary

| # | Severity | Description |
|---|----------|-------------|
| 1 | Medium | `str.format()` applied inside code blocks causes errors on technical docs |
| 2 | High | Broad top-level directory scan breaks in polyglot repos |
| 3 | Low | Non-MaLTHA `.md` files crash the build with an unhelpful error |
| 4 | Low | Atom feed path convention is implicit and error-prone |
