//! plan report, skill lint, and ultraplan prompt assembly (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// One section the structured plan template expects. `hard` sections are
/// errors when absent; the rest are warnings. Matched by case-insensitive
/// substring against the plan's `##` headers.
pub(crate) struct PlanSectionSpec {
    pub(crate) keyword: &'static str,
    /// If set, a header matching `keyword` is rejected when it also
    /// contains this string (disambiguates "Files" from "Critical Files").
    pub(crate) exclude: Option<&'static str>,
    pub(crate) label: &'static str,
    pub(crate) hard: bool,
}

pub(crate) const PLAN_SECTIONS: &[PlanSectionSpec] = &[
    PlanSectionSpec {
        keyword: "approach",
        exclude: None,
        label: "Approach",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "decision",
        exclude: None,
        label: "Decisions",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "files",
        exclude: Some("critical"),
        label: "Files",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "critical files",
        exclude: None,
        label: "Critical Files",
        hard: true,
    },
    PlanSectionSpec {
        keyword: "reusable helper",
        exclude: None,
        label: "Reusable helpers",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "risk",
        exclude: None,
        label: "Risks + gotchas",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "test",
        exclude: None,
        label: "Tests",
        hard: false,
    },
    PlanSectionSpec {
        keyword: "verification",
        exclude: None,
        label: "Verification",
        hard: true,
    },
    PlanSectionSpec {
        keyword: "followup",
        exclude: None,
        label: "Followups",
        hard: true,
    },
    PlanSectionSpec {
        keyword: "related",
        exclude: None,
        label: "Related",
        hard: false,
    },
];

/// Source-file extensions a `path:line` ref or bare path is expected to use.
/// Filters out commands (`aida pull`) and prose from the path heuristics.
pub(crate) const PLAN_SOURCE_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "toml", "md", "sh", "json", "yaml", "yml", "lock", "css",
    "html",
];

// trace:TASK-772 | ai:claude — Debug for test assertions
#[derive(PartialEq, Debug)]
pub(crate) enum PlanFindingLevel {
    Ok,
    Warn,
    Error,
}

pub(crate) struct PlanFinding {
    pub(crate) level: PlanFindingLevel,
    pub(crate) msg: String,
}

/// A confirmed drifted line ref queued for `--fix` rewriting.
pub(crate) struct PlanRefFix {
    pub(crate) line_idx: usize,
    pub(crate) old: String,
    pub(crate) new: String,
}

/// The full result of linting a plan file: the three finding groups plus the
/// confirmed drifted-ref fixes. Split out from `verify_plan` so both the CLI
/// renderer (`verify_plan`) and the read-only MCP `plan_verify` tool compute
/// from the same source of truth. trace:EPIC-27 | ai:claude
pub(crate) struct PlanReport {
    pub(crate) sections: Vec<PlanFinding>,
    pub(crate) files: Vec<PlanFinding>,
    pub(crate) refs: Vec<PlanFinding>,
    pub(crate) fixes: Vec<PlanRefFix>,
}

impl PlanReport {
    pub(crate) fn error_count(&self) -> usize {
        let c = |fs: &[PlanFinding]| {
            fs.iter()
                .filter(|f| f.level == PlanFindingLevel::Error)
                .count()
        };
        c(&self.sections) + c(&self.files) + c(&self.refs)
    }

    pub(crate) fn warn_count(&self) -> usize {
        let c = |fs: &[PlanFinding]| {
            fs.iter()
                .filter(|f| f.level == PlanFindingLevel::Warn)
                .count()
        };
        c(&self.sections) + c(&self.files) + c(&self.refs)
    }
}

// trace:STORY-447 | ai:claude
/// Walk up from the plan file to the enclosing git repo root. Paths inside
/// a plan are repo-relative, so this is what we resolve them against. Falls
/// back to the current directory when no `.git` is found.
pub(crate) fn plan_repo_root(plan_file: &std::path::Path) -> std::path::PathBuf {
    let start = plan_file
        .canonicalize()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    let mut dir = match start {
        Some(d) => d,
        None => return std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    };
    loop {
        if dir.join(".git").exists() {
            return dir;
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => {
                return std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            }
        }
    }
}

/// True when `tok` looks like a placeholder rather than a real path:
/// template stand-ins (`path/to/file.rs`, `<STORY-N>`) and globs (`*.md`).
pub(crate) fn is_plan_placeholder_path(tok: &str) -> bool {
    tok.contains('<')
        || tok.contains('>')
        || tok.contains('*')
        || tok.starts_with("path/to/")
        || tok.contains("...")
        || tok.contains("NNN")
}

// trace:TASK-772 | ai:claude
/// True when the text immediately following a backticked path ref marks the
/// file as one the plan will *create*: optional whitespace, then `(new)` or
/// `(to create)`, case-insensitive. A marked path that doesn't exist yet is
/// OK rather than an error; a marked path that already exists draws a
/// stale-annotation warning.
pub(crate) fn plan_path_marked_new(rest: &str) -> bool {
    let lower = rest.trim_start().to_ascii_lowercase();
    lower.starts_with("(new)") || lower.starts_with("(to create)")
}

// trace:TASK-162 | ai:codex
/// True when the text immediately following a file path in a plan heading
/// marks the file as planned-new, e.g. ``### `src/new.rs` — NEW: parser``.
pub(crate) fn plan_path_heading_marked_new(rest: &str) -> bool {
    let lower = rest.trim_start().to_ascii_lowercase();
    let lower = lower.trim_start_matches(['—', '-', ':']).trim_start();
    lower == "new"
        || lower.starts_with("new:")
        || lower.starts_with("new ")
        || lower.starts_with("new(")
        || lower.starts_with("new —")
}

/// True when `tok` (already stripped of any `:line` suffix) has a known
/// source-file extension.
pub(crate) fn has_plan_source_ext(tok: &str) -> bool {
    match tok.rsplit_once('.') {
        Some((_, ext)) => PLAN_SOURCE_EXTS.contains(&ext.to_ascii_lowercase().as_str()),
        None => false,
    }
}

/// Locate the 1-based line where `symbol` is *defined* in `content`.
/// Recognises Rust item kinds and the common TypeScript/JS forms. Returns
/// the first definition found. A `::`-qualified name is reduced to its
/// final segment before searching.
pub(crate) fn locate_symbol_line(content: &str, symbol: &str) -> Option<usize> {
    use regex::Regex;
    let bare = symbol.rsplit("::").next().unwrap_or(symbol);
    if bare.is_empty() || !bare.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let esc = regex::escape(bare);
    // Leading indent must be `[ \t]*`, never `\s*` — `\s` matches `\n`, so a
    // greedy `^\s*` would start the match on the *preceding* blank line and
    // throw the reported line number off by one.
    let rust_item = format!(
        r#"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(?:default[ \t]+)?(?:async[ \t]+)?(?:unsafe[ \t]+)?(?:extern[ \t]+(?:"[^"]*"[ \t]+)?)?(?:fn|struct|enum|trait|type|union|mod|const|static)[ \t]+{esc}\b"#
    );
    let pat = format!(
        r"(?m){rust_item}|^[ \t]*macro_rules![ \t]+{esc}\b|^[ \t]*(?:export[ \t]+)?(?:default[ \t]+)?(?:abstract[ \t]+)?(?:async[ \t]+)?(?:function|class|interface)[ \t]+{esc}\b|^[ \t]*(?:export[ \t]+)?(?:const|let|var)[ \t]+{esc}\b"
    );
    let re = Regex::new(&pat).ok()?;
    let m = re.find(content)?;
    Some(content[..m.start()].bytes().filter(|&b| b == b'\n').count() + 1)
}

/// Pull backtick-quoted identifiers that look like code symbols off a line
/// of plan prose: `` `fn foo` `` → `foo`, `` `Storage::update` `` → kept as
/// `Storage::update` (the locator reduces it). Leading item keywords are
/// stripped. Also catches an un-backticked `fn name`.
pub(crate) fn plan_symbols_on_line(line: &str) -> Vec<String> {
    use regex::Regex;
    let mut out = Vec::new();
    let backtick = Regex::new(r"`([^`]+)`").unwrap();
    for cap in backtick.captures_iter(line) {
        let span = cap[1].trim();
        // Drop a leading item keyword: "fn foo" -> "foo".
        let ident = span
            .strip_prefix("fn ")
            .or_else(|| span.strip_prefix("struct "))
            .or_else(|| span.strip_prefix("enum "))
            .or_else(|| span.strip_prefix("trait "))
            .or_else(|| span.strip_prefix("impl "))
            .or_else(|| span.strip_prefix("type "))
            .or_else(|| span.strip_prefix("macro_rules! "))
            .unwrap_or(span)
            .trim();
        // Keep plausible identifiers (optionally `::`-qualified). Reject
        // paths, prose, commands.
        let core = ident.split('(').next().unwrap_or(ident).trim();
        if !core.is_empty()
            && !core.contains(' ')
            && !core.contains('/')
            && !core.contains('.')
            && core
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
        {
            out.push(core.to_string());
        }
    }
    let unq = Regex::new(r"\bfn\s+([a-z_][A-Za-z0-9_]*)").unwrap();
    for cap in unq.captures_iter(line) {
        out.push(cap[1].to_string());
    }
    out.sort();
    out.dedup();
    out
}

/// Pure plan-lint pass: compute the section / file / line-ref findings (and
/// the drifted-ref fixes) for `content` resolved against repo `root`. Does no
/// I/O beyond reading the source files the plan references, and never writes
/// or exits — the rendering, `--fix` rewrite, and process exit live in the
/// callers (`verify_plan` for the CLI, the `plan_verify` MCP tool for the
/// server). trace:EPIC-27 | ai:claude
pub(crate) fn compute_plan_report(content: &str, root: &std::path::Path) -> PlanReport {
    use regex::Regex;
    let lines: Vec<&str> = content.lines().collect();

    let mut section_findings: Vec<PlanFinding> = Vec::new();
    let mut file_findings: Vec<PlanFinding> = Vec::new();
    let mut ref_findings: Vec<PlanFinding> = Vec::new();
    let mut fixes: Vec<PlanRefFix> = Vec::new();

    // --- Section pass: collect `##` headers, track section membership. ---
    let mut headers_lower: Vec<String> = Vec::new();
    let mut files_section_paths: Vec<String> = Vec::new();
    let mut planned_new_paths: HashSet<String> = HashSet::new();
    let mut critical_section_paths: HashSet<String> = HashSet::new();
    let mut in_fence = false;
    let mut in_html_comment = false;
    let mut current_section = String::new();

    let path_ref_re = Regex::new(r"([A-Za-z0-9_][A-Za-z0-9_./\-]*\.[A-Za-z0-9]+):(\d+)").unwrap();
    let bare_path_re = Regex::new(r"`([A-Za-z0-9_][A-Za-z0-9_./\-]*\.[A-Za-z0-9]+)`").unwrap();
    let sym_ref_re = Regex::new(r"\b([a-z_][A-Za-z0-9_]*):(\d+)\b").unwrap();

    for line in &lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Skip `<!-- ... -->` regions — meta-commentary, not plan content
        // (the template's own guidance cites an example bad ref there).
        if in_html_comment {
            if line.contains("-->") {
                in_html_comment = false;
            }
            continue;
        }
        if let Some(pos) = line.find("<!--") {
            if !line[pos..].contains("-->") {
                in_html_comment = true;
            }
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("## ") {
            if !trimmed.starts_with("### ") {
                current_section = h.trim().to_ascii_lowercase();
                headers_lower.push(current_section.clone());
            }
        }
        if let Some(h) = trimmed.strip_prefix("### ") {
            // `### `path/to/file.rs` — purpose` names a build-order file.
            if let Some(cap) = bare_path_re.captures(h) {
                let p = cap[1].to_string();
                let rest = &h[cap.get(0).unwrap().end()..];
                if current_section.contains("files") && !current_section.contains("critical") {
                    files_section_paths.push(p.clone());
                }
                if current_section.contains("new files") || plan_path_heading_marked_new(rest) {
                    planned_new_paths.insert(p);
                }
            }
        }
        if current_section.contains("new files") {
            for cap in bare_path_re.captures_iter(line) {
                planned_new_paths.insert(cap[1].to_string());
            }
        }
        if current_section.contains("critical files") {
            for cap in bare_path_re.captures_iter(line) {
                critical_section_paths.insert(cap[1].to_string());
            }
        }
    }

    for spec in PLAN_SECTIONS {
        let present = headers_lower
            .iter()
            .any(|h| h.contains(spec.keyword) && !spec.exclude.is_some_and(|ex| h.contains(ex)));
        if present {
            section_findings.push(PlanFinding {
                level: PlanFindingLevel::Ok,
                msg: format!("{} section present", spec.label),
            });
        } else if spec.hard {
            section_findings.push(PlanFinding {
                level: PlanFindingLevel::Error,
                msg: format!("required section missing: {}", spec.label),
            });
        } else {
            section_findings.push(PlanFinding {
                level: PlanFindingLevel::Warn,
                msg: format!("recommended section missing: {}", spec.label),
            });
        }
    }

    // Critical Files completeness: every build-order file should also be
    // enumerated in the Critical Files section.
    for p in &files_section_paths {
        if !critical_section_paths.contains(p) {
            section_findings.push(PlanFinding {
                level: PlanFindingLevel::Warn,
                msg: format!("`{p}` is in Files but not enumerated in Critical Files"),
            });
        }
    }

    // --- File existence + line-ref pass (prose only, skip code fences
    // and HTML comment regions). ---
    let mut checked_paths: HashSet<String> = HashSet::new();
    let mut current_h3_file: Option<String> = None;
    in_fence = false;
    in_html_comment = false;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if in_html_comment {
            if line.contains("-->") {
                in_html_comment = false;
            }
            continue;
        }
        if let Some(pos) = line.find("<!--") {
            if !line[pos..].contains("-->") {
                in_html_comment = true;
            }
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("### ") {
            if let Some(cap) = bare_path_re.captures(h) {
                current_h3_file = Some(cap[1].to_string());
            }
        }

        let symbols = plan_symbols_on_line(line);

        // Bare file paths in backticks — existence check. Only repo-relative
        // paths (containing `/`) are checked; a slashless `foo.rs` is prose
        // shorthand for a file fully named elsewhere, not a path claim.
        for cap in bare_path_re.captures_iter(line) {
            let tok = cap[1].to_string();
            if is_plan_placeholder_path(&tok) || !has_plan_source_ext(&tok) || !tok.contains('/') {
                continue;
            }
            // trace:TASK-772 | ai:claude — a `(new)` / `(to create)` marker
            // right after the closing backtick sanctions a to-be-created file.
            // trace:TASK-162 | ai:codex — heading-level `NEW` markers and
            // `## New files` sections sanction planned creations too.
            let marked_new = plan_path_marked_new(&line[cap.get(0).unwrap().end()..])
                || planned_new_paths.contains(&tok);
            if !checked_paths.insert(tok.clone()) {
                continue;
            }
            match (root.join(&tok).exists(), marked_new) {
                (true, false) => file_findings.push(PlanFinding {
                    level: PlanFindingLevel::Ok,
                    msg: tok,
                }),
                (true, true) => file_findings.push(PlanFinding {
                    level: PlanFindingLevel::Warn,
                    msg: format!(
                        "{tok} — marked new but already exists; stale annotation? \
                         (plan line {})",
                        idx + 1
                    ),
                }),
                (false, true) => file_findings.push(PlanFinding {
                    level: PlanFindingLevel::Ok,
                    msg: format!("{tok} — marked new (to be created)"),
                }),
                (false, false) => file_findings.push(PlanFinding {
                    level: PlanFindingLevel::Error,
                    msg: format!("{tok} — file not found (plan line {})", idx + 1),
                }),
            }
        }

        // `path.ext:NNN` refs — existence + drift.
        for cap in path_ref_re.captures_iter(line) {
            let path = cap[1].to_string();
            let claimed: usize = cap[2].parse().unwrap_or(0);
            if is_plan_placeholder_path(&path) || !has_plan_source_ext(&path) {
                continue;
            }
            let full = root.join(&path);
            if !full.exists() {
                // A slashless `main.rs:NNN` is shorthand — the file may exist
                // under some directory; warn rather than fail the build. A
                // path with a directory that doesn't resolve is a real break.
                if path.contains('/') {
                    ref_findings.push(PlanFinding {
                        level: PlanFindingLevel::Error,
                        msg: format!("{path}:{claimed} — file not found (plan line {})", idx + 1),
                    });
                } else {
                    ref_findings.push(PlanFinding {
                        level: PlanFindingLevel::Warn,
                        msg: format!(
                            "{path}:{claimed} — bare filename does not resolve; use a \
                             repo-relative path (plan line {})",
                            idx + 1
                        ),
                    });
                }
                continue;
            }
            let body = std::fs::read_to_string(&full).unwrap_or_default();
            let mut resolved = false;
            for sym in &symbols {
                if let Some(actual) = locate_symbol_line(&body, sym) {
                    resolved = true;
                    if actual == claimed {
                        ref_findings.push(PlanFinding {
                            level: PlanFindingLevel::Ok,
                            msg: format!("{path}:{claimed} — `{sym}` confirmed"),
                        });
                    } else {
                        let delta = actual as i64 - claimed as i64;
                        ref_findings.push(PlanFinding {
                            level: PlanFindingLevel::Warn,
                            msg: format!(
                                "{path}:{claimed} — `{sym}` is at line {actual} (drift {delta:+}); \
                                 prefer symbol form `{sym}` or update to {path}:{actual} (plan line {})",
                                idx + 1
                            ),
                        });
                        fixes.push(PlanRefFix {
                            line_idx: idx,
                            old: format!("{path}:{claimed}"),
                            new: format!("{path}:{actual}"),
                        });
                    }
                    break;
                }
            }
            if !resolved {
                ref_findings.push(PlanFinding {
                    level: PlanFindingLevel::Warn,
                    msg: format!(
                        "{path}:{claimed} — no named symbol on this line to verify the \
                         line number against (plan line {})",
                        idx + 1
                    ),
                });
            }
        }

        // `symbol:NNN` refs (no path) — resolve against the current
        // build-order file header, if any.
        for cap in sym_ref_re.captures_iter(line) {
            let sym = cap[1].to_string();
            let claimed: usize = cap[2].parse().unwrap_or(0);
            // Skip if this match is the tail of a longer token (a
            // `path.ext:line` ref or a `dir/seg:line` fragment) rather
            // than a standalone `symbol:line` ref.
            let mstart = cap.get(0).unwrap().start();
            if mstart > 0 {
                let prev = line.as_bytes()[mstart - 1];
                if prev == b'.' || prev == b'/' || prev == b'-' || prev.is_ascii_alphanumeric() {
                    continue;
                }
            }
            let Some(file) = &current_h3_file else {
                continue;
            };
            if is_plan_placeholder_path(file) {
                continue;
            }
            let full = root.join(file);
            let Ok(body) = std::fs::read_to_string(&full) else {
                continue;
            };
            if let Some(actual) = locate_symbol_line(&body, &sym) {
                if actual == claimed {
                    ref_findings.push(PlanFinding {
                        level: PlanFindingLevel::Ok,
                        msg: format!("{sym}:{claimed} — confirmed in {file}"),
                    });
                } else {
                    let delta = actual as i64 - claimed as i64;
                    ref_findings.push(PlanFinding {
                        level: PlanFindingLevel::Warn,
                        msg: format!(
                            "{sym}:{claimed} — `{sym}` is at line {actual} in {file} \
                             (drift {delta:+}); prefer symbol form `{sym}` (plan line {})",
                            idx + 1
                        ),
                    });
                    fixes.push(PlanRefFix {
                        line_idx: idx,
                        old: format!("{sym}:{claimed}"),
                        new: format!("{sym}:{actual}"),
                    });
                }
            }
        }
    }

    PlanReport {
        sections: section_findings,
        files: file_findings,
        refs: ref_findings,
        fixes,
    }
}

/// Count raw registry-glyph occurrences in a skill body. The set is derived
/// from `Glyph::ALL`'s unicode renderings — the same source `glyphs.rs` and
/// `scripts/glyph-lint.sh` track — so it stays in sync with the registry
/// automatically (and carries no raw glyph literal in this file, which would
/// itself trip the glyph-lint gate). A glyph in a skill body is a warning, not
/// an error: emoji in skill prose is allowed by design (the plan-ref check
/// owns the errors).
// trace:TASK-927 | ai:claude
pub(crate) fn count_registry_glyphs(content: &str) -> usize {
    crate::glyphs::Glyph::ALL
        .iter()
        .map(|g| content.matches(g.unicode()).count())
        .sum()
}

/// One referenced-plan or glyph finding for a single skill.
pub(crate) struct SkillPlanRefFinding {
    /// The repo-relative plan path as written in the skill.
    pub(crate) plan_ref: String,
    /// `Ok` if the plan exists and `aida plan verify` finds no errors;
    /// `Error` otherwise (missing file or verify errors). Glyph notes use
    /// `Warn`.
    pub(crate) level: PlanFindingLevel,
    /// Human-readable detail (e.g. "missing: docs/plans/x.md" or "3 plan
    /// error(s), 1 warning(s)").
    pub(crate) msg: String,
}

/// Extract every `docs/plans/...md` reference from a skill body. Matches the
/// path anywhere on a line — bare, in a markdown link, in backticks, or in
/// prose. De-duplicates while preserving first-seen order.
// trace:TASK-927 | ai:claude
pub(crate) fn extract_plan_refs(content: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut refs = Vec::new();
    let bytes = content.as_bytes();
    let needle = b"docs/plans/";
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            // Walk forward to the end of the path token. A path char is
            // anything that isn't whitespace or a delimiter that markdown /
            // prose uses to close a path (backtick, paren, angle bracket,
            // quote, comma, etc.).
            let start = i;
            let mut j = i + needle.len();
            while j < bytes.len() {
                let c = bytes[j] as char;
                if c.is_whitespace()
                    || matches!(c, '`' | ')' | '>' | '"' | '\'' | ',' | ']' | '(' | '<')
                {
                    break;
                }
                j += 1;
            }
            let mut path = content[start..j].to_string();
            // Trim a trailing sentence period that isn't part of `.md`.
            if path.ends_with('.') {
                path.pop();
            }
            // Skip illustrative placeholders, not real plan paths: glob
            // patterns (`docs/plans/*.md`), ellipsis stand-ins
            // (`docs/plans/...md`), and angle-bracket template markers
            // (`<docs/plans/...md>`). These appear in skill help text /
            // examples and must not be verified. trace:TASK-927
            let is_placeholder = path.contains('*') || path.contains("...");
            // Only keep things that actually look like a real plan markdown
            // file (ends in `.md`, longer than the bare `docs/plans/` prefix).
            if !is_placeholder
                && path.ends_with(".md")
                && path.len() > needle.len() + 3
                && seen.insert(path.clone())
            {
                refs.push(path);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    refs
}

/// Resolve the set of skill files to lint. With an explicit path, lint just
/// that file; otherwise every `*.md` under `.claude/skills/` (sorted, and
/// skipping `*.local.md` overrides which are merged at render time, not linted
/// on their own).
// trace:TASK-927 | ai:claude
pub(crate) fn resolve_skill_files(
    explicit: Option<&std::path::Path>,
    project_root: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>> {
    if let Some(p) = explicit {
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            project_root.join(p)
        };
        if !resolved.exists() {
            anyhow::bail!("skill file not found: {}", resolved.display());
        }
        return Ok(vec![resolved]);
    }
    let skills_dir = project_root.join(".claude").join("skills");
    if !skills_dir.is_dir() {
        anyhow::bail!(
            "no skills directory at {} — run `aida init` to scaffold skills",
            skills_dir.display()
        );
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&skills_dir)
        .with_context(|| format!("could not read {}", skills_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with(".md") && !name.ends_with(".local.md") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Lint each skill: for skills that reference a `docs/plans/*.md` file, verify
/// each referenced plan (reusing `compute_plan_report`, the same engine `aida
/// plan verify` uses) and raw-glyph-check the skill body. Prints a grouped
/// per-skill report (or JSON), and exits non-zero when any plan ref is missing
/// or its plan has verify errors.
// trace:TASK-927 | ai:claude
pub(crate) fn lint_skills(skill: Option<&std::path::Path>, json: bool, quiet: bool) -> Result<()> {
    let project_root = find_project_root().or_else(|_| std::env::current_dir())?;
    let skill_files = resolve_skill_files(skill, &project_root)?;

    // Per-skill collected findings, keyed by display label.
    struct SkillReport {
        label: String,
        findings: Vec<SkillPlanRefFinding>,
    }
    let mut reports: Vec<SkillReport> = Vec::new();
    let mut total_errors = 0usize;
    let mut total_warns = 0usize;
    let mut skills_with_refs = 0usize;

    for skill_path in &skill_files {
        let content = std::fs::read_to_string(skill_path)
            .with_context(|| format!("could not read skill {}", skill_path.display()))?;
        let plan_refs = extract_plan_refs(&content);
        if plan_refs.is_empty() {
            // Skills without a plan reference are out of scope — the lint
            // only fires on the plan-pointing subset. trace:TASK-927
            continue;
        }
        skills_with_refs += 1;
        let label = skill_path
            .strip_prefix(&project_root)
            .unwrap_or(skill_path)
            .display()
            .to_string();
        let mut findings = Vec::new();

        // 1. Verify each referenced plan with the plan-verify engine.
        for plan_ref in &plan_refs {
            let plan_path = project_root.join(plan_ref);
            if !plan_path.exists() {
                total_errors += 1;
                findings.push(SkillPlanRefFinding {
                    plan_ref: plan_ref.clone(),
                    level: PlanFindingLevel::Error,
                    msg: format!("referenced plan not found: {plan_ref}"),
                });
                continue;
            }
            let plan_content = match std::fs::read_to_string(&plan_path) {
                Ok(c) => c,
                Err(e) => {
                    total_errors += 1;
                    findings.push(SkillPlanRefFinding {
                        plan_ref: plan_ref.clone(),
                        level: PlanFindingLevel::Error,
                        msg: format!("could not read referenced plan {plan_ref}: {e}"),
                    });
                    continue;
                }
            };
            let root = plan_repo_root(&plan_path);
            let report = compute_plan_report(&plan_content, &root);
            let perrs = report.error_count();
            let pwarns = report.warn_count();
            if perrs > 0 {
                total_errors += 1;
                findings.push(SkillPlanRefFinding {
                    plan_ref: plan_ref.clone(),
                    level: PlanFindingLevel::Error,
                    msg: format!(
                        "plan verify failed: {perrs} error(s), {pwarns} warning(s) \
                         — run `aida plan verify {plan_ref}`"
                    ),
                });
            } else if pwarns > 0 {
                total_warns += 1;
                findings.push(SkillPlanRefFinding {
                    plan_ref: plan_ref.clone(),
                    level: PlanFindingLevel::Warn,
                    msg: format!("plan verify: 0 errors, {pwarns} warning(s)"),
                });
            } else {
                findings.push(SkillPlanRefFinding {
                    plan_ref: plan_ref.clone(),
                    level: PlanFindingLevel::Ok,
                    msg: format!("plan verify clean: {plan_ref}"),
                });
            }
        }

        // 2. Raw-glyph check on the skill body (warning-only — emoji in
        // skill prose is allowed by design).
        let glyph_count = count_registry_glyphs(&content);
        if glyph_count > 0 {
            total_warns += 1;
            findings.push(SkillPlanRefFinding {
                plan_ref: String::new(),
                level: PlanFindingLevel::Warn,
                msg: format!(
                    "{glyph_count} raw registry glyph literal(s) in skill body \
                     (allowed in prose; flagged for awareness)"
                ),
            });
        }

        reports.push(SkillReport { label, findings });
    }

    if json {
        let arr: Vec<serde_json::Value> = reports
            .iter()
            .map(|r| {
                serde_json::json!({
                    "skill": r.label,
                    "findings": r.findings.iter().map(|f| serde_json::json!({
                        "plan_ref": f.plan_ref,
                        "level": match f.level {
                            PlanFindingLevel::Ok => "ok",
                            PlanFindingLevel::Warn => "warn",
                            PlanFindingLevel::Error => "error",
                        },
                        "message": f.msg,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let out = serde_json::json!({
            "skills_scanned": skill_files.len(),
            "skills_with_plan_refs": skills_with_refs,
            "errors": total_errors,
            "warnings": total_warns,
            "results": arr,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        if total_errors > 0 {
            std::process::exit(1);
        }
        return Ok(());
    }

    println!(
        "{} {} skill(s) scanned, {} reference a plan",
        "Linting skills:".bold(),
        skill_files.len(),
        skills_with_refs
    );
    println!();

    for r in &reports {
        println!("{}", r.label.cyan().bold());
        for f in &r.findings {
            if quiet && f.level == PlanFindingLevel::Ok {
                continue;
            }
            let tag = match f.level {
                PlanFindingLevel::Ok => "  OK   ".green(),
                PlanFindingLevel::Warn => "  WARN ".yellow(),
                PlanFindingLevel::Error => "  ERROR".red().bold(),
            };
            println!("{} {}", tag, f.msg);
        }
        println!();
    }

    let verdict = if total_errors > 0 {
        format!("{total_errors} error(s), {total_warns} warning(s) — FAIL")
            .red()
            .bold()
            .to_string()
    } else if total_warns > 0 {
        format!("0 errors, {total_warns} warning(s) — PASS")
            .yellow()
            .to_string()
    } else {
        "all checks passed — PASS".green().bold().to_string()
    };
    println!("{} {}", "Verdict:".bold(), verdict);

    if total_errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Render a `PlanReport` as a plain (un-colored) text report for the read-only
/// `plan_verify` MCP tool. Mirrors the CLI's grouped layout + verdict line but
/// returns a string instead of printing, and never rewrites or exits (the MCP
/// server must not mutate files or kill its own process). trace:EPIC-27
pub(crate) fn render_plan_report_string(report: &PlanReport, plan_label: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Verifying plan: {}", plan_label);
    out.push('\n');

    let mut group = |title: &str, findings: &[PlanFinding]| {
        if findings.is_empty() {
            return;
        }
        let _ = writeln!(out, "{}", title);
        for f in findings {
            let tag = match f.level {
                PlanFindingLevel::Ok => "  OK   ",
                PlanFindingLevel::Warn => "  WARN ",
                PlanFindingLevel::Error => "  ERROR",
            };
            let _ = writeln!(out, "{} {}", tag, f.msg);
        }
        out.push('\n');
    };
    group("Sections", &report.sections);
    group("Files", &report.files);
    group("Line refs", &report.refs);

    let errors = report.error_count();
    let warns = report.warn_count();
    if !report.fixes.is_empty() {
        let _ = writeln!(
            out,
            "hint: run `aida plan verify <file> --fix` to rewrite {} drifted ref(s) automatically",
            report.fixes.len()
        );
        out.push('\n');
    }
    let verdict = if errors > 0 {
        format!("{} error(s), {} warning(s) — FAIL", errors, warns)
    } else if warns > 0 {
        format!("0 errors, {warns} warning(s) — PASS")
    } else {
        "all checks passed — PASS".to_string()
    };
    let _ = write!(out, "Verdict: {}", verdict);
    out
}

/// Build the `## Reusable helpers` markdown section for `target` by walking
/// the requirement graph (siblings / tag-mates / same-feature specs) and
/// harvesting their `trace:` comments. Returns `None` when no related spec
/// contributes a named helper. Shared by `aida plan helpers` and the
/// `aida ultraplan` prompt assembler. trace:TASK-94 TASK-113 | ai:claude
pub(crate) fn build_reusable_helpers_section(
    store: &aida_core::RequirementsStore,
    project_root: &std::path::Path,
    target: &aida_core::models::Requirement,
) -> Option<String> {
    // ── Related-spec discovery ── first reason wins (sibling beats
    // feature beats tag) so each spec appears once with its strongest tie.
    let parent_uuids: Vec<uuid::Uuid> = target
        .relationships
        .iter()
        .filter(|r| r.rel_type == aida_core::RelationshipType::Child)
        .map(|r| r.target_id)
        .collect();
    let target_feature = target.feature.trim().to_string();
    let target_tags = target.tags.clone();

    let mut related: Vec<(&aida_core::models::Requirement, &'static str)> = Vec::new();
    let mut seen: HashSet<uuid::Uuid> = HashSet::new();
    seen.insert(target.id);

    if !parent_uuids.is_empty() {
        for req in &store.requirements {
            if seen.contains(&req.id) {
                continue;
            }
            let is_sibling = req.relationships.iter().any(|r| {
                r.rel_type == aida_core::RelationshipType::Child
                    && parent_uuids.contains(&r.target_id)
            });
            if is_sibling {
                seen.insert(req.id);
                related.push((req, "sibling"));
            }
        }
    }
    // Shared tags before feature: a shared tag is a deliberate grouping
    // (a `batch:`, a topic) and a stronger signal than `feature`, which
    // is often a coarse project-wide bucket.
    if !target_tags.is_empty() {
        for req in &store.requirements {
            if seen.contains(&req.id) {
                continue;
            }
            if req.tags.iter().any(|t| target_tags.contains(t)) {
                seen.insert(req.id);
                related.push((req, "shared tag"));
            }
        }
    }
    if !target_feature.is_empty() {
        let feature_matches: Vec<&aida_core::models::Requirement> = store
            .requirements
            .iter()
            .filter(|r| !seen.contains(&r.id) && r.feature.trim() == target_feature)
            .collect();
        // A feature shared by dozens of specs is a project-wide bucket,
        // not a meaningful relation — skip it rather than flood the
        // section with weakly-related noise.
        const FEATURE_DISCRIMINATION_CAP: usize = 25;
        if feature_matches.len() <= FEATURE_DISCRIMINATION_CAP {
            for req in feature_matches {
                seen.insert(req.id);
                related.push((req, "same feature"));
            }
        }
    }

    let target_display = target.display_id();
    if related.is_empty() {
        return None;
    }

    // Every id-string a trace comment might use (canonical spec_id or the
    // agreed short id) → index into `related`.
    let mut id_to_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (idx, (req, _)) in related.iter().enumerate() {
        if let Some(s) = &req.spec_id {
            id_to_idx.insert(s.clone(), idx);
        }
        if let Some(a) = &req.agreed_id {
            id_to_idx.insert(a.clone(), idx);
        }
    }
    let wanted: HashSet<String> = id_to_idx.keys().cloned().collect();

    let hits = scan_trace_graph(project_root, &wanted);

    // Fold hits onto each related spec, deduped by (file, symbol).
    let mut per_spec: Vec<(usize, std::collections::BTreeMap<String, Vec<String>>)> = Vec::new();
    for (idx, _) in related.iter().enumerate() {
        per_spec.push((idx, std::collections::BTreeMap::new()));
    }
    for (id, id_hits) in &hits {
        let Some(&idx) = id_to_idx.get(id) else {
            continue;
        };
        let by_file = &mut per_spec[idx].1;
        for hit in id_hits {
            let syms = by_file.entry(hit.file.clone()).or_default();
            if let Some(sym) = &hit.symbol {
                if !syms.contains(sym) {
                    syms.push(sym.clone());
                }
            }
        }
    }

    // Keep only specs that introduce at least one *named* helper — the
    // section is "reusable helpers", and a bare file-touched hit (every
    // spec touches main.rs) carries no reuse signal. Rank by tier
    // (siblings are the closest relatives, then tag-mates, then the
    // coarse same-feature set) and within a tier by helper count, then
    // cap so the section stays a focused brief.
    const HELPERS_SPEC_CAP: usize = 12;
    const HELPERS_PER_FILE_CAP: usize = 8;
    let tier = |reason: &str| -> u8 {
        match reason {
            "sibling" => 0,
            "shared tag" => 1,
            _ => 2, // same feature — project-wide, weakest signal
        }
    };
    let mut ranked: Vec<(usize, u8, usize)> = per_spec
        .iter()
        .filter_map(|(idx, by_file)| {
            let symbols: usize = by_file.values().map(Vec::len).sum();
            (symbols > 0).then_some((*idx, tier(related[*idx].1), symbols))
        })
        .collect();
    ranked.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    let truncated = ranked.len().saturating_sub(HELPERS_SPEC_CAP);
    ranked.truncate(HELPERS_SPEC_CAP);

    // No related spec names a helper → nothing worth a section.
    if ranked.is_empty() {
        return None;
    }

    // ── Render the section ──
    let mut md = String::new();
    md.push_str("## Reusable helpers (do not reimplement)\n\n");
    md.push_str(&format!(
        "Derived from the trace graph for {target_display} — related specs that already \
         touch this area. Call these rather than re-implementing.\n\n"
    ));

    for (idx, _, _) in &ranked {
        let (req, reason) = related[*idx];
        let by_file = &per_spec[*idx].1;
        md.push_str(&format!(
            "**{}** — {} · {} · {}\n\n",
            req.display_id(),
            req.title,
            reason,
            req.status
        ));
        for (file, syms) in by_file {
            if syms.is_empty() {
                md.push_str(&format!("- `{file}`\n"));
            } else {
                let mut sorted = syms.clone();
                sorted.sort();
                let extra = sorted.len().saturating_sub(HELPERS_PER_FILE_CAP);
                sorted.truncate(HELPERS_PER_FILE_CAP);
                let mut joined = sorted
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                if extra > 0 {
                    joined.push_str(&format!(", …+{extra}"));
                }
                md.push_str(&format!("- `{file}` — {joined}\n"));
            }
        }
        md.push('\n');
    }
    if truncated > 0 {
        md.push_str(&format!(
            "_…and {truncated} more related spec(s) — narrow the scope or inspect with \
             `aida list`._\n"
        ));
    }
    Some(md)
}

/// One-line summary of a related requirement for the prompt's context block.
pub(crate) fn ultraplan_spec_line(req: &aida_core::models::Requirement) -> String {
    format!("{} — {} [{}]", req.display_id(), req.title, req.status)
}

/// Build the `## Comments` section for the ultraplan prompt — the spec's
/// enrichment comments, most recent first. `None` when the spec has no
/// non-empty comments. Caps both the comment count and the total
/// characters so a spec with a long discussion thread can't blow the
/// prompt budget; the richest planning context (long enrichment
/// comments) is exactly what TASK-247 exists to surface, so the budget
/// is deliberately generous. trace:TASK-247 | ai:claude
pub(crate) fn ultraplan_comments_section(
    comments: &[aida_core::models::Comment],
) -> Option<String> {
    const COMMENT_COUNT_CAP: usize = 20;
    const COMMENTS_CHAR_CAP: usize = 16_000;

    let non_empty = comments
        .iter()
        .filter(|c| !c.content.trim().is_empty())
        .count();
    if non_empty == 0 {
        return None;
    }

    let mut s = String::from(
        "## Comments\n\nPlanning context captured on the spec as comments, most recent first.\n\n",
    );
    let mut used = 0usize;
    let mut shown = 0usize;
    for c in comments.iter().rev() {
        let content = c.content.trim();
        if content.is_empty() {
            continue;
        }
        if shown >= COMMENT_COUNT_CAP || used >= COMMENTS_CHAR_CAP {
            break;
        }
        let reply_note = match c.replies.len() {
            0 => String::new(),
            n => format!(" · {n} repl{}", if n == 1 { "y" } else { "ies" }),
        };
        s.push_str(&format!(
            "### {} · {}{}\n\n",
            c.author,
            c.created_at.format("%Y-%m-%d"),
            reply_note
        ));
        let budget = COMMENTS_CHAR_CAP - used;
        if content.chars().count() > budget {
            let trimmed: String = content.chars().take(budget).collect();
            s.push_str(&trimmed);
            s.push_str("\n\n_(comment truncated to fit the prompt budget)_\n\n");
            used = COMMENTS_CHAR_CAP;
        } else {
            s.push_str(content);
            s.push_str("\n\n");
            used += content.chars().count();
        }
        shown += 1;
    }
    if shown < non_empty {
        s.push_str(&format!(
            "_(+{} older comment(s) omitted to fit the prompt budget)_\n\n",
            non_empty - shown
        ));
    }
    Some(s)
}

/// Assemble the AIDA planning prompt for `target`. `helpers_section` is the
/// pre-built trace-graph reusable-helpers markdown (None when there is
/// none — see TASK-94). `include_comments` pulls the spec's enrichment
/// comments into a `## Comments` section (TASK-247). Returns the prompt
/// and any warnings to surface. Pure over its inputs so it is
/// unit-testable. trace:TASK-113 TASK-247 | ai:claude
/// Assemble the rich, ultraplan-grade punt payload (STORY-306) a headless
/// advisor judges. The structured fork fields come from the punted spec's
/// recorded [`AttentionReason`](aida_core::AttentionReason) — what `aida
/// punt` captured — and the `context_markdown` body reuses the `aida
/// ultraplan` machinery (`assemble_ultraplan_prompt` + the trace-graph
/// `build_reusable_helpers_section`), so an advisor with **no session
/// context** has everything the implementer's planning prompt would have.
/// Split out as a deterministic, unit-testable transform — no session, no
/// subprocess. trace:STORY-306 | ai:claude
pub(crate) fn assemble_punt_payload(
    store: &aida_core::RequirementsStore,
    target: &aida_core::models::Requirement,
    project_root: &std::path::Path,
    attention: &aida_core::AttentionReason,
) -> punt::PuntRequest {
    let helpers = build_reusable_helpers_section(store, project_root, target);
    let (reservations, _reservation_warnings) = read_reserved_paths(project_root);
    let (context_markdown, _warnings) =
        assemble_ultraplan_prompt(store, target, helpers.as_deref(), true, &reservations);
    punt::PuntRequest {
        spec: target.display_id(),
        category: attention.category,
        question: attention.detail.clone(),
        // `aida punt` records the fork as free-form `detail` + `lean`; it
        // does not enumerate options / code-area / stakes separately, so
        // those structured fields stay empty until a future punt does.
        options: Vec::new(),
        code_area: None,
        stakes: None,
        lean: attention.lean.clone(),
        context_markdown,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReservedPath {
    pub(crate) path: String,
    pub(crate) reason: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ReservedPathsFile {
    #[serde(default)]
    pub(crate) reservations: Vec<ReservedPath>,
}

pub(crate) fn read_reserved_paths(
    project_root: &std::path::Path,
) -> (Vec<ReservedPath>, Vec<String>) {
    let path = project_root.join(".aida").join("reserved-paths.toml");
    let Ok(body) = std::fs::read_to_string(&path) else {
        return (
            Vec::new(),
            vec![format!(
                "{} not found — reserved namespace guidance omitted",
                path.display()
            )],
        );
    };

    match toml::from_str::<ReservedPathsFile>(&body) {
        Ok(parsed) => (parsed.reservations, Vec::new()),
        Err(err) => (
            Vec::new(),
            vec![format!(
                "failed to parse {} — reserved namespace guidance omitted: {err}",
                path.display()
            )],
        ),
    }
}

pub(crate) fn reserved_paths_section(reservations: &[ReservedPath]) -> Option<String> {
    if reservations.is_empty() {
        return None;
    }

    let mut out = String::from("## Reserved namespaces and conventions\n\n");
    out.push_str(
        "Before proposing new files, check these project-reserved paths and avoid \
         collisions unless the plan explicitly updates the owning convention.\n\n",
    );
    for reservation in reservations {
        out.push_str(&format!(
            "- `{}` — {}\n",
            reservation.path.trim(),
            reservation.reason.trim()
        ));
    }
    out.push('\n');
    Some(out)
}

pub(crate) fn assemble_ultraplan_prompt(
    store: &aida_core::RequirementsStore,
    target: &aida_core::models::Requirement,
    helpers_section: Option<&str>,
    include_comments: bool,
    reservations: &[ReservedPath],
) -> (String, Vec<String>) {
    // TASK-247: raised from 1800 — a truncated description dropped the
    // densest planning context. Comments now carry the long-form
    // enrichment, but the description still deserves real room.
    const DESC_CHAR_CAP: usize = 16_000;
    const SIBLING_CAP: usize = 12;
    let mut warnings: Vec<String> = Vec::new();
    let display = target.display_id();
    let resolve = |id: &uuid::Uuid| store.requirements.iter().find(|r| &r.id == id);

    let mut p = String::new();
    p.push_str(&format!(
        "Plan the implementation of {display}: {}.\n\n",
        target.title
    ));

    // ── Requirement body ──
    p.push_str("## Requirement\n\n");
    let desc = target.description.trim();
    if desc.is_empty() {
        p.push_str("_(no description on the spec)_\n\n");
        warnings.push("spec has no description — the plan will be weakly grounded".to_string());
    } else if desc.chars().count() > DESC_CHAR_CAP {
        let truncated: String = desc.chars().take(DESC_CHAR_CAP).collect();
        p.push_str(&truncated);
        p.push_str("\n\n_(description truncated to fit the prompt budget)_\n\n");
    } else {
        p.push_str(desc);
        p.push_str("\n\n");
    }

    // ── Acceptance criteria ──
    p.push_str("## Acceptance criteria\n\n");
    match extract_acceptance_section(&target.description) {
        Some(acc) if !acc.trim().is_empty() => {
            p.push_str(acc.trim());
            p.push('\n');
        }
        _ => {
            p.push_str(
                "_(none specified — add a `## Acceptance` section to the spec for a sharper plan)_\n",
            );
            warnings.push(
                "spec has no `## Acceptance` section — the plan will be weaker without \
                 explicit acceptance criteria"
                    .to_string(),
            );
        }
    }
    p.push('\n');

    // ── Comments (enrichment context) ──
    // TASK-247: a spec's comments accumulate the densest planning
    // context (anti-patterns, design forks, reusable helpers). Pull them
    // in unless the caller opted out with `--no-comments`.
    if include_comments {
        if let Some(section) = ultraplan_comments_section(&target.comments) {
            p.push_str(&section);
        }
    }

    // TASK-517: surface reserved namespaces before the plan proposes files.
    // This catches likely collisions (notably docs/aida/) while the model is
    // still choosing the implementation shape.
    if let Some(section) = reserved_paths_section(reservations) {
        p.push_str(&section);
    }

    // ── Related-spec context ──
    let parents: Vec<&aida_core::models::Requirement> = target
        .relationships
        .iter()
        .filter(|r| r.rel_type == aida_core::RelationshipType::Child)
        .filter_map(|r| resolve(&r.target_id))
        .collect();
    let children: Vec<&aida_core::models::Requirement> = target
        .relationships
        .iter()
        .filter(|r| r.rel_type == aida_core::RelationshipType::Parent)
        .filter_map(|r| resolve(&r.target_id))
        .collect();
    // Other typed relationships (verifies / references / custom) — the
    // "composes / depends-on" signal the prompt should anchor on.
    let other_rels: Vec<(String, &aida_core::models::Requirement)> = target
        .relationships
        .iter()
        .filter(|r| {
            !matches!(
                r.rel_type,
                aida_core::RelationshipType::Parent | aida_core::RelationshipType::Child
            )
        })
        .filter_map(|r| resolve(&r.target_id).map(|req| (r.rel_type.to_string(), req)))
        .collect();
    let parent_ids: HashSet<uuid::Uuid> = parents.iter().map(|r| r.id).collect();
    let siblings: Vec<&aida_core::models::Requirement> = if parent_ids.is_empty() {
        Vec::new()
    } else {
        store
            .requirements
            .iter()
            .filter(|r| {
                r.id != target.id
                    && r.relationships.iter().any(|rel| {
                        rel.rel_type == aida_core::RelationshipType::Child
                            && parent_ids.contains(&rel.target_id)
                    })
            })
            .collect()
    };

    if !parents.is_empty() || !children.is_empty() || !siblings.is_empty() || !other_rels.is_empty()
    {
        p.push_str("## Related specs\n\n");
        for parent in &parents {
            p.push_str(&format!("- Parent: {}\n", ultraplan_spec_line(parent)));
        }
        for (rel, req) in &other_rels {
            p.push_str(&format!("- {}: {}\n", rel, ultraplan_spec_line(req)));
        }
        if !children.is_empty() {
            p.push_str("- Children:\n");
            for child in &children {
                p.push_str(&format!("  - {}\n", ultraplan_spec_line(child)));
            }
        }
        if !siblings.is_empty() {
            p.push_str(&format!(
                "- Siblings (share a parent — {} total):\n",
                siblings.len()
            ));
            for sib in siblings.iter().take(SIBLING_CAP) {
                p.push_str(&format!("  - {}\n", ultraplan_spec_line(sib)));
            }
            if siblings.len() > SIBLING_CAP {
                p.push_str(&format!(
                    "  - _(+{} more siblings omitted to fit the prompt budget)_\n",
                    siblings.len() - SIBLING_CAP
                ));
            }
        }
        p.push('\n');
    }

    // ── Plan structure ──
    p.push_str("## Plan structure\n\n");
    p.push_str(ULTRAPLAN_STRUCTURE);
    p.push_str("\n\n");

    // ── Reusable helpers (trace-graph derived) ──
    match helpers_section {
        Some(section) => {
            p.push_str(section);
            if !section.ends_with('\n') {
                p.push('\n');
            }
            p.push('\n');
        }
        None => {
            warnings.push(
                "no trace-graph reusable helpers found — Reusable helpers section omitted"
                    .to_string(),
            );
        }
    }

    // ── Style notes ──
    p.push_str("## Style notes\n\n");
    p.push_str(
        "- Prefer symbol refs (`fn handle_pull_command`) over line refs (`main.rs:19713`) — \
         line refs drift fast.\n\
         - Order the Files section so each commit builds clean.\n\
         - The Verification section must be an executable bash smoke test.\n\
         - Keep the Followups section to one TASK-title-sized line per item.\n",
    );

    (p, warnings)
}
