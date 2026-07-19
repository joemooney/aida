//! Unattended-drive robustness (BUG-660).
//!
//! AIDA's overnight drive (`aida zen --no-human` / `queue work --auto-complete`
//! / `integrate --watch`) runs for hours unattended. Four real overnight-failure
//! modes had no protection; this module is the contained, platform-aware home
//! for the building blocks that close them:
//!
//! 1. [`SleepInhibitor`] — wrap a drive in a system sleep-prevention assertion
//!    (`caffeinate` on macOS, `systemd-inhibit` on Linux) so a laptop that lids
//!    or idles does not suspend mid-drive. Best-effort: a missing tool degrades
//!    to a no-op, never breaking the drive. The held child is scoped to the
//!    parent process's lifetime, so it auto-releases even on the drive handlers'
//!    `std::process::exit` (which skips `Drop`).
//! 2. [`Backoff`] — exponential backoff for retryable agent/transient errors so
//!    a wedged dependency (locked cache, GH-API blip) is not hammered.
//! 3. [`run_with_transient_backoff`] — the thin glue that retries a per-spec
//!    orchestration through [`Backoff`] when its result is
//!    [`is_retryable_orchestration`].
//!
//! The permanent exit-summary (the fourth robustness leg) is the existing
//! [`crate::drain_summary`] surface (TASK-967); this module does not duplicate
//! it. The commit-failure-preserve-for-repair leg lives next to the reset it
//! guards — [`aida_core::git_ops::preserve_dirty_worktree`], called from
//! [`aida_core::worktree_pool::return_to_pool`] before the worktree is reset.
//!
//! Everything here is unit-testable in isolation: the platform command is
//! injectable ([`SleepInhibitor::from_command`]) and the backoff is pure.
// trace:BUG-660 | ai:claude

use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// A best-effort system sleep-prevention assertion held for the lifetime of a
/// drive. On construction it spawns the platform inhibitor as a child process;
/// on [`release`](Self::release) (or `Drop`) it kills that child, lifting the
/// assertion.
///
/// The held child is deliberately tied to *this* process's pid: `caffeinate -w
/// <pid>` exits when we exit, and the Linux `systemd-inhibit` wrapper polls our
/// pid and exits when it disappears. That makes the assertion auto-release even
/// on the drive handlers' `std::process::exit`, which never runs `Drop`.
// trace:BUG-660 | ai:claude
pub(crate) struct SleepInhibitor {
    child: Option<Child>,
    /// The tool backing the live assertion (for the human notice / tests).
    /// `None` when no inhibitor is active (tool absent or platform unsupported).
    tool: Option<&'static str>,
}

impl SleepInhibitor {
    /// Arm sleep-prevention for a drive described by `reason`. Best-effort: if
    /// the platform tool is absent or the spawn fails, the returned inhibitor is
    /// inactive ([`is_active`](Self::is_active) is `false`) and the drive
    /// continues unprotected rather than erroring.
    pub(crate) fn for_drive(reason: &str) -> Self {
        match platform_inhibit_command(reason, std::process::id()) {
            Some((tool, mut cmd)) => {
                cmd.stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                match cmd.spawn() {
                    Ok(child) => Self {
                        child: Some(child),
                        tool: Some(tool),
                    },
                    // ENOENT / EACCES — the tool is not installed. Degrade
                    // gracefully: an unprotected drive is better than a broken
                    // one.
                    Err(_) => Self {
                        child: None,
                        tool: None,
                    },
                }
            }
            None => Self {
                child: None,
                tool: None,
            },
        }
    }

    /// Test seam: build an inhibitor from an explicit command instead of the
    /// platform tool, so the invoked-then-released contract can be exercised
    /// without actually calling `caffeinate` / `systemd-inhibit`.
    #[cfg(test)]
    pub(crate) fn from_command(mut cmd: Command) -> Self {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Self {
            child: cmd.spawn().ok(),
            tool: Some("mock"),
        }
    }

    /// True while a sleep-prevention assertion is live.
    #[allow(dead_code)] // invoked-then-released contract is asserted in tests; production reads tool() for the notice
    pub(crate) fn is_active(&self) -> bool {
        self.child.is_some()
    }

    /// The tool backing the live assertion, if any.
    pub(crate) fn tool(&self) -> Option<&'static str> {
        self.tool
    }

    /// Lift the assertion: kill and reap the held child. Idempotent — a second
    /// call (or a `Drop` after an explicit release) is a no-op.
    pub(crate) fn release(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for SleepInhibitor {
    fn drop(&mut self) {
        self.release();
    }
}

/// Build the platform sleep-prevention command, or `None` when this platform
/// has no supported tool. The command is *not* spawned here — the caller spawns
/// it (and tolerates an ENOENT from a missing binary).
///
/// - macOS: `caffeinate -i -m -s -w <pid>` asserts idle/disk/system sleep
///   prevention and self-terminates when our pid exits (`-w`).
/// - Linux: `systemd-inhibit ... sh -c 'while kill -0 <pid>; do sleep 5; done'`
///   holds a sleep:idle inhibitor lock for as long as a tiny shell keeps our pid
///   alive, so the lock releases when we exit (no `Drop` needed). Avoids any
///   unsafe `PR_SET_PDEATHSIG` plumbing.
// trace:BUG-660 | ai:claude
fn platform_inhibit_command(reason: &str, pid: u32) -> Option<(&'static str, Command)> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("caffeinate");
        cmd.args(["-i", "-m", "-s", "-w", &pid.to_string()]);
        let _ = reason;
        return Some(("caffeinate", cmd));
    }
    #[cfg(target_os = "linux")]
    {
        let why = sanitize_reason(reason);
        let watcher = format!("while kill -0 {pid} 2>/dev/null; do sleep 5; done");
        let mut cmd = Command::new("systemd-inhibit");
        cmd.args([
            "--what=sleep:idle",
            "--who=aida",
            &format!("--why={why}"),
            "--mode=block",
            "sh",
            "-c",
            &watcher,
        ]);
        return Some(("systemd-inhibit", cmd));
    }
    #[allow(unreachable_code)]
    {
        let _ = (reason, pid);
        None
    }
}

/// Reduce a free-text reason to a quote-safe `--why=` value (the inhibitor's
/// human label). Keeps it on one line and out of shell-meta territory.
#[cfg(target_os = "linux")]
fn sanitize_reason(reason: &str) -> String {
    let cleaned: String = reason
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | ':' | '.' | '/') {
                c
            } else {
                ' '
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "aida drive".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

/// Exponential backoff for retryable agent/transient errors. Each
/// [`next_delay`](Self::next_delay) returns the next wait (`base * factor^n`,
/// capped at `max`) and advances the attempt counter, yielding `None` once
/// `max_retries` have been handed out — the caller then stops retrying.
// trace:BUG-660 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct Backoff {
    base: Duration,
    factor: u32,
    max: Duration,
    max_retries: usize,
    attempt: usize,
}

impl Backoff {
    /// A backoff with explicit parameters.
    pub(crate) fn new(base: Duration, factor: u32, max: Duration, max_retries: usize) -> Self {
        Self {
            base,
            factor,
            max,
            max_retries,
            attempt: 0,
        }
    }

    /// The default schedule for a drive's transient-error retries: 2s, 4s, 8s,
    /// 16s, 32s (capped at 60s), then give up after 5 retries. Conservative
    /// enough not to hammer a wedged dependency, short enough that a brief blip
    /// (a sibling holding the cache lock) is waited out within one drive.
    pub(crate) fn drive_default() -> Self {
        Self::new(Duration::from_secs(2), 2, Duration::from_secs(60), 5)
    }

    /// The next backoff delay, or `None` once the retry budget is spent.
    pub(crate) fn next_delay(&mut self) -> Option<Duration> {
        if self.attempt >= self.max_retries {
            return None;
        }
        let mult = self.factor.saturating_pow(self.attempt as u32);
        let delay = self.base.saturating_mul(mult).min(self.max);
        self.attempt += 1;
        Some(delay)
    }

    /// How many delays have been handed out so far.
    pub(crate) fn attempts(&self) -> usize {
        self.attempt
    }
}

/// True when a per-spec orchestration failed for a *transient/retryable* reason
/// — a locked cache, or an inconclusive PR-verification (GH-API blip) — as
/// opposed to a genuine work/CI failure. Only these warrant a backoff-and-retry;
/// everything else flows to the drive's normal shelve/stop handling.
// trace:BUG-660 | ai:claude
pub(crate) fn is_retryable_orchestration(
    result: &crate::auto_complete::OrchestrationResult,
) -> bool {
    if result.inconclusive_reason.is_some() {
        return true;
    }
    if let Some(failure) = &result.failure {
        return matches!(
            failure.kind,
            crate::auto_complete::FailureKind::CacheLocked
                | crate::auto_complete::FailureKind::PrVerificationInconclusive
        );
    }
    false
}

/// Run a per-spec orchestration, retrying through an exponential [`Backoff`]
/// while its result is [`is_retryable_orchestration`]. A clean result (or a
/// genuine, non-transient failure) returns immediately; a transient failure
/// backs off and re-attempts until the retry budget is spent, then returns the
/// last (still-transient) result for the drive's normal handling.
// trace:BUG-660 | ai:claude
pub(crate) fn run_with_transient_backoff(
    attempt: impl FnMut() -> crate::auto_complete::OrchestrationResult,
) -> crate::auto_complete::OrchestrationResult {
    run_with_transient_backoff_using(attempt, |delay| std::thread::sleep(delay))
}

/// [`run_with_transient_backoff`] with an injectable sleeper, so the retry loop
/// is unit-testable without real waits.
fn run_with_transient_backoff_using(
    mut attempt: impl FnMut() -> crate::auto_complete::OrchestrationResult,
    mut sleep: impl FnMut(Duration),
) -> crate::auto_complete::OrchestrationResult {
    let mut backoff = Backoff::drive_default();
    loop {
        let result = attempt();
        if is_retryable_orchestration(&result) {
            if let Some(delay) = backoff.next_delay() {
                eprintln!(
                    "  transient drive error — backing off {}s before retry {} (best-effort)",
                    delay.as_secs(),
                    backoff.attempts()
                );
                sleep(delay);
                continue;
            }
        }
        return result;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_complete::{FailureKind, OrchestrationResult, Phase, PhaseFailure};
    use std::time::Duration;

    // ── SleepInhibitor: invoked-then-released around a drive ──────────────────

    #[test]
    fn sleep_inhibitor_is_active_then_released() {
        // Mock the platform command with a long-lived `sleep` so we can observe
        // the child being held, then killed on release.
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let mut inhibitor = SleepInhibitor::from_command(cmd);
        assert!(
            inhibitor.is_active(),
            "the mocked inhibitor child should be spawned and held"
        );
        inhibitor.release();
        assert!(
            !inhibitor.is_active(),
            "release must lift the assertion (reap the child)"
        );
        // A second release is a harmless no-op.
        inhibitor.release();
        assert!(!inhibitor.is_active());
    }

    #[test]
    fn sleep_inhibitor_drop_releases() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let inhibitor = SleepInhibitor::from_command(cmd);
        assert!(inhibitor.is_active());
        // Dropping without an explicit release must still reap the child (the
        // RAII safety net for non-`process::exit` paths).
        drop(inhibitor);
    }

    #[test]
    fn sleep_inhibitor_missing_tool_degrades_to_noop() {
        // A command that cannot spawn (no such binary) must not panic — the
        // drive proceeds unprotected.
        let cmd = Command::new("aida-no-such-binary-xyzzy");
        let inhibitor = SleepInhibitor::from_command(cmd);
        assert!(!inhibitor.is_active());
        assert_eq!(inhibitor.tool(), Some("mock"));
    }

    // ── Backoff: escalates on repeated retryable errors ───────────────────────

    #[test]
    fn backoff_escalates_then_exhausts() {
        let mut backoff = Backoff::new(Duration::from_secs(1), 2, Duration::from_secs(60), 5);
        let delays: Vec<u64> = std::iter::from_fn(|| backoff.next_delay())
            .map(|d| d.as_secs())
            .collect();
        // 1, 2, 4, 8, 16 — strictly escalating — then exhausted after 5.
        assert_eq!(delays, vec![1, 2, 4, 8, 16]);
        assert_eq!(backoff.attempts(), 5);
        assert!(backoff.next_delay().is_none(), "budget is spent");
    }

    #[test]
    fn backoff_caps_at_max() {
        let mut backoff = Backoff::new(Duration::from_secs(10), 4, Duration::from_secs(30), 4);
        let delays: Vec<u64> = std::iter::from_fn(|| backoff.next_delay())
            .map(|d| d.as_secs())
            .collect();
        // 10, then 40->cap 30, then 160->cap 30, ... never exceeds max.
        assert_eq!(delays, vec![10, 30, 30, 30]);
    }

    #[test]
    fn drive_default_schedule() {
        let mut backoff = Backoff::drive_default();
        let delays: Vec<u64> = std::iter::from_fn(|| backoff.next_delay())
            .map(|d| d.as_secs())
            .collect();
        assert_eq!(delays, vec![2, 4, 8, 16, 32]);
    }

    // ── retryable classification ──────────────────────────────────────────────

    fn retryable_failure(kind: FailureKind) -> OrchestrationResult {
        let mut r = OrchestrationResult::failed(Phase::Implementer);
        r.failure = Some(PhaseFailure::of(kind, "transient"));
        r
    }

    #[test]
    fn retryable_classification() {
        // Inconclusive PR-verification (GH-API blip) is retryable.
        let mut inconclusive = OrchestrationResult::ok();
        inconclusive.inconclusive_reason = Some("GH unreachable".to_string());
        assert!(is_retryable_orchestration(&inconclusive));

        // A locked cache is transient contention — retryable.
        assert!(is_retryable_orchestration(&retryable_failure(
            FailureKind::CacheLocked
        )));
        assert!(is_retryable_orchestration(&retryable_failure(
            FailureKind::PrVerificationInconclusive
        )));

        // A clean ship and a genuine CI failure are NOT retryable.
        assert!(!is_retryable_orchestration(&OrchestrationResult::ok()));
        assert!(!is_retryable_orchestration(&retryable_failure(
            FailureKind::CiRed
        )));
        assert!(!is_retryable_orchestration(&retryable_failure(
            FailureKind::NoPr
        )));
    }

    // ── backoff loop: retries a transient result, then returns ────────────────

    #[test]
    fn backoff_loop_retries_transient_then_succeeds() {
        let mut calls = 0;
        let mut slept: Vec<u64> = Vec::new();
        let result = run_with_transient_backoff_using(
            || {
                calls += 1;
                if calls < 3 {
                    // First two attempts: transient (locked cache).
                    let mut r = OrchestrationResult::failed(Phase::Implementer);
                    r.failure = Some(PhaseFailure::of(FailureKind::CacheLocked, "locked"));
                    r
                } else {
                    OrchestrationResult::ok()
                }
            },
            |d| slept.push(d.as_secs()),
        );
        assert_eq!(calls, 3, "retried twice, then the third attempt succeeded");
        assert_eq!(slept, vec![2, 4], "backoff escalated between retries");
        assert_eq!(result.exit_code, 0, "the final clean result is returned");
    }

    // ── preserve-on-fail: a recoverable failure preserves work, no loss ───────

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?} failed in {}", dir.display());
    }

    #[test]
    fn preserve_dirty_worktree_salvages_uncommitted_work() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        git(repo, &["init", "-q"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("f.txt"), "original\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", "init"]);

        // A clean tree salvages nothing.
        let salvage = repo.join(".aida").join("salvage");
        let none = aida_core::git_ops::preserve_dirty_worktree(repo, &salvage, "spec-99").unwrap();
        assert!(none.is_none(), "a clean tree has nothing to preserve");

        // Now simulate a recoverable failure mid-edit: uncommitted change.
        std::fs::write(repo.join("f.txt"), "WORK IN PROGRESS\n").unwrap();
        let saved = aida_core::git_ops::preserve_dirty_worktree(repo, &salvage, "spec-99")
            .unwrap()
            .expect("dirty work must be preserved before any reset");
        assert!(saved.exists(), "the salvage patch file is written");
        let body = std::fs::read_to_string(&saved).unwrap();
        assert!(
            body.contains("WORK IN PROGRESS"),
            "the uncommitted work is captured in the patch — not lost: {body}"
        );
        assert!(body.contains("spec-99"), "the label is recorded");
    }

    #[test]
    fn backoff_loop_gives_up_after_budget() {
        let mut calls = 0;
        let mut slept: Vec<u64> = Vec::new();
        let result = run_with_transient_backoff_using(
            || {
                calls += 1;
                let mut r = OrchestrationResult::failed(Phase::Implementer);
                r.failure = Some(PhaseFailure::of(FailureKind::CacheLocked, "still locked"));
                r
            },
            |d| slept.push(d.as_secs()),
        );
        // drive_default budget = 5 retries → 6 attempts, 5 escalating sleeps.
        assert_eq!(calls, 6);
        assert_eq!(slept, vec![2, 4, 8, 16, 32]);
        assert!(
            result.exit_code != 0,
            "the last transient result is surfaced"
        );
    }
}
