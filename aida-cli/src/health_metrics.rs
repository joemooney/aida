//! EPIC-36 MVP: the session-vs-drain misclassification-gap metric.
//!
//! The orchestrator records a drain as *failed* whenever a lifecycle phase
//! returns a non-success verdict (CI red, RequestChanges, …). But the
//! headless Claude session that drove that phase may itself have ended
//! cleanly — it did its job, the orchestrator just classified the surrounding
//! lifecycle outcome as a failure. The reverse also happens: a session that
//! was killed mid-work, hit `error_max_turns`, or whose log was truncated is
//! a genuine session-level failure regardless of how the drain was scored.
//!
//! This module derives two rates and the gap between them:
//!
//!   - `session_success_rate` — over the terminal `result` event of each
//!     `.aida/headless-logs/*.jsonl` file. A session is a success when it
//!     ended clean (`result.subtype == "success"`, no error) or was reaped by
//!     the sentinel after finishing its work (success). Mid-work-kill, error
//!     subtypes, and truncated/absent result events are failures.
//!   - `drain_success_rate` — already computed by the existing
//!     `aida usage --auto-complete` path over `~/.aida/auto-complete.jsonl`;
//!     this module takes it as an input rather than re-deriving it.
//!
//! GAP = session_success_rate − drain_success_rate. A positive gap is the
//! *orchestrator misclassification rate*: the fraction of work the session
//! actually completed but the drain recorded as a failure. A negative gap
//! means sessions are failing in ways the drain scoring doesn't catch.
//!
//! Everything here is a PURE function over the raw log lines + the supplied
//! drain rate, so the classification and the gap arithmetic are unit-testable
//! against synthetic JSONL without touching the filesystem.
//!
//! trace:EPIC-36 | ai:claude

use serde_json::Value;
use std::path::Path;

/// Read and classify every `.aida/headless-logs/*.jsonl` under `dir` into a
/// `SessionTally`. This is the one I/O boundary in the module; the
/// classification + aggregation it delegates to are pure. Best-effort: an
/// unreadable file is skipped (a session whose log we can't read tells us
/// nothing). Returns an empty tally (total 0) when the directory is absent.
/// trace:EPIC-36
pub fn tally_from_dir(dir: &Path) -> SessionTally {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return SessionTally::default();
    };
    let mut bodies: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if !entry.metadata().map(|m| m.is_file()).unwrap_or(false) {
            continue;
        }
        if let Ok(body) = std::fs::read_to_string(&path) {
            bodies.push(body);
        }
    }
    tally_sessions(bodies)
}

// trace:EPIC-36 — the five mutually-exclusive session outcome classes.
/// How one headless session ended, derived from its terminal `result` event
/// (or the absence of one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOutcome {
    /// `result.subtype == "success"` and not `is_error` — the session ran to
    /// a clean finish.
    CleanSuccess,
    /// The session was reaped by the sentinel after completing its work. The
    /// orchestrator's sentinel terminates a session once the phase's success
    /// artifact is present; the session did its job, so this counts as a
    /// success.
    SentinelReaped,
    /// The session was killed before finishing (no terminal result event but
    /// an explicit kill/abort marker is present in the stream).
    MidWorkKill,
    /// A terminal result event with `is_error` / an error subtype
    /// (`error_max_turns`, `error_during_execution`, …).
    Error,
    /// No parseable terminal result event and no kill marker — the log was
    /// truncated or the run crashed before emitting a result.
    Truncated,
}

impl SessionOutcome {
    /// `true` for the two outcome classes that count toward the success rate.
    pub fn is_success(self) -> bool {
        matches!(
            self,
            SessionOutcome::CleanSuccess | SessionOutcome::SentinelReaped
        )
    }

    /// Stable lowercase slug for JSON / display.
    pub fn slug(self) -> &'static str {
        match self {
            SessionOutcome::CleanSuccess => "clean-success",
            SessionOutcome::SentinelReaped => "sentinel-reaped",
            SessionOutcome::MidWorkKill => "mid-work-kill",
            SessionOutcome::Error => "error",
            SessionOutcome::Truncated => "truncated",
        }
    }
}

/// Classify one headless session log (the full JSONL body of a single
/// `.aida/headless-logs/*.jsonl` file) into a `SessionOutcome`.
///
/// Pure: takes the log text, returns the outcome. Scans from the end so a
/// truncated/partial trailing line can't mask a complete earlier `result`
/// event (mirrors `reviewer_summary::parse_result_event`).
///
/// Classification order:
///   1. A terminal `result` event → `CleanSuccess` (subtype `success`, not
///      error) or `Error` (is_error / any error subtype).
///   2. No result event, but a sentinel-reap marker → `SentinelReaped`.
///   3. No result event, but a kill/abort marker → `MidWorkKill`.
///   4. Otherwise → `Truncated`.
///
/// trace:EPIC-36
pub fn classify_session(jsonl: &str) -> SessionOutcome {
    let mut saw_sentinel_reap = false;
    let mut saw_kill = false;

    for line in jsonl.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        // A terminal result event is authoritative — return immediately.
        if typ == "result" {
            let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false);
            let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
            if is_error || (subtype != "success" && !subtype.is_empty()) {
                return SessionOutcome::Error;
            }
            return SessionOutcome::CleanSuccess;
        }

        // Non-result markers we remember in case there's no result event.
        if is_sentinel_reap_marker(&v) {
            saw_sentinel_reap = true;
        }
        if is_kill_marker(&v) {
            saw_kill = true;
        }
    }

    // No terminal result event — fall back to the markers we collected.
    if saw_sentinel_reap {
        SessionOutcome::SentinelReaped
    } else if saw_kill {
        SessionOutcome::MidWorkKill
    } else {
        SessionOutcome::Truncated
    }
}

/// `true` when an event marks the sentinel reaping the session after its work
/// landed. The orchestrator writes a terminal marker line when it terminates a
/// session whose success artifact is already present; we recognise both an
/// explicit `type == "aida_sentinel_reap"` event and a generic event carrying
/// `reason == "sentinel-reaped"` / `aida_sentinel: "reaped"`. trace:EPIC-36
fn is_sentinel_reap_marker(v: &Value) -> bool {
    if v.get("type").and_then(|t| t.as_str()) == Some("aida_sentinel_reap") {
        return true;
    }
    if v.get("aida_sentinel").and_then(|s| s.as_str()) == Some("reaped") {
        return true;
    }
    matches!(
        v.get("reason").and_then(|s| s.as_str()),
        Some("sentinel-reaped") | Some("sentinel_reaped")
    )
}

/// `true` when an event marks the session being killed before it finished.
/// Recognises an explicit `type == "aida_session_kill"` event and a generic
/// event carrying `aida_kill: true` or `reason == "killed"` / `"aborted"`.
/// trace:EPIC-36
fn is_kill_marker(v: &Value) -> bool {
    if v.get("type").and_then(|t| t.as_str()) == Some("aida_session_kill") {
        return true;
    }
    if v.get("aida_kill").and_then(|b| b.as_bool()) == Some(true) {
        return true;
    }
    matches!(
        v.get("reason").and_then(|s| s.as_str()),
        Some("killed") | Some("aborted") | Some("mid-work-kill")
    )
}

/// Tallies over a set of classified sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionTally {
    pub total: usize,
    pub clean_success: usize,
    pub sentinel_reaped: usize,
    pub mid_work_kill: usize,
    pub error: usize,
    pub truncated: usize,
}

impl SessionTally {
    /// Count of sessions that count as a success (clean + sentinel-reaped).
    pub fn successes(&self) -> usize {
        self.clean_success + self.sentinel_reaped
    }

    /// Fraction of sessions that succeeded, in `0.0..=1.0`. `0.0` when there
    /// are no sessions (zero-denominator edge case). trace:EPIC-36
    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.successes() as f64 / self.total as f64
        }
    }

    /// Per-class breakdown in a stable order — each outcome class paired with
    /// its count. Drives the JSON `breakdown` surface (keyed by `slug`) and any
    /// caller wanting the full distribution rather than just the rate.
    /// trace:EPIC-36
    pub fn breakdown(&self) -> [(SessionOutcome, usize); 5] {
        [
            (SessionOutcome::CleanSuccess, self.clean_success),
            (SessionOutcome::SentinelReaped, self.sentinel_reaped),
            (SessionOutcome::MidWorkKill, self.mid_work_kill),
            (SessionOutcome::Error, self.error),
            (SessionOutcome::Truncated, self.truncated),
        ]
    }

    /// Record one classified outcome.
    pub fn record(&mut self, outcome: SessionOutcome) {
        self.total += 1;
        match outcome {
            SessionOutcome::CleanSuccess => self.clean_success += 1,
            SessionOutcome::SentinelReaped => self.sentinel_reaped += 1,
            SessionOutcome::MidWorkKill => self.mid_work_kill += 1,
            SessionOutcome::Error => self.error += 1,
            SessionOutcome::Truncated => self.truncated += 1,
        }
    }
}

/// Aggregate a collection of per-session log bodies into a `SessionTally`.
///
/// Pure over the supplied log texts — the caller is responsible for reading
/// the files (so the aggregation stays filesystem-free and testable).
/// trace:EPIC-36
pub fn tally_sessions<I, S>(logs: I) -> SessionTally
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut tally = SessionTally::default();
    for body in logs {
        tally.record(classify_session(body.as_ref()));
    }
    tally
}

/// The session-vs-drain misclassification gap. trace:EPIC-36
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MisclassificationGap {
    /// Successes / total over the headless session logs.
    pub session_success_rate: f64,
    /// Successes / total over the `--auto-complete` drain log (supplied by the
    /// existing usage data path).
    pub drain_success_rate: f64,
    /// Number of sessions the session rate is computed over.
    pub session_total: usize,
    /// Number of drain runs the drain rate is computed over.
    pub drain_total: usize,
}

impl MisclassificationGap {
    /// GAP = session_success_rate − drain_success_rate. A positive value is
    /// the orchestrator misclassification rate (work that succeeded at the
    /// session level but was scored as a drain failure). trace:EPIC-36
    pub fn gap(&self) -> f64 {
        self.session_success_rate - self.drain_success_rate
    }

    /// `true` when either rate has a zero denominator — the gap is then not
    /// meaningful and callers should present it as "insufficient data".
    pub fn has_zero_denominator(&self) -> bool {
        self.session_total == 0 || self.drain_total == 0
    }
}

/// Compose the gap metric from a session tally + the already-computed drain
/// summary. Pure arithmetic so the gap computation (including the
/// zero-denominator edge case) is unit-testable. trace:EPIC-36
pub fn compute_gap(
    sessions: &SessionTally,
    drain_success_rate: f64,
    drain_total: usize,
) -> MisclassificationGap {
    MisclassificationGap {
        session_success_rate: sessions.success_rate(),
        drain_success_rate,
        session_total: sessions.total,
        drain_total,
    }
}

// ============================================================================
// STORY-530 — the remaining Tier-1 deterministic health metrics.
//
// Every function below is PURE over plain inputs extracted from the substrate
// (the `--auto-complete` telemetry log + the spec graph). The thin I/O that
// reads those sources stays in the caller (`main.rs`) so the arithmetic and
// the edge cases (zero denominators, empty windows) are unit-testable against
// synthetic data without touching the filesystem or the store.
// trace:STORY-530 | ai:claude
// ============================================================================

/// Drain halt-rate (#3): the EPIC-28 resilient drain *parks-and-continues* on a
/// shelvable phase failure (CI red, RequestChanges, build fail) but *halts* the
/// whole batch on a non-shelvable environment failure (`spawn`, `missing-tool`,
/// `internal`). This breakdown counts, over the drain failures, how many were
/// shelve-and-continue vs how many would halt the batch — the signal for how
/// often a drain stops dead vs degrades gracefully. trace:STORY-530
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HaltBreakdown {
    /// Failures whose `failure_kind` shelves (parks the spec, drain continues).
    pub shelved: usize,
    /// Failures whose `failure_kind` halts the batch (broken environment).
    pub halted: usize,
    /// Failures with an unknown / absent `failure_kind` slug — conservatively
    /// neither shelved nor halted, surfaced so the breakdown stays honest.
    pub unclassified: usize,
}

impl HaltBreakdown {
    /// Total classified-or-not failures the breakdown is computed over.
    pub fn total(&self) -> usize {
        self.shelved + self.halted + self.unclassified
    }

    /// Fraction of failures that halted the batch, in `0.0..=1.0`. `0.0` when
    /// there are no failures (zero-denominator edge). The denominator is
    /// shelved + halted (classified failures); unclassified rows are excluded
    /// so an unknown slug neither inflates nor deflates the rate.
    /// trace:STORY-530
    pub fn halt_rate(&self) -> f64 {
        let classified = self.shelved + self.halted;
        if classified == 0 {
            0.0
        } else {
            self.halted as f64 / classified as f64
        }
    }
}

/// `true` when a `failure_kind` slug is a *shelvable* (park-and-continue) kind.
/// Mirrors `auto_complete::FailureKind::is_shelvable` over the stable slugs so
/// this module stays free of the orchestrator's internal types. trace:STORY-530
pub fn failure_kind_is_shelvable(slug: &str) -> bool {
    matches!(
        slug,
        "no-pr"
            | "ci-red"
            | "ci-timeout"
            | "no-verdict"
            | "pr-verification-inconclusive"
            | "no-progress-watchdog"
            | "cache-locked"
            | "failed"
    )
}

/// `true` when a `failure_kind` slug is a *non-shelvable* (batch-halting)
/// environment failure. trace:STORY-530
pub fn failure_kind_is_halting(slug: &str) -> bool {
    matches!(slug, "spawn" | "missing-tool" | "internal")
}

/// Classify a slice of drain-failure `failure_kind` slugs into a
/// `HaltBreakdown`. A `None` slug (a failure with no recorded kind) is
/// `unclassified`. Pure over the slug list. trace:STORY-530
pub fn halt_breakdown<I, S>(failure_kinds: I) -> HaltBreakdown
where
    I: IntoIterator<Item = Option<S>>,
    S: AsRef<str>,
{
    let mut b = HaltBreakdown::default();
    for kind in failure_kinds {
        match kind.as_ref().map(|s| s.as_ref()) {
            Some(slug) if failure_kind_is_shelvable(slug) => b.shelved += 1,
            Some(slug) if failure_kind_is_halting(slug) => b.halted += 1,
            _ => b.unclassified += 1,
        }
    }
    b
}

/// Recovery latency (#4): the wall-clock gap between a drain *failure* and the
/// *next* drain run — the human-babysitting cost (how long work sat parked
/// before someone kicked off the next drain). trace:STORY-530
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecoveryLatency {
    /// One gap per failure that was followed by a later drain, in seconds.
    pub gaps_secs: Vec<i64>,
}

impl RecoveryLatency {
    /// Number of recovery gaps measured (failures that had a following drain).
    pub fn count(&self) -> usize {
        self.gaps_secs.len()
    }

    /// Mean gap in seconds, or `None` when no gaps were measured.
    /// trace:STORY-530
    pub fn mean_secs(&self) -> Option<f64> {
        if self.gaps_secs.is_empty() {
            None
        } else {
            let sum: i64 = self.gaps_secs.iter().sum();
            Some(sum as f64 / self.gaps_secs.len() as f64)
        }
    }

    /// Median gap in seconds, or `None` when no gaps were measured.
    /// trace:STORY-530
    pub fn median_secs(&self) -> Option<f64> {
        if self.gaps_secs.is_empty() {
            return None;
        }
        let mut sorted = self.gaps_secs.clone();
        sorted.sort_unstable();
        let n = sorted.len();
        Some(if n % 2 == 1 {
            sorted[n / 2] as f64
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) as f64 / 2.0
        })
    }

    /// Largest gap in seconds, or `None` when no gaps were measured.
    pub fn max_secs(&self) -> Option<i64> {
        self.gaps_secs.iter().copied().max()
    }
}

/// One drain run reduced to the two timestamps recovery-latency needs, as epoch
/// seconds, plus whether it failed. The caller parses the RFC3339 strings from
/// the telemetry log; this struct keeps the latency computation pure.
/// trace:STORY-530
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrainRun {
    /// When the run started (epoch seconds).
    pub started_at: i64,
    /// When the run finished (epoch seconds).
    pub completed_at: i64,
    /// Whether the run was a failure.
    pub failed: bool,
}

/// Compute recovery latencies from a set of drain runs. Sorts by completion
/// time, then for each *failed* run measures the gap to the *next run that
/// started after this one completed* (the next drain a human kicked off). A
/// failure with no following drain contributes no gap. Pure over the supplied
/// runs. trace:STORY-530
pub fn recovery_latency(runs: &[DrainRun]) -> RecoveryLatency {
    // Order by completion so "the next drain" is well-defined.
    let mut ordered: Vec<DrainRun> = runs.to_vec();
    ordered.sort_by_key(|r| r.completed_at);
    let mut latency = RecoveryLatency::default();
    for (i, run) in ordered.iter().enumerate() {
        if !run.failed {
            continue;
        }
        // The next drain that *started* at or after this failure completed.
        if let Some(next) = ordered
            .iter()
            .skip(i + 1)
            .find(|r| r.started_at >= run.completed_at)
        {
            let gap = next.started_at - run.completed_at;
            if gap >= 0 {
                latency.gaps_secs.push(gap);
            }
        }
    }
    latency
}

/// Draft-inbox depth (#5, ADR-3): the count of untriaged Draft specs awaiting
/// the advisor's approve/reject decision. A high number is unreviewed backlog
/// piling up. Pure over `(is_draft, is_archived)` flags; archived drafts are
/// excluded (they are no longer in the inbox). trace:STORY-530
pub fn draft_inbox_depth<I>(specs: I) -> usize
where
    I: IntoIterator<Item = (bool, bool)>,
{
    specs
        .into_iter()
        .filter(|(is_draft, is_archived)| *is_draft && !*is_archived)
        .count()
}

/// Burn-down velocity (#6): net completions per day — specs that *reached
/// Completed* minus specs that were *newly added* on the same day. A positive
/// net means the backlog is shrinking; a negative net means the advisor is
/// adding work faster than it ships. trace:STORY-530
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BurnDownVelocity {
    /// Specs that reached Completed in the window.
    pub completed: usize,
    /// Specs newly created in the window.
    pub added: usize,
    /// Number of distinct days the window spans (>= 1 when any event lands).
    pub days: usize,
}

impl BurnDownVelocity {
    /// Net change in backlog over the window (completed − added). Negative when
    /// more was added than shipped.
    pub fn net(&self) -> i64 {
        self.completed as i64 - self.added as i64
    }

    /// Net completions per day, or `None` when the window spans no days
    /// (zero-denominator edge). trace:STORY-530
    pub fn net_per_day(&self) -> Option<f64> {
        if self.days == 0 {
            None
        } else {
            Some(self.net() as f64 / self.days as f64)
        }
    }
}

/// One spec reduced to the two day-stamps burn-down needs: the ordinal day it
/// was created and (optionally) the ordinal day it reached Completed. The
/// caller derives the Completed day by walking the spec `history:` for a
/// `status` change whose `new_value` is `Completed` (falling back to
/// `modified_at` for currently-Completed specs with no history row), and the
/// created day from `created_at`. Using an ordinal day index (e.g.
/// `num_days_from_ce` or epoch-day) keeps the bucketing pure and timezone-free.
/// trace:STORY-530
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecLifecycleDays {
    /// Ordinal day the spec was created.
    pub created_day: i64,
    /// Ordinal day the spec reached Completed, if it ever did.
    pub completed_day: Option<i64>,
}

/// Compute net burn-down velocity over `[window_start_day, window_end_day]`
/// (inclusive ordinal-day bounds). Counts a completion when `completed_day`
/// falls in the window, and an add when `created_day` falls in the window. The
/// `days` denominator is the inclusive span of the window. Pure over the
/// supplied lifecycle days. trace:STORY-530
pub fn burn_down_velocity(
    specs: &[SpecLifecycleDays],
    window_start_day: i64,
    window_end_day: i64,
) -> BurnDownVelocity {
    let in_window = |d: i64| d >= window_start_day && d <= window_end_day;
    let mut v = BurnDownVelocity::default();
    for spec in specs {
        if in_window(spec.created_day) {
            v.added += 1;
        }
        if let Some(cd) = spec.completed_day {
            if in_window(cd) {
                v.completed += 1;
            }
        }
    }
    v.days = if window_end_day >= window_start_day {
        (window_end_day - window_start_day + 1) as usize
    } else {
        0
    };
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN_RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1000,"num_turns":3}"#;
    const ERROR_RESULT: &str =
        r#"{"type":"result","subtype":"error_max_turns","is_error":true,"duration_ms":1000}"#;
    const INIT: &str = r#"{"type":"system","subtype":"init","model":"x","cwd":"/tmp"}"#;
    const ASSISTANT: &str =
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"working"}]}}"#;

    #[test]
    fn clean_success_is_classified() {
        let log = format!("{INIT}\n{ASSISTANT}\n{CLEAN_RESULT}\n");
        assert_eq!(classify_session(&log), SessionOutcome::CleanSuccess);
        assert!(classify_session(&log).is_success());
    }

    #[test]
    fn error_subtype_result_is_error_class() {
        let log = format!("{INIT}\n{ERROR_RESULT}\n");
        assert_eq!(classify_session(&log), SessionOutcome::Error);
        assert!(!classify_session(&log).is_success());
    }

    #[test]
    fn is_error_true_with_no_subtype_is_error_class() {
        let log = r#"{"type":"result","is_error":true}"#;
        assert_eq!(classify_session(log), SessionOutcome::Error);
    }

    #[test]
    fn sentinel_reap_with_no_result_counts_as_success() {
        // No terminal result event, but the sentinel reaped the session after
        // its work landed — a success.
        let log = format!(
            "{INIT}\n{ASSISTANT}\n{}\n",
            r#"{"type":"aida_sentinel_reap","reason":"sentinel-reaped"}"#
        );
        let outcome = classify_session(&log);
        assert_eq!(outcome, SessionOutcome::SentinelReaped);
        assert!(outcome.is_success());
    }

    #[test]
    fn sentinel_reap_via_generic_field() {
        let log = format!(
            "{INIT}\n{}\n",
            r#"{"type":"system","aida_sentinel":"reaped"}"#
        );
        assert_eq!(classify_session(&log), SessionOutcome::SentinelReaped);
    }

    #[test]
    fn mid_work_kill_with_no_result_is_failure() {
        let log = format!(
            "{INIT}\n{ASSISTANT}\n{}\n",
            r#"{"type":"aida_session_kill","reason":"killed"}"#
        );
        let outcome = classify_session(&log);
        assert_eq!(outcome, SessionOutcome::MidWorkKill);
        assert!(!outcome.is_success());
    }

    #[test]
    fn truncated_log_with_no_terminal_event_is_truncated() {
        // Session crashed mid-stream — no result, no markers.
        let log = format!("{INIT}\n{ASSISTANT}\n");
        let outcome = classify_session(&log);
        assert_eq!(outcome, SessionOutcome::Truncated);
        assert!(!outcome.is_success());
    }

    #[test]
    fn empty_log_is_truncated() {
        assert_eq!(classify_session(""), SessionOutcome::Truncated);
        assert_eq!(classify_session("\n\n  \n"), SessionOutcome::Truncated);
    }

    #[test]
    fn result_event_wins_over_kill_marker() {
        // A kill marker earlier in the stream is irrelevant once a clean
        // terminal result event lands.
        let log = format!(
            "{INIT}\n{}\n{CLEAN_RESULT}\n",
            r#"{"type":"aida_session_kill","reason":"killed"}"#
        );
        assert_eq!(classify_session(&log), SessionOutcome::CleanSuccess);
    }

    #[test]
    fn malformed_trailing_line_does_not_mask_earlier_result() {
        // A partial/garbage trailing line must not hide a complete earlier
        // result event (scan-from-end + skip-unparseable).
        let log = format!("{INIT}\n{CLEAN_RESULT}\n{{not-json");
        assert_eq!(classify_session(&log), SessionOutcome::CleanSuccess);
    }

    #[test]
    fn tally_counts_each_class() {
        let clean = format!("{INIT}\n{CLEAN_RESULT}\n");
        let err = format!("{INIT}\n{ERROR_RESULT}\n");
        let reap = format!(
            "{INIT}\n{}\n",
            r#"{"type":"aida_sentinel_reap","reason":"sentinel-reaped"}"#
        );
        let kill = format!(
            "{INIT}\n{}\n",
            r#"{"type":"aida_session_kill","reason":"killed"}"#
        );
        let trunc = INIT.to_string();
        let tally = tally_sessions([clean, err, reap, kill, trunc]);
        assert_eq!(tally.total, 5);
        assert_eq!(tally.clean_success, 1);
        assert_eq!(tally.sentinel_reaped, 1);
        assert_eq!(tally.error, 1);
        assert_eq!(tally.mid_work_kill, 1);
        assert_eq!(tally.truncated, 1);
        // 2 successes (clean + reaped) over 5 sessions = 0.4.
        assert_eq!(tally.successes(), 2);
        assert!((tally.success_rate() - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn success_rate_is_zero_for_empty_tally() {
        let tally = SessionTally::default();
        assert_eq!(tally.total, 0);
        assert_eq!(tally.success_rate(), 0.0);
    }

    #[test]
    fn gap_is_session_minus_drain() {
        // 3 clean of 4 sessions = 0.75 session rate; drain recorded 0.50.
        // Gap = +0.25: a quarter of the work the orchestrator scored as failed
        // actually succeeded at the session level.
        let mut tally = SessionTally::default();
        tally.record(SessionOutcome::CleanSuccess);
        tally.record(SessionOutcome::CleanSuccess);
        tally.record(SessionOutcome::SentinelReaped);
        tally.record(SessionOutcome::Error);
        let gap = compute_gap(&tally, 0.50, 4);
        assert!((gap.session_success_rate - 0.75).abs() < f64::EPSILON);
        assert!((gap.drain_success_rate - 0.50).abs() < f64::EPSILON);
        assert!((gap.gap() - 0.25).abs() < f64::EPSILON);
        assert!(!gap.has_zero_denominator());
    }

    #[test]
    fn negative_gap_when_sessions_fail_below_drain_score() {
        let mut tally = SessionTally::default();
        tally.record(SessionOutcome::Error);
        tally.record(SessionOutcome::CleanSuccess);
        // 0.5 session rate vs 0.9 drain rate → gap = -0.4.
        let gap = compute_gap(&tally, 0.9, 10);
        assert!((gap.gap() + 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn zero_denominator_edge_cases() {
        // No sessions: session rate 0.0, flagged as zero-denominator.
        let empty = SessionTally::default();
        let gap = compute_gap(&empty, 0.8, 5);
        assert_eq!(gap.session_success_rate, 0.0);
        assert!(gap.has_zero_denominator());

        // No drain runs: also flagged.
        let mut tally = SessionTally::default();
        tally.record(SessionOutcome::CleanSuccess);
        let gap = compute_gap(&tally, 0.0, 0);
        assert!(gap.has_zero_denominator());

        // Both empty.
        let gap = compute_gap(&SessionTally::default(), 0.0, 0);
        assert!(gap.has_zero_denominator());
        assert_eq!(gap.gap(), 0.0);
    }

    #[test]
    fn outcome_slugs_are_stable() {
        assert_eq!(SessionOutcome::CleanSuccess.slug(), "clean-success");
        assert_eq!(SessionOutcome::SentinelReaped.slug(), "sentinel-reaped");
        assert_eq!(SessionOutcome::MidWorkKill.slug(), "mid-work-kill");
        assert_eq!(SessionOutcome::Error.slug(), "error");
        assert_eq!(SessionOutcome::Truncated.slug(), "truncated");
    }

    // ---- STORY-530: halt-rate ----------------------------------------------

    #[test]
    fn halt_breakdown_classifies_shelvable_vs_halting() {
        let kinds = [
            Some("ci-red"),
            Some("failed"),
            Some("spawn"),
            Some("missing-tool"),
            Some("internal"),
        ];
        let b = halt_breakdown(kinds);
        assert_eq!(b.shelved, 2); // ci-red + failed
        assert_eq!(b.halted, 3); // spawn + missing-tool + internal
        assert_eq!(b.unclassified, 0);
        assert_eq!(b.total(), 5);
        // 3 halting of 5 classified = 0.6.
        assert!((b.halt_rate() - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn halt_breakdown_unknown_slug_is_unclassified_and_excluded_from_rate() {
        let kinds = [Some("ci-red"), Some("spawn"), Some("mystery-kind"), None];
        let b = halt_breakdown(kinds);
        assert_eq!(b.shelved, 1);
        assert_eq!(b.halted, 1);
        assert_eq!(b.unclassified, 2); // unknown slug + None
        assert_eq!(b.total(), 4);
        // Rate over classified only (shelved + halted = 2): 1 halted → 0.5.
        assert!((b.halt_rate() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn halt_rate_is_zero_for_no_classified_failures() {
        let empty: [Option<&str>; 0] = [];
        assert_eq!(halt_breakdown(empty).halt_rate(), 0.0);
        // All-unclassified also yields a zero rate (no classified denominator).
        let b = halt_breakdown([None::<&str>, None::<&str>]);
        assert_eq!(b.unclassified, 2);
        assert_eq!(b.halt_rate(), 0.0);
    }

    #[test]
    fn shelvable_and_halting_slugs_are_disjoint_and_complete() {
        for s in [
            "no-pr",
            "ci-red",
            "ci-timeout",
            "no-verdict",
            "pr-verification-inconclusive",
            "no-progress-watchdog",
            "cache-locked",
            "failed",
        ] {
            assert!(failure_kind_is_shelvable(s), "{s} should be shelvable");
            assert!(!failure_kind_is_halting(s), "{s} should not be halting");
        }
        for s in ["spawn", "missing-tool", "internal"] {
            assert!(failure_kind_is_halting(s), "{s} should be halting");
            assert!(!failure_kind_is_shelvable(s), "{s} should not be shelvable");
        }
    }

    // ---- STORY-530: recovery latency ---------------------------------------

    fn run(started: i64, completed: i64, failed: bool) -> DrainRun {
        DrainRun {
            started_at: started,
            completed_at: completed,
            failed,
        }
    }

    #[test]
    fn recovery_latency_measures_gap_to_next_drain() {
        // Run A fails at t=100; next drain B starts at t=160 → gap 60.
        // Run B fails at t=200; next drain C starts at t=500 → gap 300.
        // Run C succeeds → no gap measured from it.
        let runs = [
            run(50, 100, true),
            run(160, 200, true),
            run(500, 560, false),
        ];
        let lat = recovery_latency(&runs);
        assert_eq!(lat.count(), 2);
        assert_eq!(lat.gaps_secs, vec![60, 300]);
        assert_eq!(lat.max_secs(), Some(300));
        assert!((lat.mean_secs().unwrap() - 180.0).abs() < f64::EPSILON);
        assert!((lat.median_secs().unwrap() - 180.0).abs() < f64::EPSILON);
    }

    #[test]
    fn recovery_latency_failure_with_no_following_drain_yields_no_gap() {
        // A single failing run, nothing after it → nothing to recover toward.
        let runs = [run(0, 100, true)];
        let lat = recovery_latency(&runs);
        assert_eq!(lat.count(), 0);
        assert_eq!(lat.mean_secs(), None);
        assert_eq!(lat.median_secs(), None);
        assert_eq!(lat.max_secs(), None);
    }

    #[test]
    fn recovery_latency_orders_unsorted_input_by_completion() {
        // Supplied out of order; the gap from the t=100 failure is still 50.
        let runs = [
            run(200, 260, false),
            run(150, 200, false),
            run(0, 100, true),
        ];
        let lat = recovery_latency(&runs);
        // Failure completes at 100; next run that STARTS >= 100 is the 150-run.
        assert_eq!(lat.gaps_secs, vec![50]);
    }

    #[test]
    fn recovery_latency_empty_is_empty() {
        let lat = recovery_latency(&[]);
        assert_eq!(lat.count(), 0);
        assert_eq!(lat.mean_secs(), None);
    }

    #[test]
    fn recovery_latency_median_even_count_averages_middle_two() {
        // Gaps 10, 20, 30, 100 (from four failures each followed by a drain).
        let runs = [
            run(0, 10, true),
            run(20, 30, true),   // gap from first failure: 20-10 = 10
            run(60, 70, true),   // gap from second failure: 60-30 = 30
            run(120, 130, true), // gap from third failure: 120-70 = 50
            run(1000, 1010, false),
        ];
        let lat = recovery_latency(&runs);
        // Failures at completions 10,30,70,130; following starts 20,60,120,1000.
        // gaps: 10, 30, 50, 870 → median = (30+50)/2 = 40.
        assert_eq!(lat.gaps_secs, vec![10, 30, 50, 870]);
        assert!((lat.median_secs().unwrap() - 40.0).abs() < f64::EPSILON);
    }

    // ---- STORY-530: draft-inbox depth --------------------------------------

    #[test]
    fn draft_inbox_depth_counts_unarchived_drafts() {
        // (is_draft, is_archived)
        let specs = [
            (true, false),  // counts
            (true, false),  // counts
            (true, true),   // archived draft — excluded
            (false, false), // not a draft — excluded
            (false, true),  // neither — excluded
        ];
        assert_eq!(draft_inbox_depth(specs), 2);
    }

    #[test]
    fn draft_inbox_depth_is_zero_when_no_drafts() {
        assert_eq!(draft_inbox_depth([(false, false), (false, true)]), 0);
        let empty: [(bool, bool); 0] = [];
        assert_eq!(draft_inbox_depth(empty), 0);
    }

    // ---- STORY-530: burn-down velocity -------------------------------------

    fn spec(created: i64, completed: Option<i64>) -> SpecLifecycleDays {
        SpecLifecycleDays {
            created_day: created,
            completed_day: completed,
        }
    }

    #[test]
    fn burn_down_velocity_nets_completions_against_adds() {
        // Window days 10..=12 (3 days).
        let specs = [
            spec(10, Some(11)), // added + completed in window
            spec(11, Some(12)), // added + completed in window
            spec(12, None),     // added only
            spec(5, Some(10)),  // added before window, completed in window
            spec(11, Some(20)), // added in window, completed after window
        ];
        let v = burn_down_velocity(&specs, 10, 12);
        // adds in window: days 10,11,12,11 = 4 (the day-5 add is out of window).
        assert_eq!(v.added, 4);
        // completions in window: days 11,12,10 = 3 (the day-20 completion is out).
        assert_eq!(v.completed, 3);
        assert_eq!(v.days, 3);
        assert_eq!(v.net(), -1); // 3 completed − 4 added
        assert!((v.net_per_day().unwrap() - (-1.0 / 3.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn burn_down_velocity_positive_net_when_shipping_faster_than_adding() {
        let specs = [
            spec(1, Some(5)),
            spec(2, Some(5)),
            spec(3, Some(5)),
            spec(5, None),
        ];
        let v = burn_down_velocity(&specs, 5, 5); // single day
        assert_eq!(v.completed, 3);
        assert_eq!(v.added, 1);
        assert_eq!(v.days, 1);
        assert_eq!(v.net(), 2);
        assert!((v.net_per_day().unwrap() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn burn_down_velocity_empty_window_has_no_per_day() {
        // end < start → zero-day window, net_per_day is None.
        let v = burn_down_velocity(&[spec(1, Some(1))], 5, 4);
        assert_eq!(v.days, 0);
        assert_eq!(v.completed, 0);
        assert_eq!(v.added, 0);
        assert_eq!(v.net_per_day(), None);
    }

    #[test]
    fn burn_down_velocity_no_specs_is_zero_net() {
        let v = burn_down_velocity(&[], 0, 9);
        assert_eq!(v.completed, 0);
        assert_eq!(v.added, 0);
        assert_eq!(v.days, 10);
        assert_eq!(v.net(), 0);
        assert_eq!(v.net_per_day(), Some(0.0));
    }
}
