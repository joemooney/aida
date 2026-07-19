//! End-of-command summary for a standalone `aida queue work <PR-N>
//! --role reviewer` invocation (BUG-226).
//!
//! # The problem
//!
//! A standalone `aida queue work PR-65 --role reviewer --no-human` ran a
//! headless reviewer to completion, posted a real review comment, and
//! **exited silently to the shell prompt** — no pass/fail, no cost, no
//! pointer to the artifacts. The user had to `jq` the JSONL log by hand to
//! learn the outcome. This is separate from the `--auto-complete`
//! orchestrator's phase-3 reviewer: that path has its own verdict-file →
//! merge handshake; the standalone path had no end-of-command summary at
//! all.
//!
//! # The fix
//!
//! The standalone reviewer launch spawns `claude` (instead of `exec`'ing
//! it) so `aida queue work` survives the launch, then prints a summary
//! assembled here from two artifacts:
//!
//!   - the verdict file `.aida/review-verdicts/PR-N.json` the `/aida-review`
//!     skill writes (verdict, one-line rationale, comment URL, mode);
//!   - the headless run's `stream-json` JSONL log (cost, turns, duration,
//!     `is_error`) — absent for an interactive review.
//!
//! Everything here is pure string/JSON munging so it unit-tests without
//! spawning `claude`.
//!
//! trace:BUG-226 | ai:claude

use std::path::Path;

/// The `{"type":"result",…}` event Claude Code's `--output-format
/// stream-json` emits as the final line of a headless run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultEvent {
    /// `true` when the headless run ended in an error (`error_max_turns`,
    /// `error_during_execution`, …).
    pub is_error: bool,
    /// Conversation turns the run took.
    pub num_turns: Option<u64>,
    /// Billed cost in USD.
    pub total_cost_usd: Option<f64>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: Option<u64>,
    /// `result` event subtype (`success`, `error_max_turns`, …).
    pub subtype: Option<String>,
    /// The `result` text — the assistant's final message, or an error
    /// description when `is_error`.
    pub result_text: Option<String>,
}

/// Parse the final `result` event out of a `stream-json` JSONL log.
///
/// The log is newline-delimited JSON; the run's tally is the *last* line
/// whose `type` is `result`. Scans from the end so a truncated/partial
/// trailing line can't mask a complete earlier one. Returns `None` when
/// no `result` event is present (run crashed before emitting one, or the
/// log is empty). trace:BUG-226 | ai:claude
pub fn parse_result_event(jsonl: &str) -> Option<ResultEvent> {
    for line in jsonl.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("result") {
            continue;
        }
        return Some(ResultEvent {
            is_error: v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false),
            num_turns: v.get("num_turns").and_then(|n| n.as_u64()),
            total_cost_usd: v.get("total_cost_usd").and_then(|n| n.as_f64()),
            duration_ms: v.get("duration_ms").and_then(|n| n.as_u64()),
            subtype: v
                .get("subtype")
                .and_then(|s| s.as_str())
                .map(str::to_string),
            result_text: v.get("result").and_then(|s| s.as_str()).map(str::to_string),
        });
    }
    None
}

/// The `merge` field value a reviewer writes into its verdict file when it
/// escalates the merge decision to a human rather than auto-deciding it.
/// The orchestrator's phase-3 handshake keys off this exact string.
/// trace:STORY-306 | ai:claude
pub const MERGE_ESCALATED_TO_HUMAN: &str = "escalated-to-human";

/// The `/aida-review` skill's view of the `.aida/review-verdicts/PR-N.json`
/// verdict file — the fields the standalone summary renders.
///
/// The on-disk file also carries a `mode` field (`standalone` |
/// `orchestrator-phase-3`) for *external* consumers that want to tell a
/// one-off review from an orchestrator handshake artifact; the summary
/// itself has no use for it (a standalone summary is always reading a
/// standalone file), so it is not deserialized here. serde ignores it.
/// The `verdict` field is the load-bearing one; the orchestrator's
/// `read_verdict_file` additionally honours `merge` (STORY-306).
/// trace:BUG-226 | ai:claude
#[derive(Debug, Clone, serde::Deserialize)]
pub struct VerdictFile {
    /// `Approved` | `RequestChanges` | `Rejected`.
    pub verdict: String,
    /// One-line rationale.
    #[serde(default)]
    pub summary: Option<String>,
    /// URL of the consolidated review comment posted on the PR.
    #[serde(default)]
    pub comment_url: Option<String>,
    /// IDs of follow-up TASKs a headless drain filed (STORY-278).
    #[serde(default)]
    pub findings_filed: Option<Vec<String>>,
    /// STORY-306: `escalated-to-human` when the reviewer escalated the *merge*
    /// decision to a human rather than auto-deciding it (uncertain zen
    /// provenance, an irreversible call). Absent — or any other value — means
    /// the reviewer reached a normal verdict. The `--auto-complete`
    /// orchestrator reads this to stop cleanly (exit `0`, no merge), leaving
    /// the PR for a person; the verdict file always exists so the phase-3
    /// handshake artifact is never missing. trace:STORY-306 | ai:claude
    #[serde(default)]
    pub merge: Option<String>,
    /// STORY-439: the diff-grounded complexity the reviewer assessed —
    /// `low` / `med` / `high`. Advisory only (not part of the
    /// PASS/CHANGES/FAIL decision); fuels the three-way calibration
    /// view. Absent on older verdict files and on interactive reviews
    /// where the reviewer skipped the field. trace:STORY-439 | ai:claude
    #[serde(default)]
    pub implementation_complexity: Option<String>,
    /// STORY-439: the reviewer's call on whether the implementer's
    /// ship-side complexity estimate matched the diff —
    /// `matched` / `implementer-underestimated` / `implementer-overestimated`.
    /// Absent when no ship-side estimate existed to compare against, or
    /// when the reviewer didn't volunteer one. trace:STORY-439 | ai:claude
    #[serde(default)]
    pub complexity_agreement: Option<String>,
    /// STORY-451: reviewer's effort estimate from the observed diff.
    /// Advisory only; captured into `.aida/effort-calibration/<SPEC>.yaml`
    /// as the review touchpoint. Buckets: `15m`, `1h`, `4h`, `1d`, `1w`.
    /// trace:STORY-451 | ai:codex
    #[serde(default)]
    pub implementation_effort: Option<String>,
}

/// Parse a verdict file's JSON. `None` on absent/unreadable/malformed —
/// the summary then prints `verdict: UNKNOWN (no verdict file)`.
pub fn parse_verdict_file(json: &str) -> Option<VerdictFile> {
    serde_json::from_str(json).ok()
}

/// Map a verdict-file `verdict` string to the PASS / CHANGES REQUESTED /
/// FAIL vocabulary the per-spec checklist uses. Case-tolerant. Returns
/// `None` for an unrecognised verdict so the caller can surface the raw
/// string. trace:BUG-226 | ai:claude
fn verdict_label(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "approved" | "approve" | "pass" => Some("PASS"),
        "requestchanges" | "request_changes" | "request-changes" | "changes" | "partial" => {
            Some("CHANGES REQUESTED")
        }
        "rejected" | "reject" | "fail" => Some("FAIL"),
        _ => None,
    }
}

/// Render a millisecond duration compactly: `38s`, `4m38s`, `1h04m38s`.
fn fmt_duration(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

/// First line of `s`, trimmed and truncated to `max` chars (with an
/// ellipsis when truncated). Keeps a verbose error `result` text to one
/// readable line in the summary.
fn first_line_trunc(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or("").trim();
    if first.chars().count() > max {
        let head: String = first.chars().take(max).collect();
        format!("{head}…")
    } else {
        first.to_string()
    }
}

/// Build the `4m38s, 25 turns, $1.45` metrics parenthetical from a result
/// event. `None` when there is no result event (interactive review) or it
/// carried none of the three fields.
fn metrics_line(result: Option<&ResultEvent>) -> Option<String> {
    let r = result?;
    let mut parts = Vec::new();
    if let Some(d) = r.duration_ms {
        parts.push(fmt_duration(d));
    }
    if let Some(t) = r.num_turns {
        parts.push(format!("{t} turns"));
    }
    if let Some(c) = r.total_cost_usd {
        parts.push(format!("${c:.2}"));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// Assemble the end-of-command summary for a standalone reviewer run.
///
/// - `pr` — the reviewed PR number (for the recovery hint).
/// - `verdict_json` — contents of the verdict file, `None` if absent.
/// - `result` — the parsed headless `result` event, `None` for an
///   interactive review or a missing log.
/// - `verdict_path` / `log_path` — artifact locations, surfaced as
///   pointers (`log_path` is `None` for an interactive review).
/// - `exit_code` — `claude`'s exit code; a non-zero value flips the
///   summary to the failure shape even without an `is_error` result event.
///
/// trace:BUG-226 | ai:claude
pub fn format_reviewer_summary(
    pr: u64,
    verdict_json: Option<&str>,
    result: Option<&ResultEvent>,
    verdict_path: &Path,
    log_path: Option<&Path>,
    exit_code: Option<i32>,
) -> String {
    let verdict = verdict_json.and_then(parse_verdict_file);
    let metrics = metrics_line(result);
    // A run failed if the result event says so, or `claude` exited non-zero.
    let failed =
        result.map(|r| r.is_error).unwrap_or(false) || exit_code.map(|c| c != 0).unwrap_or(false);

    // trace:TASK-840 | ai:claude — route the verdict markers through the
    // registry (resolve the profile once). Default Unicode reproduces the
    // historical check/cross literals byte-for-byte.
    let profile = crate::glyphs::active_profile(crate::find_project_root().ok().as_deref());
    let check = crate::glyphs::Glyph::Check.render(profile);
    let cross = crate::glyphs::Glyph::Cross.render(profile);

    let mut out = String::new();
    if failed {
        match &metrics {
            Some(m) => out.push_str(&format!("{cross} review failed ({m})\n")),
            None => out.push_str(&format!("{cross} review failed\n")),
        }
        // Failure summary: same shape, error reason replaces the verdict.
        let reason = result
            .and_then(|r| {
                r.result_text
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| first_line_trunc(s, 200))
                    .or_else(|| r.subtype.clone())
            })
            .or_else(|| exit_code.map(|c| format!("claude exited with code {c}")))
            .unwrap_or_else(|| "unknown error (no result event)".to_string());
        out.push_str(&format!("  error: {reason}\n"));
    } else {
        match &metrics {
            Some(m) => out.push_str(&format!("{check} review complete ({m})\n")),
            None => out.push_str(&format!("{check} review complete\n")),
        }
        match &verdict {
            Some(v) => {
                let shown = match verdict_label(&v.verdict) {
                    Some(label) => label.to_string(),
                    None => format!("UNKNOWN ({})", v.verdict),
                };
                match v
                    .summary
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    Some(s) => out.push_str(&format!("  verdict: {shown} — {s}\n")),
                    None => out.push_str(&format!("  verdict: {shown}\n")),
                }
            }
            None => {
                // Skill regressed — it produced no verdict file. Say so,
                // and point at how to recover. trace:BUG-226
                out.push_str("  verdict: UNKNOWN (no verdict file)\n");
                out.push_str(&format!(
                    "    /aida-review wrote no verdict to {} — re-run: aida queue work PR-{pr} --role reviewer\n",
                    verdict_path.display(),
                ));
            }
        }
    }

    // STORY-306: a standalone reviewer that escalated the merge decision to
    // a human — surface it so the one-off review's escalation isn't silent.
    if verdict
        .as_ref()
        .and_then(|v| v.merge.as_deref())
        .map(str::trim)
        == Some(MERGE_ESCALATED_TO_HUMAN)
    {
        out.push_str("  merge: escalated to a human — left unmerged for a person to decide\n");
    }

    // STORY-439: surface the reviewer's diff-grounded complexity assessment
    // and (when volunteered) their agreement call against the implementer's
    // ship-side estimate. Advisory only — these feed the three-way
    // calibration view, not the PASS/CHANGES/FAIL decision.
    if let Some(level) = verdict
        .as_ref()
        .and_then(|v| v.implementation_complexity.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("  implementation complexity: {level}\n"));
    }
    if let Some(agreement) = verdict
        .as_ref()
        .and_then(|v| v.complexity_agreement.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("  complexity agreement: {agreement}\n"));
    }
    if let Some(effort) = verdict
        .as_ref()
        .and_then(|v| v.implementation_effort.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("  implementation effort: {effort}\n"));
    }

    // Comment URL — when the verdict file recorded one.
    if let Some(url) = verdict
        .as_ref()
        .and_then(|v| v.comment_url.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("  comment: {url}\n"));
    }
    // Follow-up TASKs a headless drain filed (STORY-278) — surface them
    // so a one-off review's findings aren't lost in the JSONL log.
    if let Some(filed) = verdict
        .as_ref()
        .and_then(|v| v.findings_filed.as_deref())
        .filter(|f| !f.is_empty())
    {
        out.push_str(&format!("  findings filed: {}\n", filed.join(", ")));
    }
    // Verdict file pointer — only when the file actually landed.
    if verdict.is_some() {
        out.push_str(&format!("  verdict file: {}\n", verdict_path.display()));
    }
    // JSONL log pointer — headless runs only.
    if let Some(lp) = log_path {
        out.push_str(&format!("  JSONL log: {}\n", lp.display()));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A representative final `result` line from a `stream-json` log.
    const RESULT_LINE: &str = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":278000,"num_turns":25,"total_cost_usd":1.4523,"result":"Verdict: PASS","session_id":"abc"}"#;

    fn vpath() -> PathBuf {
        PathBuf::from(".aida/review-verdicts/PR-65.json")
    }
    fn lpath() -> PathBuf {
        PathBuf::from(".aida/headless-logs/pr-65-abc.jsonl")
    }

    #[test]
    fn parse_result_event_extracts_the_tally() {
        let log = format!(
            "{}\n{}\n",
            r#"{"type":"assistant","message":{}}"#, RESULT_LINE
        );
        let ev = parse_result_event(&log).expect("result event present");
        assert!(!ev.is_error);
        assert_eq!(ev.num_turns, Some(25));
        assert_eq!(ev.duration_ms, Some(278000));
        assert_eq!(ev.total_cost_usd, Some(1.4523));
    }

    #[test]
    fn parse_result_event_none_when_absent() {
        let log = "{\"type\":\"assistant\"}\n{\"type\":\"user\"}\n";
        assert!(parse_result_event(log).is_none());
        assert!(parse_result_event("").is_none());
    }

    #[test]
    fn parse_result_event_skips_trailing_garbage() {
        // A truncated final line must not mask the real result above it.
        let log = format!("{RESULT_LINE}\n{{\"type\":\"resul");
        assert!(parse_result_event(&log).is_some());
    }

    #[test]
    fn summary_pass_has_metrics_verdict_and_pointers() {
        let verdict = r#"{"verdict":"Approved","summary":"all 6 specs pass","comment_url":"https://github.com/x/pull/65#issuecomment-1","mode":"standalone"}"#;
        let ev = parse_result_event(RESULT_LINE).unwrap();
        let s = format_reviewer_summary(
            65,
            Some(verdict),
            Some(&ev),
            &vpath(),
            Some(&lpath()),
            Some(0),
        );
        // trace:TASK-840 | ai:claude — expected marker via the registry.
        let check = crate::glyphs::Glyph::Check.render(crate::glyphs::active_profile(None));
        assert!(
            s.contains(&format!("{check} review complete (4m38s, 25 turns, $1.45)")),
            "{s}"
        );
        assert!(s.contains("verdict: PASS — all 6 specs pass"), "{s}");
        assert!(s.contains("comment: https://github.com/x/pull/65"), "{s}");
        assert!(
            s.contains("verdict file: .aida/review-verdicts/PR-65.json"),
            "{s}"
        );
        assert!(
            s.contains("JSONL log: .aida/headless-logs/pr-65-abc.jsonl"),
            "{s}"
        );
    }

    #[test]
    fn summary_surfaces_findings_filed() {
        let verdict = r#"{"verdict":"Approved","findings_filed":["TASK-303","TASK-304"]}"#;
        let s = format_reviewer_summary(65, Some(verdict), None, &vpath(), None, Some(0));
        assert!(s.contains("findings filed: TASK-303, TASK-304"), "{s}");
        // An empty array prints no line.
        let none = format_reviewer_summary(
            65,
            Some(r#"{"verdict":"Approved","findings_filed":[]}"#),
            None,
            &vpath(),
            None,
            Some(0),
        );
        assert!(!none.contains("findings filed:"), "{none}");
    }

    #[test]
    fn summary_surfaces_merge_escalation() {
        // STORY-306: a standalone reviewer that escalated the merge decision.
        let verdict = r#"{"verdict":"Approved","merge":"escalated-to-human","summary":"irreversible migration"}"#;
        let s = format_reviewer_summary(65, Some(verdict), None, &vpath(), None, Some(0));
        assert!(s.contains("verdict: PASS"), "{s}");
        assert!(s.contains("merge: escalated to a human"), "{s}");
        // A normal verdict file shows no merge line.
        let normal = format_reviewer_summary(
            65,
            Some(r#"{"verdict":"Approved"}"#),
            None,
            &vpath(),
            None,
            Some(0),
        );
        assert!(!normal.contains("merge:"), "{normal}");
    }

    #[test]
    fn summary_maps_request_changes_and_rejected() {
        let changes = format_reviewer_summary(
            65,
            Some(r#"{"verdict":"RequestChanges"}"#),
            None,
            &vpath(),
            None,
            Some(0),
        );
        assert!(changes.contains("verdict: CHANGES REQUESTED"), "{changes}");
        let rejected = format_reviewer_summary(
            65,
            Some(r#"{"verdict":"Rejected"}"#),
            None,
            &vpath(),
            None,
            Some(0),
        );
        assert!(rejected.contains("verdict: FAIL"), "{rejected}");
    }

    #[test]
    fn summary_unknown_when_no_verdict_file() {
        let s = format_reviewer_summary(65, None, None, &vpath(), None, Some(0));
        assert!(s.contains("verdict: UNKNOWN (no verdict file)"), "{s}");
        // Recovery hint names the file and the re-run command.
        assert!(s.contains("aida queue work PR-65 --role reviewer"), "{s}");
        // No verdict file → no `verdict file:` pointer line.
        assert!(!s.contains("verdict file:"), "{s}");
    }

    #[test]
    fn summary_interactive_omits_metrics_and_jsonl() {
        // No result event, no log path — an interactive standalone review.
        let s = format_reviewer_summary(
            65,
            Some(r#"{"verdict":"Approved"}"#),
            None,
            &vpath(),
            None,
            Some(0),
        );
        let check = crate::glyphs::Glyph::Check.render(crate::glyphs::active_profile(None));
        assert!(s.starts_with(&format!("{check} review complete\n")), "{s}");
        assert!(!s.contains("JSONL log:"), "{s}");
        assert!(s.contains("verdict: PASS"), "{s}");
    }

    #[test]
    fn summary_failure_on_is_error() {
        let err_line = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"duration_ms":130000,"num_turns":8,"total_cost_usd":0.40,"result":"hit the turn limit before posting a verdict"}"#;
        let ev = parse_result_event(err_line).unwrap();
        let s = format_reviewer_summary(65, None, Some(&ev), &vpath(), Some(&lpath()), Some(0));
        let cross = crate::glyphs::Glyph::Cross.render(crate::glyphs::active_profile(None));
        assert!(
            s.contains(&format!("{cross} review failed (2m10s, 8 turns, $0.40)")),
            "{s}"
        );
        assert!(s.contains("error: hit the turn limit"), "{s}");
        // JSONL log still surfaces so the failure is debuggable.
        assert!(s.contains("JSONL log:"), "{s}");
    }

    #[test]
    fn summary_failure_on_nonzero_exit() {
        // No result event at all, but `claude` exited non-zero.
        let s = format_reviewer_summary(65, None, None, &vpath(), Some(&lpath()), Some(2));
        let cross = crate::glyphs::Glyph::Cross.render(crate::glyphs::active_profile(None));
        assert!(s.contains(&format!("{cross} review failed")), "{s}");
        assert!(s.contains("claude exited with code 2"), "{s}");
    }

    #[test]
    fn fmt_duration_spans_units() {
        assert_eq!(fmt_duration(38_000), "38s");
        assert_eq!(fmt_duration(278_000), "4m38s");
        assert_eq!(fmt_duration(3_900_000), "1h05m00s");
    }

    #[test]
    fn verdict_file_parses_implementation_complexity_field() {
        let body = r#"{
            "verdict": "Approved",
            "implementation_complexity": "high",
            "complexity_agreement": "implementer-underestimated",
            "implementation_effort": "1d"
        }"#;
        let v = parse_verdict_file(body).expect("parses");
        assert_eq!(v.implementation_complexity.as_deref(), Some("high"));
        assert_eq!(
            v.complexity_agreement.as_deref(),
            Some("implementer-underestimated")
        );
        assert_eq!(v.implementation_effort.as_deref(), Some("1d"));
    }

    #[test]
    fn verdict_file_back_compat_no_complexity_fields() {
        // A verdict file written before STORY-439 parses cleanly with both
        // new fields defaulting to `None`.
        let body = r#"{"verdict":"Approved","summary":"all green"}"#;
        let v = parse_verdict_file(body).expect("parses");
        assert!(v.implementation_complexity.is_none());
        assert!(v.complexity_agreement.is_none());
    }

    #[test]
    fn summary_surfaces_implementation_complexity_and_agreement() {
        let verdict = r#"{
            "verdict":"Approved",
            "summary":"smoke",
            "implementation_complexity":"high",
            "complexity_agreement":"implementer-underestimated",
            "implementation_effort":"1d"
        }"#;
        let s = format_reviewer_summary(65, Some(verdict), None, &vpath(), None, Some(0));
        assert!(s.contains("verdict: PASS"), "{s}");
        assert!(
            s.contains("implementation complexity: high"),
            "missing complexity line: {s}"
        );
        assert!(
            s.contains("complexity agreement: implementer-underestimated"),
            "missing agreement line: {s}"
        );
        assert!(
            s.contains("implementation effort: 1d"),
            "missing effort line: {s}"
        );
    }

    #[test]
    fn summary_omits_complexity_lines_when_unset() {
        let s = format_reviewer_summary(
            65,
            Some(r#"{"verdict":"Approved"}"#),
            None,
            &vpath(),
            None,
            Some(0),
        );
        assert!(!s.contains("implementation complexity:"), "{s}");
        assert!(!s.contains("complexity agreement:"), "{s}");
        assert!(!s.contains("implementation effort:"), "{s}");
    }
}
