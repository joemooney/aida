//! SPIKE-67: observe-only rule-adherence field study.
//!
//! The gate-vs-rule ablation program (STORY-655) reached its terminus: five
//! controlled cells, all 100% rule-only / 0 gate-saves — every single-variable
//! conjecture for *why* a confident LLM drops a stated rule was falsified in the
//! lab. The methodological conclusion: a clean ablation cannot reproduce
//! rule-dropping at all; the lone observed drop lives in a regime controlled
//! designs cannot reach (large real codebase, long-horizon autonomy). The only
//! remaining instrument is FIELD telemetry.
//!
//! This module is that instrument's first slice. Rather than wire a live,
//! universal git-hook (speculative blast radius for a study that may not pan
//! out) or touch the keystone orchestrator, **the git log IS the planted
//! sensor**: every commit already records the message + diff. `aida field-study
//! scan` recomputes the stated-rule verdicts (commit-format, trace-presence)
//! over recent commits and appends one observation per (commit, rule) to a
//! local-only JSONL log. `aida field-study report` aggregates it — the core
//! lens being "does the would-block rate rise with task span?" (the unmeasured
//! residual the ablations could not reach).
//!
//! Opt-in: default OFF. Enable with `AIDA_FIELD_STUDY=1` or `[field_study]
//! enabled = true` in `.aida/config.toml`. Honors the global `AIDA_TELEMETRY=0`
//! kill-switch.
//!
//! Privacy floor (same as the usage log): NO commit message text, NO file
//! paths, NO diff content. An observation carries only the commit SHA (a public
//! git identifier), the rule name, the boolean verdict, an optional single
//! inferred SPEC-ID (an identifier, not content), the count of changed code
//! files (the task-span proxy), and a repo-size bucket.
//!
//! trace:SPIKE-67 | ai:claude

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One recorded verdict: did a stated-rule check pass for one commit?
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleObservation {
    /// Scan timestamp (RFC3339). When the verdict was recorded, not when the
    /// commit was authored.
    pub ts: String,
    /// Short commit SHA — idempotency key + a public git identifier (not
    /// content).
    pub sha: String,
    /// Which stated-rule check: `commit_format` or `trace_presence`.
    pub rule: String,
    /// The sensor reading: would this stated-rule check have BLOCKED the commit
    /// (true) or did the commit adhere (false)?
    pub would_block: bool,
    /// The single SPEC-ID inferred from the commit's traces, when unambiguous.
    /// An identifier breadcrumb (same kind that lives in commits / trace
    /// comments), never requirement content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    /// Task-span proxy: number of changed code files in the commit.
    pub span_code_files: usize,
    /// Repo-size bucket at scan time (`small`/`medium`/`large`/`huge`). Lets a
    /// later cross-repo harvest correlate adherence with repo size.
    pub repo_bucket: String,
    /// The AI vendor that authored the commit, parsed from the `[AI:tool]`
    /// subject tag (`claude`, `codex`, `antigravity+claude`, …); `None` for an
    /// untagged / human commit. A structural identifier (the same token already
    /// public in the commit subject), never message content. The axis that asks
    /// "do prose rules port across vendors?" (EPIC-48). trace:TASK-891 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// The conventional-commit type, parsed from the subject (`feat`, `fix`,
    /// `docs`, …); `None` if the subject isn't conventional. A structural label
    /// (a category, like `repo_bucket`), never message content. Lets the report
    /// control for commit type so span stops masquerading as the cause.
    /// trace:TASK-891 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_type: Option<String>,
}

/// The two stated rules the sensor evaluates per commit.
const RULE_COMMIT_FORMAT: &str = "commit_format";
const RULE_TRACE_PRESENCE: &str = "trace_presence";

/// File extensions that count as "code" for the trace-presence rule. A commit
/// that changes only docs/config is not expected to carry a trace comment.
const CODE_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "rb", "c", "h", "hpp", "cc", "cpp", "cs",
    "kt", "swift", "php", "sh",
];

/// Resolve `~/.aida/field-study.jsonl`. `None` when the home dir is unknown.
pub fn log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".aida").join("field-study.jsonl"))
}

/// Whether the field study is active. Resolution:
///   1. global kill-switch — if `usage::is_enabled` is false (AIDA_TELEMETRY=0
///      or `[telemetry] enabled = false`), the study is off too.
///   2. opt-in — `AIDA_FIELD_STUDY` truthy OR `[field_study] enabled = true`.
///   3. default: OFF (privacy-conservative; the operator opts in to plant it).
pub fn is_enabled(project_dir: Option<&Path>) -> bool {
    if !crate::usage::is_enabled(project_dir) {
        return false;
    }
    if let Ok(v) = std::env::var("AIDA_FIELD_STUDY") {
        if matches!(v.trim(), "1" | "true" | "yes" | "on") {
            return true;
        }
    }
    let Some(dir) = project_dir else {
        return false;
    };
    let path = dir.join(".aida").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return false;
    };
    parse_field_study_enabled(&content).unwrap_or(false)
}

/// Parse `[field_study] enabled = true` out of a TOML string. `Some(bool)` when
/// present, `None` when absent. Mirrors `usage::parse_telemetry_enabled` so the
/// two opt-outs read config the same way. Pulled out for unit testing.
pub fn parse_field_study_enabled(content: &str) -> Option<bool> {
    let mut in_section = false;
    for raw in content.lines() {
        let line = raw.split('#').next()?.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_section = rest.trim_end_matches(']').trim() == "field_study";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(rest) = line.strip_prefix("enabled") {
            let val = rest.split('=').nth(1)?.trim().trim_matches('"');
            return match val {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
        }
    }
    None
}

/// Append observations as JSONL. Errors are swallowed (a study log must never
/// break the foreground command).
pub fn append_observations(obs: &[RuleObservation]) {
    if obs.is_empty() {
        return;
    }
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        for o in obs {
            if let Ok(json) = serde_json::to_string(o) {
                let _ = writeln!(f, "{}", json);
            }
        }
    }
}

/// Read every recorded observation. Best-effort — malformed lines skipped.
pub fn read_observations() -> Vec<RuleObservation> {
    let Some(path) = log_path() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<RuleObservation>(l).ok())
        .collect()
}

/// Inputs needed to evaluate one commit's rule adherence. Kept as a plain struct
/// so `evaluate_commit` is pure and unit-testable without git.
pub struct CommitFacts {
    /// First line of the commit message.
    pub subject: String,
    /// Distinct SPEC-IDs referenced by `trace:` comments in the diff.
    pub trace_specs: BTreeSet<String>,
    /// Whether the diff carries an AI-authored trace (`trace:ID | ai:...`).
    pub has_ai_trace: bool,
    /// Number of changed files whose extension is a code extension.
    pub changed_code_files: usize,
}

/// Evaluate the stated-rule checks for one commit, producing zero or more
/// observations. Pure — no git, no clock, no I/O — so the rule logic is
/// directly testable. The caller stamps `ts` + `repo_bucket`.
///
/// Rules:
///   - `commit_format`: would the commit-msg hook (the `validate_message`
///     mirror) have rejected the subject? AI-trace presence drives the
///     `[AI:tool]` requirement, exactly as at commit time.
///   - `trace_presence`: a commit that changes code files but carries no trace
///     comment at all — the discipline that is NOT enforced today, so every
///     miss currently slips through. Only emitted when the commit touches code.
pub fn evaluate_commit(facts: &CommitFacts) -> Vec<(String, bool, Option<String>)> {
    let mut out = Vec::new();

    // commit_format: reuse the exact validator the hook mirrors.
    let format_blocks =
        crate::commit::validate_message(&facts.subject, facts.has_ai_trace).is_err();
    out.push((
        RULE_COMMIT_FORMAT.to_string(),
        format_blocks,
        single_spec(&facts.trace_specs),
    ));

    // trace_presence: only meaningful when the commit touches code.
    if facts.changed_code_files > 0 {
        let trace_missing = facts.trace_specs.is_empty();
        out.push((
            RULE_TRACE_PRESENCE.to_string(),
            trace_missing,
            single_spec(&facts.trace_specs),
        ));
    }

    out
}

/// The single inferred SPEC-ID when exactly one is referenced, else `None`
/// (ambiguous → omit), matching `aida commit`'s inference rule.
fn single_spec(specs: &BTreeSet<String>) -> Option<String> {
    if specs.len() == 1 {
        specs.iter().next().cloned()
    } else {
        None
    }
}

/// Parse the AI vendor from a commit subject's leading `[AI:tool]` tag.
///
/// Handles the documented forms: `[AI:claude]`, `[AI:claude:med]` (trailing
/// confidence stripped), and `[AI:antigravity+claude]` / `[AI:tool1+tool2:med]`
/// (multi-agent authorship kept whole). Returns the lowercased tool token(s),
/// or `None` when the subject carries no `[AI:…]` tag (an untagged/human
/// commit). Pure — used at scan time. trace:TASK-891 | ai:claude
pub fn parse_vendor(subject: &str) -> Option<String> {
    let s = subject.trim_start();
    let rest = s.strip_prefix("[AI:").or_else(|| s.strip_prefix("[ai:"))?;
    let inner = rest.split(']').next()?; // text between `[AI:` and `]`
                                         // `<tools>[:confidence]` — tools never contain ':', so the first
                                         // colon-delimited segment is the vendor list.
    let tools = inner.split(':').next()?.trim().to_ascii_lowercase();
    if tools.is_empty() {
        None
    } else {
        Some(tools)
    }
}

/// Parse the conventional-commit type from a subject, skipping any leading
/// `[AI:tool]` tag. `feat(scope): …` / `fix!: …` / `docs: …` → `feat`/`fix`/
/// `docs`. Returns the lowercased type word, or `None` when the subject isn't
/// conventional (no `type:` / `type(scope):` prefix). Pure — used at scan time.
/// trace:TASK-891 | ai:claude
pub fn parse_commit_type(subject: &str) -> Option<String> {
    let mut s = subject.trim_start();
    // Skip a leading [AI:...] tag if present.
    if let Some(rest) = s.strip_prefix('[') {
        if let Some(after) = rest.split_once(']') {
            s = after.1.trim_start();
        }
    }
    // Take the run of ascii-alpha up to `(` (scope), `!` (breaking), or `:`.
    let end = s.find(['(', '!', ':']).filter(|_| s.contains(':'))?;
    let word = s[..end].trim().to_ascii_lowercase();
    if !word.is_empty() && word.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(word)
    } else {
        None
    }
}

/// Strip a trailing ` (#1234)` GitHub squash-merge PR-number suffix from a
/// commit subject.
///
/// A squash-merge appends `(#NNNN)` to the subject, which pushes any
/// authoring-time `(REQ-ID)` out of end-of-line position. The commit-format
/// validator anchors the REQ-ID at `$`, so a squash-merged `feat`/`fix` that
/// the agent authored *correctly* (ending in `(REQ-ID)`, passing the local
/// commit-msg hook pre-squash) reads as a false "missing-(REQ-ID)" miss when
/// the sensor re-evaluates the rewritten subject off `main`. Removing the PR
/// suffix reconstructs the subject the agent actually wrote, so the sensor
/// measures rule-adherence at authoring time rather than GitHub's rewrite — and
/// it never masks a real miss (a genuinely REQ-ID-less subject still fails after
/// the strip). The real hook is untouched; this normalization is study-local.
/// trace:TASK-891 | ai:claude
pub fn strip_pr_suffix(subject: &str) -> String {
    let trimmed = subject.trim_end();
    if let Some(open) = trimmed.rfind("(#") {
        let tail = &trimmed[open..];
        if let Some(digits) = tail.strip_prefix("(#").and_then(|t| t.strip_suffix(')')) {
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return trimmed[..open].trim_end().to_string();
            }
        }
    }
    trimmed.to_string()
}

/// True when a changed path's extension is a code extension.
pub fn is_code_path(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    CODE_EXTS.contains(&ext.as_str())
}

/// Bucket a repo's commit count into a coarse size label.
pub fn repo_bucket_for(commit_count: usize) -> &'static str {
    match commit_count {
        0..=499 => "small",
        500..=1999 => "medium",
        2000..=9999 => "large",
        _ => "huge",
    }
}

/// Harvest result for reporting back to the command handler.
pub struct ScanOutcome {
    pub commits_scanned: usize,
    pub observations_added: usize,
    pub already_recorded: usize,
}

/// Walk recent commits and append fresh observations. Idempotent: a
/// (sha, rule) pair already in the log is never re-appended. `since` is any
/// git revision range expression (e.g. `HEAD~200`, a tag, a date via
/// `--since`); `limit` caps how many commits are inspected.
pub fn scan(root: &Path, since: Option<&str>, limit: usize) -> ScanOutcome {
    let seen: BTreeSet<(String, String)> = read_observations()
        .into_iter()
        .map(|o| (o.sha.clone(), o.rule.clone()))
        .collect();

    let bucket = repo_bucket_for(commit_count(root)).to_string();
    let ts = chrono::Utc::now().to_rfc3339();

    let shas = recent_shas(root, since, limit);
    let mut fresh = Vec::new();
    let mut scanned = 0usize;
    let mut already = 0usize;

    for sha in shas {
        // Skip merge commits — their synthetic subjects always "fail" the
        // format check and they carry no authored diff of their own.
        if is_merge(root, &sha) {
            continue;
        }
        scanned += 1;
        let Some(facts) = commit_facts(root, &sha) else {
            continue;
        };
        // Vendor + commit-type are derived once per commit from the subject
        // (structural identifiers already public in the log, not message text).
        let vendor = parse_vendor(&facts.subject);
        let commit_type = parse_commit_type(&facts.subject);
        for (rule, would_block, spec) in evaluate_commit(&facts) {
            if seen.contains(&(sha.clone(), rule.clone())) {
                already += 1;
                continue;
            }
            fresh.push(RuleObservation {
                ts: ts.clone(),
                sha: sha.clone(),
                rule,
                would_block,
                spec,
                span_code_files: facts.changed_code_files,
                repo_bucket: bucket.clone(),
                vendor: vendor.clone(),
                commit_type: commit_type.clone(),
            });
        }
    }

    let added = fresh.len();
    append_observations(&fresh);
    ScanOutcome {
        commits_scanned: scanned,
        observations_added: added,
        already_recorded: already,
    }
}

/// Gather the per-commit facts from git. `None` if the commit can't be read.
fn commit_facts(root: &Path, sha: &str) -> Option<CommitFacts> {
    let raw_subject = git_stdout(root, &["show", "-s", "--format=%s", sha])?
        .trim()
        .to_string();
    // Reconstruct the authoring-time subject: a squash-merge `(#NNNN)` suffix
    // shadows any trailing `(REQ-ID)` and would read as a false format miss.
    let subject = strip_pr_suffix(&raw_subject);
    let diff = git_stdout(root, &["show", "--format=", "--unified=0", sha]).unwrap_or_default();
    let names = git_stdout(root, &["show", "--name-only", "--format=", sha]).unwrap_or_default();

    let trace_re =
        regex::Regex::new(r"(?i)trace:([A-Z]+(-[A-Z0-9_]+)?-[0-9]+)").expect("valid trace regex");
    let ai_re = regex::Regex::new(r"(?i)trace:[A-Z]+(-[A-Z0-9_]+)?-[0-9]+\s*\|\s*ai:")
        .expect("valid ai-trace regex");

    let mut trace_specs = BTreeSet::new();
    for cap in trace_re.captures_iter(&diff) {
        if let Some(m) = cap.get(1) {
            trace_specs.insert(m.as_str().to_uppercase());
        }
    }
    let has_ai_trace = ai_re.is_match(&diff);
    let changed_code_files = names
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| is_code_path(l))
        .count();

    Some(CommitFacts {
        subject,
        trace_specs,
        has_ai_trace,
        changed_code_files,
    })
}

/// Short SHAs of recent commits, newest first.
fn recent_shas(root: &Path, since: Option<&str>, limit: usize) -> Vec<String> {
    let mut args = vec!["log".to_string(), "--no-merges".to_string()];
    // `--no-merges` already drops merges; the explicit is_merge guard is a
    // belt-and-suspenders for ranges passed as raw revs.
    args.push(format!("--max-count={}", limit));
    args.push("--format=%h".to_string());
    if let Some(s) = since {
        args.push(s.to_string());
    }
    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    git_stdout(root, &argv)
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Total commit count reachable from HEAD (repo-size proxy).
fn commit_count(root: &Path) -> usize {
    git_stdout(root, &["rev-list", "--count", "HEAD"])
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

/// True if the commit has 2+ parents (a merge).
fn is_merge(root: &Path, sha: &str) -> bool {
    git_stdout(root, &["show", "-s", "--format=%P", sha])
        .map(|s| s.split_whitespace().count() >= 2)
        .unwrap_or(false)
}

/// Capture stdout of a git command; `None` on failure.
fn git_stdout(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// One rule's aggregated adherence, overall and bucketed by task span.
pub struct RuleSummary {
    pub rule: String,
    pub total: usize,
    pub would_block: usize,
    /// (span-bucket label, total, would_block) rows, the hypothesis lens.
    pub by_span: Vec<(&'static str, usize, usize)>,
}

/// Coarse task-span buckets for the "does adherence fall as span grows?" lens.
fn span_bucket(n: usize) -> &'static str {
    match n {
        0 => "0",
        1 => "1",
        2..=3 => "2-3",
        4..=9 => "4-9",
        _ => "10+",
    }
}

const SPAN_BUCKETS: &[&str] = &["0", "1", "2-3", "4-9", "10+"];

/// Safe ratio; 0.0 for an empty denominator. Local mirror of `main::rate` so the
/// analysis layer is self-contained and unit-testable.
fn rate(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

/// Build the (span-bucket, total, would_block) curve over an arbitrary row
/// slice, dropping empty buckets. The shared kernel behind every span lens —
/// overall, per-vendor, and feat/fix-only. trace:TASK-891 | ai:claude
pub fn span_curve(rows: &[&RuleObservation]) -> Vec<(&'static str, usize, usize)> {
    SPAN_BUCKETS
        .iter()
        .map(|&b| {
            let in_bucket: Vec<&&RuleObservation> = rows
                .iter()
                .filter(|o| span_bucket(o.span_code_files) == b)
                .collect();
            let t = in_bucket.len();
            let wb = in_bucket.iter().filter(|o| o.would_block).count();
            (b, t, wb)
        })
        .filter(|(_, t, _)| *t > 0)
        .collect()
}

/// The smallest-span vs largest-span endpoints of a span curve — the cheap,
/// mechanical "does the would-block rate rise with span?" read that lets the
/// report state, per control, whether the span/load effect SURVIVES.
pub struct SpanTrend {
    pub first_bucket: &'static str,
    pub first_rate: f64,
    pub last_bucket: &'static str,
    pub last_rate: f64,
}

impl SpanTrend {
    /// last-bucket rate minus first-bucket rate.
    pub fn delta(&self) -> f64 {
        self.last_rate - self.first_rate
    }
    /// Coarse verdict: a ≥15-point swing reads as rises/falls; otherwise flat.
    /// "flat" is the null result SPIKE-67 treats as a valid, reportable outcome.
    pub fn verdict(&self) -> &'static str {
        let d = self.delta();
        if d >= 0.15 {
            "rises"
        } else if d <= -0.15 {
            "falls"
        } else {
            "flat"
        }
    }
}

/// Endpoint trend of a span curve. `None` when fewer than two buckets are
/// populated (no curve to read).
pub fn span_trend(curve: &[(&'static str, usize, usize)]) -> Option<SpanTrend> {
    let populated: Vec<&(&'static str, usize, usize)> =
        curve.iter().filter(|(_, t, _)| *t > 0).collect();
    if populated.len() < 2 {
        return None;
    }
    let first = populated.first().unwrap();
    let last = populated.last().unwrap();
    Some(SpanTrend {
        first_bucket: first.0,
        first_rate: rate(first.2, first.1),
        last_bucket: last.0,
        last_rate: rate(last.2, last.1),
    })
}

/// Aggregate the log into one summary per rule.
pub fn summarize(obs: &[RuleObservation]) -> Vec<RuleSummary> {
    let mut rules: Vec<String> = obs.iter().map(|o| o.rule.clone()).collect();
    rules.sort();
    rules.dedup();

    rules
        .into_iter()
        .map(|rule| {
            let rows: Vec<&RuleObservation> = obs.iter().filter(|o| o.rule == rule).collect();
            let total = rows.len();
            let would_block = rows.iter().filter(|o| o.would_block).count();
            let by_span = span_curve(&rows);
            RuleSummary {
                rule,
                total,
                would_block,
                by_span,
            }
        })
        .collect()
}

/// Resolve `~/.aida/auto-complete.jsonl` — the autonomous-drain run log the
/// drain-vs-interactive join reads. `None` when the home dir is unknown.
pub fn auto_complete_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".aida").join("auto-complete.jsonl"))
}

/// The set of SPEC-IDs that appear as a drained spec in `auto-complete.jsonl`
/// (uppercased for case-insensitive join). Best-effort: a missing/garbled log
/// yields an empty set (every commit then classifies `interactive`). This is
/// **spec-level** attribution — the spec was driven by an `--auto-complete`
/// orchestrator at some point — not commit-level session attribution.
/// trace:TASK-891 | ai:claude
pub fn drain_spec_set() -> BTreeSet<String> {
    let Some(path) = auto_complete_path() else {
        return BTreeSet::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return BTreeSet::new();
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| {
            v.get("spec_id")
                .and_then(|s| s.as_str())
                .map(|s| s.to_ascii_uppercase())
        })
        .collect()
}

/// One vendor's adherence for a rule, with its span trend (the per-vendor
/// "does adherence degrade under load for THIS vendor?" read — EPIC-48).
pub struct VendorCut {
    pub vendor: String,
    pub total: usize,
    pub would_block: usize,
    pub trend: Option<SpanTrend>,
}

/// The drain-vs-interactive split. `headless`/`supervised` is NOT separable
/// from `auto-complete.jsonl` (it records no human-mode flag), so this axis
/// stops at drain-vs-interactive; the report says so rather than fabricating it.
pub struct DrainCut {
    pub drain_total: usize,
    pub drain_block: usize,
    pub interactive_total: usize,
    pub interactive_block: usize,
    /// Observations with no inferred SPEC-ID — can't be joined either way.
    pub unattributed_total: usize,
    pub unattributed_block: usize,
}

/// The type control: the span trend over all commits vs over feat/fix-only
/// commits. If the all-types curve `rises` but the feat/fix-only curve goes
/// `flat`, the span effect was commit-type masquerading as span.
pub struct TypeControl {
    pub all_trend: Option<SpanTrend>,
    pub featfix_total: usize,
    pub featfix_block: usize,
    pub featfix_curve: Vec<(&'static str, usize, usize)>,
    pub featfix_trend: Option<SpanTrend>,
}

/// All three slice-2 controls computed for one rule. Paired positionally with
/// the matching [`RuleSummary`] by the report, so the rule name lives there.
pub struct RuleControls {
    pub vendors: Vec<VendorCut>,
    pub drain: DrainCut,
    pub type_control: TypeControl,
}

/// Label an observation's vendor for grouping — `None` becomes `untagged` so
/// human/untagged commits are still accounted for in the vendor breakdown.
fn vendor_label(o: &RuleObservation) -> String {
    o.vendor.clone().unwrap_or_else(|| "untagged".to_string())
}

/// True when an observation is a feat/fix commit (the types the strict
/// commit-format rule disproportionately applies to).
fn is_feat_or_fix(o: &RuleObservation) -> bool {
    matches!(o.commit_type.as_deref(), Some("feat") | Some("fix"))
}

/// Compute the three slice-2 controls for one rule. `drain_specs` is the join
/// set from [`drain_spec_set`]; passed in so the analysis is pure/testable.
/// trace:TASK-891 | ai:claude
pub fn controls_for(
    obs: &[RuleObservation],
    rule: &str,
    drain_specs: &BTreeSet<String>,
) -> RuleControls {
    let rows: Vec<&RuleObservation> = obs.iter().filter(|o| o.rule == rule).collect();

    // (a) vendor
    let mut names: Vec<String> = rows.iter().map(|o| vendor_label(o)).collect();
    names.sort();
    names.dedup();
    let vendors = names
        .into_iter()
        .map(|v| {
            let vrows: Vec<&RuleObservation> = rows
                .iter()
                .filter(|o| vendor_label(o) == v)
                .copied()
                .collect();
            let total = vrows.len();
            let would_block = vrows.iter().filter(|o| o.would_block).count();
            let trend = span_trend(&span_curve(&vrows));
            VendorCut {
                vendor: v,
                total,
                would_block,
                trend,
            }
        })
        .collect();

    // (b) drain-vs-interactive
    let mut drain = DrainCut {
        drain_total: 0,
        drain_block: 0,
        interactive_total: 0,
        interactive_block: 0,
        unattributed_total: 0,
        unattributed_block: 0,
    };
    for o in &rows {
        match &o.spec {
            Some(s) if drain_specs.contains(&s.to_ascii_uppercase()) => {
                drain.drain_total += 1;
                drain.drain_block += o.would_block as usize;
            }
            Some(_) => {
                drain.interactive_total += 1;
                drain.interactive_block += o.would_block as usize;
            }
            None => {
                drain.unattributed_total += 1;
                drain.unattributed_block += o.would_block as usize;
            }
        }
    }

    // (c) type control: all-types trend vs feat/fix-only trend
    let all_trend = span_trend(&span_curve(&rows));
    let ff: Vec<&RuleObservation> = rows.iter().filter(|o| is_feat_or_fix(o)).copied().collect();
    let featfix_total = ff.len();
    let featfix_block = ff.iter().filter(|o| o.would_block).count();
    let featfix_curve = span_curve(&ff);
    let featfix_trend = span_trend(&featfix_curve);

    RuleControls {
        vendors,
        drain,
        type_control: TypeControl {
            all_trend,
            featfix_total,
            featfix_block,
            featfix_curve,
            featfix_trend,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(subject: &str, specs: &[&str], ai: bool, code_files: usize) -> CommitFacts {
        CommitFacts {
            subject: subject.to_string(),
            trace_specs: specs.iter().map(|s| s.to_string()).collect(),
            has_ai_trace: ai,
            changed_code_files: code_files,
        }
    }

    #[test]
    fn conforming_commit_adheres_to_both_rules() {
        let f = facts(
            "[AI:claude] feat(x): do thing (TASK-1)",
            &["TASK-1"],
            true,
            2,
        );
        let out = evaluate_commit(&f);
        // Both rules emitted, neither would block.
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|(_, wb, _)| !*wb));
    }

    #[test]
    fn nonconventional_subject_would_block_format() {
        let f = facts("wip random stuff", &[], false, 1);
        let format = out_for(&evaluate_commit(&f), RULE_COMMIT_FORMAT);
        assert!(format, "non-conventional subject should trip commit_format");
    }

    #[test]
    fn code_change_without_trace_would_block_trace_presence() {
        let f = facts("chore: tidy", &[], false, 3);
        let trace = out_for(&evaluate_commit(&f), RULE_TRACE_PRESENCE);
        assert!(
            trace,
            "code change with no trace should trip trace_presence"
        );
    }

    #[test]
    fn docs_only_commit_emits_no_trace_presence_rule() {
        // No code files changed → the trace-presence rule is not applicable.
        let f = facts("docs: update readme", &[], false, 0);
        let out = evaluate_commit(&f);
        assert!(out.iter().all(|(r, _, _)| r != RULE_TRACE_PRESENCE));
    }

    #[test]
    fn ambiguous_traces_omit_the_single_spec() {
        let f = facts(
            "[AI:claude] feat(x): y (TASK-1)",
            &["TASK-1", "TASK-2"],
            true,
            1,
        );
        assert!(out_spec(&evaluate_commit(&f), RULE_COMMIT_FORMAT).is_none());
    }

    #[test]
    fn serialized_observation_carries_no_message_or_path() {
        // Privacy floor: the JSON line must not be able to leak the subject or
        // file paths — the struct has no field for them.
        let o = RuleObservation {
            ts: "2026-06-20T00:00:00Z".to_string(),
            sha: "deadbee".to_string(),
            rule: RULE_COMMIT_FORMAT.to_string(),
            would_block: true,
            spec: Some("TASK-9".to_string()),
            span_code_files: 4,
            repo_bucket: "large".to_string(),
            vendor: Some("claude".to_string()),
            commit_type: Some("feat".to_string()),
        };
        let json = serde_json::to_string(&o).unwrap();
        assert!(json.contains("\"sha\":\"deadbee\""));
        assert!(json.contains("\"would_block\":true"));
        // vendor/commit_type are structural labels, not message text or paths.
        assert!(json.contains("\"vendor\":\"claude\""));
        assert!(json.contains("\"commit_type\":\"feat\""));
        // Round-trips cleanly.
        let back: RuleObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sha, "deadbee");
        assert_eq!(back.span_code_files, 4);
        assert_eq!(back.vendor.as_deref(), Some("claude"));
    }

    #[test]
    fn legacy_observation_without_new_fields_deserializes() {
        // A pre-slice-2 log line has no vendor/commit_type — serde defaults them
        // to None so old logs read cleanly (rescan to backfill the attribution).
        let line = r#"{"ts":"t","sha":"abc","rule":"commit_format","would_block":false,"span_code_files":1,"repo_bucket":"small"}"#;
        let o: RuleObservation = serde_json::from_str(line).unwrap();
        assert!(o.vendor.is_none());
        assert!(o.commit_type.is_none());
    }

    #[test]
    fn parse_vendor_handles_documented_forms() {
        assert_eq!(
            parse_vendor("[AI:claude] feat(x): y (TASK-1)").as_deref(),
            Some("claude")
        );
        // Trailing confidence stripped.
        assert_eq!(
            parse_vendor("[AI:claude:med] fix(x): y").as_deref(),
            Some("claude")
        );
        // Multi-agent authorship kept whole.
        assert_eq!(
            parse_vendor("[AI:antigravity+claude] test(x): y").as_deref(),
            Some("antigravity+claude")
        );
        assert_eq!(
            parse_vendor("[AI:tool1+tool2:med] feat(x): y").as_deref(),
            Some("tool1+tool2")
        );
        // Untagged / human commit.
        assert_eq!(parse_vendor("docs: update readme"), None);
    }

    #[test]
    fn strip_pr_suffix_recovers_authoring_subject() {
        // Squash suffix shadows the trailing (REQ-ID): stripping recovers it.
        assert_eq!(
            strip_pr_suffix("[AI:claude] fix(init): x (BUG-604) (#1079)"),
            "[AI:claude] fix(init): x (BUG-604)"
        );
        // No PR suffix → unchanged.
        assert_eq!(
            strip_pr_suffix("[AI:claude] feat(x): y (TASK-1)"),
            "[AI:claude] feat(x): y (TASK-1)"
        );
        // A trailing non-numeric paren (a real REQ-ID) is NOT stripped.
        assert_eq!(
            strip_pr_suffix("feat(x): y (TASK-9)"),
            "feat(x): y (TASK-9)"
        );

        // End-to-end: the squash subject would falsely block; the strip fixes it.
        let squash = facts(
            "[AI:claude] feat(x): y (TASK-1) (#1078)",
            &["TASK-1"],
            true,
            2,
        );
        // Pre-strip the squash subject DOES trip the validator…
        assert!(crate::commit::validate_message(&squash.subject, true).is_err());
        // …but the scanner strips it first, so evaluate sees the clean subject.
        let cleaned = facts(&strip_pr_suffix(&squash.subject), &["TASK-1"], true, 2);
        let format = out_for(&evaluate_commit(&cleaned), RULE_COMMIT_FORMAT);
        assert!(
            !format,
            "stripped squash subject should adhere to commit_format"
        );
    }

    #[test]
    fn parse_commit_type_skips_ai_tag_and_scope() {
        assert_eq!(
            parse_commit_type("[AI:claude] feat(field-study): y (TASK-1)").as_deref(),
            Some("feat")
        );
        assert_eq!(parse_commit_type("fix: a bug").as_deref(), Some("fix"));
        assert_eq!(
            parse_commit_type("docs(readme): tidy").as_deref(),
            Some("docs")
        );
        assert_eq!(
            parse_commit_type("feat!: breaking").as_deref(),
            Some("feat")
        );
        // Non-conventional subject → None.
        assert_eq!(parse_commit_type("wip random stuff"), None);
        assert_eq!(parse_commit_type("Merge branch 'x'"), None);
    }

    #[test]
    fn span_trend_reads_endpoints_and_verdict() {
        // Rising curve: 8% at the small end, 95% at the large end.
        let curve = vec![("0", 100, 8), ("1", 50, 34), ("4-9", 20, 19)];
        let t = span_trend(&curve).unwrap();
        assert_eq!(t.first_bucket, "0");
        assert_eq!(t.last_bucket, "4-9");
        assert!(t.delta() > 0.8);
        assert_eq!(t.verdict(), "rises");
        // Flat curve → null result.
        let flat = vec![("0", 10, 5), ("4-9", 10, 5)];
        assert_eq!(span_trend(&flat).unwrap().verdict(), "flat");
        // Single populated bucket → no trend.
        assert!(span_trend(&[("1", 10, 1)]).is_none());
    }

    #[test]
    fn controls_split_vendor_drain_and_type() {
        let mk = |sha: &str,
                  wb: bool,
                  span: usize,
                  spec: Option<&str>,
                  vendor: Option<&str>,
                  ctype: Option<&str>| RuleObservation {
            ts: "t".into(),
            sha: sha.into(),
            rule: RULE_COMMIT_FORMAT.into(),
            would_block: wb,
            spec: spec.map(|s| s.into()),
            span_code_files: span,
            repo_bucket: "small".into(),
            vendor: vendor.map(|v| v.into()),
            commit_type: ctype.map(|c| c.into()),
        };
        let obs = vec![
            // claude, drained spec, feat, small span, adheres
            mk("a", false, 1, Some("TASK-1"), Some("claude"), Some("feat")),
            // claude, drained spec, feat, big span, would-block
            mk("b", true, 9, Some("TASK-1"), Some("claude"), Some("feat")),
            // codex, interactive spec, docs, would-block
            mk("c", true, 2, Some("TASK-2"), Some("codex"), Some("docs")),
            // untagged, no spec, big span, would-block
            mk("d", true, 12, None, None, None),
        ];
        let drain: BTreeSet<String> = ["TASK-1".to_string()].into_iter().collect();
        let c = controls_for(&obs, RULE_COMMIT_FORMAT, &drain);

        // Vendor breakdown: claude, codex, untagged.
        let claude = c.vendors.iter().find(|v| v.vendor == "claude").unwrap();
        assert_eq!(claude.total, 2);
        assert_eq!(claude.would_block, 1);
        assert!(c.vendors.iter().any(|v| v.vendor == "untagged"));

        // Drain split: TASK-1 (2 rows) drain; TASK-2 interactive; untagged none.
        assert_eq!(c.drain.drain_total, 2);
        assert_eq!(c.drain.drain_block, 1);
        assert_eq!(c.drain.interactive_total, 1);
        assert_eq!(c.drain.unattributed_total, 1);

        // Type control: only the two feat rows are in the feat/fix cut.
        assert_eq!(c.type_control.featfix_total, 2);
        assert_eq!(c.type_control.featfix_block, 1);
    }

    #[test]
    fn drain_class_empty_log_marks_everything_interactive() {
        // With an empty drain set, a spec'd row is interactive, an unspec'd row
        // is unattributed — never falsely "drain".
        let mk = |spec: Option<&str>| RuleObservation {
            ts: "t".into(),
            sha: "s".into(),
            rule: RULE_COMMIT_FORMAT.into(),
            would_block: false,
            spec: spec.map(|s| s.into()),
            span_code_files: 1,
            repo_bucket: "small".into(),
            vendor: None,
            commit_type: None,
        };
        let obs = vec![mk(Some("TASK-9")), mk(None)];
        let c = controls_for(&obs, RULE_COMMIT_FORMAT, &BTreeSet::new());
        assert_eq!(c.drain.drain_total, 0);
        assert_eq!(c.drain.interactive_total, 1);
        assert_eq!(c.drain.unattributed_total, 1);
    }

    #[test]
    fn config_opt_in_parses() {
        assert_eq!(
            parse_field_study_enabled("[field_study]\nenabled = true\n"),
            Some(true)
        );
        assert_eq!(parse_field_study_enabled("[other]\nenabled = true\n"), None);
        assert_eq!(
            parse_field_study_enabled("[field_study]\n# nothing\n"),
            None
        );
    }

    #[test]
    fn repo_and_span_buckets() {
        assert_eq!(repo_bucket_for(10), "small");
        assert_eq!(repo_bucket_for(1500), "medium");
        assert_eq!(repo_bucket_for(5000), "large");
        assert_eq!(repo_bucket_for(50000), "huge");
        assert_eq!(span_bucket(0), "0");
        assert_eq!(span_bucket(3), "2-3");
        assert_eq!(span_bucket(99), "10+");
    }

    #[test]
    fn is_code_path_classifies_by_extension() {
        assert!(is_code_path("aida-cli/src/main.rs"));
        assert!(!is_code_path("docs/readme.md"));
        assert!(!is_code_path("Cargo.toml"));
    }

    #[test]
    fn summarize_buckets_by_span() {
        let mk = |rule: &str, wb: bool, span: usize| RuleObservation {
            ts: "t".into(),
            sha: "s".into(),
            rule: rule.into(),
            would_block: wb,
            spec: None,
            span_code_files: span,
            repo_bucket: "small".into(),
            vendor: None,
            commit_type: None,
        };
        let obs = vec![
            mk(RULE_TRACE_PRESENCE, false, 1),
            mk(RULE_TRACE_PRESENCE, true, 10),
            mk(RULE_TRACE_PRESENCE, true, 12),
        ];
        let s = summarize(&obs);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].total, 3);
        assert_eq!(s[0].would_block, 2);
        // The 10+ bucket should hold the two blocks.
        let big = s[0].by_span.iter().find(|(b, _, _)| *b == "10+").unwrap();
        assert_eq!(big.1, 2);
        assert_eq!(big.2, 2);
    }

    // Helpers to pluck a rule's verdict out of evaluate_commit's output.
    fn out_for(out: &[(String, bool, Option<String>)], rule: &str) -> bool {
        out.iter()
            .find(|(r, _, _)| r == rule)
            .map(|(_, wb, _)| *wb)
            .unwrap()
    }
    fn out_spec(out: &[(String, bool, Option<String>)], rule: &str) -> Option<String> {
        out.iter()
            .find(|(r, _, _)| r == rule)
            .and_then(|(_, _, s)| s.clone())
    }
}
