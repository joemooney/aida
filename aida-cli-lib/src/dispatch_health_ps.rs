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

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
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
    /// The drain wave that ran this session was STOPPED (SIGTERM: systemd
    /// `RuntimeMaxSec` / `OOMPolicy=stop`, `aida drain stop --now`, a manual
    /// kill) and its handler stamped the lease `interrupted_at` before
    /// releasing the drain lock. The process is gone and the tree is clean —
    /// not a crash, not a stall: an interrupted session whose worktree is
    /// intact and whose next step is the ordinary resume. A dirty tree still
    /// reads [`Salvageable`](Self::Salvageable): the diff at risk outranks the
    /// provenance of the death.
    // trace:TASK-1518 | ai:claude
    Stopped,
    /// The work is FINISHED and waiting on the integrator, not on this
    /// session: the spec is done with an approving review recorded, or its
    /// branch is riding an open integration PR. The process is gone and the
    /// tree is clean — the same shape as [`Stalled`](Self::Stalled) — but
    /// there is nothing here to pick back up, and saying otherwise is
    /// actively harmful: an agent that follows a resume hint on landed work
    /// re-implements it. Carries an informational note and no command.
    // trace:BUG-1681 | ai:claude
    AwaitingIntegration,
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
            // trace:TASK-1518 | ai:claude
            DispatchState::Stopped => "stopped",
            // trace:BUG-1681 | ai:claude
            DispatchState::AwaitingIntegration => "awaiting-integration",
        }
    }
}

/// TASK-1518: fold the lease's interruption stamp into the classified state.
/// A drain stopped by SIGTERM stamps `interrupted_at` on its in-flight leases
/// as it releases the drain lock; a lease so stamped whose process is
/// demonstrably gone and whose tree is clean (the [`DispatchState::Stalled`]
/// dead-process arm) reads [`DispatchState::Stopped`] instead — an
/// interrupted session with an intact worktree, not a crashed agent. Every
/// other state is unchanged: a dirty tree keeps its salvage urgency, a live
/// process (the wave's vendor child outliving a manual kill) keeps its
/// movement reading, and undeterminable liveness stays unknown. Pure so the
/// matrix is unit-testable on fixtures.
// trace:TASK-1518 | ai:claude
pub(crate) fn apply_interruption(
    state: DispatchState,
    pid_alive: Option<bool>,
    interrupted: bool,
) -> DispatchState {
    if interrupted && state == DispatchState::Stalled && pid_alive == Some(false) {
        DispatchState::Stopped
    } else {
        state
    }
}

/// BUG-1681: what is known about a finished session's INTEGRATION standing —
/// the evidence that says "this is waiting on the integrator, not on you".
/// Built by [`integration_standing`] from facts the caller already has locally
/// (the spec's status and its recorded review), never from a forge round-trip.
// trace:BUG-1681 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IntegrationStanding {
    /// The integration PR carrying this work, when the local record names one.
    pub(crate) pr: Option<u64>,
    /// The integration branch the approving review was recorded against, when
    /// that is not this session's own branch — the batched-integration shape,
    /// where a reviewer approves the spec on the batch branch rather than on
    /// the implementer's.
    pub(crate) batch_branch: Option<String>,
}

/// BUG-1681: is this session's work finished and waiting on integration?
///
/// Evidence-gated, in the PRIN-5 sense that absent evidence is never good
/// evidence: an approving recorded review is necessary, and on its own not
/// sufficient. One of two further facts must hold — the spec itself is done
/// (the ordinary "implementer finished, batch not landed yet" shape), or the
/// approval was recorded against a DIFFERENT branch from this session's, which
/// is what a batched integration PR looks like from here (the reviewer reviews
/// the batch branch, not the implementer's). An approval on the session's own
/// branch while the spec is still open is an ordinary mid-flight review and
/// says nothing about integration.
///
/// Pure over five plain values so the whole matrix is fixture-testable.
// trace:BUG-1681 | ai:claude
pub(crate) fn integration_standing(
    spec_finished: bool,
    verdict_approves: bool,
    reviewed_branch: Option<&str>,
    lease_branch: &str,
    comment_url: Option<&str>,
) -> Option<IntegrationStanding> {
    if !verdict_approves {
        return None;
    }
    let batch_branch = reviewed_branch
        .map(str::trim)
        .filter(|b| !b.is_empty() && !b.eq_ignore_ascii_case(lease_branch.trim()))
        .map(str::to_string);
    if !spec_finished && batch_branch.is_none() {
        return None;
    }
    Some(IntegrationStanding {
        pr: comment_url.and_then(pr_number_from_comment_url),
        batch_branch,
    })
}

/// BUG-1681: the PR/MR number a recorded review's comment URL points at, so
/// the row can name the PR the reader should look at. Recognizes both forge
/// shapes (`/pull/<n>`, `/merge_requests/<n>`); anything else yields `None`
/// rather than a guess.
// trace:BUG-1681 | ai:claude
pub(crate) fn pr_number_from_comment_url(url: &str) -> Option<u64> {
    let rest = url
        .split("/pull/")
        .nth(1)
        .or_else(|| url.split("/merge_requests/").nth(1))?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// BUG-1681: fold the integration standing into the classified state. Only the
/// two "nothing is moving here" readings are reframed — a
/// [`Salvageable`](DispatchState::Salvageable) row keeps its urgency (an
/// uncommitted diff is at risk however finished the spec is), and a moving or
/// undeterminable row is left exactly as classified.
// trace:BUG-1681 | ai:claude
pub(crate) fn apply_integration(
    state: DispatchState,
    standing: Option<&IntegrationStanding>,
) -> DispatchState {
    match (state, standing) {
        (DispatchState::Stalled | DispatchState::Stopped, Some(_)) => {
            DispatchState::AwaitingIntegration
        }
        _ => state,
    }
}

/// BUG-1681: the note a [`DispatchState::AwaitingIntegration`] row carries.
/// It names the standing (and the PR or batch branch when known) and stops
/// there — deliberately no command, because there is nothing for the reader to
/// run and a resume would restart work that has already shipped.
// trace:BUG-1681 | ai:claude
pub(crate) fn integration_hint(standing: Option<&IntegrationStanding>) -> String {
    let where_it_sits = match standing {
        Some(s) => match (s.pr, s.batch_branch.as_deref()) {
            (Some(pr), _) => format!(" (PR #{pr})"),
            (None, Some(branch)) => format!(" (batch branch {branch})"),
            (None, None) => String::new(),
        },
        None => String::new(),
    };
    format!(
        "done and approved — awaiting integration{where_it_sits}; \
         nothing here needs picking back up, and the worktree is cleaned up \
         once the batch lands"
    )
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
    /// BUG-1656: seconds since the NEWEST dirty (modified / untracked) file
    /// in the worktree was written. `None` when the tree is clean or no dirty
    /// path could be stat'ed. A small value means the tree is still changing
    /// under someone's hands — an Agent-tool subagent editing inside a leased
    /// worktree leaves exactly this signature while its spec lease's pid
    /// reads dead.
    // trace:BUG-1656 | ai:claude
    pub(crate) dirty_newest_mtime_age_secs: Option<u64>,
    /// BUG-1680: True when the dirty state consists exclusively of untracked files
    /// (no tracked modified/deleted/staged changes).
    // trace:BUG-1680 | ai:antigravity
    pub(crate) untracked_only: bool,
}

/// BUG-1656: how recently the newest dirty file must have been written for
/// the worktree to count as "still moving" regardless of pid liveness. Ten
/// minutes: an agent mid-change writes files every few seconds to a couple
/// of minutes apart; a genuinely dead session's diff stops aging-in within
/// that window and reverts to the ordinary dead-process matrix.
// trace:BUG-1656 | ai:claude
pub(crate) const DEFAULT_DIRTY_MOVEMENT_FRESH_SECS: u64 = 10 * 60;

/// BUG-1656: is the dirty tree still changing? Pure over the probe's newest
/// dirty mtime age; `None` (clean, or unknown) is never fresh.
// trace:BUG-1656 | ai:claude
pub(crate) fn dirty_movement_is_fresh(newest_age_secs: Option<u64>, fresh_secs: u64) -> bool {
    newest_age_secs.is_some_and(|age| age < fresh_secs)
}

/// BUG-1656: the paths `git status --porcelain -z` reports as changed or
/// untracked, relative to `worktree`, that count toward "the tree is still
/// changing". Renames and copies yield the destination path. Empty when the
/// probe fails.
///
/// The output is read raw (never trimmed) and NUL-separated: trimming strips
/// the leading space of an unstaged ` M` / ` D` record and shifts every
/// column, which silently cut the first character off the path.
// trace:BUG-1656 | ai:claude
pub(crate) fn dirty_paths(worktree: &Path) -> Vec<std::path::PathBuf> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["status", "--porcelain", "-z", "--untracked-files=all"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_porcelain_z(&String::from_utf8_lossy(&out.stdout))
        .into_iter()
        .filter(|(xy, path)| counts_toward_recent_movement(xy, path))
        .map(|(_, path)| worktree.join(path))
        .collect()
}

/// BUG-1656: parse `git status --porcelain -z` into `(XY, path)` records.
/// Each record is `XY<space>path\0`; a rename or copy (`R` / `C` in either
/// status column) is followed by one extra `\0`-terminated field holding the
/// ORIGINAL path, which is consumed and dropped — the first path is the
/// destination, the file that exists now.
// trace:BUG-1656 | ai:claude
pub(crate) fn parse_porcelain_z(out: &str) -> Vec<(String, String)> {
    let mut records = Vec::new();
    let mut fields = out.split('\0');
    while let Some(field) = fields.next() {
        if field.len() < 4 || !field.is_char_boundary(2) || !field.is_char_boundary(3) {
            continue;
        }
        let xy = &field[..2];
        let path = &field[3..];
        if xy.contains('R') || xy.contains('C') {
            let _original = fields.next();
        }
        records.push((xy.to_string(), path.to_string()));
    }
    records
}

/// BUG-1680: parse `git status --porcelain -z` directly from bytes into `(XY, PathBuf)`
/// records, preserving non-UTF-8 filenames without lossy string conversion.
// trace:BUG-1680 | ai:antigravity
pub(crate) fn parse_porcelain_z_bytes(out: &[u8]) -> Vec<(String, PathBuf)> {
    let mut records = Vec::new();
    let mut fields = out.split(|&b| b == 0);
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let xy = match std::str::from_utf8(&field[..2]) {
            Ok(s) => s.to_string(),
            Err(_) => continue,
        };
        if field[2] != b' ' {
            continue;
        }
        #[cfg(unix)]
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(&field[3..]));
        #[cfg(not(unix))]
        let path = PathBuf::from(String::from_utf8_lossy(&field[3..]).into_owned());

        if xy.contains('R') || xy.contains('C') {
            let _original = fields.next();
        }
        records.push((xy, path));
    }
    records
}

/// BUG-1656: does this porcelain record count toward "recent movement"?
// The exact rule: every TRACKED change (any XY other than `??` untracked and
// `!!` ignored) counts; an UNTRACKED file (`??`) counts only when no path
// component is hidden (starts with `.`) and its file name is not editor
// swap/backup/lock style (`.*.swp`, `*~`, `.#*`). Ignored files never count.
// So an editor swap file or an unignored tool directory (`.codegraph/`, ...)
// cannot keep an abandoned worktree looking active forever.
// trace:BUG-1656 | ai:claude
pub(crate) fn counts_toward_recent_movement(xy: &str, path: &str) -> bool {
    match xy {
        "!!" => false,
        "??" => {
            let path = path.trim_end_matches('/');
            let hidden = path.split('/').any(|c| c.starts_with('.'));
            let name = path.rsplit('/').next().unwrap_or(path);
            let swap = (name.starts_with('.') && name.ends_with(".swp"))
                || name.ends_with('~')
                || name.starts_with(".#");
            !hidden && !swap
        }
        _ => true,
    }
}

/// BUG-1656: age in seconds of the newest file among `paths`, relative to
/// `now`. Deleted paths (dirty because they are gone) are skipped, and so is
/// a modification time in the FUTURE (clock skew, a restored archive): it is
/// unknown, never "recent" — a future stamp would otherwise read as age 0
/// and keep the tree looking active indefinitely.
// trace:BUG-1656 | ai:claude
pub(crate) fn newest_mtime_age_secs(
    paths: &[std::path::PathBuf],
    now: std::time::SystemTime,
) -> Option<u64> {
    paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok()?.modified().ok())
        .filter_map(|m| now.duration_since(m).ok().map(|d| d.as_secs()))
        .min()
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
    // trace:BUG-1656 | ai:claude
    let dirty_newest_mtime_age_secs = if dirty {
        newest_mtime_age_secs(&dirty_paths(worktree_path), std::time::SystemTime::now())
    } else {
        None
    };
    // BUG-1680: distinguish untracked-only files from salvageable changes.
    // trace:BUG-1680 | ai:antigravity
    let untracked_only = if dirty {
        probe_untracked_only(worktree_path)
    } else {
        false
    };
    WorktreeGitProbe {
        dirty,
        ahead_of_main,
        last_commit_subject,
        dirty_newest_mtime_age_secs,
        untracked_only,
    }
}

/// BUG-1680: True when an untracked file is inside a directory tracked in git.
/// Root untracked files are checked against tracked files in the repository;
/// fails closed (returns true) if git ls-files fails.
// trace:BUG-1680 | ai:antigravity
pub(crate) fn is_untracked_inside_tracked_dir<P: AsRef<Path>>(
    worktree: &Path,
    rel_path: P,
) -> bool {
    let path = rel_path.as_ref();
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() && p != Path::new(".") => p,
        _ => {
            // Root untracked file: check if the repository root has tracked files.
            return match Command::new("git")
                .arg("-C")
                .arg(worktree)
                .args(["ls-files"])
                .output()
            {
                Ok(out) => {
                    if !out.status.success() {
                        // Fail closed: unresolved git state treated as tracked work
                        true
                    } else {
                        !out.stdout.is_empty()
                    }
                }
                Err(_) => true, // Fail closed on process execution error
            };
        }
    };

    let mut current = Some(parent);
    while let Some(dir) = current {
        if dir.as_os_str().is_empty() || dir == Path::new(".") {
            break;
        }
        let out = Command::new("git")
            .arg("-C")
            .arg(worktree)
            .arg("ls-files")
            .arg(dir)
            .output();
        match out {
            Ok(output) => {
                if !output.status.success() {
                    // Fail closed if git ls-files fails
                    return true;
                }
                if !output.stdout.is_empty() {
                    return true;
                }
            }
            Err(_) => {
                // Fail closed if git command execution fails
                return true;
            }
        }
        current = dir.parent();
    }
    false
}

/// BUG-1680: True when every changed path in the worktree is an untracked file (`??`)
/// outside of tracked directories, and none are tracked modifications, deletions, or staged changes.
// trace:BUG-1680 | ai:antigravity
pub(crate) fn probe_untracked_only(worktree: &Path) -> bool {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["status", "--porcelain", "-z", "--untracked-files=all"])
        .output()
    else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let records = parse_porcelain_z_bytes(&out.stdout);
    if records.is_empty() {
        return false;
    }
    let has_tracked_change = records.iter().any(|(xy, _)| xy != "??" && xy != "!!");
    if has_tracked_change {
        return false;
    }
    let untracked_in_tracked_dir = records
        .iter()
        .filter(|(xy, _)| xy == "??")
        .any(|(_, path)| is_untracked_inside_tracked_dir(worktree, path));
    !untracked_in_tracked_dir
}

/// BUG-1656: [`dispatch_state`] plus the "is the dirty tree still changing"
/// signal. A worktree whose newest dirty file was written within the
/// freshness window is MOVING whatever the pid says: the lease's recorded
/// process may be dead while an Agent-tool subagent (which runs inside the
/// parent claude process and never appears in the worktree's cwd probe) is
/// editing in place. Reading that as Salvageable produced the
/// `git add -A && git commit -m "wip: salvage"` hint against a tree an agent
/// was still writing — following it would commit half-done work under the
/// agent and race its edits. The hand-entered AwaitingAgent short-circuit
/// stays first (it requires a clean tree, so the two never overlap).
// trace:BUG-1656 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn dispatch_state_with_movement(
    pid_alive: Option<bool>,
    worktree_dirty: bool,
    branch_ahead_of_main: u32,
    elapsed_secs: u64,
    stalled_threshold_secs: u64,
    manual_enter_secs: Option<u64>,
    awaiting_agent_grace_secs: u64,
    dirty_movement_fresh: bool,
) -> DispatchState {
    if worktree_dirty && dirty_movement_fresh && pid_alive != Some(true) {
        return DispatchState::Moving;
    }
    dispatch_state(
        pid_alive,
        worktree_dirty,
        branch_ahead_of_main,
        elapsed_secs,
        stalled_threshold_secs,
        manual_enter_secs,
        awaiting_agent_grace_secs,
    )
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
// trace:BUG-1680 | ai:antigravity
pub(crate) const PROTECTED_BRANCHES: &[&str] =
    &["main", "master", "trunk", "develop", "aida-store", "HEAD"];

/// Return the branch name currently checked out at `worktree` (its HEAD).
/// `None` if detached or git fails.
// trace:BUG-1680 | ai:antigravity
pub(crate) fn current_branch_at(worktree: &Path) -> Option<String> {
    git_stdout(worktree, &["symbolic-ref", "--short", "HEAD"])
}

fn normalize_branch_name(branch: &str) -> &str {
    let b = branch.trim();
    if let Some(short) = b.strip_prefix("refs/heads/") {
        return short;
    }
    if let Some(remotes) = b.strip_prefix("refs/remotes/") {
        if let Some((_remote, name)) = remotes.split_once('/') {
            return name;
        }
    }
    if let Some((remote, name)) = b.split_once('/') {
        if remote.eq_ignore_ascii_case("origin") || remote.eq_ignore_ascii_case("upstream") {
            return name;
        }
    }
    b
}

fn is_protected_branch_name(branch: &str) -> bool {
    let normalized = normalize_branch_name(branch);
    if PROTECTED_BRANCHES
        .iter()
        .any(|p| p.eq_ignore_ascii_case(normalized))
    {
        return true;
    }
    PROTECTED_BRANCHES
        .iter()
        .any(|p| p.eq_ignore_ascii_case(branch.trim()))
}

/// BUG-1680: Is `branch` a protected or default branch where automated salvage
/// commits should never be recommended? Checks standard branch names as well as
/// dynamically discovered default branch of `worktree` via the default-branch ref probe.
// trace:BUG-1680 | ai:antigravity
pub(crate) fn is_protected_branch_at(worktree: &Path, branch: &str) -> bool {
    let branch = branch.trim();
    if branch.is_empty() {
        return false;
    }
    if is_protected_branch_name(branch) {
        return true;
    }
    // Dynamic default-branch ref discovery for the repository
    if let Some(default_ref) = crate::detect_default_branch_ref(worktree) {
        let default_short = default_ref
            .strip_prefix("origin/")
            .unwrap_or(&default_ref)
            .strip_prefix("upstream/")
            .unwrap_or(&default_ref);
        let normalized = normalize_branch_name(branch);
        if normalized.eq_ignore_ascii_case(default_short)
            || branch.eq_ignore_ascii_case(default_short)
            || branch.eq_ignore_ascii_case(&default_ref)
        {
            return true;
        }
    }
    false
}

/// BUG-1680: Hint with untracked-only distinction and protected-branch guard.
// trace:BUG-1680 | ai:antigravity
pub(crate) fn next_command_hint_with_untracked(
    state: DispatchState,
    worktree_path: &Path,
    branch: &str,
    last_commit_subject: Option<&str>,
    spec: Option<&str>,
    manual_enter: bool,
    untracked_only: bool,
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

    // BUG-1680: Resolve actual checked-out branch at worktree path before offering salvage.
    // A stale lease naming a feature branch must not offer salvage-commit if the worktree
    // is currently on main or a protected default branch.
    // trace:BUG-1680 | ai:antigravity
    let actual_branch = current_branch_at(worktree_path);
    let effective_branch = actual_branch.as_deref().unwrap_or(branch);
    let is_protected = is_protected_branch_at(worktree_path, effective_branch)
        || is_protected_branch_at(worktree_path, branch);

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
                "entered by hand — worktree {wt} (branch {effective_branch}) is yours and no agent has been \
                 launched in it yet; start one there, or {release} to hand the lease back"
            ))
        }
        // BUG-778: a hand-entered worktree never had an agent to lose, so its
        // lead-in says "nothing running" rather than "dead process" — the diff
        // is still at risk and still worth salvaging, but the framing must not
        // imply a crash that never happened. trace:BUG-778 | ai:claude
        DispatchState::Salvageable => {
            // BUG-1680: distinguish untracked-only files from salvageable changes.
            // trace:BUG-1680 | ai:antigravity
            let work_desc = if untracked_only {
                "untracked files only"
            } else {
                "uncommitted work"
            };
            let lead = if manual_enter {
                format!("nothing running, {work_desc}")
            } else {
                format!("dead process, {work_desc}")
            };
            if is_protected {
                // BUG-1680: Never recommend a salvage commit on the default branch
                // (or any protected branch); instead suggest inspecting the files.
                // trace:BUG-1680 | ai:antigravity
                Some(format!(
                    "{lead} in {wt} (branch {effective_branch}, last commit \"{last_commit}\") — \
                     inspect the files: git -C {wt} status \
                     — then: {rebrief}"
                ))
            } else {
                Some(format!(
                    "{lead} in {wt} (branch {effective_branch}, last commit \"{last_commit}\") — \
                     salvage-commit then rebrief: git -C {wt} add -A && git -C {wt} commit -m \"wip: salvage {effective_branch}\" \
                     — then: {rebrief}"
                ))
            }
        }
        DispatchState::Stalled => Some(format!(
            "no branch/dirty movement in {wt} (branch {branch}, last commit \"{last_commit}\") — resume/rebrief: {rebrief}"
        )),
        // TASK-1518: the wave was stopped, not crashed — the drain's SIGTERM
        // handler released the lock and marked this lease on the way out, so
        // the worktree is intact and the next step is the plain resume.
        // trace:TASK-1518 | ai:claude
        DispatchState::Stopped => Some(format!(
            "drain wave stopped — nothing running in {wt} (branch {branch}, last commit \"{last_commit}\"), \
             worktree intact — resume/rebrief: {rebrief}"
        )),
        // BUG-752: no pid was recorded and the worktree probe can't see a
        // harness-hosted worker — liveness is unknown, NOT dead. Never emit
        // the salvage-commit command here: an agent may still be writing this
        // worktree, and salvage-committing under it would capture half-done
        // work and double-dispatch. trace:BUG-752 | ai:claude
        // BUG-1681: finished work waiting on the integrator. The caller that
        // resolved the standing renders the detailed note via
        // [`integration_hint`]; this generic form keeps the state's contract
        // (every non-Moving state explains itself) without inventing a PR.
        // trace:BUG-1681 | ai:claude
        DispatchState::AwaitingIntegration => Some(integration_hint(None)),
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
        let hint = next_command_hint_with_untracked(
            DispatchState::Unknown,
            Path::new("/tmp/wt-harness"),
            "worktree-agent-abc",
            None,
            None,
            false,
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
            next_command_hint_with_untracked(
                DispatchState::Moving,
                Path::new("/tmp/wt"),
                "story-1",
                Some("wip"),
                Some("STORY-1"),
                false,
                false,
            ),
            None
        );
    }

    #[test]
    fn salvageable_hint_names_worktree_branch_commit_and_salvage_command() {
        let hint = next_command_hint_with_untracked(
            DispatchState::Salvageable,
            Path::new("/tmp/wt-salvage"),
            "task-1090-x",
            Some("wip: partial edit"),
            Some("TASK-1090"),
            false,
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
        let hint = next_command_hint_with_untracked(
            DispatchState::Salvageable,
            Path::new("/tmp/wt-noscope"),
            "harness-worktree",
            None,
            None,
            false,
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
        let hint = next_command_hint_with_untracked(
            DispatchState::Stalled,
            Path::new("/tmp/wt-stalled"),
            "story-42",
            Some("prior commit"),
            Some("STORY-42"),
            false,
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
        let hint = next_command_hint_with_untracked(
            DispatchState::AwaitingAgent,
            Path::new("/tmp/wt-entered"),
            "task-1169-launcher",
            None,
            Some("TASK-1169"),
            true,
            false,
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
        let stalled = next_command_hint_with_untracked(
            DispatchState::Stalled,
            Path::new("/tmp/wt-entered"),
            "task-1169-launcher",
            Some("prior commit"),
            Some("TASK-1169"),
            true,
            false,
        )
        .expect("Stalled always produces a hint");
        assert!(!stalled.contains("aida queue work"), "{stalled}");
        assert!(stalled.contains("cd /tmp/wt-entered"), "{stalled}");
        assert!(stalled.contains("competing"), "{stalled}");

        let salvageable = next_command_hint_with_untracked(
            DispatchState::Salvageable,
            Path::new("/tmp/wt-entered"),
            "task-1169-launcher",
            None,
            Some("TASK-1169"),
            true,
            false,
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
