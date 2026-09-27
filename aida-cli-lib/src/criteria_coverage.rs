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
use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Per-spec detail behind (b)/(c): every authored work spec (all time)
    /// with parseable criteria. Not part of the printed figures; written to
    /// the cache so later readers (the intent-capture floor) can slice by
    /// completion date without re-scanning the tree.
    #[serde(skip_serializing)]
    #[serde(default)]
    pub(crate) specs: Vec<SpecCoverage>,
}

/// One authored work spec with acceptance criteria, and how many of those
/// criteria have a traced test.
// trace:STORY-1487 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SpecCoverage {
    pub(crate) spec_id: String,
    pub(crate) status: String,
    pub(crate) created_at: DateTime<Utc>,
    /// Completion time for Completed/Done specs: the recorded completion
    /// stamp, else the last-modified time (the digest's rule). `None` while
    /// the spec is still open.
    pub(crate) completed_at: Option<DateTime<Utc>>,
    pub(crate) criteria: usize,
    pub(crate) traced_criteria: usize,
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
    if is_auto_drafted(&req.tags, &req.description) {
        return true;
    }
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
    false
}

/// Tagged `auto-drafted`, or described as drafted by `aida queue work`: the
/// one definition of an auto-drafted record (the draft inbox lens uses it
/// too).
// trace:STORY-1487 | ai:claude
pub(crate) fn is_auto_drafted<'a>(
    tags: impl IntoIterator<Item = &'a String>,
    description: &str,
) -> bool {
    tags.into_iter().any(|t| t == "auto-drafted")
        || description
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
    /// Criterion tokens attached to discovered tests (`aida criteria <ID>`).
    pub(crate) test_criterion_tokens: Vec<String>,
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
            test_criterion_tokens: Vec::new(),
            trace_comments: comments.len(),
            source: "git grep over tracked files".to_string(),
        },
        _ => {
            let (criterion_tokens, trace_comments) = walk_trace_tokens(project_root);
            TraceTokens {
                criterion_tokens,
                test_criterion_tokens: Vec::new(),
                trace_comments,
                source: "filesystem walk (not a git checkout)".to_string(),
            }
        }
    }
}

// trace:TASK-1546 | ai:codex
fn window_start(now: DateTime<Utc>, window_days: u64) -> DateTime<Utc> {
    i64::try_from(window_days)
        .ok()
        .and_then(chrono::Duration::try_days)
        .and_then(|duration| now.checked_sub_signed(duration))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
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
    let traced: BTreeSet<&str> = tokens
        .test_criterion_tokens
        .iter()
        .map(|s| s.as_str())
        .collect();
    let since = window_start(now, window_days);

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

    let specs: Vec<SpecCoverage> = store
        .requirements
        .iter()
        .filter(|r| is_authored_work_spec(r))
        .filter_map(|req| {
            let id = display_id(req)?;
            let parsed = crate::criteria::parse_acceptance_criteria(id, &req.description);
            if parsed.is_empty() {
                return None;
            }
            let traced_criteria = parsed
                .iter()
                .filter(|c| traced.contains(c.id.to_ascii_uppercase().as_str()))
                .count();
            let completed_at = matches!(
                req.status,
                aida_core::models::RequirementStatus::Completed
                    | aida_core::models::RequirementStatus::Done
            )
            .then(|| {
                req.implementation_info
                    .as_ref()
                    .and_then(|i| i.completed_at)
                    .unwrap_or(req.modified_at)
            });
            Some(SpecCoverage {
                spec_id: id.to_string(),
                status: format!("{:?}", req.status),
                created_at: req.created_at,
                completed_at,
                criteria: parsed.len(),
                traced_criteria,
            })
        })
        .collect();

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
        specs,
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
    let since = window_start(now, window_days);
    // trace:TASK-1546 | ai:codex
    // Git does not parse Chrono's minimum year. A window reaching that far
    // back is equivalent to all reachable commits.
    let subjects_window = commit_subjects(
        project_root,
        (since != DateTime::<Utc>::MIN_UTC).then_some(since),
    );
    let subjects_all = commit_subjects(project_root, None);
    let mut tokens = collect_trace_tokens(project_root);
    // trace:TASK-1546 | ai:codex
    // Reuse the per-spec test scanner so prose and unattached markers cannot
    // turn a criterion green. The raw git-grep count above stays comparable
    // with the SPIKE-86 script.
    let mut scanned_specs = BTreeSet::new();
    for req in store
        .requirements
        .iter()
        .filter(|r| is_authored_work_spec(r))
    {
        let Some(id) = display_id(req) else { continue };
        if !scanned_specs.insert(id.to_ascii_uppercase()) {
            continue;
        }
        if crate::criteria::parse_acceptance_criteria(id, &req.description).is_empty() {
            continue;
        }
        if let Ok(tests) = crate::criteria::scan_tests_for_criteria(project_root, id) {
            tokens
                .test_criterion_tokens
                .extend(tests.into_iter().flat_map(|test| {
                    test.traces
                        .into_iter()
                        .filter(|trace| trace.contains('.'))
                        .map(|trace| trace.to_ascii_uppercase())
                }));
        }
    }
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

/// `aida criteria <ARG>`: the project-wide report for `coverage` / `gap`,
/// one spec's criterion report otherwise. Both CLI dispatch paths call this.
// trace:STORY-1487 | ai:claude
pub(crate) fn dispatch_criteria(
    project_root: &Path,
    store: &RequirementsStore,
    raw: &str,
    window_days: u64,
    json: bool,
) -> Result<()> {
    match resolve_target(raw) {
        CriteriaTarget::Coverage => {
            handle_criteria_coverage(project_root, store, window_days, json)
        }
        CriteriaTarget::Spec(id) => {
            crate::criteria::handle_criteria_command(project_root, store, &id, json)
        }
    }
}

// --- cache (read by cheap surfaces such as `aida status`) ---------------------------

/// Where every report run writes its result, relative to the project root.
pub(crate) const CACHE_REL_PATH: &str = ".aida/cache/capture-coverage.json";
/// Bump when the cache shape changes; readers ignore other versions.
pub(crate) const CACHE_SCHEMA_VERSION: u32 = 1;

/// The cache file: the report plus its per-spec detail and a freshness
/// stamp (the full code HEAD the report was taken at).
// trace:STORY-1487 | ai:claude
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CoverageCache {
    pub(crate) schema_version: u32,
    pub(crate) head_full: Option<String>,
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) report: CoverageReport,
    pub(crate) specs: Vec<SpecCoverage>,
}

fn git_head_full(project_root: &Path) -> Option<String> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Write the cache under `.aida/cache/`. Only in an initialised project
/// (`.aida/` exists); failures are ignored — the report never depends on it.
// trace:STORY-1487 | ai:claude
pub(crate) fn write_coverage_cache(
    project_root: &Path,
    report: &CoverageReport,
    now: DateTime<Utc>,
) -> Option<std::path::PathBuf> {
    if !project_root.join(".aida").is_dir() {
        return None;
    }
    let cache = CoverageCache {
        schema_version: CACHE_SCHEMA_VERSION,
        head_full: git_head_full(project_root),
        generated_at: now,
        report: report.clone(),
        specs: report.specs.clone(),
    };
    let path = project_root.join(CACHE_REL_PATH);
    std::fs::create_dir_all(path.parent()?).ok()?;
    let body = serde_json::to_string_pretty(&cache).ok()?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    Some(path)
}

/// Read the cache when it is present, parseable, the current schema, taken
/// at the current code HEAD, and no older than `max_age`. `None` otherwise,
/// so a reader stays silent instead of reporting stale figures.
// trace:STORY-1487 | ai:claude
// Read by the status-line intent-capture indicator, which lands separately.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn load_fresh_coverage_cache(
    project_root: &Path,
    now: DateTime<Utc>,
    max_age: chrono::Duration,
) -> Option<CoverageCache> {
    let raw = std::fs::read_to_string(project_root.join(CACHE_REL_PATH)).ok()?;
    let mut cache: CoverageCache = serde_json::from_str(&raw).ok()?;
    if cache.schema_version != CACHE_SCHEMA_VERSION {
        return None;
    }
    if now - cache.generated_at > max_age || cache.generated_at > now {
        return None;
    }
    if cache.head_full != git_head_full(project_root) {
        return None;
    }
    cache.report.specs = cache.specs.clone();
    Some(cache)
}

/// `aida criteria coverage` / `aida criteria gap`.
// trace:STORY-1487 | ai:claude
pub(crate) fn handle_criteria_coverage(
    project_root: &Path,
    store: &RequirementsStore,
    window_days: u64,
    json: bool,
) -> Result<()> {
    let now = Utc::now();
    let report = build_coverage_report(project_root, store, now, window_days);
    let _ = write_coverage_cache(project_root, &report, now);
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
