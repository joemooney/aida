//! Per-row dispatch-health classification for `aida ps` (TASK-1090, SPIKE-76
//! slice 3).
//!
//! # Why this exists
//!
//! `aida ps` (STORY-696) already answers "what is running" — a lease-backed
//! table of live/dormant/stale sessions, each pointed at its worktree. What it
//! did not answer is the operator's very next question when a row looks off:
//! *is this actually moving, and if not, what is the ONE command that
//! unsticks it?* SPIKE-76's slice-1 (STORY-759) explored that question as a
//! standalone `aida agent dispatch-health` report keyed off the multi-vendor
//! agent registry; TASK-1090's review decision (a grooming comment on the
//! spec) was explicit: don't build a SECOND report that duplicates `aida ps`
//! — extend the one running-work surface that already exists with an
//! actionable per-row hint instead. This module is that extension's pure
//! core: a classifier plus a read-only git-state probe, kept separate from
//! `main.rs` so the decision matrix is exhaustively unit-testable on fixtures
//! (the same discipline `integrate_view` / `drive_robustness` follow).
//!
//! # Read-only, by construction
//!
//! Nothing in this module writes to disk, mints a lease, or mutates git
//! state. [`probe_worktree`] only ever shells out to inspection-only git
//! subcommands (`status --porcelain`, `rev-list --count`, `log -1`); the
//! salvage/resume commands [`next_command_hint`] renders are TEXT for a human
//! or agent to run themselves — `aida ps` never executes them. This keeps the
//! contrast sharp with `aida_core::git_ops::preserve_dirty_worktree` (which
//! DOES write a patch file, called only from the pool-eviction path in
//! `worktree_pool::return_to_pool`) — this module reuses that helper's
//! *notion* of "a worktree with uncommitted work" via
//! [`aida_core::git_ops::worktree_is_dirty`], not the write path itself.
//!
//! trace:TASK-1090 | ai:claude

use std::path::Path;
use std::process::Command;

/// The three dispatch-health states a single `aida ps` row can classify into.
// trace:TASK-1090 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispatchState {
    /// Forward progress is visible: either the branch already carries
    /// commits ahead of `main`, or the session is alive and (dirty diff /
    /// still within the grace window) plausibly still working.
    Moving,
    /// Alive process, but no branch movement and no uncommitted diff for at
    /// least [`DEFAULT_STALLED_THRESHOLD_SECS`] — the idle-producing-nothing
    /// case. Or: dead process with nothing to lose (clean tree) — also
    /// "not moving right now", just with no salvage urgency.
    Stalled,
    /// Dead process AND uncommitted work sitting in the worktree — the
    /// urgent case: that diff is one `worktree reset`/cleanup away from
    /// being lost. Always the highest-priority state to surface.
    Salvageable,
    /// Process liveness could not be determined — no pid was recorded on the
    /// lease AND the cwd-based worktree probe cannot see the worker (an
    /// Agent-tool / harness subagent runs inside the parent claude process,
    /// whose cwd is the parent project root, never the isolation worktree).
    /// Absence of evidence is NOT death here: the dangerous salvage-commit
    /// hint must never fire for this state — mid-drain it would commit
    /// half-done work out from under a live agent and double-dispatch.
    // trace:BUG-752 | ai:claude
    Unknown,
    /// The worktree was handed to a HUMAN by `aida worktree enter|add` — that
    /// verb takes the implementer lease but deliberately launches NO agent, so
    /// nothing is expected to back the lease until the operator starts one
    /// themselves. Within the grace window this is the normal, healthy shape,
    /// NOT a dead process: the absent worker is the operator's next keystroke.
    /// Never carries a re-dispatch hint — `aida queue work <spec>` on a
    /// hand-entered spec starts a SECOND session competing with the human.
    // trace:BUG-778 | ai:claude
    AwaitingAgent,
}

impl DispatchState {
    /// Lowercase machine/human label (mirrors `LeaseState::label`).
    pub(crate) fn label(self) -> &'static str {
        match self {
            DispatchState::Moving => "moving",
            DispatchState::Stalled => "stalled",
            DispatchState::Salvageable => "salvageable",
            // trace:BUG-752 | ai:claude
            DispatchState::Unknown => "unknown",
            // trace:BUG-778 | ai:claude
            DispatchState::AwaitingAgent => "awaiting-agent",
        }
    }
}

/// Default elapsed-time bar (seconds) past which an alive-but-idle row (no
/// branch movement, no dirty diff) reads STALLED instead of MOVING — the
/// single-snapshot proxy for "produced nothing" the acceptance criteria call
/// for (a true diff/HEAD-delta comparator would need a persisted
/// prior-snapshot, which would add state — out of scope for a READ-ONLY
/// report).
///
/// Chosen as 30 minutes to match `integrate_view::DEFAULT_IDLE_THRESHOLD_MINS`
/// — the existing "is `main` moving" idle bar this same `aida ps`
/// neighborhood already uses — rather than `aida status <spec>`'s BUG-623
/// bar (180m/3h default). BUG-623's bar debounces a SLOWER signal (did the
/// spec's YAML `modified_at` move — only bumped by an explicit `aida edit`);
/// this one debounces a FASTER signal (did the worktree's git state move —
/// bumped by every commit/save), so the shorter bar is the closer analog. A
/// session that has produced zero commits and zero uncommitted diff half an
/// hour after starting is worth a glance; three hours would let a genuinely
/// wedged session sit unnoticed for most of a workday.
// trace:TASK-1090 | ai:claude
pub(crate) const DEFAULT_STALLED_THRESHOLD_SECS: u64 = 30 * 60;

/// BUG-778: how long a HAND-ENTERED worktree (`aida worktree enter|add` — takes
/// the lease, launches no agent) reads as AWAITING-AGENT before the ordinary
/// agent-expected matrix takes over.
///
/// Deliberately the same 30 minutes as [`DEFAULT_STALLED_THRESHOLD_SECS`]: the
/// question both bars answer is "how long may a worktree show zero movement
/// before it is worth a glance?", and there is no reason a human's launch-lag
/// budget should differ from an agent's produce-something budget. The window
/// only has to be comfortably longer than the seconds-to-minutes between
/// `worktree enter` and the operator's `claude` — the observed false positive
/// fired ~30 SECONDS in.
///
/// After the window a hand-entered lease still ages into STALLED (an entered
/// worktree nobody ever worked is genuinely worth surfacing) — what it never
/// does, at any age, is claim "process dead" or offer the `aida queue work`
/// re-dispatch, because no agent was ever expected here and re-dispatching
/// would start a session competing with the human.
// trace:BUG-778 | ai:claude
pub(crate) const DEFAULT_AWAITING_AGENT_GRACE_SECS: u64 = 30 * 60;

/// Pure classifier: given the four signals `aida ps` already has (or can
/// cheaply probe) per row, decide MOVING / STALLED / SALVAGEABLE / UNKNOWN.
/// No I/O, no mutable state — every input is a plain value so the full
/// decision matrix is exercisable from unit-test fixtures.
///
/// `pid_alive` is a tri-state: `Some(true)` = a live process demonstrably
/// backs the lease, `Some(false)` = liveness was checked and the process is
/// dead, `None` = liveness is UNDETERMINABLE (a harness/Agent-tool lease that
/// recorded no pid — the worker runs inside the parent claude process, out of
/// reach of the cwd-based worktree probe). `None` classifies Unknown, never
/// Salvageable: the salvage hint fires only on a genuinely determined death.
/// (It was a plain `bool` before the third false-negative variant.)
///
/// `manual_enter_secs` (BUG-778) is `Some(seconds since the worktree was handed
/// to a human by `aida worktree enter|add`)` for a hand-entered lease and `None`
/// for every orchestrator-spawned one — the "was an agent ever expected here?"
/// provenance. A hand-entered lease with nothing yet to show, still inside
/// `awaiting_agent_grace_secs`, short-circuits to `AwaitingAgent` before the
/// agent-expected matrix below can call its absent worker dead.
///
/// Decision matrix (documented so a future reader doesn't have to reverse it
/// out of the `if` chain):
///
/// | pid alive | branch ahead | worktree dirty | elapsed        | state       |
/// |-----------|--------------|-----------------|----------------|-------------|
/// | not live  | 0            | no              | hand-entered,  | Awaiting-   |
/// |           |              |                 | < grace        | Agent       |
/// | unknown   | —            | —               | —              | Unknown     |
/// | no        | —            | yes             | —              | Salvageable |
/// | no        | —            | no              | —              | Stalled     |
/// | yes       | >0           | —               | —              | Moving      |
/// | yes       | 0            | yes             | —              | Moving      |
/// | yes       | 0            | no              | < threshold    | Moving      |
/// | yes       | 0            | no              | >= threshold   | Stalled     |
///
/// Rationale for the judgment calls:
/// - **Dead + clean → Stalled, not Moving**: nothing is being produced right
///   now (the driving process is gone), but nothing uncommitted is at risk
///   either, so it doesn't carry Salvageable's urgency.
/// - **Alive + dirty (any elapsed) → Moving**: a single snapshot cannot tell
///   whether a dirty diff is still GROWING, so we give the benefit of the
///   doubt to "still working" whenever there's *any* uncommitted diff. Only
///   a genuinely clean, unmoved worktree — no diff to point to at all — ages
///   into Stalled.
/// - **Unknown pid → Unknown, regardless of git state**: with no liveness
///   evidence either way, claiming Moving would overstate and claiming
///   Salvageable would invite a mid-drain salvage-commit of a live agent's
///   half-done work. Surface the uncertainty honestly instead.
/// - **Fresh hand-enter → AwaitingAgent, not "dead"**: `worktree enter` mints
///   the lease and stops; the operator launches the agent themselves moments
///   later. Reading that gap as a dead process (and hinting `aida queue work`)
///   would re-dispatch a spec a human is about to work by hand. Guarded on a
///   still-untouched worktree so a hand-entered session that DID produce
///   something and then died keeps its salvage urgency.
// trace:TASK-1090 | ai:claude
// trace:BUG-752 | ai:claude
// trace:BUG-778 | ai:claude
pub(crate) fn dispatch_state(
    pid_alive: Option<bool>,
    worktree_dirty: bool,
    branch_ahead_of_main: u32,
    elapsed_secs: u64,
    stalled_threshold_secs: u64,
    manual_enter_secs: Option<u64>,
    awaiting_agent_grace_secs: u64,
) -> DispatchState {
    // BUG-778: hand-entered, still pristine, still inside the launch-lag
    // window — the missing worker is the operator's next keystroke, not a
    // corpse. Checked FIRST so the dead/unknown arms below never see it.
    // trace:BUG-778 | ai:claude
    if let Some(since_enter) = manual_enter_secs {
        if pid_alive != Some(true)
            && !worktree_dirty
            && branch_ahead_of_main == 0
            && since_enter < awaiting_agent_grace_secs
        {
            return DispatchState::AwaitingAgent;
        }
    }
    let Some(pid_alive) = pid_alive else {
        // trace:BUG-752 | ai:claude
        return DispatchState::Unknown;
    };
    if !pid_alive {
        return if worktree_dirty {
            DispatchState::Salvageable
        } else {
            DispatchState::Stalled
        };
    }
    if branch_ahead_of_main > 0 {
        return DispatchState::Moving;
    }
    if !worktree_dirty && elapsed_secs >= stalled_threshold_secs {
        return DispatchState::Stalled;
    }
    DispatchState::Moving
}

/// Read-only git-state probe result for one lease's worktree — the signals
/// [`dispatch_state`] needs beyond PID liveness (which `aida ps` already
/// computes via [`crate::lease_state_for`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorktreeGitProbe {
    /// True when `git status --porcelain` reports any tracked or untracked
    /// change (gitignored paths excluded).
    pub(crate) dirty: bool,
    /// Commits on `HEAD` not reachable from `origin/main` (falls back to
    /// local `main` when no `origin/main` ref resolves — e.g. an offline
    /// clone). `0` when the count can't be determined.
    pub(crate) ahead_of_main: u32,
    /// The worktree's current `HEAD` commit subject, for the "name the...
    /// last commit" acceptance line. `None` when the worktree has no commits
    /// yet or `git log` fails.
    pub(crate) last_commit_subject: Option<String>,
}

fn git_stdout(worktree: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Probe `worktree_path` for the three git-state signals dispatch-health
/// needs. Read-only (inspection-only git subcommands, no writes) and
/// tolerant of a missing/removed worktree — a gone directory degrades to the
/// zero default rather than erroring, matching the STALE-lease framing
/// `aida ps` already uses for that case (BUG-660's dead-worktree handling).
// trace:TASK-1090 | ai:claude
pub(crate) fn probe_worktree(worktree_path: &Path) -> WorktreeGitProbe {
    if !worktree_path.is_dir() {
        return WorktreeGitProbe::default();
    }
    let dirty = aida_core::git_ops::worktree_is_dirty(worktree_path);
    let ahead_of_main = git_stdout(worktree_path, &["rev-list", "--count", "origin/main..HEAD"])
        .or_else(|| git_stdout(worktree_path, &["rev-list", "--count", "main..HEAD"]))
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let last_commit_subject = git_stdout(worktree_path, &["log", "-1", "--format=%s"]);
    WorktreeGitProbe {
        dirty,
        ahead_of_main,
        last_commit_subject,
    }
}

/// The exact next command for a row's dispatch state — "no interpretation
/// left to the reader" per the TASK-1090 acceptance. `None` for `Moving`
/// (nothing to unstick). The Salvageable hint is a plain `git commit` — the
/// commit-early discipline STORY-759 already documents under
/// `docs/agents/` — never the patch-file salvage path
/// (`preserve_dirty_worktree`), which this read-only report must not invoke.
///
/// BUG-778: `manual_enter` says the worktree was handed to a HUMAN by `aida
/// worktree enter|add`. Every hint for such a row resumes it IN PLACE — the
/// re-dispatch verb is withheld at ANY age, because running it would start a
/// second session on a spec someone is working by hand.
// trace:TASK-1090 | ai:claude
// trace:BUG-778 | ai:claude
pub(crate) fn next_command_hint(
    state: DispatchState,
    worktree_path: &Path,
    branch: &str,
    last_commit_subject: Option<&str>,
    spec: Option<&str>,
    manual_enter: bool,
) -> Option<String> {
    let wt = worktree_path.display();
    let last_commit = last_commit_subject.unwrap_or("(no commits yet)");
    // trace:BUG-778 | ai:claude
    let rebrief = if manual_enter {
        format!(
            "pick it back up in place — cd {wt} and start your agent there \
             (this worktree was entered by hand, so re-dispatching the spec would \
             start a second session competing with you)"
        )
    } else {
        match spec {
            Some(id) => format!("aida queue work {id}"),
            None => format!("aida agent new claude --cwd {wt}"),
        }
    };
    match state {
        DispatchState::Moving => None,
        // BUG-778: the hand-entered, launch-pending shape. Names the ONE thing
        // the operator has left to do (start an agent in the worktree they just
        // stepped into) plus the release verb if they changed their mind — and
        // deliberately says nothing about a dead process or a re-dispatch.
        // trace:BUG-778 | ai:claude
        DispatchState::AwaitingAgent => {
            let release = match spec {
                Some(id) => format!("aida session end {id}"),
                None => "aida session end".to_string(),
            };
            Some(format!(
                "entered by hand — worktree {wt} (branch {branch}) is yours and no agent has been \
                 launched in it yet; start one there, or {release} to hand the lease back"
            ))
        }
        // BUG-778: a hand-entered worktree never had an agent to lose, so its
        // lead-in says "nothing running" rather than "dead process" — the diff
        // is still at risk and still worth salvaging, but the framing must not
        // imply a crash that never happened. trace:BUG-778 | ai:claude
        DispatchState::Salvageable => {
            let lead = if manual_enter {
                "nothing running, uncommitted work"
            } else {
                "dead process, uncommitted work"
            };
            Some(format!(
                "{lead} in {wt} (branch {branch}, last commit \"{last_commit}\") — \
                 salvage-commit then rebrief: git -C {wt} add -A && git -C {wt} commit -m \"wip: salvage {branch}\" \
                 — then: {rebrief}"
            ))
        }
        DispatchState::Stalled => Some(format!(
            "no branch/dirty movement in {wt} (branch {branch}, last commit \"{last_commit}\") — resume/rebrief: {rebrief}"
        )),
        // BUG-752: no pid was recorded and the worktree probe can't see a
        // harness-hosted worker — liveness is unknown, NOT dead. Never emit
        // the salvage-commit command here: an agent may still be writing this
        // worktree, and salvage-committing under it would capture half-done
        // work and double-dispatch. trace:BUG-752 | ai:claude
        DispatchState::Unknown => Some(format!(
            "liveness unknown — no pid recorded for {wt} (branch {branch}, last commit \"{last_commit}\"); \
             an agent may still be working here — verify before any cleanup"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── dispatch_state: the full decision matrix ──────────────────────────

    #[test]
    fn dead_pid_dirty_worktree_is_salvageable() {
        assert_eq!(
            dispatch_state(
                Some(false),
                true,
                0,
                999,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Salvageable
        );
        // Dirty + ahead: still Salvageable — a dead process always wins on
        // the salvage-urgency axis regardless of what's already pushed.
        assert_eq!(
            dispatch_state(
                Some(false),
                true,
                3,
                10,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Salvageable
        );
    }

    #[test]
    fn dead_pid_clean_worktree_is_stalled_not_moving() {
        assert_eq!(
            dispatch_state(
                Some(false),
                false,
                0,
                5,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Stalled
        );
        // Nothing to lose even if commits already landed — a dead process
        // still needs a resume decision, so this stays Stalled rather than
        // Moving.
        assert_eq!(
            dispatch_state(
                Some(false),
                false,
                4,
                5,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Stalled
        );
    }

    #[test]
    fn alive_branch_ahead_of_main_is_moving() {
        assert_eq!(
            dispatch_state(
                Some(true),
                false,
                1,
                99_999,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Moving
        );
        // Dirty on top of ahead — still Moving.
        assert_eq!(
            dispatch_state(
                Some(true),
                true,
                2,
                99_999,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Moving
        );
    }

    #[test]
    fn alive_dirty_no_commits_is_moving_regardless_of_elapsed() {
        // A single snapshot can't tell if the diff is still growing — benefit
        // of the doubt goes to Moving whenever there IS a diff, no matter how
        // long the session has been running.
        assert_eq!(
            dispatch_state(
                Some(true),
                true,
                0,
                0,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Moving
        );
        assert_eq!(
            dispatch_state(
                Some(true),
                true,
                0,
                999_999,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Moving
        );
    }

    #[test]
    fn alive_clean_no_commits_under_threshold_is_moving() {
        // Fresh session, nothing produced yet, but still within the grace
        // window — too early to call it stalled.
        assert_eq!(
            dispatch_state(
                Some(true),
                false,
                0,
                60,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Moving
        );
    }

    #[test]
    fn alive_clean_no_commits_past_threshold_is_stalled() {
        assert_eq!(
            dispatch_state(
                Some(true),
                false,
                0,
                DEFAULT_STALLED_THRESHOLD_SECS,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Stalled
        );
        assert_eq!(
            dispatch_state(
                Some(true),
                false,
                0,
                DEFAULT_STALLED_THRESHOLD_SECS + 1,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Stalled
        );
    }

    #[test]
    fn state_label_round_trips() {
        assert_eq!(DispatchState::Moving.label(), "moving");
        assert_eq!(DispatchState::Stalled.label(), "stalled");
        assert_eq!(DispatchState::Salvageable.label(), "salvageable");
        // trace:BUG-752 | ai:claude
        assert_eq!(DispatchState::Unknown.label(), "unknown");
    }

    // BUG-752: undeterminable pid liveness (a harness lease with no recorded
    // pid) must classify Unknown — never Salvageable, even with a dirty
    // worktree. Absence of evidence is not death; the salvage hint fires only
    // on a genuinely determined dead process. trace:BUG-752 | ai:claude
    #[test]
    fn unknown_pid_liveness_is_never_salvageable() {
        // Dirty worktree — the exact shape the false negative misread as
        // "dead process, salvage-commit then rebrief" mid-drain.
        assert_eq!(
            dispatch_state(
                None,
                true,
                0,
                999_999,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Unknown
        );
        // Clean, ahead, fresh, aged — Unknown regardless of git state.
        assert_eq!(
            dispatch_state(
                None,
                false,
                3,
                10,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Unknown
        );
        assert_eq!(
            dispatch_state(
                None,
                false,
                0,
                999_999,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Unknown
        );
    }

    // trace:BUG-752 | ai:claude
    #[test]
    fn unknown_hint_never_contains_the_salvage_commit_command() {
        let hint = next_command_hint(
            DispatchState::Unknown,
            Path::new("/tmp/wt-harness"),
            "worktree-agent-abc",
            None,
            None,
            false,
        )
        .expect("Unknown must surface an explanatory hint");
        assert!(hint.contains("liveness unknown"), "{hint}");
        assert!(hint.contains("no pid recorded"), "{hint}");
        assert!(hint.contains("/tmp/wt-harness"), "{hint}");
        // The dangerous parts must be absent: no salvage-commit, no rebrief
        // command that would double-dispatch a possibly-live agent.
        assert!(!hint.contains("add -A"), "{hint}");
        assert!(!hint.contains("salvage-commit"), "{hint}");
        assert!(!hint.contains("aida queue work"), "{hint}");
        assert!(!hint.contains("aida agent new"), "{hint}");
    }

    // ── probe_worktree: read-only, tolerant of a missing worktree ─────────

    #[test]
    fn probe_missing_worktree_degrades_to_zero_default() {
        let probe = probe_worktree(Path::new("/nonexistent/aida-dispatch-health-probe"));
        assert_eq!(probe, WorktreeGitProbe::default());
        assert!(!probe.dirty);
        assert_eq!(probe.ahead_of_main, 0);
        assert_eq!(probe.last_commit_subject, None);
    }

    fn git(dir: &Path, args: &[&str]) {
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
    fn probe_clean_worktree_reports_clean_and_last_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        git(repo, &["init", "-q"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("f.txt"), "hello\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", "initial commit"]);

        let probe = probe_worktree(repo);
        assert!(!probe.dirty, "freshly committed tree is clean");
        assert_eq!(probe.last_commit_subject.as_deref(), Some("initial commit"));
    }

    #[test]
    fn probe_dirty_worktree_reports_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        git(repo, &["init", "-q"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("f.txt"), "hello\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", "initial commit"]);

        std::fs::write(repo.join("f.txt"), "uncommitted change\n").unwrap();
        let probe = probe_worktree(repo);
        assert!(probe.dirty, "an uncommitted edit must flip dirty=true");
    }

    // ── next_command_hint: no interpretation left to the reader ───────────

    #[test]
    fn moving_has_no_hint() {
        assert_eq!(
            next_command_hint(
                DispatchState::Moving,
                Path::new("/tmp/wt"),
                "story-1",
                Some("wip"),
                Some("STORY-1"),
                false
            ),
            None
        );
    }

    #[test]
    fn salvageable_hint_names_worktree_branch_commit_and_salvage_command() {
        let hint = next_command_hint(
            DispatchState::Salvageable,
            Path::new("/tmp/wt-salvage"),
            "task-1090-x",
            Some("wip: partial edit"),
            Some("TASK-1090"),
            false,
        )
        .expect("Salvageable must always produce a hint");
        assert!(hint.contains("/tmp/wt-salvage"), "{hint}");
        assert!(hint.contains("task-1090-x"), "{hint}");
        assert!(hint.contains("wip: partial edit"), "{hint}");
        assert!(hint.contains("git -C /tmp/wt-salvage add -A"), "{hint}");
        assert!(hint.contains("git -C /tmp/wt-salvage commit"), "{hint}");
        assert!(hint.contains("aida queue work TASK-1090"), "{hint}");
        // Never the write-side patch-preserve path — this is a read-only
        // report; the hint is a plain commit, not an invocation of
        // `preserve_dirty_worktree`.
        assert!(!hint.contains("preserve_dirty_worktree"), "{hint}");
    }

    #[test]
    fn salvageable_hint_falls_back_to_generic_rebrief_without_a_resolved_spec() {
        let hint = next_command_hint(
            DispatchState::Salvageable,
            Path::new("/tmp/wt-noscope"),
            "harness-worktree",
            None,
            None,
            false,
        )
        .unwrap();
        assert!(
            hint.contains("aida agent new claude --cwd /tmp/wt-noscope"),
            "{hint}"
        );
        assert!(hint.contains("(no commits yet)"), "{hint}");
    }

    #[test]
    fn stalled_hint_names_worktree_branch_and_resume_command() {
        let hint = next_command_hint(
            DispatchState::Stalled,
            Path::new("/tmp/wt-stalled"),
            "story-42",
            Some("prior commit"),
            Some("STORY-42"),
            false,
        )
        .expect("Stalled must always produce a hint");
        assert!(hint.contains("/tmp/wt-stalled"), "{hint}");
        assert!(hint.contains("story-42"), "{hint}");
        assert!(hint.contains("aida queue work STORY-42"), "{hint}");
        // Stalled is a resume hint, not a salvage-commit instruction — no
        // dirty-work commit is implied.
        assert!(!hint.contains("git -C /tmp/wt-stalled add"), "{hint}");
    }

    // ── BUG-778: hand-entered worktrees ──────────────────────────────────

    /// The reported false positive: `aida worktree enter <SPEC>` mints the
    /// lease and stops, and for the ~30s before the operator launches their
    /// agent the row read "process dead / resume: aida queue work <SPEC>".
    /// A pristine hand-entered worktree inside the grace window is
    /// AwaitingAgent — never one of the dead-process states.
    // trace:BUG-778 | ai:claude
    #[test]
    fn hand_entered_worktree_inside_grace_is_awaiting_agent_not_dead() {
        // 30 seconds after `worktree enter`: no pid, clean tree, no commits.
        assert_eq!(
            dispatch_state(
                Some(false),
                false,
                0,
                30,
                DEFAULT_STALLED_THRESHOLD_SECS,
                Some(30),
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::AwaitingAgent
        );
        // Right up to the last second of the window.
        assert_eq!(
            dispatch_state(
                Some(false),
                false,
                0,
                DEFAULT_AWAITING_AGENT_GRACE_SECS - 1,
                DEFAULT_STALLED_THRESHOLD_SECS,
                Some(DEFAULT_AWAITING_AGENT_GRACE_SECS - 1),
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::AwaitingAgent
        );
        // Undeterminable liveness on a hand-entered lease is the same shape —
        // still nobody's corpse.
        assert_eq!(
            dispatch_state(
                None,
                false,
                0,
                30,
                DEFAULT_STALLED_THRESHOLD_SECS,
                Some(30),
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::AwaitingAgent
        );
    }

    /// The other half of the acceptance: the grace window is a window, not a
    /// blanket amnesty. Past it, a hand-entered worktree that still shows
    /// nothing ages into STALLED like any other idle row — and a genuinely
    /// dead AGENT lease (no hand-enter stamp) stalls exactly as it always did.
    // trace:BUG-778 | ai:claude
    #[test]
    fn dead_lease_still_stalls_after_the_grace_window() {
        // Hand-entered, but the operator never launched anything.
        assert_eq!(
            dispatch_state(
                Some(false),
                false,
                0,
                DEFAULT_AWAITING_AGENT_GRACE_SECS,
                DEFAULT_STALLED_THRESHOLD_SECS,
                Some(DEFAULT_AWAITING_AGENT_GRACE_SECS),
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Stalled
        );
        // An orchestrator-spawned lease whose agent died: unchanged behavior,
        // at any age — no hand-enter stamp, so no grace at all.
        assert_eq!(
            dispatch_state(
                Some(false),
                false,
                0,
                5,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Stalled
        );
        assert_eq!(
            dispatch_state(
                Some(false),
                true,
                0,
                5,
                DEFAULT_STALLED_THRESHOLD_SECS,
                None,
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Salvageable
        );
    }

    /// The grace only covers a PRISTINE worktree. A hand-entered session that
    /// produced something and then lost its process keeps the salvage urgency
    /// (that diff is still one cleanup away from gone), and one that is
    /// demonstrably alive is Moving.
    // trace:BUG-778 | ai:claude
    #[test]
    fn hand_entered_grace_never_masks_work_in_flight() {
        // Dirty inside the window → Salvageable, not AwaitingAgent.
        assert_eq!(
            dispatch_state(
                Some(false),
                true,
                0,
                10,
                DEFAULT_STALLED_THRESHOLD_SECS,
                Some(10),
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Salvageable
        );
        // Commits already on the branch inside the window → the ordinary
        // matrix (dead + clean = Stalled).
        assert_eq!(
            dispatch_state(
                Some(false),
                false,
                2,
                10,
                DEFAULT_STALLED_THRESHOLD_SECS,
                Some(10),
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Stalled
        );
        // The operator launched their agent → live → Moving.
        assert_eq!(
            dispatch_state(
                Some(true),
                false,
                0,
                10,
                DEFAULT_STALLED_THRESHOLD_SECS,
                Some(10),
                DEFAULT_AWAITING_AGENT_GRACE_SECS
            ),
            DispatchState::Moving
        );
    }

    // trace:BUG-778 | ai:claude
    #[test]
    fn awaiting_agent_hint_names_the_launch_and_never_redispatches() {
        let hint = next_command_hint(
            DispatchState::AwaitingAgent,
            Path::new("/tmp/wt-entered"),
            "task-1169-launcher",
            None,
            Some("TASK-1169"),
            true,
        )
        .expect("AwaitingAgent must surface an explanatory hint");
        assert!(hint.contains("entered by hand"), "{hint}");
        assert!(hint.contains("/tmp/wt-entered"), "{hint}");
        assert!(hint.contains("aida session end TASK-1169"), "{hint}");
        // The dangerous parts: re-dispatch would start a session competing
        // with the human, and nothing here died.
        assert!(!hint.contains("aida queue work"), "{hint}");
        assert!(!hint.contains("process dead"), "{hint}");
        assert!(!hint.contains("dead process"), "{hint}");
        assert!(!hint.contains("add -A"), "{hint}");
    }

    /// Past the grace window a hand-entered row does get a stalled/salvageable
    /// hint — but still never the re-dispatch verb, and never the
    /// "dead process" framing for an agent that was never launched.
    // trace:BUG-778 | ai:claude
    #[test]
    fn hand_entered_hints_withhold_redispatch_at_any_age() {
        let stalled = next_command_hint(
            DispatchState::Stalled,
            Path::new("/tmp/wt-entered"),
            "task-1169-launcher",
            Some("prior commit"),
            Some("TASK-1169"),
            true,
        )
        .expect("Stalled always produces a hint");
        assert!(!stalled.contains("aida queue work"), "{stalled}");
        assert!(stalled.contains("cd /tmp/wt-entered"), "{stalled}");
        assert!(stalled.contains("competing"), "{stalled}");

        let salvageable = next_command_hint(
            DispatchState::Salvageable,
            Path::new("/tmp/wt-entered"),
            "task-1169-launcher",
            None,
            Some("TASK-1169"),
            true,
        )
        .expect("Salvageable always produces a hint");
        assert!(!salvageable.contains("aida queue work"), "{salvageable}");
        assert!(!salvageable.contains("dead process"), "{salvageable}");
        // The diff is still at risk — the salvage-commit stays on offer.
        assert!(
            salvageable.contains("git -C /tmp/wt-entered add -A"),
            "{salvageable}"
        );
    }

    // trace:BUG-778 | ai:claude
    #[test]
    fn awaiting_agent_label_round_trips() {
        assert_eq!(DispatchState::AwaitingAgent.label(), "awaiting-agent");
    }
}
