//! SPIKE-67 slice 3: live drain-path instrumentation for stated-rule
//! violations — the real-time gate-vs-rule evidence path.
//!
//! The gate-vs-rule thesis (substrate-as-bouncer): a rule merely *stated* in a
//! prompt / CLAUDE.md / brief is unreliable against a confident LLM; to
//! guarantee an invariant you need a programmatic GATE. Slices 1-2 (the
//! [`crate::field_study`] git-log-as-sensor) measure rule adherence
//! *retrospectively* by recomputing verdicts over the commit log. This slice
//! is the *real-time* complement: during a real `aida queue work
//! --auto-complete` drain, the orchestrator already learns when a stated rule
//! was violated — CI came back red on a `fmt` / `clippy` / provenance
//! (`/// SPEC-ID` doc-comment leak) check that CLAUDE.md / a brief *stated*, or
//! an implementer *punted* citing a rule. Those moments are exactly "a stated
//! rule was broken and no gate stopped it" — the drain caught it only
//! after-the-fact, in CI, not before the commit. This module records one
//! structured event per such moment so the field study can answer the
//! load-bearing question: *which stated rules need to become gates?*
//!
//! Observe-only: it never blocks, never changes the drain outcome. It is fed
//! the failure the orchestrator *already* classified
//! ([`crate::auto_complete::OrchestrationResult`]) and turns the subset that
//! corresponds to a known stated-rule into a log line.
//!
//! Opt-in + privacy floor: shares [`crate::field_study::is_enabled`]
//! (default OFF; `AIDA_FIELD_STUDY=1` / `[field_study] enabled = true`; honors
//! the `AIDA_TELEMETRY=0` global kill-switch). An event carries only
//! identifiers and categories — the spec id (a breadcrumb), the rule slug, the
//! drain phase, how it surfaced, whether the run was headless, the variant, and
//! a repo-size bucket. NEVER the failure message text, file paths, diff
//! content, or requirement content.
//!
//! trace:SPIKE-67 | ai:claude

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One observed stated-rule violation during a real drain — appended
/// one-per-event to `~/.aida/rule-violations.jsonl`.
// trace:SPIKE-67 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuleViolation {
    /// RFC3339 — when the violation was observed (drain completion time).
    pub ts: String,
    /// The spec the drain was driving. An identifier breadcrumb (the same kind
    /// already public in commits / trace comments), never requirement content.
    pub spec_id: String,
    /// Which stated-rule was violated: `fmt`, `clippy`, `provenance-leak`,
    /// `commit-format`, `advisor-code-gate`, or `other` — a stable category,
    /// never the underlying message text.
    pub rule: String,
    /// Where the drain caught the violation: the phase slug (`ci`,
    /// `implementer`, `reviewer`, …). The distance between where the rule was
    /// stated (the brief, pre-commit) and where it was caught (CI) is itself
    /// the gate-vs-rule signal.
    pub caught_in_phase: String,
    /// How the violation surfaced: `ci-red` (CI tripped it), `punt` (the
    /// implementer punted citing the rule), or `reviewer` (a RequestChanges on
    /// the rule). A category, not a message.
    pub via: String,
    /// Whether the run was a headless (`--no-human`) drain. The
    /// context-pressure axis the hypothesis most wants — do stated rules fail
    /// more under unattended autonomy?
    pub headless: bool,
    /// Which `--auto-complete` variant ran (`full`, `through-ci`, …). Span /
    /// load context, a structural label.
    pub variant: String,
    /// Repo-size bucket (`small`/`medium`/`large`/`huge`) so a later cross-repo
    /// harvest can correlate violations with codebase size — the regime the
    /// controlled ablations could not reach.
    pub repo_bucket: String,
    /// Short build SHA of the `aida` binary (release tracking).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_sha: Option<String>,
}

/// Stable rule slugs. A stated-rule is one written in a brief / CLAUDE.md /
/// commit convention — the kind the gate-vs-rule thesis says is unreliable.
pub const RULE_FMT: &str = "fmt";
pub const RULE_CLIPPY: &str = "clippy";
pub const RULE_PROVENANCE_LEAK: &str = "provenance-leak";
pub const RULE_COMMIT_FORMAT: &str = "commit-format";
pub const RULE_ADVISOR_CODE_GATE: &str = "advisor-code-gate";
// trace:TASK-917 | ai:claude
/// A commit made with `git commit --no-verify`: the pre-commit hook was bypassed
/// wholesale, so every stated-rule gate the pre-commit hook enforces (the
/// advisor-code-gate, the doc-comment SPEC-ID leak, fmt auto-fix) was skipped at
/// once. The bypass itself is the violation — caught DIRECTLY (a post-commit
/// detector noticing the pre-commit sentinel is absent), not indirectly as later
/// CI red on whatever the bypassed gate would have caught.
pub const RULE_NO_VERIFY_BYPASS: &str = "no-verify-bypass";

// trace:TASK-917 | ai:claude
/// Was the just-made commit's pre-commit hook BYPASSED (i.e. was the commit made
/// with `git commit --no-verify`)?
///
/// The pre-commit hook cannot detect its own bypass — `--no-verify` is exactly
/// "don't run the pre-commit hook". So the detection is inverted and runs
/// *after* the commit: the pre-commit hook leaves a sentinel marker for the tree
/// it validated; a post-commit detector checks whether that sentinel matches the
/// committed tree. Absent/stale sentinel means the pre-commit hook never ran for
/// this commit, so `--no-verify` was used.
///
/// Pure: the caller supplies whether the sentinel matched the committed tree;
/// this is the one-line predicate so the inversion is unit-testable and the IO
/// (reading the sentinel, hashing the tree) lives at the call site.
pub fn detect_no_verify_bypass(precommit_sentinel_matched: bool) -> bool {
    !precommit_sentinel_matched
}

/// The minimal failure context the detector needs — decoupled from
/// `auto_complete::OrchestrationResult` so [`detect`] is a pure function that
/// is trivially unit-testable without constructing a full orchestration.
#[derive(Debug, Clone)]
pub struct DrainOutcome<'a> {
    /// Phase slug where the drain stopped/failed (`ci`, `reviewer`, …), if any.
    pub failed_phase_slug: Option<&'a str>,
    /// The orchestrator's failure-kind slug (`ci-red`, `no-verdict`, …), if any.
    pub failure_kind: Option<&'a str>,
    /// The one-line failure reason. Parsed for rule signatures (fmt / clippy /
    /// the `/// SPEC-ID` leak) and then DISCARDED — never logged.
    pub failure_message: Option<&'a str>,
    /// The punt reason, if the implementer punted (also parsed-then-discarded).
    pub punt_reason: Option<&'a str>,
}

/// Map a stated-rule signature out of a failure / punt message. Pure + private:
/// the message text is inspected here and never leaves this function. Returns
/// the rule slug when the message names a known stated-rule.
fn classify_rule(message: &str) -> Option<&'static str> {
    let m = message.to_lowercase();
    // Provenance / the recurring `/// SPEC-ID` doc-comment leak (BUG-624's gate).
    if m.contains("/// ")
        || m.contains("doc comment")
        || m.contains("doc-comment")
        || m.contains("provenance")
        || m.contains("spec-id leak")
        || m.contains("trace:")
    {
        return Some(RULE_PROVENANCE_LEAK);
    }
    // rustfmt drift.
    if m.contains("rustfmt")
        || m.contains("fmt --check")
        || m.contains("cargo fmt")
        || m.contains("not formatted")
    {
        return Some(RULE_FMT);
    }
    // clippy.
    if m.contains("clippy") {
        return Some(RULE_CLIPPY);
    }
    // The advisor-code-gate (STORY-684): advisor authored implementation code.
    if m.contains("advisor-code") || m.contains("advisor code gate") || m.contains("code-gate") {
        return Some(RULE_ADVISOR_CODE_GATE);
    }
    // Commit-message format convention.
    if m.contains("commit format")
        || m.contains("commit-format")
        || m.contains("commit message format")
    {
        return Some(RULE_COMMIT_FORMAT);
    }
    None
}

/// Detect zero or more stated-rule violations from a drain outcome. Pure: no
/// IO, no message text retained in the returned events. The detectors are:
///
/// 1. **CI-red on a stated rule** — `failure_kind == ci-red` and the reason
///    names a fmt / clippy / provenance / commit-format check. The drain caught
///    it in CI; the rule was *stated* (in CLAUDE.md / the brief) but no
///    pre-commit gate stopped it. The headline gate-vs-rule case.
/// 2. **Reviewer RequestChanges citing a rule** — the reviewer (a stated-rule
///    enforcer in prose) flagged a rule the substrate could have gated.
/// 3. **Punt citing a rule** — the implementer punted and the punt reason
///    names a known stated-rule.
///
/// Each yields at most one event per rule (deduped). A clean run yields `[]`.
// trace:SPIKE-67 | ai:claude
pub fn detect(outcome: &DrainOutcome) -> Vec<&'static str> {
    let mut rules: Vec<&'static str> = Vec::new();

    // Detector 1 + 2: a phase failure whose message names a stated rule.
    if let Some(msg) = outcome.failure_message {
        let is_ci_red = outcome.failure_kind == Some("ci-red");
        let is_reviewer = outcome.failed_phase_slug == Some("reviewer")
            || outcome.failure_kind == Some("no-verdict");
        if is_ci_red || is_reviewer {
            if let Some(rule) = classify_rule(msg) {
                rules.push(rule);
            }
        }
    }

    // Detector 3: an implementer punt that cites a rule.
    if let Some(punt) = outcome.punt_reason {
        if let Some(rule) = classify_rule(punt) {
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
    }

    rules
}

/// Derive the `via` category from how the violation surfaced.
fn via_for(outcome: &DrainOutcome) -> &'static str {
    if outcome.punt_reason.is_some() && outcome.failure_kind.is_none() {
        "punt"
    } else if outcome.failure_kind == Some("ci-red") {
        "ci-red"
    } else if outcome.failed_phase_slug == Some("reviewer")
        || outcome.failure_kind == Some("no-verdict")
    {
        "reviewer"
    } else if outcome.punt_reason.is_some() {
        "punt"
    } else {
        "other"
    }
}

/// Build the recordable events for a completed drain. Pulled out from
/// [`record`] so event construction is testable without touching the
/// filesystem.
// trace:SPIKE-67 | ai:claude
#[allow(clippy::too_many_arguments)]
pub fn build_events(
    outcome: &DrainOutcome,
    ts: &str,
    spec_id: &str,
    headless: bool,
    variant: &str,
    repo_bucket: &str,
    binary_sha: Option<String>,
) -> Vec<RuleViolation> {
    let via = via_for(outcome).to_string();
    let caught_in_phase = outcome
        .failed_phase_slug
        .unwrap_or("implementer")
        .to_string();
    detect(outcome)
        .into_iter()
        .map(|rule| RuleViolation {
            ts: ts.to_string(),
            spec_id: spec_id.to_string(),
            rule: rule.to_string(),
            caught_in_phase: caught_in_phase.clone(),
            via: via.clone(),
            headless,
            variant: variant.to_string(),
            repo_bucket: repo_bucket.to_string(),
            binary_sha: binary_sha.clone(),
        })
        .collect()
}

/// Detect + record any stated-rule violations from a completed drain. The
/// single entry point the orchestrator-telemetry path calls. No-op unless the
/// field study is enabled (shares the opt-in + privacy floor). Best-effort —
/// never blocks or delays the drain.
// trace:SPIKE-67 | ai:claude
#[allow(clippy::too_many_arguments)]
pub fn record(
    project_root: &Path,
    outcome: &DrainOutcome,
    spec_id: &str,
    headless: bool,
    variant: &str,
    binary_sha: Option<String>,
) {
    if !crate::field_study::is_enabled(Some(project_root)) {
        return;
    }
    // Cheap pre-check: skip the repo-size git call when there's nothing to log.
    if detect(outcome).is_empty() {
        return;
    }
    let bucket = crate::field_study::repo_bucket_for(repo_commit_count(project_root)).to_string();
    let ts = chrono::Utc::now().to_rfc3339();
    let events = build_events(
        outcome, &ts, spec_id, headless, variant, &bucket, binary_sha,
    );
    append_events(&events);
}

// trace:TASK-917 | ai:claude
/// Build the single `no-verify-bypass` event for a commit whose pre-commit hook
/// was bypassed. Pulled out from [`record_no_verify_bypass`] so event
/// construction is testable without the filesystem. The `spec_id` is the spec
/// inferred from the bypassed commit when one is unambiguous (an identifier
/// breadcrumb, never message content); `unknown` when none could be inferred.
/// `caught_in_phase` is `post-commit` (where the detector runs) and `via` is
/// `no-verify` (how it surfaced) — both stable categories, never message text.
pub fn build_no_verify_event(
    ts: &str,
    spec_id: &str,
    headless: bool,
    repo_bucket: &str,
    binary_sha: Option<String>,
) -> RuleViolation {
    RuleViolation {
        ts: ts.to_string(),
        spec_id: spec_id.to_string(),
        rule: RULE_NO_VERIFY_BYPASS.to_string(),
        caught_in_phase: "post-commit".to_string(),
        via: "no-verify".to_string(),
        headless,
        variant: "n/a".to_string(),
        repo_bucket: repo_bucket.to_string(),
        binary_sha,
    }
}

// trace:TASK-917 | ai:claude
/// Record a `--no-verify` bypass event. Called from the post-commit detector
/// once it has established (via the sentinel check) that the pre-commit hook was
/// skipped. Shares the field study's opt-in + privacy floor: a no-op unless the
/// study is enabled, and the event carries only identifiers/categories — never
/// the commit message, diff, or file paths. Best-effort — never blocks or fails
/// the (already-completed) commit.
pub fn record_no_verify_bypass(
    project_root: &Path,
    spec_id: &str,
    headless: bool,
    binary_sha: Option<String>,
) {
    if !crate::field_study::is_enabled(Some(project_root)) {
        return;
    }
    let bucket = crate::field_study::repo_bucket_for(repo_commit_count(project_root)).to_string();
    let ts = chrono::Utc::now().to_rfc3339();
    let event = build_no_verify_event(&ts, spec_id, headless, &bucket, binary_sha);
    append_events(std::slice::from_ref(&event));
}

/// HEAD commit count (the repo-size proxy). `0` on any error.
fn repo_commit_count(root: &Path) -> usize {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

/// Resolve `~/.aida/rule-violations.jsonl`. `None` when the home dir can't be
/// located (treat as "off" — never error out).
pub fn log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".aida").join("rule-violations.jsonl"))
}

/// Append events as JSONL. Best-effort: errors are swallowed — instrumentation
/// must never break or delay the drain.
pub fn append_events(events: &[RuleViolation]) {
    if events.is_empty() {
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
        for ev in events {
            if let Ok(json) = serde_json::to_string(ev) {
                let _ = writeln!(f, "{json}");
            }
        }
    }
}

/// Read every recorded violation (insertion order). Best-effort: malformed
/// lines are skipped.
pub fn read_events() -> Vec<RuleViolation> {
    let Some(path) = log_path() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<RuleViolation>(l).ok())
        .collect()
}

/// A `(rule, count)` tally, descending by count then rule name.
pub fn by_rule(events: &[RuleViolation]) -> Vec<(String, usize)> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for ev in events {
        *counts.entry(ev.rule.clone()).or_insert(0) += 1;
    }
    let mut rows: Vec<(String, usize)> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows
}

/// Split a tally of violations by the headless axis: `(headless, supervised)`.
/// The gate-vs-rule hypothesis predicts headless > supervised if context
/// pressure drives rule-dropping.
pub fn headless_split(events: &[RuleViolation]) -> (usize, usize) {
    let headless = events.iter().filter(|e| e.headless).count();
    (headless, events.len() - headless)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome<'a>(
        kind: Option<&'a str>,
        phase: Option<&'a str>,
        msg: Option<&'a str>,
        punt: Option<&'a str>,
    ) -> DrainOutcome<'a> {
        DrainOutcome {
            failed_phase_slug: phase,
            failure_kind: kind,
            failure_message: msg,
            punt_reason: punt,
        }
    }

    #[test]
    fn ci_red_on_fmt_is_detected() {
        let o = outcome(
            Some("ci-red"),
            Some("ci"),
            Some("cargo fmt --check found drift"),
            None,
        );
        assert_eq!(detect(&o), vec![RULE_FMT]);
    }

    #[test]
    fn ci_red_on_clippy_is_detected() {
        let o = outcome(
            Some("ci-red"),
            Some("ci"),
            Some("clippy::correctness lint failed"),
            None,
        );
        assert_eq!(detect(&o), vec![RULE_CLIPPY]);
    }

    #[test]
    fn ci_red_on_provenance_leak_is_detected() {
        // The recurring `/// SPEC-ID` doc-comment leak that needed BUG-624's gate.
        let o = outcome(
            Some("ci-red"),
            Some("ci"),
            Some("pre-commit blocked: /// trace:STORY-1 in a doc comment"),
            None,
        );
        assert_eq!(detect(&o), vec![RULE_PROVENANCE_LEAK]);
    }

    #[test]
    fn reviewer_request_changes_on_a_rule_is_detected() {
        let o = outcome(
            Some("no-verdict"),
            Some("reviewer"),
            Some("clippy warnings unaddressed"),
            None,
        );
        assert_eq!(detect(&o), vec![RULE_CLIPPY]);
    }

    #[test]
    fn punt_citing_a_rule_is_detected() {
        let o = outcome(
            None,
            None,
            None,
            Some("unsure whether advisor-code gate applies here"),
        );
        assert_eq!(detect(&o), vec![RULE_ADVISOR_CODE_GATE]);
    }

    #[test]
    fn ci_red_unrelated_to_a_stated_rule_yields_nothing() {
        // A genuine test failure is not a *stated-rule* violation — no false event.
        let o = outcome(
            Some("ci-red"),
            Some("ci"),
            Some("test_foo assertion failed"),
            None,
        );
        assert!(detect(&o).is_empty());
    }

    #[test]
    fn clean_run_yields_no_events() {
        let o = outcome(None, None, None, None);
        assert!(detect(&o).is_empty());
        assert!(build_events(
            &o,
            "2026-06-26T00:00:00Z",
            "TASK-1",
            false,
            "full",
            "large",
            None
        )
        .is_empty());
    }

    #[test]
    fn duplicate_rule_across_ci_and_punt_is_deduped() {
        let o = outcome(
            Some("ci-red"),
            Some("ci"),
            Some("cargo fmt drift"),
            Some("punting because fmt --check keeps failing"),
        );
        assert_eq!(detect(&o), vec![RULE_FMT]);
    }

    #[test]
    fn build_events_carries_context_and_no_message_text() {
        let o = outcome(
            Some("ci-red"),
            Some("ci"),
            Some("cargo fmt --check drift in some/secret/path.rs"),
            None,
        );
        let events = build_events(
            &o,
            "2026-06-26T12:00:00Z",
            "STORY-7",
            true,
            "full",
            "huge",
            Some("abc1234".into()),
        );
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.rule, RULE_FMT);
        assert_eq!(ev.caught_in_phase, "ci");
        assert_eq!(ev.via, "ci-red");
        assert!(ev.headless);
        assert_eq!(ev.repo_bucket, "huge");
        // Privacy floor: the message text / path must NOT leak into the event.
        let json = serde_json::to_string(ev).unwrap();
        assert!(!json.contains("secret"));
        assert!(!json.contains("path.rs"));
    }

    #[test]
    fn event_round_trips_through_json() {
        let ev = RuleViolation {
            ts: "2026-06-26T00:00:00Z".into(),
            spec_id: "TASK-1".into(),
            rule: RULE_FMT.into(),
            caught_in_phase: "ci".into(),
            via: "ci-red".into(),
            headless: false,
            variant: "full".into(),
            repo_bucket: "large".into(),
            binary_sha: Some("abc1234".into()),
        };
        let line = serde_json::to_string(&ev).unwrap();
        let parsed: RuleViolation = serde_json::from_str(&line).unwrap();
        assert_eq!(ev, parsed);
    }

    #[test]
    fn by_rule_ranks_descending_and_splits_headless() {
        let mk = |rule: &str, headless: bool| RuleViolation {
            ts: "t".into(),
            spec_id: "S".into(),
            rule: rule.into(),
            caught_in_phase: "ci".into(),
            via: "ci-red".into(),
            headless,
            variant: "full".into(),
            repo_bucket: "large".into(),
            binary_sha: None,
        };
        let events = vec![
            mk(RULE_FMT, true),
            mk(RULE_FMT, false),
            mk(RULE_CLIPPY, true),
        ];
        assert_eq!(
            by_rule(&events),
            vec![("fmt".into(), 2), ("clippy".into(), 1)]
        );
        assert_eq!(headless_split(&events), (2, 1));
    }

    #[test]
    fn no_verify_bypass_detected_when_sentinel_absent() {
        // The pre-commit hook left no matching sentinel → it never ran for this
        // commit → --no-verify was used.
        assert!(detect_no_verify_bypass(false));
        // Sentinel matched the committed tree → the hook ran → not a bypass.
        assert!(!detect_no_verify_bypass(true));
    }

    #[test]
    fn no_verify_event_carries_post_commit_context() {
        let ev = build_no_verify_event(
            "2026-06-26T12:00:00Z",
            "TASK-917",
            true,
            "huge",
            Some("abc1234".into()),
        );
        assert_eq!(ev.rule, RULE_NO_VERIFY_BYPASS);
        assert_eq!(ev.caught_in_phase, "post-commit");
        assert_eq!(ev.via, "no-verify");
        assert!(ev.headless);
        assert_eq!(ev.repo_bucket, "huge");
        assert_eq!(ev.spec_id, "TASK-917");
        // Privacy floor: the schema has no field for message text / paths, and
        // none of the categories we set leak content.
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"rule\":\"no-verify-bypass\""));
        assert!(json.contains("\"via\":\"no-verify\""));
    }

    #[test]
    fn no_verify_event_round_trips_and_tallies_with_other_rules() {
        let ev = build_no_verify_event("2026-06-26T00:00:00Z", "STORY-9", false, "large", None);
        let line = serde_json::to_string(&ev).unwrap();
        let parsed: RuleViolation = serde_json::from_str(&line).unwrap();
        assert_eq!(ev, parsed);
        // It tallies through the same by_rule surface `aida field-study
        // violations` reads, alongside the SPIKE-67 drain rules.
        let events = vec![ev];
        assert_eq!(by_rule(&events), vec![("no-verify-bypass".into(), 1)]);
    }

    #[test]
    fn log_path_ends_correctly() {
        if let Some(p) = log_path() {
            assert!(p.ends_with("rule-violations.jsonl"));
            assert!(p.to_string_lossy().contains(".aida"));
        }
    }
}
