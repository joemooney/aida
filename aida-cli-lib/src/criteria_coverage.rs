//! Capture-coverage report (STORY-1487): how well placed this project is to
//! be rebuilt from its store, as a product metric.
//!
//! Four figures, each as numerator / denominator, for a configurable window
//! (default 90 days) and for all time:
//!
//! - (a) share of commits whose subject ends in a `(SPEC-ID)` trailer;
//! - (b) share of AUTHORED work specs (machine-filed failure stubs and
//!   Review-PR records excluded) that carry parseable acceptance criteria;
//! - (c) share of those criteria with at least one traced test
//!   (`trace:<SPEC>.<criterion>`), plus the raw count of criterion-level
//!   trace tokens the SPIKE-86 scripts print;
//! - (d) the count of `trace:<SPEC>` comments in tracked source.
//!
//! The figures mirror `docs/positioning/spike-86-scripts/` (`report.py`
//! `coverage`, `corpus.py` machine/authored split and criteria parse) so a
//! run at the same pinned commit and window can be compared against them.
//! Reachable as `aida criteria coverage` (alias `aida criteria gap`); the
//! word `gap` is never read as a spec id.
//!
//! trace:STORY-1487 | ai:claude

use anyhow::Result;
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aida_core::models::RequirementType;
use aida_core::{Requirement, RequirementsStore};

/// Default lookback for the windowed figures.
pub(crate) const DEFAULT_WINDOW_DAYS: u64 = 90;

/// Source pathspecs the `trace:<SPEC>` comment count scans — the same set
/// `report.py` passes to `git grep`.
pub(crate) const TRACE_SOURCE_PATHSPECS: &[&str] = &["*.rs", "*.ts", "*.tsx", "*.py", "*.sh"];

/// What `aida criteria <ARG>` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CriteriaTarget {
    /// One spec's criterion-to-test report.
    Spec(String),
    /// The corpus-wide capture-coverage report.
    Coverage,
}

/// `gap` and `coverage` name the corpus-wide report; everything else is a
/// spec id. Pure, so "gap is never a spec id" is a unit test.
// trace:STORY-1487 | ai:claude
pub(crate) fn resolve_target(raw: &str) -> CriteriaTarget {
    let t = raw.trim();
    if t.eq_ignore_ascii_case("gap") || t.eq_ignore_ascii_case("coverage") {
        CriteriaTarget::Coverage
    } else {
        CriteriaTarget::Spec(t.to_string())
    }
}

/// One figure: numerator over denominator, never only a percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub(crate) struct Share {
    pub(crate) numerator: usize,
    pub(crate) denominator: usize,
}

impl Share {
    /// Percentage, `None` when the denominator is zero (reported as such,
    /// never as 0% or 100%).
    pub(crate) fn percent(&self) -> Option<f64> {
        (self.denominator > 0).then(|| 100.0 * self.numerator as f64 / self.denominator as f64)
    }

    fn render(&self) -> String {
        match self.percent() {
            Some(p) => format!("{}/{} = {:.1}%", self.numerator, self.denominator, p),
            None => format!(
                "{}/{} (nothing to measure)",
                self.numerator, self.denominator
            ),
        }
    }
}

/// The windowed figures. `window_days = None` is all time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct WindowFigures {
    pub(crate) label: String,
    pub(crate) window_days: Option<u64>,
    /// (a) commits (no merges) whose subject ends in a `(SPEC-ID)` trailer.
    pub(crate) commits_with_trailer: Share,
    /// (b) authored work specs created in the window with parseable criteria.
    pub(crate) specs_with_criteria: Share,
    /// (c) those specs' criteria with at least one traced test.
    pub(crate) criteria_with_traced_test: Share,
}

/// The whole report; `--json` emits exactly this.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct CoverageReport {
    pub(crate) generated_at: String,
    pub(crate) head: Option<String>,
    pub(crate) windows: Vec<WindowFigures>,
    /// (c-raw) criterion-level trace tokens in tracked files, and the
    /// distinct specs they name — the two numbers `report.py` prints.
    pub(crate) criterion_trace_tokens: usize,
    pub(crate) criterion_traced_specs: usize,
    /// (d) `trace:<SPEC>` comments in tracked source.
    pub(crate) trace_comments: usize,
    pub(crate) trace_comment_pathspecs: Vec<String>,
    /// How the token counts were gathered: `git grep` over tracked files, or
    /// a filesystem walk when the project is not a git checkout.
    pub(crate) token_source: String,
}

// --- (a) commit trailers -------------------------------------------------------

/// Does a commit subject end in a `(SPEC-ID …)` trailer (a trailing `(#N)`
/// PR group is skipped)? Shares the parser every other trailer surface uses.
// trace:STORY-1487 | ai:claude
pub(crate) fn subject_has_spec_trailer(subject: &str) -> bool {
    let mut ids = Vec::new();
    crate::push_paren_spec_ids_from_line(subject, &mut ids);
    !ids.is_empty()
}

/// Commit subjects (no merges) reachable from HEAD, optionally since a time.
fn commit_subjects(project_root: &Path, since: Option<DateTime<Utc>>) -> Vec<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C")
        .arg(project_root)
        .args(["log", "--no-merges", "--format=%s"]);
    if let Some(since) = since {
        cmd.arg(format!("--since={}", since.to_rfc3339()));
    }
    cmd.arg("HEAD");
    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

/// (a) over a subject list. Pure.
// trace:STORY-1487 | ai:claude
pub(crate) fn trailer_share(subjects: &[String]) -> Share {
    Share {
        numerator: subjects
            .iter()
            .filter(|s| subject_has_spec_trailer(s))
            .count(),
        denominator: subjects.len(),
    }
}

// --- (b) authored work specs with criteria -----------------------------------------

/// The requirement types that are work items (the SPIKE-86 corpus set):
/// stateless / organisational / documentation-layer types are not.
// trace:STORY-1487 | ai:claude
pub(crate) fn is_work_type(t: &RequirementType) -> bool {
    matches!(
        t,
        RequirementType::Functional
            | RequirementType::NonFunctional
            | RequirementType::ChangeRequest
            | RequirementType::Bug
            | RequirementType::Epic
            | RequirementType::Story
            | RequirementType::Task
            | RequirementType::Spike
    )
}

/// Machine-filed records: auto-complete failure stubs, Review-PR records,
/// and anything tagged or described as auto-drafted. Excluded from (b) so
/// the figure measures what people (and their agents) authored on purpose.
// trace:STORY-1487 | ai:claude
pub(crate) fn is_machine_filed(req: &Requirement) -> bool {
    let title = req.title.trim();
    let lower = title.to_ascii_lowercase();
    if lower.starts_with("auto-complete failure") {
        return true;
    }
    if let Some(rest) = title.strip_prefix("Review PR-") {
        if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    req.tags.iter().any(|t| t == "auto-drafted")
        || req
            .description
            .trim_start()
            .starts_with("Auto-drafted by `aida queue work")
}

/// An authored work spec: a work type that is not machine-filed.
pub(crate) fn is_authored_work_spec(req: &Requirement) -> bool {
    is_work_type(&req.req_type) && !is_machine_filed(req)
}

/// The spec's display id for criterion ids (`<SPEC>.<label>`).
fn display_id(req: &Requirement) -> Option<&str> {
    req.spec_id.as_deref().or(req.agreed_id.as_deref())
}

// --- (c)/(d) trace tokens ----------------------------------------------------------

/// Every `trace:` token (`SPEC-ID` or `SPEC-ID.<criterion>`) in tracked
/// files, as `git grep` sees them, plus how they were gathered. Falls back
/// to a filesystem walk of the test-file set when `git grep` is unavailable
/// (not a checkout, or git missing).
// trace:STORY-1487 | ai:claude
pub(crate) struct TraceTokens {
    /// Criterion-level tokens (`SPEC.crit`), upper-cased, with duplicates.
    pub(crate) criterion_tokens: Vec<String>,
    /// Count of `trace:<SPEC>` occurrences in the source pathspecs.
    pub(crate) trace_comments: usize,
    pub(crate) source: String,
}

fn git_grep_lines(project_root: &Path, pattern: &str, pathspecs: &[&str]) -> Option<Vec<String>> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C")
        .arg(project_root)
        .args(["grep", "-hoE", "-e", pattern, "--"]);
    for p in pathspecs {
        cmd.arg(p);
    }
    let out = cmd.output().ok()?;
    // Exit 1 = no matches, which is a valid empty answer; anything else is a
    // real failure (not a repo, git missing).
    match out.status.code() {
        Some(0) => Some(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
        ),
        Some(1) => Some(Vec::new()),
        _ => None,
    }
}

const CRITERION_TOKEN_PATTERN: &str = r"trace:[A-Z]+-[0-9-]+\.(ac|AC|a|A)[0-9A-Za-z_-]+";
const TRACE_COMMENT_PATTERN: &str = r"trace:[A-Z]+-[0-9][0-9-]*";

pub(crate) fn collect_trace_tokens(project_root: &Path) -> TraceTokens {
    let criterion = git_grep_lines(project_root, CRITERION_TOKEN_PATTERN, &[]);
    let comments = git_grep_lines(project_root, TRACE_COMMENT_PATTERN, TRACE_SOURCE_PATHSPECS);
    match (criterion, comments) {
        (Some(criterion), Some(comments)) => TraceTokens {
            criterion_tokens: criterion
                .iter()
                .filter_map(|l| crate::parse_trace_id_token(l))
                .filter(|id| id.contains('.'))
                .map(|id| id.to_ascii_uppercase())
                .collect(),
            trace_comments: comments.len(),
            source: "git grep over tracked files".to_string(),
        },
        _ => {
            let (criterion_tokens, trace_comments) = walk_trace_tokens(project_root);
            TraceTokens {
                criterion_tokens,
                trace_comments,
                source: "filesystem walk (not a git checkout)".to_string(),
            }
        }
    }
}

/// Filesystem fallback: walk source files (skipping the usual vendor / build
/// dirs) and pull every `trace:` token out of every line.
fn walk_trace_tokens(root: &Path) -> (Vec<String>, usize) {
    let mut files = Vec::new();
    walk_source_files(root, &mut files);
    let mut criterion = Vec::new();
    let mut comments = 0usize;
    for path in files {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in content.lines() {
            let mut rest = line;
            while let Some(pos) = rest.find("trace:") {
                let hit = &rest[pos..];
                let end = hit["trace:".len()..]
                    .find(char::is_whitespace)
                    .map(|i| i + "trace:".len())
                    .unwrap_or(hit.len());
                if let Some(id) = crate::parse_trace_id_token(&hit[..end]) {
                    comments += 1;
                    if id.contains('.') {
                        criterion.push(id.to_ascii_uppercase());
                    }
                }
                rest = &hit["trace:".len()..];
            }
        }
    }
    (criterion, comments)
}

fn walk_source_files(root: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if path.is_dir() {
            if matches!(
                name,
                ".git" | ".aida" | ".aida-store" | "target" | "node_modules" | "dist" | "build"
            ) {
                continue;
            }
            walk_source_files(&path, out);
        } else if path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|e| matches!(e, "rs" | "ts" | "tsx" | "py" | "sh"))
        {
            out.push(path);
        }
    }
}

// --- the report --------------------------------------------------------------------

/// Pure core over already-gathered inputs: commit subjects per window, the
/// store, the trace tokens. Everything the CLI prints comes from here, so a
/// fixture can pin the arithmetic without git.
// trace:STORY-1487 | ai:claude
pub(crate) fn coverage_from_parts(
    store: &RequirementsStore,
    now: DateTime<Utc>,
    window_days: u64,
    subjects_window: &[String],
    subjects_all: &[String],
    tokens: &TraceTokens,
    head: Option<String>,
) -> CoverageReport {
    let traced: BTreeSet<&str> = tokens.criterion_tokens.iter().map(|s| s.as_str()).collect();
    let since = now - chrono::Duration::days(window_days as i64);

    let figures = |label: &str, days: Option<u64>, subjects: &[String]| {
        let mut specs = Share::default();
        let mut criteria = Share::default();
        for req in store
            .requirements
            .iter()
            .filter(|r| is_authored_work_spec(r))
            .filter(|r| days.is_none() || r.created_at >= since)
        {
            let Some(id) = display_id(req) else {
                continue;
            };
            specs.denominator += 1;
            let parsed = crate::criteria::parse_acceptance_criteria(id, &req.description);
            if parsed.is_empty() {
                continue;
            }
            specs.numerator += 1;
            for c in parsed {
                criteria.denominator += 1;
                if traced.contains(c.id.to_ascii_uppercase().as_str()) {
                    criteria.numerator += 1;
                }
            }
        }
        WindowFigures {
            label: label.to_string(),
            window_days: days,
            commits_with_trailer: trailer_share(subjects),
            specs_with_criteria: specs,
            criteria_with_traced_test: criteria,
        }
    };

    let mut by_spec: BTreeMap<String, usize> = BTreeMap::new();
    for tok in &tokens.criterion_tokens {
        let spec = tok.split('.').next().unwrap_or(tok).to_string();
        *by_spec.entry(spec).or_default() += 1;
    }

    CoverageReport {
        generated_at: now.to_rfc3339(),
        head,
        windows: vec![
            figures(
                &format!("last {window_days} days"),
                Some(window_days),
                subjects_window,
            ),
            figures("all time", None, subjects_all),
        ],
        criterion_trace_tokens: tokens.criterion_tokens.len(),
        criterion_traced_specs: by_spec.len(),
        trace_comments: tokens.trace_comments,
        trace_comment_pathspecs: TRACE_SOURCE_PATHSPECS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        token_source: tokens.source.clone(),
    }
}

/// Gather from the project (git + store) and build the report.
// trace:STORY-1487 | ai:claude
pub(crate) fn build_coverage_report(
    project_root: &Path,
    store: &RequirementsStore,
    now: DateTime<Utc>,
    window_days: u64,
) -> CoverageReport {
    let since = now - chrono::Duration::days(window_days as i64);
    let subjects_window = commit_subjects(project_root, Some(since));
    let subjects_all = commit_subjects(project_root, None);
    let tokens = collect_trace_tokens(project_root);
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    coverage_from_parts(
        store,
        now,
        window_days,
        &subjects_window,
        &subjects_all,
        &tokens,
        head,
    )
}

/// `aida criteria coverage` / `aida criteria gap`.
// trace:STORY-1487 | ai:claude
pub(crate) fn handle_criteria_coverage(
    project_root: &Path,
    store: &RequirementsStore,
    window_days: u64,
    json: bool,
) -> Result<()> {
    let report = build_coverage_report(project_root, store, Utc::now(), window_days);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
    }
    Ok(())
}

fn print_human(report: &CoverageReport) {
    let head = report
        .head
        .as_deref()
        .map(|h| format!(" @ {h}"))
        .unwrap_or_default();
    println!("{}", format!("Capture coverage{head}").bold());
    println!(
        "  {}",
        "How well placed this project is to be rebuilt from its store: every figure is count/total."
            .dimmed()
    );
    for w in &report.windows {
        println!();
        println!("{}", w.label.bold());
        println!(
            "  (a) commits with a (SPEC-ID) trailer            {}",
            w.commits_with_trailer.render()
        );
        println!(
            "  (b) authored work specs with acceptance criteria {}",
            w.specs_with_criteria.render()
        );
        println!(
            "  (c) those criteria with a traced test            {}",
            w.criteria_with_traced_test.render()
        );
    }
    println!();
    println!("{}", "source".bold());
    println!(
        "  criterion-level trace tokens: {} across {} spec(s)",
        report.criterion_trace_tokens, report.criterion_traced_specs
    );
    println!(
        "  (d) trace:<SPEC> comments in tracked source ({}): {}",
        report.trace_comment_pathspecs.join(" "),
        report.trace_comments
    );
    println!("  gathered by {}", report.token_source.dimmed());
    if report.criterion_trace_tokens == 0 {
        println!(
            "  {}",
            "no criterion-level traces yet: put `trace:<SPEC>.<criterion>` above a test to start"
                .dimmed()
        );
    }
}

#[cfg(test)]
#[path = "tests/story_1487_capture_coverage_tests.rs"]
mod story_1487_capture_coverage_tests;
