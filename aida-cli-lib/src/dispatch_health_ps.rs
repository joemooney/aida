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
/// Decision matrix (documented so a future reader doesn't have to reverse it
/// out of the `if` chain):
///
/// | pid alive | branch ahead | worktree dirty | elapsed        | state       |
/// |-----------|--------------|-----------------|----------------|-------------|
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
// trace:TASK-1090 | ai:claude
// trace:BUG-752 | ai:claude
pub(crate) fn dispatch_state(
    pid_alive: Option<bool>,
    worktree_dirty: bool,
    branch_ahead_of_main: u32,
    elapsed_secs: u64,
    stalled_threshold_secs: u64,
) -> DispatchState {
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
// trace:TASK-1090 | ai:claude
pub(crate) fn next_command_hint(
    state: DispatchState,
    worktree_path: &Path,
    branch: &str,
    last_commit_subject: Option<&str>,
    spec: Option<&str>,
) -> Option<String> {
    let wt = worktree_path.display();
    let last_commit = last_commit_subject.unwrap_or("(no commits yet)");
    let rebrief = match spec {
        Some(id) => format!("aida queue work {id}"),
        None => format!("aida agent new claude --cwd {wt}"),
    };
    match state {
        DispatchState::Moving => None,
        DispatchState::Salvageable => Some(format!(
            "dead process, uncommitted work in {wt} (branch {branch}, last commit \"{last_commit}\") — \
             salvage-commit then rebrief: git -C {wt} add -A && git -C {wt} commit -m \"wip: salvage {branch}\" \
             — then: {rebrief}"
        )),
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
            dispatch_state(Some(false), true, 0, 999, DEFAULT_STALLED_THRESHOLD_SECS),
            DispatchState::Salvageable
        );
        // Dirty + ahead: still Salvageable — a dead process always wins on
        // the salvage-urgency axis regardless of what's already pushed.
        assert_eq!(
            dispatch_state(Some(false), true, 3, 10, DEFAULT_STALLED_THRESHOLD_SECS),
            DispatchState::Salvageable
        );
    }

    #[test]
    fn dead_pid_clean_worktree_is_stalled_not_moving() {
        assert_eq!(
            dispatch_state(Some(false), false, 0, 5, DEFAULT_STALLED_THRESHOLD_SECS),
            DispatchState::Stalled
        );
        // Nothing to lose even if commits already landed — a dead process
        // still needs a resume decision, so this stays Stalled rather than
        // Moving.
        assert_eq!(
            dispatch_state(Some(false), false, 4, 5, DEFAULT_STALLED_THRESHOLD_SECS),
            DispatchState::Stalled
        );
    }

    #[test]
    fn alive_branch_ahead_of_main_is_moving() {
        assert_eq!(
            dispatch_state(Some(true), false, 1, 99_999, DEFAULT_STALLED_THRESHOLD_SECS),
            DispatchState::Moving
        );
        // Dirty on top of ahead — still Moving.
        assert_eq!(
            dispatch_state(Some(true), true, 2, 99_999, DEFAULT_STALLED_THRESHOLD_SECS),
            DispatchState::Moving
        );
    }

    #[test]
    fn alive_dirty_no_commits_is_moving_regardless_of_elapsed() {
        // A single snapshot can't tell if the diff is still growing — benefit
        // of the doubt goes to Moving whenever there IS a diff, no matter how
        // long the session has been running.
        assert_eq!(
            dispatch_state(Some(true), true, 0, 0, DEFAULT_STALLED_THRESHOLD_SECS),
            DispatchState::Moving
        );
        assert_eq!(
            dispatch_state(Some(true), true, 0, 999_999, DEFAULT_STALLED_THRESHOLD_SECS),
            DispatchState::Moving
        );
    }

    #[test]
    fn alive_clean_no_commits_under_threshold_is_moving() {
        // Fresh session, nothing produced yet, but still within the grace
        // window — too early to call it stalled.
        assert_eq!(
            dispatch_state(Some(true), false, 0, 60, DEFAULT_STALLED_THRESHOLD_SECS),
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
                DEFAULT_STALLED_THRESHOLD_SECS
            ),
            DispatchState::Stalled
        );
        assert_eq!(
            dispatch_state(
                Some(true),
                false,
                0,
                DEFAULT_STALLED_THRESHOLD_SECS + 1,
                DEFAULT_STALLED_THRESHOLD_SECS
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
            dispatch_state(None, true, 0, 999_999, DEFAULT_STALLED_THRESHOLD_SECS),
            DispatchState::Unknown
        );
        // Clean, ahead, fresh, aged — Unknown regardless of git state.
        assert_eq!(
            dispatch_state(None, false, 3, 10, DEFAULT_STALLED_THRESHOLD_SECS),
            DispatchState::Unknown
        );
        assert_eq!(
            dispatch_state(None, false, 0, 999_999, DEFAULT_STALLED_THRESHOLD_SECS),
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
                Some("STORY-1")
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
        )
        .expect("Stalled must always produce a hint");
        assert!(hint.contains("/tmp/wt-stalled"), "{hint}");
        assert!(hint.contains("story-42"), "{hint}");
        assert!(hint.contains("aida queue work STORY-42"), "{hint}");
        // Stalled is a resume hint, not a salvage-commit instruction — no
        // dirty-work commit is implied.
        assert!(!hint.contains("git -C /tmp/wt-stalled add"), "{hint}");
    }
}
