//! Idle CI-wait timeout that re-arms on progress, with a separate absolute
//! ceiling.
//!
//! Inspired by no-mistakes' `ci_timeout`: a long-lived PR that keeps making CI
//! progress (checks transition/appear, the PR gets rebased onto an advanced
//! base tip) must keep its monitor — only a genuinely STALLED wait should die.
//! AIDA's worker historically used a flat ABSOLUTE timeout for the CI-wait
//! loop, so a drain legitimately blocked on slow-but-moving CI got killed and
//! auto-paused.
//!
//! The CI-wait now runs TWO timers:
//!   - **idle** (re-arming): the deadline resets on every observed progress
//!     event (a CI check transitions/appears, the check set changes, or the
//!     PR head / base tip advances). Only genuine no-progress past the idle
//!     window expires it.
//!   - **absolute** (hard ceiling): bounds the TOTAL wait so a
//!     forever-progressing PR still terminates eventually.
//!
//! Both the decision and the progress fingerprint are pure so they can be
//! unit-tested without spawning `gh`/`git`. trace:TASK-968 | ai:claude

/// Default idle window (seconds): a CI wait that observes no progress for this
/// long is treated as genuinely stalled. Re-armed on every progress event, so
/// an actively-moving PR never hits it.
pub(crate) const DEFAULT_CI_IDLE_SECS: u64 = 600; // 10 min

/// Default absolute ceiling (seconds): even a continuously-progressing wait
/// stops here so a runaway monitor can't run forever.
pub(crate) const DEFAULT_CI_ABSOLUTE_SECS: u64 = 5400; // 90 min

/// The outcome of one CI-wait timer evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CiWaitVerdict {
    /// Neither timer has fired — keep waiting.
    Continue,
    /// No progress within the idle window — the wait is genuinely stalled.
    IdleTimeout,
    /// The total wait exceeded the absolute hard ceiling.
    AbsoluteTimeout,
}

/// Pure timeout decision for the idle+absolute CI-wait.
///
/// `total_elapsed` is seconds since the wait started; `idle_elapsed` is seconds
/// since the last observed progress event. The **absolute ceiling is checked
/// first** so a forever-progressing wait (which keeps re-arming the idle timer)
/// can't outrun the hard cap. A window of `0` disables that timer (it never
/// fires) — used so an operator can opt out of either bound independently.
// trace:TASK-968 | ai:claude
pub(crate) fn ci_wait_verdict(
    total_elapsed: u64,
    idle_elapsed: u64,
    idle_window: u64,
    absolute_ceiling: u64,
) -> CiWaitVerdict {
    if absolute_ceiling != 0 && total_elapsed >= absolute_ceiling {
        return CiWaitVerdict::AbsoluteTimeout;
    }
    if idle_window != 0 && idle_elapsed >= idle_window {
        return CiWaitVerdict::IdleTimeout;
    }
    CiWaitVerdict::Continue
}

/// Pure: a stable progress fingerprint over the `gh pr list` rollup JSON (the
/// same `[{number, statusCheckRollup, headRefOid}]` shape `parse_ci_probe`
/// consumes) folded together with the base tip SHA.
///
/// Each check contributes `name=status/conclusion` (CheckRun shape) or
/// `context=state` (StatusContext shape); entries are sorted + joined so the
/// fingerprint changes iff any check **appears, disappears, or transitions**.
/// The PR head (`headRefOid`) and the base tip are folded in so a **rebase**
/// (PR head advances) or a **base/default-tip advance** also counts as
/// progress and re-arms the idle deadline.
///
/// Degrades gracefully: malformed/empty JSON yields a base-tip-only
/// fingerprint, so a rebase against an advancing base still re-arms even if the
/// rollup lookup failed.
// trace:TASK-968 | ai:claude
pub(crate) fn ci_progress_fingerprint(rollup_json: &str, base_tip: Option<&str>) -> String {
    let mut head = String::new();
    let mut parts: Vec<String> = Vec::new();
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(rollup_json.trim()) {
        if let Some(pr) = parsed.get(0) {
            if let Some(h) = pr.get("headRefOid").and_then(|v| v.as_str()) {
                head = h.to_string();
            }
            if let Some(rollup) = pr.get("statusCheckRollup").and_then(|v| v.as_array()) {
                for check in rollup {
                    if let Some(status) = check.get("status").and_then(|v| v.as_str()) {
                        // CheckRun shape.
                        let name = check
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("check");
                        let conclusion = check
                            .get("conclusion")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        parts.push(format!("{name}={status}/{conclusion}"));
                    } else if let Some(state) = check.get("state").and_then(|v| v.as_str()) {
                        // StatusContext shape.
                        let ctx = check
                            .get("context")
                            .and_then(|v| v.as_str())
                            .unwrap_or("status");
                        parts.push(format!("{ctx}={state}"));
                    }
                }
            }
        }
    }
    parts.sort();
    format!(
        "base:{}|head:{}|checks:{}",
        base_tip.unwrap_or(""),
        head,
        parts.join(",")
    )
}

/// Idle-window length (seconds) for the CI wait. `AIDA_WORKER_CI_IDLE`
/// overrides; default [`DEFAULT_CI_IDLE_SECS`]. A non-numeric value falls back
/// to the default.
// trace:TASK-968 | ai:claude
pub(crate) fn ci_idle_window_secs() -> u64 {
    std::env::var("AIDA_WORKER_CI_IDLE")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_CI_IDLE_SECS)
}

/// Absolute ceiling (seconds) for the CI wait. `AIDA_WORKER_CI_ABSOLUTE`
/// overrides; default [`DEFAULT_CI_ABSOLUTE_SECS`]. A non-numeric value falls
/// back to the default.
// trace:TASK-968 | ai:claude
pub(crate) fn ci_absolute_ceiling_secs() -> u64 {
    std::env::var("AIDA_WORKER_CI_ABSOLUTE")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_CI_ABSOLUTE_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continues_while_within_both_windows() {
        // 5 min elapsed, 2 min since progress; idle=10m, abs=90m → keep going.
        assert_eq!(
            ci_wait_verdict(300, 120, 600, 5400),
            CiWaitVerdict::Continue
        );
    }

    #[test]
    fn idle_timeout_fires_on_genuine_stall() {
        // Total elapsed modest, but 11 min with no progress past a 10m idle
        // window → stalled.
        assert_eq!(
            ci_wait_verdict(660, 660, 600, 5400),
            CiWaitVerdict::IdleTimeout
        );
    }

    #[test]
    fn progress_re_arms_so_idle_never_fires() {
        // A long wait that keeps making progress: total is huge (80 min) but
        // idle_elapsed stays small because it re-arms each poll → never idle.
        assert_eq!(
            ci_wait_verdict(4800, 30, 600, 5400),
            CiWaitVerdict::Continue
        );
    }

    #[test]
    fn absolute_ceiling_bounds_a_forever_progressing_wait() {
        // Progress keeps re-arming the idle timer (idle_elapsed tiny), but the
        // total wait crosses the 90m absolute ceiling → hard stop wins.
        assert_eq!(
            ci_wait_verdict(5400, 10, 600, 5400),
            CiWaitVerdict::AbsoluteTimeout
        );
    }

    #[test]
    fn absolute_ceiling_takes_precedence_over_idle() {
        // Both windows blown at once: absolute is evaluated first.
        assert_eq!(
            ci_wait_verdict(6000, 6000, 600, 5400),
            CiWaitVerdict::AbsoluteTimeout
        );
    }

    #[test]
    fn idle_boundary_is_inclusive() {
        // Exactly at the idle window → fires (>=).
        assert_eq!(
            ci_wait_verdict(600, 600, 600, 5400),
            CiWaitVerdict::IdleTimeout
        );
        // One second short → still waiting.
        assert_eq!(
            ci_wait_verdict(599, 599, 600, 5400),
            CiWaitVerdict::Continue
        );
    }

    #[test]
    fn zero_window_disables_that_timer() {
        // idle disabled (0): a long stall never idle-times-out, only absolute.
        assert_eq!(
            ci_wait_verdict(1000, 1000, 0, 5400),
            CiWaitVerdict::Continue
        );
        // absolute disabled (0): runs forever as long as progress re-arms idle.
        assert_eq!(
            ci_wait_verdict(100_000, 10, 600, 0),
            CiWaitVerdict::Continue
        );
        // both disabled → never expires.
        assert_eq!(
            ci_wait_verdict(u64::MAX, u64::MAX, 0, 0),
            CiWaitVerdict::Continue
        );
    }

    fn rollup(checks: &str, head: &str) -> String {
        format!(r#"[{{"number":7,"headRefOid":"{head}","statusCheckRollup":[{checks}]}}]"#)
    }

    #[test]
    fn fingerprint_changes_when_a_check_transitions() {
        let running = rollup(
            r#"{"name":"build","status":"IN_PROGRESS","conclusion":""}"#,
            "abc",
        );
        let done = rollup(
            r#"{"name":"build","status":"COMPLETED","conclusion":"SUCCESS"}"#,
            "abc",
        );
        assert_ne!(
            ci_progress_fingerprint(&running, Some("base1")),
            ci_progress_fingerprint(&done, Some("base1")),
            "a check transitioning IN_PROGRESS->COMPLETED must register as progress"
        );
    }

    #[test]
    fn fingerprint_changes_when_a_new_check_appears() {
        let one = rollup(
            r#"{"name":"build","status":"IN_PROGRESS","conclusion":""}"#,
            "abc",
        );
        let two = rollup(
            r#"{"name":"build","status":"IN_PROGRESS","conclusion":""},{"name":"test","status":"QUEUED","conclusion":""}"#,
            "abc",
        );
        assert_ne!(
            ci_progress_fingerprint(&one, Some("base1")),
            ci_progress_fingerprint(&two, Some("base1")),
            "the check set growing must register as progress"
        );
    }

    #[test]
    fn fingerprint_is_stable_when_nothing_moves() {
        let same = rollup(
            r#"{"name":"build","status":"IN_PROGRESS","conclusion":""}"#,
            "abc",
        );
        assert_eq!(
            ci_progress_fingerprint(&same, Some("base1")),
            ci_progress_fingerprint(&same, Some("base1")),
            "an unchanged poll must NOT register as progress (so the idle timer keeps counting)"
        );
    }

    #[test]
    fn fingerprint_is_order_independent() {
        let ab = rollup(
            r#"{"name":"build","status":"IN_PROGRESS","conclusion":""},{"name":"test","status":"QUEUED","conclusion":""}"#,
            "abc",
        );
        let ba = rollup(
            r#"{"name":"test","status":"QUEUED","conclusion":""},{"name":"build","status":"IN_PROGRESS","conclusion":""}"#,
            "abc",
        );
        assert_eq!(
            ci_progress_fingerprint(&ab, Some("base1")),
            ci_progress_fingerprint(&ba, Some("base1")),
            "gh check ordering jitter must not be mistaken for progress"
        );
    }

    #[test]
    fn fingerprint_changes_on_rebase_pr_head_advance() {
        let before = rollup(
            r#"{"name":"build","status":"IN_PROGRESS","conclusion":""}"#,
            "oldsha",
        );
        let after = rollup(
            r#"{"name":"build","status":"IN_PROGRESS","conclusion":""}"#,
            "newsha",
        );
        assert_ne!(
            ci_progress_fingerprint(&before, Some("base1")),
            ci_progress_fingerprint(&after, Some("base1")),
            "the PR head advancing (a rebase) must register as progress"
        );
    }

    #[test]
    fn fingerprint_changes_on_base_tip_advance() {
        let same = rollup(
            r#"{"name":"build","status":"IN_PROGRESS","conclusion":""}"#,
            "abc",
        );
        assert_ne!(
            ci_progress_fingerprint(&same, Some("base1")),
            ci_progress_fingerprint(&same, Some("base2")),
            "the base/default tip advancing must register as progress"
        );
    }

    #[test]
    fn fingerprint_degrades_to_base_on_bad_json() {
        // Malformed rollup → base-tip-only fingerprint, still distinguishes a
        // base advance so rebases re-arm even when the rollup lookup failed.
        assert_ne!(
            ci_progress_fingerprint("not json", Some("base1")),
            ci_progress_fingerprint("not json", Some("base2")),
        );
        assert_eq!(
            ci_progress_fingerprint("", Some("base1")),
            ci_progress_fingerprint("[]", Some("base1")),
        );
    }
}
