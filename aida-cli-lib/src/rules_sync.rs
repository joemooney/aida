//! SPIKE-31: emit Claude Code path-gated rules from the AIDA spec graph.
//!
//! For each spec that is actively being worked (status in {InProgress, Done}
//! AND has at least one `// trace:SPEC-ID` comment in the code), write a
//! rule file at `.claude/rules/aida-specs/<SPEC-ID>.md` with `paths:` glob
//! frontmatter matching the traced files. Claude Code's runtime loads the
//! rule on demand when the implementer reads or edits one of those files —
//! making the spec's scope and acceptance criteria self-enforcing at the
//! exact moment they're load-bearing.
//!
//! Substrate-as-bouncer: AIDA decides what gets enforced; Claude Code's
//! runtime enforces it. The rule file is the handshake between them.
//!
//! Files are gitignored by convention (per-clone derived state); sync
//! creates the directory if missing and reconciles desired vs on-disk
//! (writes new/changed, removes specs no longer in the active set).
//!
//! trace:SPIKE-31 | ai:claude

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use aida_core::{ArchiveFilter, DatabaseBackend, ListFilter, Requirement, RequirementSummary};
use anyhow::{Context, Result};

/// SPIKE-31 outcome of one `aida rules sync` invocation. Returned to
/// callers so `--dry-run` and the apply path share the same data shape.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub written: Vec<PathBuf>,
    pub unchanged: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    pub skipped_no_traces: Vec<String>,
    /// SPIKE-35: REVIEW.md output state, if `--review-md` was set.
    /// Populated by `sync_review_md`; left None when the flag is off.
    pub review_md: Option<ReviewMdReport>,
}

/// SPIKE-35: outcome of one REVIEW.md emit attempt. Mirrors SyncReport's
/// shape so the caller can render written/unchanged uniformly.
#[derive(Debug, Default)]
pub struct ReviewMdReport {
    pub path: PathBuf,
    pub written: bool,
    pub unchanged: bool,
    pub specs_included: Vec<String>,
}

/// Status filter for `is_active_for_rules`. A spec is active for rule
/// emission iff it's being worked NOW or recently shipped but not yet
/// merged — `InProgress` and `Done`. `Approved` and `Planned` specs
/// typically have no code yet (no trace comments to gate on), and
/// `Completed`/`Released`/`Rejected`/`Draft` specs aren't active scope.
pub fn is_active_for_rules(status: &str) -> bool {
    matches!(status, "In Progress" | "InProgress" | "Done")
}

fn rules_root(project_root: &Path) -> PathBuf {
    project_root
        .join(".claude")
        .join("rules")
        .join("aida-specs")
}

/// Plan + (optionally) apply the rule-file sync. With `dry_run = true`,
/// computes the report without touching disk. Otherwise reconciles
/// `.claude/rules/aida-specs/` against the desired set.
pub fn sync(
    project_root: &Path,
    backend: &aida_core::CachedGitBackend,
    trace_graph: &HashMap<String, Vec<TracedFile>>,
    dry_run: bool,
) -> Result<SyncReport> {
    // Step 1: pull every spec that could be in scope (avoid loading the
    // full store; the cache summary already has status + title + tags).
    let summaries = backend.list_summaries(&ListFilter {
        archive: ArchiveFilter::Both,
        ..Default::default()
    })?;

    // Build spec-id → summary lookup for both agreed and node-aware ids.
    let mut by_id: HashMap<String, &RequirementSummary> = HashMap::new();
    for s in &summaries {
        if let Some(id) = s.agreed_id.as_deref() {
            by_id.insert(id.to_string(), s);
        }
        if let Some(id) = s.spec_id.as_deref() {
            by_id.insert(id.to_string(), s);
        }
    }

    // Desired files: spec is active AND has trace hits.
    let mut desired: HashMap<String, String> = HashMap::new();
    let mut report = SyncReport::default();
    for (spec_id, files) in trace_graph {
        let Some(summary) = by_id.get(spec_id.as_str()) else {
            continue;
        };
        if !is_active_for_rules(&summary.status) {
            continue;
        }
        if files.is_empty() {
            report.skipped_no_traces.push(spec_id.clone());
            continue;
        }
        // Pull the full Requirement for description + acceptance criteria.
        // Try the agreed/short id first, then fall back to the spec id.
        let full = backend
            .get_requirement_by_spec_id(spec_id)
            .ok()
            .flatten()
            .or_else(|| {
                summary
                    .spec_id
                    .as_deref()
                    .and_then(|id| backend.get_requirement_by_spec_id(id).ok().flatten())
            });
        let rendered = render_rule(spec_id, summary, full.as_ref(), files);
        desired.insert(spec_id.clone(), rendered);
    }

    // Discover existing files in .claude/rules/aida-specs/.
    let dir = rules_root(project_root);
    let existing: HashMap<String, PathBuf> = if dir.is_dir() {
        std::fs::read_dir(&dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let stem = path.file_stem()?.to_str()?.to_string();
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    Some((stem, path))
                } else {
                    None
                }
            })
            .collect()
    } else {
        HashMap::new()
    };

    // Reconcile: write new/changed, leave unchanged untouched, remove
    // any existing file whose spec is no longer in the active set.
    if !dry_run && !desired.is_empty() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create rules dir {}", dir.display()))?;
    }
    let desired_keys: HashSet<&String> = desired.keys().collect();
    for (spec_id, body) in &desired {
        let path = dir.join(format!("{}.md", spec_id));
        let unchanged = std::fs::read_to_string(&path).is_ok_and(|s| s == *body);
        if unchanged {
            report.unchanged.push(path);
        } else {
            if !dry_run {
                std::fs::write(&path, body)
                    .with_context(|| format!("write rule {}", path.display()))?;
            }
            report.written.push(path);
        }
    }
    for (spec_id, path) in &existing {
        if desired_keys.contains(spec_id) {
            continue;
        }
        if !dry_run {
            let _ = std::fs::remove_file(path);
        }
        report.removed.push(path.clone());
    }

    Ok(report)
}

/// One row in the trace harvest passed to [`sync`]. The CLI populates this
/// from `scan_trace_graph` so the module stays free of regex / file-walk
/// concerns (and unit-testable without a project on disk).
#[derive(Debug, Clone)]
pub struct TracedFile {
    pub path: String,
    pub symbol: Option<String>,
}

/// SPIKE-35: emit `REVIEW.md` at project root from the spec graph. One
/// file (Anthropic's managed Code Review reads exactly one REVIEW.md per
/// repo as the highest-priority injection into every reviewer agent).
/// Aggregates every active spec's acceptance criteria + a severity
/// calibration tying 🔴 Important to acceptance-criteria violations.
///
/// Substrate-as-bouncer for the reviewer surface — same shape as
/// SPIKE-31 for path-gated rules but targeting Code Review's reviewer
/// pipeline rather than implementer file-loads. trace:SPIKE-35
fn review_fragments_root(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("review")
}

/// SPIKE-35 v2: emit per-spec `REVIEW` fragments under `.aida/review/<SPEC-ID>.md`.
/// Discovers active specs that carry trace comments, renders their fragment,
/// and reconciles `.aida/review/` against the active set (writes new/changed,
/// removes stale specs).
/// trace:SPIKE-35 | ai:antigravity
pub fn sync_review_md(
    project_root: &Path,
    backend: &aida_core::CachedGitBackend,
    trace_graph: &HashMap<String, Vec<TracedFile>>,
    dry_run: bool,
) -> Result<ReviewMdReport> {
    let summaries = backend.list_summaries(&ListFilter {
        archive: ArchiveFilter::Both,
        ..Default::default()
    })?;
    let mut by_id: HashMap<String, &RequirementSummary> = HashMap::new();
    for s in &summaries {
        if let Some(id) = s.agreed_id.as_deref() {
            by_id.insert(id.to_string(), s);
        }
        if let Some(id) = s.spec_id.as_deref() {
            by_id.insert(id.to_string(), s);
        }
    }

    // Gather active specs with trace comments
    let mut active: Vec<(&str, &RequirementSummary, &Vec<TracedFile>)> = Vec::new();
    for (spec_id, files) in trace_graph {
        let Some(summary) = by_id.get(spec_id.as_str()) else {
            continue;
        };
        if !is_active_for_rules(&summary.status) {
            continue;
        }
        if files.is_empty() {
            continue;
        }
        active.push((spec_id.as_str(), *summary, files));
    }
    active.sort_by_key(|(id, _, _)| id.to_string());

    let dir = review_fragments_root(project_root);
    if !dry_run && !active.is_empty() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create review fragments dir {}", dir.display()))?;
    }

    // Discover existing fragment files on disk
    let existing: HashMap<String, PathBuf> = if dir.is_dir() {
        std::fs::read_dir(&dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let stem = path.file_stem()?.to_str()?.to_string();
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    Some((stem, path))
                } else {
                    None
                }
            })
            .collect()
    } else {
        HashMap::new()
    };

    let mut written_count = 0;
    let mut unchanged_count = 0;
    let mut desired_keys = HashSet::new();

    for (spec_id, summary, files) in &active {
        desired_keys.insert(spec_id.to_string());
        let body = render_spec_fragment(spec_id, summary, backend, files);
        let path = dir.join(format!("{}.md", spec_id));
        let is_unchanged = std::fs::read_to_string(&path).is_ok_and(|s| s == body);

        if is_unchanged {
            unchanged_count += 1;
        } else {
            if !dry_run {
                std::fs::write(&path, &body)
                    .with_context(|| format!("write review fragment {}", path.display()))?;
            }
            written_count += 1;
        }
    }

    // Remove stale fragments
    let mut removed_any = false;
    for (spec_id, path) in &existing {
        if desired_keys.contains(spec_id) {
            continue;
        }
        if !dry_run {
            let _ = std::fs::remove_file(path);
        }
        removed_any = true;
    }

    let report = ReviewMdReport {
        path: dir,
        written: written_count > 0 || removed_any,
        unchanged: written_count == 0 && !removed_any && unchanged_count > 0,
        specs_included: active.iter().map(|(id, _, _)| id.to_string()).collect(),
    };

    Ok(report)
}

fn render_spec_fragment(
    spec_id: &str,
    summary: &RequirementSummary,
    backend: &aida_core::CachedGitBackend,
    files: &[TracedFile],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("### {}: {}\n\n", spec_id, summary.title));
    out.push_str(&format!("**Status**: {}\n\n", summary.status));
    let full = backend
        .get_requirement_by_spec_id(spec_id)
        .ok()
        .flatten()
        .or_else(|| {
            summary
                .spec_id
                .as_deref()
                .and_then(|id| backend.get_requirement_by_spec_id(id).ok().flatten())
        });
    let description = full
        .as_ref()
        .map(|r| r.description.as_str())
        .unwrap_or(summary.description.as_str())
        .trim();
    // Prefer the Acceptance section when present; fall back to
    // the full description.
    if let Some(section) = description.lines().map(|l| l.trim_start()).position(|t| {
        (t.starts_with("## ") && t[3..].trim_start().starts_with("Acceptance"))
            || (t.starts_with("### ") && t[4..].trim_start().starts_with("Acceptance"))
    }) {
        let mut accept = String::new();
        for line in description.lines().skip(section + 1) {
            let trimmed = line.trim_start();
            if trimmed.starts_with("## ") || trimmed.starts_with("### ") {
                break;
            }
            accept.push_str(line);
            accept.push('\n');
        }
        let trimmed = accept.trim();
        if !trimmed.is_empty() {
            out.push_str("**Acceptance criteria:**\n\n");
            out.push_str(trimmed);
            out.push_str("\n\n");
        }
    } else if !description.is_empty() {
        // No explicit Acceptance section — surface the spec
        // description as the criteria proxy so reviewers see
        // the spec's expectations. Truncate at first heading
        // to keep REVIEW.md compact.
        let body: String = description
            .lines()
            .take_while(|l| !l.trim_start().starts_with("## "))
            .collect::<Vec<_>>()
            .join("\n");
        let trimmed = body.trim();
        if !trimmed.is_empty() {
            out.push_str("**Spec description (no explicit Acceptance section):**\n\n");
            out.push_str(trimmed);
            out.push_str("\n\n");
        }
    }

    // Surface the paths the spec touches so the reviewer knows
    // which files to scrutinize against the criteria.
    let mut paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    paths.dedup();
    if !paths.is_empty() {
        out.push_str("**Files in scope:**\n\n");
        for p in &paths {
            out.push_str(&format!("- `{}`\n", p));
        }
        out.push('\n');
    }
    out
}

/// SPIKE-35: Assemble all active fragments under `.aida/review/` into a root `REVIEW.md`
/// and save it to `output_path` (defaulting to `<project_root>/REVIEW.md`).
/// trace:SPIKE-35 | ai:antigravity
pub fn assemble_review_md(project_root: &Path, output_path: Option<&Path>) -> Result<PathBuf> {
    let dir = review_fragments_root(project_root);
    let mut fragments: Vec<(String, String)> = Vec::new();
    if dir.is_dir() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                let spec_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                let body = std::fs::read_to_string(&path)?;
                fragments.push((spec_id, body));
            }
        }
    }
    // Stable spec ordering by SPEC-ID so concurrent assembles don't produce different bytes.
    fragments.sort_by_key(|(spec_id, _)| spec_id.clone());

    let mut out = String::new();
    out.push_str("# Review instructions\n\n");
    out.push_str(
        "<!-- This file is auto-generated by `aida review assemble` from per-spec fragments. -->\n",
    );
    out.push_str(
        "<!-- Edit specs (acceptance criteria), not this file. trace:SPIKE-35 | ai:claude -->\n\n",
    );

    out.push_str("## What Important means here\n\n");
    out.push_str(
        "Reserve 🔴 Important for findings that violate the acceptance criteria of an active spec \
         this PR touches (listed below). A bug that breaks behavior the spec requires is Important. \
         Style, naming, and refactoring suggestions outside the acceptance criteria are 🟡 Nit at most.\n\n",
    );
    out.push_str(
        "🟣 Pre-existing applies to bugs that exist in the codebase but weren't introduced by this PR.\n\n",
    );

    out.push_str("## Always check\n\n");
    out.push_str(
        "- Every commit subject ends with `(SPEC-ID)` matching one of the active specs below.\n",
    );
    out.push_str(
        "- Source-file changes carry `// trace:SPEC-ID | ai:<tool>` comments linking to a spec.\n",
    );
    out.push_str(
        "- Files traced to a spec NOT listed below are out-of-scope churn — flag as 🔴 Important.\n\n",
    );

    if fragments.is_empty() {
        out.push_str("## Active specs\n\n");
        out.push_str("_No active specs (In Progress or Done) have trace comments in the codebase right now. \
                      Review the diff against general project conventions in `CLAUDE.md`._\n\n");
    } else {
        out.push_str(&format!(
            "## Active specs in scope for this PR ({})\n\n",
            fragments.len()
        ));
        for (_, body) in &fragments {
            out.push_str(body);
            if !body.ends_with("\n\n") {
                out.push('\n');
            }
        }
    }

    out.push_str("## Skip\n\n");
    out.push_str(
        "- Generated files (per repo `.gitignore`); no need to flag formatting in build output.\n",
    );
    out.push_str("- Pure documentation rewording when no source-file changes accompany it.\n");
    out.push_str(
        "- CLAUDE.md / AGENTS.md edits unless they introduce contradictions with active specs.\n\n",
    );

    out.push_str("## Summary shape\n\n");
    out.push_str(
        "Open the review with a one-line tally — e.g. `2 Important, 4 Nit, 0 Pre-existing` — \
         and if no Important findings exist, lead with `No blocking issues against active spec criteria.`\n",
    );

    let output = output_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| project_root.join("REVIEW.md"));

    std::fs::write(&output, &out)
        .with_context(|| format!("write REVIEW.md at {}", output.display()))?;

    Ok(output)
}

fn render_rule(
    spec_id: &str,
    summary: &RequirementSummary,
    full: Option<&Requirement>,
    files: &[TracedFile],
) -> String {
    let mut paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    paths.sort();
    paths.dedup();

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("paths:\n");
    for p in &paths {
        out.push_str(&format!("  - \"{}\"\n", p));
    }
    out.push_str("---\n\n");
    out.push_str(&format!("# {}: {}\n\n", spec_id, summary.title));
    out.push_str(&format!("**Status**: {}\n", summary.status));
    if !summary.priority.is_empty() {
        out.push_str(&format!("**Priority**: {}\n", summary.priority));
    }
    let mut tags: Vec<&String> = summary.tags.iter().collect();
    tags.sort();
    if !tags.is_empty() {
        let joined: Vec<&str> = tags.iter().map(|s| s.as_str()).collect();
        out.push_str(&format!("**Tags**: {}\n", joined.join(", ")));
    }
    out.push('\n');

    // Description: prefer the full Requirement when available, otherwise
    // fall back to the cache summary's projection.
    let description = full
        .map(|r| r.description.as_str())
        .unwrap_or(summary.description.as_str())
        .trim();
    if !description.is_empty() {
        out.push_str("## Spec description\n\n");
        out.push_str(description);
        out.push_str("\n\n");
        // Surface acceptance criteria if the description has an
        // "## Acceptance" section — implementer guard-rail.
        if let Some(section) = extract_section(description, "Acceptance") {
            out.push_str("## Acceptance criteria\n\n");
            out.push_str(section.trim());
            out.push_str("\n\n");
        }
    }

    out.push_str("## Files this spec touches\n\n");
    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
    for f in files {
        let entry = grouped.entry(f.path.clone()).or_default();
        if let Some(sym) = &f.symbol {
            entry.push(sym.clone());
        }
    }
    let mut keys: Vec<String> = grouped.keys().cloned().collect();
    keys.sort();
    for k in &keys {
        let mut syms = grouped.remove(k).unwrap_or_default();
        syms.sort();
        syms.dedup();
        if syms.is_empty() {
            out.push_str(&format!("- `{}`\n", k));
        } else {
            out.push_str(&format!("- `{}` — {}\n", k, syms.join(", ")));
        }
    }
    out.push('\n');

    out.push_str("---\n");
    out.push_str(&format!(
        "*Auto-generated by `aida rules sync` from the AIDA spec graph. Edit the spec ({}), not this file. trace:SPIKE-31*\n",
        spec_id
    ));
    out
}

/// Extract a markdown section by header name. Matches `## Header` or
/// `### Header` and returns the body until the next `##`/`###` heading
/// (or end of input). Returns None when not found.
fn extract_section(body: &str, header: &str) -> Option<String> {
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        let is_match = (t.starts_with("## ") && t[3..].trim_start().starts_with(header))
            || (t.starts_with("### ") && t[4..].trim_start().starts_with(header));
        if !is_match {
            continue;
        }
        let mut section = String::new();
        for next in lines.by_ref() {
            let nt = next.trim_start();
            if nt.starts_with("## ") || nt.starts_with("### ") {
                break;
            }
            section.push_str(next);
            section.push('\n');
        }
        return Some(section);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_active_for_rules_admits_in_progress_and_done_only() {
        assert!(is_active_for_rules("In Progress"));
        assert!(is_active_for_rules("Done"));
        assert!(!is_active_for_rules("Approved"));
        assert!(!is_active_for_rules("Planned"));
        assert!(!is_active_for_rules("Completed"));
        assert!(!is_active_for_rules("Released"));
        assert!(!is_active_for_rules("Rejected"));
        assert!(!is_active_for_rules("Draft"));
    }

    #[test]
    fn extract_section_returns_body_until_next_heading() {
        let body =
            "Intro\n\n## Acceptance\n\n- must do X\n- must do Y\n\n## Followups\n\n- defer Z";
        let s = extract_section(body, "Acceptance").unwrap();
        assert!(s.contains("must do X"));
        assert!(s.contains("must do Y"));
        assert!(!s.contains("defer Z"));
    }

    #[test]
    fn extract_section_none_when_not_present() {
        let body = "Just a description with no acceptance section.";
        assert!(extract_section(body, "Acceptance").is_none());
    }

    fn test_summary() -> RequirementSummary {
        RequirementSummary {
            id: uuid::Uuid::new_v4(),
            spec_id: Some("TASK-99".into()),
            agreed_id: None,
            title: "Test spec".into(),
            description: String::new(),
            status: "In Progress".into(),
            priority: "high".into(),
            owner: String::new(),
            assignee: None,
            feature: String::new(),
            req_type: "Task".into(),
            tags: Vec::new(),
            created_at: String::new(),
            modified_at: String::new(),
            archived: false,
            archived_at: None,
            deferred: false,
            deferred_at: None,
            deferred_until: None,
            in_degree: 0,
            out_degree: 0,
            heft: 0,
            // trace:TASK-902 | ai:claude
            blocked: false,
            // trace:TASK-1065 | ai:claude
            has_pending_decision: false,
            execution_mode: None,
            yaml_path: String::new(),
        }
    }

    #[test]
    fn render_rule_has_paths_frontmatter_and_paths_section() {
        let summary = test_summary();
        let files = vec![
            TracedFile {
                path: "aida-cli/src/main.rs".into(),
                symbol: Some("handle_x".into()),
            },
            TracedFile {
                path: "aida-cli/src/main.rs".into(),
                symbol: Some("handle_y".into()),
            },
            TracedFile {
                path: "aida-core/src/lib.rs".into(),
                symbol: None,
            },
        ];
        let r = render_rule("TASK-99", &summary, None, &files);
        assert!(r.starts_with("---\npaths:\n"));
        assert!(r.contains("\"aida-cli/src/main.rs\""));
        assert!(r.contains("\"aida-core/src/lib.rs\""));
        assert!(r.contains("# TASK-99: Test spec"));
        assert!(r.contains("**Status**: In Progress"));
        assert!(r.contains("handle_x, handle_y"));
        assert!(r.contains("trace:SPIKE-31"));
    }

    #[test]
    fn test_assemble_review_md_stability() {
        let tempdir = tempfile::tempdir().unwrap();
        let project_root = tempdir.path();
        let review_dir = project_root.join(".aida").join("review");
        std::fs::create_dir_all(&review_dir).unwrap();

        std::fs::write(review_dir.join("STORY-1.md"), "### STORY-1\n").unwrap();
        std::fs::write(review_dir.join("TASK-2.md"), "### TASK-2\n").unwrap();

        let output = assemble_review_md(project_root, None).unwrap();
        assert!(output.exists());
        let content = std::fs::read_to_string(output).unwrap();
        assert!(content.contains("STORY-1"));
        assert!(content.contains("TASK-2"));
        // Stable sort order: STORY-1 then TASK-2
        let story_pos = content.find("STORY-1").unwrap();
        let task_pos = content.find("TASK-2").unwrap();
        assert!(story_pos < task_pos);
    }
}
