//! `aida digest` — narrative work report in a time window. trace:STORY-252
//!
//! The advisor's curated story-of-progress: Released, Major progress (completed
//! specs), Strategic direction (new EPICs / foundational STORYs), Next iteration
//! (in-flight + queued), Process artifacts (memory entries). Editorial rules are
//! mechanical — drop typo / chore / style commits, collapse cluster-PRs to one
//! theme, keep only rejected specs that carry a supersedes / pivoted-from link,
//! strip SPEC-IDs in customer audience.
//!
//! Per-clone cadence marker at `.aida/last-digest.toml` lets bare `aida digest`
//! resume from the last window-end. Marker is covered by `.aida/*`
//! deny-by-default (trace:BUG-73) — no allow-line.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use aida_core::{
    RelationshipType, Requirement, RequirementStatus, RequirementType, RequirementsStore,
};

use crate::global_queue;

/// `--audience` enum. `self` is a Rust keyword, so the variant is `Slf` and
/// clap accepts it as `self` via `#[value(name = "self")]`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DigestAudience {
    Customer,
    Team,
    #[value(name = "self")]
    Slf,
    /// CLI-surface diff for the day-to-day operator / power-user: new commands,
    /// changed flags & behaviors, fixes-you'll-notice, new skills — value-framed,
    /// SPEC-IDs stripped, dev-noise dropped.
    // trace:STORY-541 | ai:claude — plain `//` keeps it out of `--help` (BUG-523).
    Operator,
}

impl DigestAudience {
    fn keep_spec_ids(self) -> bool {
        !matches!(self, DigestAudience::Customer | DigestAudience::Operator)
    }

    /// The operator lens replaces the work-narrative sections (Released / Major
    /// progress / …) with a capabilities-surface view. trace:STORY-541
    fn is_capabilities_lens(self) -> bool {
        matches!(self, DigestAudience::Operator)
    }
}

/// `--format` enum. `Brief` is a single-paragraph TL;DR.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DigestFormat {
    Markdown,
    Plain,
    Json,
    Brief,
}

/// Resolved options for one digest run. Times are UTC.
#[derive(Debug, Clone)]
pub struct DigestOptions {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub audience: DigestAudience,
    pub format: DigestFormat,
    pub include_next: bool,
    pub include_process: bool,
    pub out: Option<PathBuf>,
}

// ============================================================================
// `--since` parser — duration / ISO date / git ref / marker fallback.
// ============================================================================

/// Parse the `--since` argument into a UTC instant. Fallthrough:
///   1. `Nd|Nh|Nm` duration → now − duration
///   2. `YYYY-MM-DD` ISO date → that day at 00:00 UTC
///   3. any other token → resolved via `git log -1 --format=%cI <ref>`
///   4. absent → marker's `window_end`, else now − 24h
pub fn parse_digest_since(raw: Option<&str>, project_root: &Path) -> Result<DateTime<Utc>> {
    let now = Utc::now();
    let trimmed = raw.map(|s| s.trim()).unwrap_or("");
    if trimmed.is_empty() {
        return Ok(match DigestMarker::load(project_root) {
            Some(m) => m.window_end,
            None => now - chrono::Duration::hours(24),
        });
    }
    if let Ok(dur) = crate::parse_days_arg(trimmed) {
        return Ok(now - dur);
    }
    if let Some(t) = parse_iso_date(trimmed) {
        return Ok(t);
    }
    if let Some(t) = resolve_git_ref_date(project_root, trimmed) {
        return Ok(t);
    }
    anyhow::bail!(
        "--since {} is not a duration (e.g. 7d), an ISO date (YYYY-MM-DD), or a known git ref/tag",
        trimmed
    );
}

fn parse_iso_date(raw: &str) -> Option<DateTime<Utc>> {
    let nd = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    Utc.with_ymd_and_hms(nd.year(), nd.month(), nd.day(), 0, 0, 0)
        .single()
}

fn resolve_git_ref_date(project_root: &Path, refspec: &str) -> Option<DateTime<Utc>> {
    let out = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", "-1", "--format=%cI", refspec])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(&s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

// ============================================================================
// Released — git tags matching `v*` whose creation falls in the window.
// ============================================================================

#[derive(Debug, Clone)]
pub struct ReleaseTag {
    pub name: String,
    pub date: DateTime<Utc>,
    pub subject: Option<String>,
}

pub fn list_release_tags(
    project_root: &Path,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Vec<ReleaseTag> {
    let Ok(out) = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "for-each-ref",
            "--sort=creatordate",
            "--format=%(refname:short)|%(creatordate:iso-strict)",
            "refs/tags/v*",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut tags = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(2, '|');
        let name = parts.next().unwrap_or("").trim().to_string();
        let date_raw = parts.next().unwrap_or("").trim();
        if name.is_empty() || date_raw.is_empty() {
            continue;
        }
        let Ok(parsed) = DateTime::parse_from_rfc3339(date_raw) else {
            continue;
        };
        let date = parsed.with_timezone(&Utc);
        if date < since || date > until {
            continue;
        }
        tags.push(ReleaseTag {
            subject: tag_subject(project_root, &name),
            name,
            date,
        });
    }
    tags
}

fn tag_subject(project_root: &Path, tag: &str) -> Option<String> {
    let out = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", "-1", "--format=%s", tag])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

// ============================================================================
// Completed specs (Major progress).
// ============================================================================

#[derive(Debug, Clone)]
pub struct CompletedSpec {
    pub display_id: String,
    pub title: String,
    pub req_type: RequirementType,
    pub completed_at: DateTime<Utc>,
}

pub fn collect_completed(store: &RequirementsStore, opts: &DigestOptions) -> Vec<CompletedSpec> {
    let mut out = Vec::new();
    for req in &store.requirements {
        if !matches!(
            req.status,
            RequirementStatus::Completed | RequirementStatus::Done
        ) {
            continue;
        }
        // D7: prefer implementation_info.completed_at; fall back to modified_at
        // for specs that bumped before the STORY-86 auto-bump stamped it.
        let when = req
            .implementation_info
            .as_ref()
            .and_then(|i| i.completed_at)
            .unwrap_or(req.modified_at);
        if when < opts.since || when > opts.until {
            continue;
        }
        out.push(CompletedSpec {
            display_id: req.display_id(),
            title: req.title.clone(),
            req_type: req.req_type.clone(),
            completed_at: when,
        });
    }
    out.sort_by(|a, b| b.completed_at.cmp(&a.completed_at));
    out
}

// ============================================================================
// Commit scanning + cluster-PR collapse + noise filter.
// ============================================================================

#[derive(Debug, Clone)]
#[allow(dead_code)] // sha + date are part of the record; kept for future JSON / debug use.
pub struct CommitRec {
    pub sha: String,
    pub subject: String,
    pub date: DateTime<Utc>,
    pub spec_ids: Vec<String>,
    pub pr: Option<u64>,
}

pub fn scan_commits(
    project_root: &Path,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Vec<CommitRec> {
    // `--pretty=format` with NUL between fields and record separator so commit
    // bodies containing newlines do not confuse the parser.
    let since_arg = since.to_rfc3339();
    let until_arg = until.to_rfc3339();
    let Ok(out) = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "log",
            &format!("--since={}", since_arg),
            &format!("--until={}", until_arg),
            "--pretty=format:%H%x1f%cI%x1f%s%x1f%b%x1e",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let body = String::from_utf8_lossy(&out.stdout);
    let mut commits = Vec::new();
    for record in body.split('\u{1e}') {
        let rec = record.trim_matches('\n');
        if rec.is_empty() {
            continue;
        }
        let mut fields = rec.splitn(4, '\u{1f}');
        let sha = fields.next().unwrap_or("").trim().to_string();
        let date_raw = fields.next().unwrap_or("").trim();
        let subject = fields.next().unwrap_or("").to_string();
        let body_str = fields.next().unwrap_or("");
        if sha.is_empty() || subject.is_empty() {
            continue;
        }
        let Ok(parsed) = DateTime::parse_from_rfc3339(date_raw) else {
            continue;
        };
        let full = if body_str.is_empty() {
            subject.clone()
        } else {
            format!("{}\n\n{}", subject, body_str)
        };
        let mut spec_ids = crate::extract_spec_ids_from_commit(&full);
        for id in crate::extract_referenced_spec_ids_from_commit(&full) {
            if !spec_ids.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
                spec_ids.push(id);
            }
        }
        commits.push(CommitRec {
            sha,
            pr: crate::extract_pr_number_from_commit_subject(&subject),
            subject,
            date: parsed.with_timezone(&Utc),
            spec_ids,
        });
    }
    commits
}

#[derive(Debug, Clone)]
pub struct PrTheme {
    pub pr: u64,
    pub subject: String,
    pub spec_ids: Vec<String>,
}

/// Group commits sharing a `(#N)` PR number when ≥2 *distinct* spec IDs landed
/// in that PR — that's a cluster PR and renders as a single theme line. PRs
/// covering a lone spec are NOT collapsed; they fall through to the per-spec
/// line in Major progress.
pub fn collapse_cluster_prs(commits: &[CommitRec]) -> Vec<PrTheme> {
    let mut by_pr: BTreeMap<u64, (String, BTreeSet<String>)> = BTreeMap::new();
    for c in commits {
        let Some(pr) = c.pr else {
            continue;
        };
        let entry = by_pr
            .entry(pr)
            .or_insert_with(|| (c.subject.clone(), BTreeSet::new()));
        for id in &c.spec_ids {
            entry.1.insert(id.clone());
        }
    }
    by_pr
        .into_iter()
        .filter_map(|(pr, (subject, ids))| {
            if ids.len() >= 2 {
                Some(PrTheme {
                    pr,
                    subject,
                    spec_ids: ids.into_iter().collect(),
                })
            } else {
                None
            }
        })
        .collect()
}

/// Drop docs / style / chore / revert subjects and anything mentioning "typo".
/// These are the OUT list — not interesting in a customer-or-team digest.
pub fn is_noise_commit(subject: &str) -> bool {
    let lower = subject.trim().to_lowercase();
    for prefix in [
        "docs:", "docs(", "style:", "style(", "chore:", "chore(", "revert:", "revert(",
    ] {
        if lower.starts_with(prefix) {
            return true;
        }
    }
    lower.contains("typo")
}

// ============================================================================
// Capabilities lens (operator audience). trace:STORY-541
//
// The operator wants the CLI-SURFACE diff, not the work narrative: what changed
// that they can now DO + the command to try it. This is the Layer-1 deterministic
// candidate set — it classifies window commits into surface buckets and DROPS
// zero-impact dev work (clippy/lint/refactor/internal-MCP/test-only). The Layer-2
// LLM judgment (user-impact translation of a kept fix → value framing) lives in
// the `/aida-digest` skill, which reads this candidate set and rewrites it.
// ============================================================================

/// Which user-facing surface a commit touches. Drives the operator grouping.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CapabilitySurface {
    /// A brand-new command/subcommand a user can now run.
    NewCommand,
    /// A changed flag or behavior on an existing surface.
    ChangedBehavior,
    /// A fix a user will notice (crash, wrong output, broken flag).
    Fix,
    /// A new skill / slash command.
    NewSkill,
}

#[derive(Debug, Clone)]
pub struct CapabilityChange {
    // Retained on the record for JSON/debug + future per-surface rendering;
    // the bucketing match reads the local `surface`, not this field.
    #[allow(dead_code)]
    pub surface: CapabilitySurface,
    /// Value-framed line, SPEC-IDs already stripped. The Layer-2 skill may
    /// rewrite this; the deterministic default is the cleaned commit subject.
    pub value: String,
}

/// True when a commit is zero-user-impact dev work that the operator lens must
/// DROP: lint/clippy hygiene, internal refactors, internal-only plumbing,
/// test-only, plus the existing docs/style/chore/revert/typo noise. The cut is
/// "did it change what the user can do / experience" — NOT "feat vs chore".
/// trace:STORY-541
/// Strip a leading `[AI:tool] ` (or `[AI:tool1+tool2:conf] `) authorship prefix
/// so the conventional-commit type is at the start of the string for the noise /
/// classification checks. trace:STORY-541
fn strip_ai_prefix(subject: &str) -> &str {
    let t = subject.trim_start();
    if let Some(rest) = t.strip_prefix("[AI:") {
        if let Some(close) = rest.find(']') {
            return rest[close + 1..].trim_start();
        }
    }
    t
}

pub fn is_dev_only_commit(subject: &str) -> bool {
    let subject = strip_ai_prefix(subject);
    if is_noise_commit(subject) {
        return true;
    }
    let lower = subject.trim().to_lowercase();
    // Conventional-commit type prefixes that never change a user surface.
    for prefix in [
        "refactor:",
        "refactor(",
        "test:",
        "test(",
        "build:",
        "build(",
        "ci:",
        "ci(",
        "perf:",
        "perf(",
    ] {
        if lower.starts_with(prefix) {
            return true;
        }
    }
    // Substantive-noise keywords: clippy / lint hygiene, internal-only changes
    // that touch no user surface (the operator never sees them).
    const DEV_KEYWORDS: [&str; 4] = ["clippy", "lint", "warning", "rustfmt"];
    if DEV_KEYWORDS.iter().any(|k| lower.contains(k)) {
        return true;
    }
    // "internal" MCP/plumbing wording — only when paired with an internal cue,
    // so we don't drop a real feature that merely mentions "internal".
    if lower.contains("internal")
        && (lower.contains("refactor")
            || lower.contains("plumb")
            || lower.contains("rename")
            || lower.contains("cleanup"))
    {
        return true;
    }
    false
}

/// Classify a (non-dev-only) commit subject into its capability surface. Reads
/// the conventional-commit type + light keyword cues. Returns None when the
/// subject does not clearly map to a user-facing surface (it is then dropped —
/// the operator lens errs toward signal over completeness). trace:STORY-541
pub fn classify_capability(subject: &str) -> Option<CapabilitySurface> {
    let lower = strip_ai_prefix(subject).to_lowercase();
    let is_feat = lower.starts_with("feat:") || lower.starts_with("feat(");
    let is_fix = lower.starts_with("fix:") || lower.starts_with("fix(");
    let mentions_skill =
        lower.contains("skill") || lower.contains("slash command") || lower.contains("/aida-");

    // A `feat` that adds a skill / slash command lands in New skills (checked
    // before the generic feat branch). A `fix` that merely touches a skill is
    // still a Fix, not a new skill.
    if is_feat && mentions_skill {
        return Some(CapabilitySurface::NewSkill);
    }
    if is_fix {
        return Some(CapabilitySurface::Fix);
    }
    if is_feat {
        // A feat that adds a command/subcommand vs one that changes a flag.
        let adds_command = lower.contains("command")
            || lower.contains("subcommand")
            || lower.contains(" add ")
            || lower.contains("new ");
        if adds_command {
            return Some(CapabilitySurface::NewCommand);
        }
        return Some(CapabilitySurface::ChangedBehavior);
    }
    None
}

#[derive(Debug, Clone, Default)]
pub struct CapabilitiesReport {
    pub new_commands: Vec<CapabilityChange>,
    pub changed_behaviors: Vec<CapabilityChange>,
    pub fixes: Vec<CapabilityChange>,
    pub new_skills: Vec<CapabilityChange>,
}

impl CapabilitiesReport {
    pub fn is_empty(&self) -> bool {
        self.new_commands.is_empty()
            && self.changed_behaviors.is_empty()
            && self.fixes.is_empty()
            && self.new_skills.is_empty()
    }
}

/// Build the deterministic capabilities candidate set from the window's commits.
/// Dev-only commits are dropped; the survivors are value-cleaned (PR suffix +
/// SPEC-IDs stripped) and bucketed by surface. trace:STORY-541
pub fn collect_capabilities(commits: &[CommitRec]) -> CapabilitiesReport {
    let mut report = CapabilitiesReport::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for c in commits {
        if is_dev_only_commit(&c.subject) {
            continue;
        }
        let Some(surface) = classify_capability(&c.subject) else {
            continue;
        };
        // Value framing: drop the `[AI:tool]` authorship prefix, the trailing PR
        // suffix, and the SPEC-ID tokens so the operator sees the change, not the
        // dev-task breadcrumb.
        let value = strip_spec_ids(&strip_pr_suffix(strip_ai_prefix(&c.subject)));
        if value.is_empty() || !seen.insert(value.to_lowercase()) {
            continue;
        }
        let change = CapabilityChange { surface, value };
        match surface {
            CapabilitySurface::NewCommand => report.new_commands.push(change),
            CapabilitySurface::ChangedBehavior => report.changed_behaviors.push(change),
            CapabilitySurface::Fix => report.fixes.push(change),
            CapabilitySurface::NewSkill => report.new_skills.push(change),
        }
    }
    report
}

/// True when any spec completed in the window carries captured
/// `interface_changes` — i.e. the deterministic Layer-1 source (STORY-542) is
/// populated and the operator lens should read from it instead of re-inferring
/// from git commit subjects. trace:STORY-542
pub fn has_captured_interface_changes(store: &RequirementsStore, opts: &DigestOptions) -> bool {
    store.requirements.iter().any(|req| {
        if !matches!(
            req.status,
            RequirementStatus::Completed | RequirementStatus::Done
        ) {
            return false;
        }
        let when = req
            .implementation_info
            .as_ref()
            .and_then(|i| i.completed_at)
            .unwrap_or(req.modified_at);
        if when < opts.since || when > opts.until {
            return false;
        }
        req.interface_changes
            .as_ref()
            .is_some_and(|ic| !ic.is_empty())
    })
}

/// Build the operator capabilities candidate set from the DETERMINISTIC
/// `interface_changes` captured at spec close (STORY-542), rather than
/// re-inferring surface deltas from git commit subjects. Only specs completed
/// in the window with a non-empty `interface_changes` contribute; a spec with
/// no captured changes (or `Some(empty)` — decided "no impact") is silently
/// skipped, which is exactly the deterministic noise-filter the operator digest
/// wants.
///
/// `cli`/`tui`/`other` lines bucket by their heuristic surface (a "new
/// command" line vs a changed-behavior line vs a fix line); `mcp` lines bucket
/// as changed-behavior since the operator's MCP surface is API-shaped. The
/// Layer-2 `/aida-digest` skill still value-frames; this just yields the
/// candidate set deterministically. trace:STORY-542
pub fn collect_capabilities_from_store(
    store: &RequirementsStore,
    opts: &DigestOptions,
) -> CapabilitiesReport {
    let mut report = CapabilitiesReport::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    // Newest-completed first so the operator sees recent changes at the top.
    let mut specs: Vec<&Requirement> = store
        .requirements
        .iter()
        .filter(|req| {
            matches!(
                req.status,
                RequirementStatus::Completed | RequirementStatus::Done
            )
        })
        .filter(|req| {
            let when = req
                .implementation_info
                .as_ref()
                .and_then(|i| i.completed_at)
                .unwrap_or(req.modified_at);
            when >= opts.since && when <= opts.until
        })
        .filter(|req| {
            req.interface_changes
                .as_ref()
                .is_some_and(|ic| !ic.is_empty())
        })
        .collect();
    specs.sort_by(|a, b| {
        let aw = a
            .implementation_info
            .as_ref()
            .and_then(|i| i.completed_at)
            .unwrap_or(a.modified_at);
        let bw = b
            .implementation_info
            .as_ref()
            .and_then(|i| i.completed_at)
            .unwrap_or(b.modified_at);
        bw.cmp(&aw)
    });

    // Bucket a single captured surface-line. `dedupe` keeps a spec from
    // contributing the same line twice across surfaces.
    let mut push = |surface: CapabilitySurface, line: &str, report: &mut CapabilitiesReport| {
        let value = strip_spec_ids(line.trim());
        if value.is_empty() || !seen.insert(value.to_lowercase()) {
            return;
        }
        let change = CapabilityChange { surface, value };
        match surface {
            CapabilitySurface::NewCommand => report.new_commands.push(change),
            CapabilitySurface::ChangedBehavior => report.changed_behaviors.push(change),
            CapabilitySurface::Fix => report.fixes.push(change),
            CapabilitySurface::NewSkill => report.new_skills.push(change),
        }
    };

    for req in specs {
        let Some(ic) = req.interface_changes.as_ref() else {
            continue;
        };
        for line in &ic.cli {
            push(classify_interface_line(line), line, &mut report);
        }
        // MCP surface lines are API-shaped — bucket as changed-behavior unless
        // the line clearly reads as a fix / new tool.
        for line in &ic.mcp {
            push(classify_interface_line(line), line, &mut report);
        }
        for line in &ic.tui {
            push(classify_interface_line(line), line, &mut report);
        }
        for line in &ic.other {
            push(classify_interface_line(line), line, &mut report);
        }
    }
    report
}

/// Heuristically bucket a captured interface-change line (free-form prose
/// authored by the implementer) into a [`CapabilitySurface`]. Light keyword
/// cues only — the implementer already decided this line is user-facing, so we
/// never DROP, we only group. trace:STORY-542
pub fn classify_interface_line(line: &str) -> CapabilitySurface {
    let lower = line.to_lowercase();
    if lower.contains("skill") || lower.contains("slash command") || lower.contains("/aida-") {
        return CapabilitySurface::NewSkill;
    }
    if lower.contains("no longer")
        || lower.contains("fix")
        || lower.contains("crash")
        || lower.contains("panic")
        || lower.contains("broken")
    {
        return CapabilitySurface::Fix;
    }
    if lower.contains("new command")
        || lower.contains("new subcommand")
        || lower.contains("— new")
        || lower.contains("- new")
        || lower.contains("added")
        || lower.contains("new tool")
    {
        return CapabilitySurface::NewCommand;
    }
    CapabilitySurface::ChangedBehavior
}

// ============================================================================
// Strategic filings — EPICs (and STORYs tagged foundational/strategic) created
// in the window. Always SPEC-ID-aware (customer rendering strips later).
// ============================================================================

#[derive(Debug, Clone)]
pub struct StrategicFiling {
    pub display_id: String,
    pub title: String,
    pub req_type: RequirementType,
    pub created_at: DateTime<Utc>,
}

pub fn collect_strategic(store: &RequirementsStore, opts: &DigestOptions) -> Vec<StrategicFiling> {
    let mut out = Vec::new();
    for req in &store.requirements {
        if req.created_at < opts.since || req.created_at > opts.until {
            continue;
        }
        let qualifies = matches!(req.req_type, RequirementType::Epic)
            || (matches!(req.req_type, RequirementType::Story)
                && (req.tags.iter().any(|t| t == "foundational")
                    || req.tags.iter().any(|t| t == "strategic")));
        if !qualifies {
            continue;
        }
        out.push(StrategicFiling {
            display_id: req.display_id(),
            title: req.title.clone(),
            req_type: req.req_type.clone(),
            created_at: req.created_at,
        });
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out
}

// ============================================================================
// Rejected-with-supersedes pivots — file rejected specs only when the rejection
// led somewhere (a supersedes / pivoted-from link or tag).
// ============================================================================

#[derive(Debug, Clone)]
pub struct Pivot {
    pub rejected_id: String,
    pub rejected_title: String,
    pub successor_id: Option<String>,
    pub successor_title: Option<String>,
}

pub fn collect_rejected_pivots(store: &RequirementsStore, opts: &DigestOptions) -> Vec<Pivot> {
    let mut out = Vec::new();
    let lookup: HashMap<uuid::Uuid, &Requirement> =
        store.requirements.iter().map(|r| (r.id, r)).collect();
    for req in &store.requirements {
        if !matches!(req.status, RequirementStatus::Rejected) {
            continue;
        }
        if req.modified_at < opts.since || req.modified_at > opts.until {
            continue;
        }
        let mut successor: Option<(String, String)> = None;
        // (D8) Custom relationship whose label hints supersedes/pivot.
        for rel in &req.relationships {
            if let RelationshipType::Custom(name) = &rel.rel_type {
                let lower = name.to_lowercase();
                if lower.contains("supersed") || lower.contains("pivot") {
                    if let Some(target) = lookup.get(&rel.target_id) {
                        successor = Some((target.display_id(), target.title.clone()));
                        break;
                    }
                }
            }
        }
        // (D8) Tag-shaped fallback: `supersedes:STORY-244` / `pivoted-from:STORY-241`.
        if successor.is_none() {
            for tag in &req.tags {
                let lower = tag.to_lowercase();
                let Some(spec_part) = lower
                    .strip_prefix("supersedes:")
                    .or_else(|| lower.strip_prefix("pivoted-from:"))
                else {
                    continue;
                };
                let target_id = spec_part.trim().to_uppercase();
                if let Some(target) = store.requirements.iter().find(|r| r.matches_id(&target_id)) {
                    successor = Some((target.display_id(), target.title.clone()));
                    break;
                }
            }
        }
        let Some((sid, stitle)) = successor else {
            continue;
        };
        out.push(Pivot {
            rejected_id: req.display_id(),
            rejected_title: req.title.clone(),
            successor_id: Some(sid),
            successor_title: Some(stitle),
        });
    }
    out
}

// ============================================================================
// Next iteration — in-flight (Done not yet Completed) + queued items, grouped
// by `batch:` tag where applicable.
// ============================================================================

#[derive(Debug, Clone, Default)]
pub struct NextSection {
    pub in_flight: Vec<InFlightSpec>,
    pub queued_batches: BTreeMap<String, Vec<String>>,
    pub queued_loose: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct InFlightSpec {
    pub display_id: String,
    pub title: String,
}

pub fn collect_next(store: &RequirementsStore, _project_root: &Path) -> NextSection {
    let mut section = NextSection::default();
    for req in &store.requirements {
        if matches!(req.status, RequirementStatus::Done) {
            section.in_flight.push(InFlightSpec {
                display_id: req.display_id(),
                title: req.title.clone(),
            });
        }
    }
    // Queued items: walk the implementer role's global queue (the primary
    // doer-role surface). Other roles can be added when their queues matter.
    if let Ok(entries) = global_queue::load("implementer") {
        for entry in entries {
            let label = entry
                .agreed_id
                .clone()
                .or(entry.spec_id.clone())
                .unwrap_or_else(|| format!("(uuid {})", entry.requirement_id));
            // Resolve the spec in the local store so we can read its tags and
            // pull a `batch:` membership.
            let req = store
                .requirements
                .iter()
                .find(|r| r.id == entry.requirement_id);
            let batch_tag = req.and_then(|r| {
                r.tags
                    .iter()
                    .find(|t| t.starts_with("batch:"))
                    .and_then(|t| t.strip_prefix("batch:"))
                    .map(|s| s.to_string())
            });
            match batch_tag {
                Some(name) => section.queued_batches.entry(name).or_default().push(label),
                None => section.queued_loose.push(label),
            }
        }
    }
    section
}

// ============================================================================
// Notable plans — `docs/plans/YYYY-MM-DD-<slug>.md` files dated in the window.
// ============================================================================

#[derive(Debug, Clone)]
#[allow(dead_code)] // path retained for future "open this plan" UX.
pub struct PlanRef {
    pub date: NaiveDate,
    pub slug: String,
    pub path: PathBuf,
}

pub fn collect_plans(project_root: &Path, opts: &DigestOptions) -> Vec<PlanRef> {
    let plans_dir = project_root.join("docs").join("plans");
    let Ok(entries) = std::fs::read_dir(&plans_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let since_date = opts.since.date_naive();
    let until_date = opts.until.date_naive();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.ends_with(".md") || name.starts_with('_') {
            continue;
        }
        // Filename shape: YYYY-MM-DD-<slug>.md
        if name.len() < 11 {
            continue;
        }
        let date_part = &name[..10];
        let Ok(date) = NaiveDate::parse_from_str(date_part, "%Y-%m-%d") else {
            continue;
        };
        if date < since_date || date > until_date {
            continue;
        }
        let slug = name[11..name.len() - 3].to_string();
        out.push(PlanRef {
            date,
            slug,
            path: path.clone(),
        });
    }
    out.sort_by(|a, b| b.date.cmp(&a.date));
    out
}

// ============================================================================
// Process artifacts (memory entries). Best-effort. Read MEMORY.md for the
// project, parse one-line index entries `- [title](file.md) — hook`. Files
// carrying `audience: public` in frontmatter are eligible for the customer
// audience too. Missing MEMORY.md → silently skipped (D9).
// ============================================================================

#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub title: String,
    pub hook: String,
    pub public: bool,
}

pub fn collect_process(project_root: &Path, opts: &DigestOptions) -> Vec<MemoryEntry> {
    let Some(dir) = memory_dir_for(project_root) else {
        return Vec::new();
    };
    let index = dir.join("MEMORY.md");
    let Ok(content) = std::fs::read_to_string(&index) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        let rest = match line.strip_prefix("- ") {
            Some(r) => r,
            None => continue,
        };
        let Some((title, after_title)) = split_md_link(rest) else {
            continue;
        };
        let file = match extract_md_link_target(after_title) {
            Some(f) => f,
            None => continue,
        };
        let hook = after_title
            .split_once("—")
            .map(|x| x.1)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let file_path = dir.join(&file);
        let public = is_public_memory(&file_path).unwrap_or(false);
        if matches!(opts.audience, DigestAudience::Customer) && !public {
            continue;
        }
        out.push(MemoryEntry {
            title,
            hook,
            public,
        });
    }
    out
}

fn memory_dir_for(project_root: &Path) -> Option<PathBuf> {
    let slug = global_queue::project_name_for(project_root);
    // Claude Code mirrors the project root: `-home-joe-ai-aida` style. We try
    // both the slug-from-path mirror and the basename — whichever exists.
    let home = dirs::home_dir()?;
    let base = home.join(".claude").join("projects");
    let mirror = project_root
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "-");
    let candidates = [
        base.join(format!("-{}", mirror.trim_start_matches('-'))),
        base.join(&slug),
    ];
    for c in &candidates {
        let mem = c.join("memory");
        if mem.is_dir() {
            return Some(mem);
        }
    }
    None
}

fn split_md_link(line: &str) -> Option<(String, &str)> {
    let open = line.find('[')?;
    let close = line[open + 1..].find(']')?;
    let title = line[open + 1..open + 1 + close].to_string();
    let rest = &line[open + 1 + close + 1..];
    Some((title, rest))
}

fn extract_md_link_target(line: &str) -> Option<String> {
    let open = line.find('(')?;
    let close = line[open + 1..].find(')')?;
    Some(line[open + 1..open + 1 + close].to_string())
}

fn is_public_memory(file_path: &Path) -> Option<bool> {
    let content = std::fs::read_to_string(file_path).ok()?;
    let (fm, _) = crate::split_md_frontmatter(&content)?;
    Some(crate::frontmatter_field(fm, "audience") == Some("public"))
}

// ============================================================================
// Cadence marker — `.aida/last-digest.toml`. Per-clone runtime state covered by
// the deny-by-default `.aida/*` gitignore block (D10, trace:BUG-73).
// ============================================================================

const MARKER_FILE: &str = "last-digest.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DigestMarker {
    pub window_end: DateTime<Utc>,
    pub written_at: DateTime<Utc>,
}

fn marker_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(MARKER_FILE)
}

impl DigestMarker {
    pub fn load(project_root: &Path) -> Option<Self> {
        let body = std::fs::read_to_string(marker_path(project_root)).ok()?;
        toml::from_str(&body).ok()
    }

    pub fn write(&self, project_root: &Path) -> Result<()> {
        let path = marker_path(project_root);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("Failed to create {} for digest marker", dir.display()))?;
        }
        let body = toml::to_string_pretty(self).context("serialize digest marker")?;
        aida_core::write_atomic(&path, body)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    pub fn clear(project_root: &Path) -> Result<()> {
        let path = marker_path(project_root);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("Failed to remove {}", path.display())),
        }
    }
}

// ============================================================================
// Assembled report + renderers.
// ============================================================================

#[derive(Debug, Clone)]
pub struct DigestReport {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub releases: Vec<ReleaseTag>,
    pub completed: Vec<CompletedSpec>,
    pub cluster_prs: Vec<PrTheme>,
    pub strategic: Vec<StrategicFiling>,
    pub pivots: Vec<Pivot>,
    pub next: NextSection,
    pub plans: Vec<PlanRef>,
    pub process: Vec<MemoryEntry>,
    /// Operator-audience capabilities lens (CLI-surface diff). trace:STORY-541
    pub capabilities: CapabilitiesReport,
}

/// Render the operator (capabilities) lens: grouped by surface, value-framed,
/// SPEC-IDs stripped, including any releases in the window. trace:STORY-541
fn render_capabilities_markdown(report: &DigestReport, _opts: &DigestOptions) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# AIDA — what changed for you {} → {}\n\n",
        report.since.format("%Y-%m-%d"),
        report.until.format("%Y-%m-%d")
    ));

    if !report.releases.is_empty() {
        s.push_str("## Released\n\n");
        for r in &report.releases {
            let subject = r.subject.as_deref().unwrap_or("");
            let subject = strip_spec_ids(subject);
            s.push_str(&format!("- **{}** ({})", r.name, r.date.format("%Y-%m-%d")));
            if !subject.is_empty() && subject != r.name {
                s.push_str(&format!(" — {}", subject));
            }
            s.push('\n');
        }
        s.push('\n');
    }

    let cap = &report.capabilities;
    let section = |s: &mut String, heading: &str, items: &[CapabilityChange]| {
        if items.is_empty() {
            return;
        }
        s.push_str(&format!("## {}\n\n", heading));
        for c in items {
            s.push_str(&format!("- {}\n", c.value));
        }
        s.push('\n');
    };
    section(&mut s, "New commands", &cap.new_commands);
    section(&mut s, "Changed flags & behaviors", &cap.changed_behaviors);
    section(&mut s, "Fixes you'll notice", &cap.fixes);
    section(&mut s, "New skills", &cap.new_skills);

    if cap.is_empty() && report.releases.is_empty() {
        s.push_str("_No user-facing changes in this window._\n");
    }
    s
}

pub fn render_markdown(report: &DigestReport, opts: &DigestOptions) -> String {
    // The operator lens is a capabilities-surface view, not a work narrative.
    // trace:STORY-541
    if opts.audience.is_capabilities_lens() {
        return render_capabilities_markdown(report, opts);
    }

    let mut s = String::new();
    s.push_str(&format!(
        "# AIDA — work digest {} → {}\n\n",
        report.since.format("%Y-%m-%d"),
        report.until.format("%Y-%m-%d")
    ));

    if !report.releases.is_empty() {
        s.push_str("## Released\n\n");
        for r in &report.releases {
            let subject = r.subject.as_deref().unwrap_or("");
            s.push_str(&format!("- **{}** ({})", r.name, r.date.format("%Y-%m-%d")));
            if !subject.is_empty() && subject != r.name {
                s.push_str(&format!(" — {}", subject));
            }
            s.push('\n');
        }
        s.push('\n');
    }

    let cluster_specs: BTreeSet<String> = report
        .cluster_prs
        .iter()
        .flat_map(|p| p.spec_ids.iter().cloned())
        .collect();

    if !report.completed.is_empty() || !report.cluster_prs.is_empty() {
        s.push_str("## Major progress\n\n");
        for theme in &report.cluster_prs {
            let subject = strip_pr_suffix(&theme.subject);
            let subject = if opts.audience.keep_spec_ids() {
                subject
            } else {
                strip_spec_ids(&subject)
            };
            s.push_str(&format!("- **PR #{}** — {}", theme.pr, subject));
            if opts.audience.keep_spec_ids() {
                s.push_str(&format!(" _({} specs)_", theme.spec_ids.len()));
            }
            s.push('\n');
        }
        for c in &report.completed {
            if opts.audience == DigestAudience::Customer && cluster_specs.contains(&c.display_id) {
                continue; // member of a cluster, surfaced as the theme line
            }
            let title = if opts.audience.keep_spec_ids() {
                c.title.clone()
            } else {
                strip_spec_ids(&c.title)
            };
            let line = if opts.audience.keep_spec_ids() {
                format!("- {}: {}\n", c.display_id, title)
            } else {
                format!("- {}\n", title)
            };
            s.push_str(&line);
        }
        s.push('\n');
    }

    if !report.strategic.is_empty() || !report.pivots.is_empty() {
        s.push_str("## Strategic direction\n\n");
        for f in &report.strategic {
            let line = if opts.audience.keep_spec_ids() {
                format!("- New: **{}** — {}\n", f.display_id, f.title)
            } else {
                format!("- New initiative: {}\n", f.title)
            };
            s.push_str(&line);
        }
        for p in &report.pivots {
            let (sid, stitle) = (
                p.successor_id.as_deref().unwrap_or("?"),
                p.successor_title.as_deref().unwrap_or(""),
            );
            let line = if opts.audience.keep_spec_ids() {
                format!("- Pivot: {} → **{}** ({})\n", p.rejected_id, sid, stitle)
            } else {
                format!("- Pivot: {}\n", stitle)
            };
            s.push_str(&line);
        }
        s.push('\n');
    }

    if opts.include_next
        && (!report.next.in_flight.is_empty()
            || !report.next.queued_batches.is_empty()
            || !report.next.queued_loose.is_empty())
    {
        s.push_str("## Next iteration\n\n");
        if !report.next.in_flight.is_empty() {
            s.push_str("In flight (Done, awaiting merge):\n");
            for ifs in &report.next.in_flight {
                let line = if opts.audience.keep_spec_ids() {
                    format!("- {}: {}\n", ifs.display_id, ifs.title)
                } else {
                    format!("- {}\n", ifs.title)
                };
                s.push_str(&line);
            }
            s.push('\n');
        }
        if !report.next.queued_batches.is_empty() || !report.next.queued_loose.is_empty() {
            s.push_str("Queued for the implementer:\n");
            for (name, ids) in &report.next.queued_batches {
                if opts.audience.keep_spec_ids() {
                    s.push_str(&format!(
                        "- batch:{} ({} items): {}\n",
                        name,
                        ids.len(),
                        ids.join(", ")
                    ));
                } else {
                    s.push_str(&format!("- {} ({} items)\n", name, ids.len()));
                }
            }
            if !report.next.queued_loose.is_empty() {
                if opts.audience.keep_spec_ids() {
                    s.push_str(&format!(
                        "- Loose: {}\n",
                        report.next.queued_loose.join(", ")
                    ));
                } else {
                    s.push_str(&format!(
                        "- {} more queued item(s)\n",
                        report.next.queued_loose.len()
                    ));
                }
            }
            s.push('\n');
        }
    }

    if !report.plans.is_empty() && opts.audience.keep_spec_ids() {
        s.push_str("## Notable plans\n\n");
        for p in &report.plans {
            s.push_str(&format!("- {} — {}\n", p.date, p.slug));
        }
        s.push('\n');
    }

    if opts.include_process && !report.process.is_empty() {
        s.push_str("## Process artifacts\n\n");
        for m in &report.process {
            if !m.hook.is_empty() {
                s.push_str(&format!("- **{}** — {}\n", m.title, m.hook));
            } else {
                s.push_str(&format!("- {}\n", m.title));
            }
        }
        s.push('\n');
    }

    s
}

/// Strip every `<PREFIX>-<digits>` SPEC-ID token (and any trailing parens that
/// hold *only* SPEC-IDs / `PR-N` tokens) from a string. Used in customer
/// audience to keep titles ID-free.
fn strip_spec_ids(s: &str) -> String {
    use std::sync::OnceLock;
    // Matches `[trace:]?PREFIX-DIGITS` where PREFIX is 2-6 uppercase letters.
    // Also catches `PR-123`.
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"(?:\btrace:)?\b[A-Z]{2,6}-\d+\b").expect("valid digest spec-id regex")
    });

    // First: drop trailing parenthesized lists that hold only spec-ids /
    // `#N` tokens (e.g. " (BUG-85 BUG-87 BUG-88)" or " (#42)").
    let mut t = s.trim().to_string();
    loop {
        let trimmed = t.trim_end();
        if !trimmed.ends_with(')') {
            t = trimmed.to_string();
            break;
        }
        let Some(open) = trimmed.rfind('(') else {
            break;
        };
        let inner = &trimmed[open + 1..trimmed.len() - 1];
        let all_id_like = inner
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|tok| !tok.is_empty())
            .all(|tok| {
                let tok = tok.trim_start_matches('#');
                re.is_match(tok) || tok.chars().all(|c| c.is_ascii_digit())
            });
        if all_id_like {
            t = trimmed[..open].trim_end().to_string();
        } else {
            t = trimmed.to_string();
            break;
        }
    }

    // Then: nuke any remaining `[trace:]?PREFIX-DIGITS` tokens inline.
    let stripped = re.replace_all(&t, "").to_string();

    // Cleanup: collapse "  " → " ", drop dangling colons/commas, prune empty
    // `()` / `[]` brackets left by removed contents, and trim repeated punct.
    let mut out = stripped;
    for _ in 0..3 {
        out = out.replace("()", "").replace("(  )", "").replace("[]", "");
        out = out.replace(" /", "").replace("//", "/");
        let collapsed: String = out
            .chars()
            .fold(String::with_capacity(out.len()), |mut acc, c| {
                if c == ' ' && acc.ends_with(' ') {
                    return acc;
                }
                acc.push(c);
                acc
            });
        out = collapsed;
    }
    out.trim()
        .trim_start_matches([':', ',', ';', '.'])
        .trim_end_matches([':', ',', ';', '.'])
        .trim()
        .replace("— —", "—")
        .trim()
        .to_string()
}

fn strip_pr_suffix(subject: &str) -> String {
    // "(#42)" trailing — drop it for the theme line.
    let t = subject.trim();
    if let Some(open) = t.rfind('(') {
        if t.ends_with(')')
            && t[open + 1..t.len() - 1]
                .strip_prefix('#')
                .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
        {
            return t[..open].trim_end().to_string();
        }
    }
    t.to_string()
}

/// Strip markdown emphasis / headings — used by the plain renderer.
fn render_plain(report: &DigestReport, opts: &DigestOptions) -> String {
    let md = render_markdown(report, opts);
    let mut out = String::with_capacity(md.len());
    for line in md.lines() {
        let mut s = line.to_string();
        // strip leading `# `, `## `, `### `
        for prefix in ["#### ", "### ", "## ", "# "] {
            if let Some(rest) = s.strip_prefix(prefix) {
                s = rest.to_string();
                break;
            }
        }
        // strip ** bold markers
        let s = s.replace("**", "").replace("_(", "(").replace(")_", ")");
        out.push_str(&s);
        out.push('\n');
    }
    out
}

fn render_brief(report: &DigestReport, opts: &DigestOptions) -> String {
    let releases: Vec<String> = report.releases.iter().map(|r| r.name.clone()).collect();
    let major = report.completed.len();
    let strategic = report.strategic.len();
    let next_total = report.next.in_flight.len()
        + report
            .next
            .queued_batches
            .values()
            .map(|v| v.len())
            .sum::<usize>()
        + report.next.queued_loose.len();
    let window = format!(
        "{} → {}",
        report.since.format("%Y-%m-%d"),
        report.until.format("%Y-%m-%d")
    );
    let mut parts: Vec<String> = Vec::new();
    if !releases.is_empty() {
        parts.push(format!("Released {}", releases.join(", ")));
    }
    if major > 0 {
        parts.push(format!("{} spec(s) completed", major));
    }
    if strategic > 0 {
        parts.push(format!("{} strategic filing(s)", strategic));
    }
    if opts.include_next && next_total > 0 {
        parts.push(format!("{} item(s) queued or in flight", next_total));
    }
    if parts.is_empty() {
        return format!("AIDA digest {}: no notable activity.\n", window);
    }
    format!("AIDA digest {}: {}.\n", window, parts.join("; "))
}

fn render_json(report: &DigestReport, opts: &DigestOptions) -> Result<String> {
    #[derive(Serialize)]
    struct DigestJson<'a> {
        since: String,
        until: String,
        audience: DigestAudience,
        releases: Vec<ReleaseJson<'a>>,
        completed: Vec<CompletedJson<'a>>,
        cluster_prs: Vec<&'a PrTheme>,
        strategic: Vec<StrategicJson<'a>>,
        pivots: Vec<&'a Pivot>,
        next_in_flight: Vec<&'a InFlightSpec>,
        next_queued_batches: &'a BTreeMap<String, Vec<String>>,
        next_queued_loose: &'a Vec<String>,
        plans: Vec<PlanJson<'a>>,
        process: Vec<&'a MemoryEntry>,
        /// Operator-lens candidate set (Layer-1 facts the skill renders from).
        #[serde(skip_serializing_if = "Option::is_none")]
        capabilities: Option<CapabilitiesJson<'a>>,
    }
    #[derive(Serialize)]
    struct CapabilitiesJson<'a> {
        new_commands: Vec<&'a str>,
        changed_behaviors: Vec<&'a str>,
        fixes: Vec<&'a str>,
        new_skills: Vec<&'a str>,
    }
    #[derive(Serialize)]
    struct ReleaseJson<'a> {
        name: &'a str,
        date: String,
        subject: Option<&'a str>,
    }
    #[derive(Serialize)]
    struct CompletedJson<'a> {
        id: &'a str,
        title: &'a str,
        req_type: String,
        completed_at: String,
    }
    #[derive(Serialize)]
    struct StrategicJson<'a> {
        id: &'a str,
        title: &'a str,
        req_type: String,
        created_at: String,
    }
    #[derive(Serialize)]
    struct PlanJson<'a> {
        date: String,
        slug: &'a str,
    }

    let cluster_prs: Vec<&PrTheme> = report.cluster_prs.iter().collect();
    let pivots: Vec<&Pivot> = report.pivots.iter().collect();
    let in_flight: Vec<&InFlightSpec> = report.next.in_flight.iter().collect();
    let process: Vec<&MemoryEntry> = report.process.iter().collect();
    let capabilities = if opts.audience.is_capabilities_lens() {
        let cap = &report.capabilities;
        fn values(items: &[CapabilityChange]) -> Vec<&str> {
            items.iter().map(|c| c.value.as_str()).collect()
        }
        Some(CapabilitiesJson {
            new_commands: values(&cap.new_commands),
            changed_behaviors: values(&cap.changed_behaviors),
            fixes: values(&cap.fixes),
            new_skills: values(&cap.new_skills),
        })
    } else {
        None
    };

    let doc = DigestJson {
        since: report.since.to_rfc3339(),
        until: report.until.to_rfc3339(),
        audience: opts.audience,
        releases: report
            .releases
            .iter()
            .map(|r| ReleaseJson {
                name: &r.name,
                date: r.date.to_rfc3339(),
                subject: r.subject.as_deref(),
            })
            .collect(),
        completed: report
            .completed
            .iter()
            .map(|c| CompletedJson {
                id: &c.display_id,
                title: &c.title,
                req_type: c.req_type.to_string(),
                completed_at: c.completed_at.to_rfc3339(),
            })
            .collect(),
        cluster_prs,
        strategic: report
            .strategic
            .iter()
            .map(|s| StrategicJson {
                id: &s.display_id,
                title: &s.title,
                req_type: s.req_type.to_string(),
                created_at: s.created_at.to_rfc3339(),
            })
            .collect(),
        pivots,
        next_in_flight: in_flight,
        next_queued_batches: &report.next.queued_batches,
        next_queued_loose: &report.next.queued_loose,
        plans: report
            .plans
            .iter()
            .map(|p| PlanJson {
                date: p.date.format("%Y-%m-%d").to_string(),
                slug: &p.slug,
            })
            .collect(),
        process,
        capabilities,
    };
    serde_json::to_string_pretty(&doc).context("serialize digest as JSON")
}

// Internal Serialize for shapes only used by render_json (the cluster_prs /
// pivot / in_flight / memory pass-throughs).
mod json_shapes {
    use super::*;
    impl Serialize for PrTheme {
        fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
            use serde::ser::SerializeStruct;
            let mut st = s.serialize_struct("PrTheme", 3)?;
            st.serialize_field("pr", &self.pr)?;
            st.serialize_field("subject", &self.subject)?;
            st.serialize_field("spec_ids", &self.spec_ids)?;
            st.end()
        }
    }
    impl Serialize for Pivot {
        fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
            use serde::ser::SerializeStruct;
            let mut st = s.serialize_struct("Pivot", 4)?;
            st.serialize_field("rejected_id", &self.rejected_id)?;
            st.serialize_field("rejected_title", &self.rejected_title)?;
            st.serialize_field("successor_id", &self.successor_id)?;
            st.serialize_field("successor_title", &self.successor_title)?;
            st.end()
        }
    }
    impl Serialize for InFlightSpec {
        fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
            use serde::ser::SerializeStruct;
            let mut st = s.serialize_struct("InFlight", 2)?;
            st.serialize_field("id", &self.display_id)?;
            st.serialize_field("title", &self.title)?;
            st.end()
        }
    }
    impl Serialize for MemoryEntry {
        fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
            use serde::ser::SerializeStruct;
            let mut st = s.serialize_struct("MemoryEntry", 3)?;
            st.serialize_field("title", &self.title)?;
            st.serialize_field("hook", &self.hook)?;
            st.serialize_field("public", &self.public)?;
            st.end()
        }
    }
}

// ============================================================================
// Orchestrator.
// ============================================================================

/// Render the digest to a string without writing it anywhere. Used by
/// the --copy flow in main.rs which needs the text content for the
/// clipboard. Also used by `run` (which writes the text to stdout/file).
/// Does NOT touch the cadence marker. trace:TASK-381 | ai:claude
pub fn render_string(
    opts: &DigestOptions,
    project_root: &Path,
    store: &RequirementsStore,
) -> Result<String> {
    let releases = list_release_tags(project_root, opts.since, opts.until);
    let completed = collect_completed(store, opts);
    let commits = scan_commits(project_root, opts.since, opts.until);
    // The capabilities lens (operator) prefers the DETERMINISTIC source: when
    // any in-window completed spec carries captured `interface_changes`
    // (STORY-542), read the candidate set from those rather than re-inferring
    // surface deltas from git commit subjects (STORY-541's fallback). Specs
    // with no captured changes are deterministically absent — no clippy/internal
    // noise to filter. Falls back to the commit-classifier when nothing has
    // been captured yet (pre-STORY-542 history). trace:STORY-542 STORY-541
    let capabilities = if opts.audience.is_capabilities_lens() {
        if has_captured_interface_changes(store, opts) {
            collect_capabilities_from_store(store, opts)
        } else {
            collect_capabilities(&commits)
        }
    } else {
        CapabilitiesReport::default()
    };
    let interesting: Vec<CommitRec> = commits
        .into_iter()
        .filter(|c| !is_noise_commit(&c.subject))
        .collect();
    let cluster_prs = collapse_cluster_prs(&interesting);
    let strategic = collect_strategic(store, opts);
    let pivots = collect_rejected_pivots(store, opts);
    let next = if opts.include_next {
        collect_next(store, project_root)
    } else {
        NextSection::default()
    };
    let plans = collect_plans(project_root, opts);
    let process = if opts.include_process {
        collect_process(project_root, opts)
    } else {
        Vec::new()
    };

    let report = DigestReport {
        since: opts.since,
        until: opts.until,
        releases,
        completed,
        cluster_prs,
        strategic,
        pivots,
        next,
        plans,
        process,
        capabilities,
    };

    Ok(match opts.format {
        DigestFormat::Markdown => render_markdown(&report, opts),
        DigestFormat::Plain => render_plain(&report, opts),
        DigestFormat::Brief => render_brief(&report, opts),
        DigestFormat::Json => render_json(&report, opts)?,
    })
}

pub fn run(
    opts: DigestOptions,
    project_root: &Path,
    store: &RequirementsStore,
    reset: bool,
) -> Result<()> {
    if reset {
        DigestMarker::clear(project_root)?;
        eprintln!("Cleared digest marker.");
        return Ok(());
    }

    let text = render_string(&opts, project_root, store)?;

    match &opts.out {
        Some(path) => {
            aida_core::write_atomic(path, &text)
                .with_context(|| format!("Failed to write {}", path.display()))?;
            eprintln!("Wrote digest to {}", path.display());
        }
        None => {
            print!("{}", text);
            if !text.ends_with('\n') {
                println!();
            }
        }
    }

    // Always update the marker (D6 / D10 / Phase 3): even when piping JSON, the
    // user's intent was "I read up through `until`." `--reset` early-returns
    // above, so it never lands here.
    let marker = DigestMarker {
        window_end: opts.until,
        written_at: Utc::now(),
    };
    if let Err(e) = marker.write(project_root) {
        eprintln!("Warning: could not update digest marker: {}", e);
    }

    Ok(())
}

// ============================================================================
// Tests.
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as ProcessCommand;
    use tempfile::TempDir;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    fn opts_at(since: DateTime<Utc>, until: DateTime<Utc>) -> DigestOptions {
        DigestOptions {
            since,
            until,
            audience: DigestAudience::Team,
            format: DigestFormat::Markdown,
            include_next: true,
            include_process: false,
            out: None,
        }
    }

    #[test]
    fn parse_digest_since_handles_duration() {
        let tmp = TempDir::new().unwrap();
        let then = parse_digest_since(Some("7d"), tmp.path()).unwrap();
        let delta = now() - then;
        assert!(delta.num_hours() >= 167 && delta.num_hours() <= 169);
    }

    #[test]
    fn parse_digest_since_handles_iso_date() {
        let tmp = TempDir::new().unwrap();
        let t = parse_digest_since(Some("2024-01-15"), tmp.path()).unwrap();
        assert_eq!(
            t.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2024-01-15 00:00:00"
        );
    }

    #[test]
    fn parse_digest_since_handles_git_tag() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let run = |args: &[&str]| {
            ProcessCommand::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(root.join("a.txt"), "hi").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        run(&["tag", "v0.1.0"]);
        let dt = parse_digest_since(Some("v0.1.0"), root).expect("git tag resolves");
        assert!(now() - dt < chrono::Duration::minutes(5));
    }

    #[test]
    fn parse_digest_since_rejects_garbage() {
        let tmp = TempDir::new().unwrap();
        let err = parse_digest_since(Some("not-a-window"), tmp.path());
        assert!(err.is_err());
    }

    #[test]
    fn parse_digest_since_falls_back_to_marker() {
        let tmp = TempDir::new().unwrap();
        // No marker → 24h.
        let t = parse_digest_since(None, tmp.path()).unwrap();
        let delta = now() - t;
        assert!(delta.num_hours() >= 23 && delta.num_hours() <= 25);
        // With marker → marker's window_end.
        let mark_at = Utc::now() - chrono::Duration::days(3);
        DigestMarker {
            window_end: mark_at,
            written_at: Utc::now(),
        }
        .write(tmp.path())
        .unwrap();
        let t2 = parse_digest_since(None, tmp.path()).unwrap();
        // Round-trip via TOML can lose sub-second precision; compare to seconds.
        assert_eq!(t2.timestamp(), mark_at.timestamp());
    }

    #[test]
    fn digest_marker_round_trip() {
        let tmp = TempDir::new().unwrap();
        let when = Utc::now() - chrono::Duration::days(5);
        let m = DigestMarker {
            window_end: when,
            written_at: Utc::now(),
        };
        m.write(tmp.path()).unwrap();
        let loaded = DigestMarker::load(tmp.path()).unwrap();
        assert_eq!(loaded.window_end.timestamp(), when.timestamp());
        DigestMarker::clear(tmp.path()).unwrap();
        assert!(DigestMarker::load(tmp.path()).is_none());
    }

    fn commit(sha: &str, subject: &str, ids: &[&str], pr: Option<u64>) -> CommitRec {
        CommitRec {
            sha: sha.into(),
            subject: subject.into(),
            date: Utc::now(),
            spec_ids: ids.iter().map(|s| s.to_string()).collect(),
            pr,
        }
    }

    #[test]
    fn collapse_cluster_pr_groups_multi_spec() {
        let commits = vec![
            commit("a", "feat: thing (STORY-1) (#42)", &["STORY-1"], Some(42)),
            commit("b", "feat: other (STORY-2) (#42)", &["STORY-2"], Some(42)),
            commit("c", "fix: lone (BUG-9) (#43)", &["BUG-9"], Some(43)),
        ];
        let themes = collapse_cluster_prs(&commits);
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].pr, 42);
        assert_eq!(themes[0].spec_ids.len(), 2);
    }

    /// TASK-1003 / SPIKE-70: a single-branch cluster PR — N coupled members,
    /// each committed with its own `(SPEC-ID)` trailer onto ONE shared branch,
    /// merged as ONE PR (`#88`) — folds into a single cluster theme linking
    /// every member. Confirms `collapse_cluster_prs` already models the
    /// single-branch one-PR-many-specs shape. trace:TASK-1003 | ai:claude
    #[test]
    fn collapse_cluster_pr_models_single_branch_cluster() {
        let commits = vec![
            commit(
                "a",
                "feat(tui): increment 1 (TASK-1) (#88)",
                &["TASK-1"],
                Some(88),
            ),
            commit(
                "b",
                "feat(tui): increment 2 (TASK-2) (#88)",
                &["TASK-2"],
                Some(88),
            ),
            commit(
                "c",
                "feat(tui): increment 3 (TASK-3) (#88)",
                &["TASK-3"],
                Some(88),
            ),
        ];
        let themes = collapse_cluster_prs(&commits);
        // ONE cluster theme for the one PR, linking all three coupled members.
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].pr, 88);
        assert_eq!(themes[0].spec_ids.len(), 3);
        for id in ["TASK-1", "TASK-2", "TASK-3"] {
            assert!(
                themes[0].spec_ids.iter().any(|s| s == id),
                "cluster PR must link {id}"
            );
        }
    }

    #[test]
    fn is_noise_commit_drops_typo_and_chore() {
        assert!(is_noise_commit("docs: README polish"));
        assert!(is_noise_commit("style: cargo fmt"));
        assert!(is_noise_commit("chore(deps): bump tokio"));
        assert!(is_noise_commit("revert: bad change"));
        assert!(is_noise_commit("fix: typo in error message"));
        assert!(!is_noise_commit("feat(queue): new flag (STORY-42)"));
        assert!(!is_noise_commit("fix(api): null response (BUG-9)"));
    }

    fn fake_req(id: &str, ty: RequirementType, status: RequirementStatus) -> Requirement {
        let mut r = Requirement::new(format!("Title for {}", id), "desc".into());
        r.spec_id = Some(id.to_string());
        r.req_type = ty;
        r.status = status;
        r
    }

    #[test]
    fn collect_rejected_pivots_keeps_only_superseded() {
        let mut store = RequirementsStore::default();
        let mut rejected_with = fake_req(
            "STORY-241",
            RequirementType::Story,
            RequirementStatus::Rejected,
        );
        let successor = fake_req(
            "STORY-244",
            RequirementType::Story,
            RequirementStatus::Approved,
        );
        rejected_with
            .tags
            .insert("supersedes:STORY-244".to_string());
        let just_rejected = fake_req(
            "STORY-99",
            RequirementType::Story,
            RequirementStatus::Rejected,
        );
        store.requirements.push(rejected_with);
        store.requirements.push(successor);
        store.requirements.push(just_rejected);
        // Make the rejected specs' modified_at fall in the window.
        let now = Utc::now();
        for r in store.requirements.iter_mut() {
            r.modified_at = now - chrono::Duration::hours(1);
        }
        let opts = opts_at(
            now - chrono::Duration::days(7),
            now + chrono::Duration::hours(1),
        );
        let pivots = collect_rejected_pivots(&store, &opts);
        assert_eq!(pivots.len(), 1);
        assert_eq!(pivots[0].rejected_id, "STORY-241");
        assert_eq!(pivots[0].successor_id.as_deref(), Some("STORY-244"));
    }

    #[test]
    fn customer_audience_strips_spec_ids() {
        let mut store = RequirementsStore::default();
        let mut done = fake_req(
            "STORY-50",
            RequirementType::Story,
            RequirementStatus::Completed,
        );
        done.title = "Visible feature".into();
        store.requirements.push(done);
        let now = Utc::now();
        for r in store.requirements.iter_mut() {
            r.modified_at = now - chrono::Duration::hours(1);
        }
        let mut opts = opts_at(
            now - chrono::Duration::days(7),
            now + chrono::Duration::hours(1),
        );
        opts.audience = DigestAudience::Customer;
        let report = DigestReport {
            since: opts.since,
            until: opts.until,
            releases: vec![],
            completed: collect_completed(&store, &opts),
            cluster_prs: vec![],
            strategic: vec![],
            pivots: vec![],
            next: NextSection::default(),
            plans: vec![],
            process: vec![],
            capabilities: CapabilitiesReport::default(),
        };
        let customer = render_markdown(&report, &opts);
        assert!(!customer.contains("STORY-"));
        assert!(!customer.contains("TASK-"));
        assert!(!customer.contains("EPIC-"));
        opts.audience = DigestAudience::Team;
        let team = render_markdown(&report, &opts);
        assert!(team.contains("STORY-50"));
    }

    #[test]
    fn render_brief_is_single_paragraph() {
        let now = Utc::now();
        let opts = opts_at(now - chrono::Duration::days(7), now);
        let report = DigestReport {
            since: opts.since,
            until: opts.until,
            releases: vec![ReleaseTag {
                name: "v0.1.0".into(),
                date: now,
                subject: Some("first".into()),
            }],
            completed: vec![],
            cluster_prs: vec![],
            strategic: vec![],
            pivots: vec![],
            next: NextSection::default(),
            plans: vec![],
            process: vec![],
            capabilities: CapabilitiesReport::default(),
        };
        let brief = render_brief(&report, &opts);
        assert!(!brief.contains("##"));
        // Single paragraph means no internal blank line.
        let trimmed = brief.trim_end();
        assert!(!trimmed.contains("\n\n"));
    }

    // ---- operator (capabilities) lens — STORY-541 ----

    #[test]
    fn is_dev_only_commit_drops_clippy_and_refactor_keeps_feat() {
        // Zero-impact dev work → dropped.
        assert!(is_dev_only_commit(
            "fix: 3 clippy correctness errors hidden by CI (TASK-9)"
        ));
        assert!(is_dev_only_commit(
            "refactor(core): extract helper (TASK-1)"
        ));
        assert!(is_dev_only_commit("test(digest): add cases (TASK-2)"));
        assert!(is_dev_only_commit("perf: faster scan"));
        assert!(is_dev_only_commit(
            "refactor(mcp): internal plumbing rename (TASK-3)"
        ));
        // Existing noise still dropped.
        assert!(is_dev_only_commit("chore(deps): bump tokio"));
        assert!(is_dev_only_commit("docs: README polish"));
        // User-facing work → kept.
        assert!(!is_dev_only_commit(
            "feat(queue): add --batch flag (STORY-42)"
        ));
        assert!(!is_dev_only_commit("fix(core): no longer panics on emoji"));
    }

    #[test]
    fn ai_authorship_prefix_does_not_defeat_noise_and_classify() {
        // `[AI:tool]` prefix must not hide the conventional-commit type.
        assert!(is_dev_only_commit(
            "[AI:claude] docs(skills): migrate preamble (TASK-1)"
        ));
        assert!(is_dev_only_commit(
            "[AI:codex] test(templates): widen guard (TASK-2)"
        ));
        // A real feat behind the prefix still classifies + value-strips it.
        let commits = vec![commit(
            "a",
            "[AI:claude] feat(queue): add prune --merged command (STORY-1)",
            &["STORY-1"],
            None,
        )];
        let report = collect_capabilities(&commits);
        assert_eq!(report.new_commands.len(), 1);
        let v = &report.new_commands[0].value;
        assert!(!v.contains("[AI:"));
        assert!(!v.contains("STORY-"));
        assert!(v.starts_with("feat(queue)"));
    }

    #[test]
    fn classify_capability_buckets_by_surface() {
        assert_eq!(
            classify_capability("feat(digest): add operator audience command (STORY-541)"),
            Some(CapabilitySurface::NewCommand)
        );
        assert_eq!(
            classify_capability("feat(queue): --auto-complete now skips reviewer (STORY-1)"),
            Some(CapabilitySurface::ChangedBehavior)
        );
        assert_eq!(
            classify_capability("fix(core): no longer crashes on non-ASCII (BUG-1)"),
            Some(CapabilitySurface::Fix)
        );
        assert_eq!(
            classify_capability("feat: new /aida-digest skill (STORY-2)"),
            Some(CapabilitySurface::NewSkill)
        );
    }

    #[test]
    fn collect_capabilities_drops_noise_strips_ids_and_buckets() {
        let commits = vec![
            commit(
                "a",
                "feat(plan): add aida plan verify command (TASK-93)",
                &["TASK-93"],
                None,
            ),
            commit(
                "b",
                "fix(core): no longer crashes on emoji titles (BUG-7)",
                &["BUG-7"],
                None,
            ),
            // dev-only noise — must be dropped.
            commit(
                "c",
                "fix: clippy correctness warnings (TASK-9)",
                &["TASK-9"],
                None,
            ),
            commit("d", "refactor(core): tidy (TASK-10)", &["TASK-10"], None),
            commit("e", "chore(deps): bump", &[], None),
        ];
        let report = collect_capabilities(&commits);
        assert_eq!(report.new_commands.len(), 1);
        assert_eq!(report.fixes.len(), 1);
        assert!(report.changed_behaviors.is_empty());
        assert!(report.new_skills.is_empty());
        // SPEC-IDs stripped from the value framing.
        let all: String = report
            .new_commands
            .iter()
            .chain(&report.fixes)
            .map(|c| c.value.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!all.contains("TASK-"));
        assert!(!all.contains("BUG-"));
        assert!(all.contains("aida plan verify"));
    }

    #[test]
    fn operator_lens_renders_capabilities_not_work_narrative() {
        let now = Utc::now();
        let mut opts = opts_at(now - chrono::Duration::days(7), now);
        opts.audience = DigestAudience::Operator;
        let mut capabilities = CapabilitiesReport::default();
        capabilities.new_commands.push(CapabilityChange {
            surface: CapabilitySurface::NewCommand,
            value: "feat(plan): add aida plan verify command".into(),
        });
        let report = DigestReport {
            since: opts.since,
            until: opts.until,
            releases: vec![],
            // Work-narrative content that the operator lens must IGNORE.
            completed: vec![CompletedSpec {
                display_id: "STORY-99".into(),
                title: "internal refactor".into(),
                req_type: RequirementType::Story,
                completed_at: now,
            }],
            cluster_prs: vec![],
            strategic: vec![],
            pivots: vec![],
            next: NextSection::default(),
            plans: vec![],
            process: vec![],
            capabilities,
        };
        let md = render_markdown(&report, &opts);
        assert!(md.contains("## New commands"));
        assert!(md.contains("aida plan verify"));
        // Work-narrative section + SPEC-IDs absent.
        assert!(!md.contains("## Major progress"));
        assert!(!md.contains("STORY-99"));
    }

    // ---- STORY-542: deterministic interface_changes source ----

    fn completed_with_ic(
        id: &str,
        ic: aida_core::InterfaceChanges,
        when: DateTime<Utc>,
    ) -> Requirement {
        let mut r = fake_req(id, RequirementType::Story, RequirementStatus::Completed);
        r.interface_changes = Some(ic);
        // Put it in the window via implementation_info.completed_at.
        let mut info = aida_core::ImplementationInfo::default();
        info.completed_at = Some(when);
        r.implementation_info = Some(info);
        r
    }

    #[test]
    fn interface_changes_is_empty_semantics() {
        assert!(aida_core::InterfaceChanges::default().is_empty());
        let ic = aida_core::InterfaceChanges {
            cli: vec!["aida foo — new command".into()],
            ..Default::default()
        };
        assert!(!ic.is_empty());
    }

    #[test]
    fn classify_interface_line_buckets() {
        assert_eq!(
            classify_interface_line("aida mailbox list — new command"),
            CapabilitySurface::NewCommand
        );
        assert_eq!(
            classify_interface_line("aida agent new: native default now (was bypass)"),
            CapabilitySurface::ChangedBehavior
        );
        assert_eq!(
            classify_interface_line("no longer crashes on non-ASCII descriptions"),
            CapabilitySurface::Fix
        );
        assert_eq!(
            classify_interface_line("/aida-digest skill now renders an operator lens"),
            CapabilitySurface::NewSkill
        );
    }

    #[test]
    fn has_captured_interface_changes_window_and_nonempty() {
        let now = Utc::now();
        let opts = opts_at(
            now - chrono::Duration::days(7),
            now + chrono::Duration::days(1),
        );
        let mut store = RequirementsStore::default();

        // In-window, non-empty → counts.
        store.requirements.push(completed_with_ic(
            "STORY-1",
            aida_core::InterfaceChanges {
                cli: vec!["aida foo — new command".into()],
                ..Default::default()
            },
            now,
        ));
        assert!(has_captured_interface_changes(&store, &opts));

        // Empty captured (decided no-impact) → does NOT count.
        let mut store2 = RequirementsStore::default();
        store2.requirements.push(completed_with_ic(
            "STORY-2",
            aida_core::InterfaceChanges::default(),
            now,
        ));
        assert!(!has_captured_interface_changes(&store2, &opts));

        // Out of window → does NOT count.
        let mut store3 = RequirementsStore::default();
        store3.requirements.push(completed_with_ic(
            "STORY-3",
            aida_core::InterfaceChanges {
                cli: vec!["aida bar — new command".into()],
                ..Default::default()
            },
            now - chrono::Duration::days(30),
        ));
        assert!(!has_captured_interface_changes(&store3, &opts));
    }

    #[test]
    fn collect_capabilities_from_store_buckets_and_skips_empty() {
        let now = Utc::now();
        let opts = opts_at(
            now - chrono::Duration::days(7),
            now + chrono::Duration::days(1),
        );
        let mut store = RequirementsStore::default();

        store.requirements.push(completed_with_ic(
            "STORY-10",
            aida_core::InterfaceChanges {
                cli: vec!["aida mailbox list — new command (STORY-10)".into()],
                mcp: vec!["queue_add — now advisor-gated".into()],
                tui: vec![],
                other: vec![],
            },
            now,
        ));
        // No-impact spec — must contribute nothing.
        store.requirements.push(completed_with_ic(
            "STORY-11",
            aida_core::InterfaceChanges::default(),
            now,
        ));
        // A spec with NO interface_changes field at all — also nothing.
        store.requirements.push(fake_req(
            "STORY-12",
            RequirementType::Story,
            RequirementStatus::Completed,
        ));

        let report = collect_capabilities_from_store(&store, &opts);
        // New command bucketed; SPEC-ID stripped.
        assert_eq!(report.new_commands.len(), 1);
        assert!(report.new_commands[0].value.contains("aida mailbox list"));
        assert!(!report.new_commands[0].value.contains("STORY-10"));
        // MCP gating line is changed-behavior.
        assert_eq!(report.changed_behaviors.len(), 1);
        assert!(report.changed_behaviors[0].value.contains("advisor-gated"));
        // Nothing leaked from the empty / absent specs.
        assert!(report.fixes.is_empty());
        assert!(report.new_skills.is_empty());
    }

    #[test]
    fn operator_lens_prefers_captured_over_git() {
        // With captured interface_changes present, render must surface them and
        // NOT re-derive from commit subjects.
        let now = Utc::now();
        let mut opts = opts_at(
            now - chrono::Duration::days(7),
            now + chrono::Duration::days(1),
        );
        opts.audience = DigestAudience::Operator;
        let mut store = RequirementsStore::default();
        store.requirements.push(completed_with_ic(
            "STORY-20",
            aida_core::InterfaceChanges {
                cli: vec!["aida widgets — new command".into()],
                ..Default::default()
            },
            now,
        ));
        assert!(has_captured_interface_changes(&store, &opts));
        let report = collect_capabilities_from_store(&store, &opts);
        assert_eq!(report.new_commands.len(), 1);
        assert!(report.new_commands[0].value.contains("aida widgets"));
    }
}
