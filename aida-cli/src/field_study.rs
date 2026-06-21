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
    let subject = git_stdout(root, &["show", "-s", "--format=%s", sha])?
        .trim()
        .to_string();
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
            let by_span = SPAN_BUCKETS
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
                .collect();
            RuleSummary {
                rule,
                total,
                would_block,
                by_span,
            }
        })
        .collect()
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
        };
        let json = serde_json::to_string(&o).unwrap();
        assert!(json.contains("\"sha\":\"deadbee\""));
        assert!(json.contains("\"would_block\":true"));
        // Round-trips cleanly.
        let back: RuleObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sha, "deadbee");
        assert_eq!(back.span_code_files, 4);
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
