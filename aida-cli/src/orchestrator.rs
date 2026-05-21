//! Orchestrator-run corroboration (BUG-233, TASK-336).
//!
//! # The problem
//!
//! An `aida queue work --auto-complete` orchestrator launches each phase as a
//! child `aida queue work <SPEC>` subprocess and exports `AIDA_AUTO_COMPLETE=1`
//! to it. That child is otherwise **indistinguishable** from a standalone
//! `aida queue work <SPEC>` the user typed: same argv, same env shape. The bare
//! `AIDA_AUTO_COMPLETE=1` flag is unverifiable — a child cannot tell whether it
//! is (a) a legitimate phase child of a live orchestrator or (b) carrying a
//! stale value. So it *guesses*, and a wrong guess does real harm: an
//! orchestrated child that thinks it is standalone runs interactive menus that
//! break the orchestrator chain; a standalone session that thinks it is
//! orchestrated suppresses menus and stalls waiting for an orchestrator that
//! does not exist.
//!
//! # The corroboration token
//!
//! For the lifetime of each spec's orchestration the orchestrator mints a
//! per-run UUID and records it as [`crate::drain_state::DrainState::run_uuid`]
//! on the live drain-state file ([`crate::drain_state`]). It passes
//! `AIDA_AUTO_COMPLETE_TOKEN=<uuid>` to every phase child alongside
//! `AIDA_AUTO_COMPLETE=1`. TASK-336 folded the run-UUID into the drain-state
//! file — before that this module owned a sidecar marker file
//! `.aida/orchestrator-runs/<uuid>`, which has been removed since the drain-
//! state file already records every other field corroboration needs (PID,
//! current spec, `--zen` flag, started-at).
//!
//! A child trusts orchestrator-mode ([`OrchestratorContext::Orchestrated`])
//! **only** when all three hold:
//!
//! 1. `AIDA_AUTO_COMPLETE` is set, AND
//! 2. `AIDA_AUTO_COMPLETE_TOKEN` is set, AND
//! 3. the token matches [`crate::drain_state::DrainState::run_uuid`] on the
//!    live drain-state file whose recorded PID is still alive.
//!
//! A bare `AIDA_AUTO_COMPLETE=1` with no valid live token is
//! [`OrchestratorContext::Uncorroborated`]: treated exactly as interactive,
//! plus a single *informational* (never alarming) note. There is no leak to
//! hunt — see BUG-233's corrected diagnosis.
//!
//! # Why a CLI command for the skills, not a second env var
//!
//! Skills key their orchestrator-aware behavior off `aida orchestrator status`,
//! which re-runs [`detect`] live. A second *propagated* env var
//! (`AIDA_ORCHESTRATED=1`) would just reintroduce the same unverifiable-bare-
//! flag bug one layer down. The command cannot go stale: it re-checks the
//! drain-state file + PID every call.
//!
//! trace:BUG-233 trace:TASK-336 | ai:claude

use std::path::Path;

use crate::drain_state;
use crate::process_probe;

/// The orchestrator → child signal that a phase subprocess belongs to an
/// `--auto-complete` run. On its own it is **not** trusted — see [`TOKEN_ENV`].
pub(crate) const AUTO_COMPLETE_ENV: &str = "AIDA_AUTO_COMPLETE";

/// The corroboration token: a per-run UUID matching
/// [`crate::drain_state::DrainState::run_uuid`] on the live drain-state file.
/// Set by the orchestrator alongside [`AUTO_COMPLETE_ENV`] on every phase
/// child. trace:TASK-336
pub(crate) const TOKEN_ENV: &str = "AIDA_AUTO_COMPLETE_TOKEN";

/// The 1-based phase index (`1`..=`6`) the current process is running, set by
/// the orchestrator on each Claude-launching phase child (phase 1 implementer,
/// phase 3 reviewer). The child's statusline reads it to show `auto:N/6` so an
/// interactive phase advertises that it is an orchestrator step the user must
/// act in. Unset for a standalone session. trace:TASK-306 | ai:claude
pub(crate) const PHASE_ENV: &str = "AIDA_AUTO_COMPLETE_PHASE";

/// The `--no-human` mode slug (`reviewer-only` | `both`) in effect for an
/// `--auto-complete` run, propagated to phase children so the statusline can
/// show the headless scope alongside the phase. Absent for a fully
/// interactive run. trace:TASK-306 | ai:claude
pub(crate) const NO_HUMAN_MODE_ENV: &str = "AIDA_NO_HUMAN_MODE";

/// A token is only ever a bare UUID. Rejecting anything else keeps a crafted
/// `AIDA_AUTO_COMPLETE_TOKEN` from ever being treated as live, regardless of
/// what happens to land in the drain-state file. trace:BUG-233
fn is_valid_token(token: &str) -> bool {
    uuid::Uuid::parse_str(token).is_ok()
}

/// The diagnostic view of the live orchestrator run owning the current
/// process — derived from [`crate::drain_state::DrainState`] (TASK-336). Only
/// [`RunMarker::pid`] and the corroborated [`RunMarker::zen`] are load-bearing
/// — `spec` and `started_at` are diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunMarker {
    /// PID of the orchestrator process that owns this run.
    pub(crate) pid: u32,
    /// The spec the run is draining (diagnostic).
    pub(crate) spec: String,
    /// RFC-3339 timestamp the drain started (diagnostic).
    pub(crate) started_at: String,
    /// BUG-237: whether the current spec's orchestration was started with
    /// `--zen`. A phase child trusts an inherited `AIDA_ZEN=1` only when this
    /// is true.
    pub(crate) zen: bool,
}

/// Why a session carrying `AIDA_AUTO_COMPLETE` is *not* trusted as a genuine
/// orchestrator child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UncorroboratedReason {
    /// `AIDA_AUTO_COMPLETE` is set but no `AIDA_AUTO_COMPLETE_TOKEN` accompanies
    /// it — the bare-flag case that originally misfired as an "env leak".
    NoToken,
    /// A token is present but no live orchestrator owns it (missing marker,
    /// malformed token, or the recorded PID is dead).
    DeadOrchestrator,
}

/// The corroborated verdict on whether this process is an orchestrator child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrchestratorContext {
    /// `AIDA_AUTO_COMPLETE=1` corroborated by a token naming a live orchestrator
    /// run. Orchestrator-aware skill behavior is correct here.
    Orchestrated,
    /// No `AIDA_AUTO_COMPLETE` — an ordinary interactive session.
    Interactive,
    /// `AIDA_AUTO_COMPLETE` is set but uncorroborated. Treated *exactly* as
    /// interactive; callers additionally surface [`Self::informational_note`].
    Uncorroborated(UncorroboratedReason),
}

impl OrchestratorContext {
    /// True only for a corroborated orchestrator run. Both other variants are
    /// interactive — the skills and `queue work` must behave identically for
    /// them apart from the [`Self::informational_note`].
    pub(crate) fn is_orchestrated(self) -> bool {
        matches!(self, OrchestratorContext::Orchestrated)
    }

    /// The single word `aida orchestrator status` prints — the machine-readable
    /// signal skills branch on.
    pub(crate) fn status_word(self) -> &'static str {
        if self.is_orchestrated() {
            "orchestrated"
        } else {
            "interactive"
        }
    }

    /// A stable slug for `--json` consumers describing *why* the verdict landed
    /// where it did.
    pub(crate) fn reason_slug(self) -> &'static str {
        match self {
            OrchestratorContext::Orchestrated => "live-token",
            OrchestratorContext::Interactive => "no-auto-complete",
            OrchestratorContext::Uncorroborated(UncorroboratedReason::NoToken) => "no-token",
            OrchestratorContext::Uncorroborated(UncorroboratedReason::DeadOrchestrator) => {
                "dead-orchestrator"
            }
        }
    }

    /// An *informational* (never alarming) note for the `Uncorroborated` case,
    /// explaining that a bare `AIDA_AUTO_COMPLETE` is being treated as
    /// interactive. `None` for the two unambiguous variants. The note never
    /// tells the user to `unset` anything — there is no leak to chase
    /// (BUG-233's corrected diagnosis).
    pub(crate) fn informational_note(self) -> Option<&'static str> {
        match self {
            OrchestratorContext::Uncorroborated(UncorroboratedReason::NoToken) => Some(
                "AIDA_AUTO_COMPLETE is set but carries no orchestrator token — \
                 treating this as a normal interactive session.",
            ),
            OrchestratorContext::Uncorroborated(UncorroboratedReason::DeadOrchestrator) => Some(
                "AIDA_AUTO_COMPLETE is set but no live orchestrator run owns it — \
                 treating this as a normal interactive session.",
            ),
            OrchestratorContext::Orchestrated | OrchestratorContext::Interactive => None,
        }
    }
}

/// Pure classification: given the two env values and a liveness probe for a
/// token, return the verdict. Split out from [`detect`] so the decision logic
/// is unit-testable without touching the process environment or filesystem.
pub(crate) fn classify(
    auto_complete: Option<&str>,
    token: Option<&str>,
    run_is_live: impl Fn(&str) -> bool,
) -> OrchestratorContext {
    // An unset *or empty* `AIDA_AUTO_COMPLETE` is an ordinary interactive
    // session — nothing to corroborate.
    let auto_complete_set = auto_complete.map(|v| !v.is_empty()).unwrap_or(false);
    if !auto_complete_set {
        return OrchestratorContext::Interactive;
    }
    match token.filter(|t| !t.is_empty()) {
        None => OrchestratorContext::Uncorroborated(UncorroboratedReason::NoToken),
        Some(token) if run_is_live(token) => OrchestratorContext::Orchestrated,
        Some(_) => OrchestratorContext::Uncorroborated(UncorroboratedReason::DeadOrchestrator),
    }
}

/// Is `token` owned by a live orchestrator run? True iff it is a valid UUID
/// that matches [`crate::drain_state::DrainState::run_uuid`] on the drain-
/// state file under `project_root` whose recorded `orchestrator_pid` is
/// alive. trace:TASK-336 | ai:claude
pub(crate) fn run_is_live(project_root: &Path, token: &str) -> bool {
    if !is_valid_token(token) {
        return false;
    }
    let Some(state) = drain_state::DrainState::read(project_root) else {
        return false;
    };
    if state.run_uuid.is_empty() || state.run_uuid != token {
        return false;
    }
    process_probe::pid_is_alive(state.orchestrator_pid)
}

/// The corroborated verdict for the current process, reading `AIDA_AUTO_COMPLETE`
/// + `AIDA_AUTO_COMPLETE_TOKEN` from the environment and checking the drain-
/// state file under `project_root` (resolve it with `find_main_worktree_root`
/// so a child running in a sibling worktree reads the orchestrator's `.aida/`).
pub(crate) fn detect(project_root: &Path) -> OrchestratorContext {
    classify(
        std::env::var(AUTO_COMPLETE_ENV).ok().as_deref(),
        std::env::var(TOKEN_ENV).ok().as_deref(),
        |token| run_is_live(project_root, token),
    )
}

/// The diagnostic view of the *live, corroborated* orchestrator run that owns
/// the current process, or `None`. `Some` exactly when [`detect`] would return
/// [`OrchestratorContext::Orchestrated`] — so a caller that needs a field off
/// the run (zen-mode corroboration, BUG-237) gets it without re-deriving the
/// corroboration. Derived from [`crate::drain_state::DrainState`] (TASK-336).
/// trace:BUG-237 trace:TASK-336 | ai:claude
pub(crate) fn live_run_marker(project_root: &Path) -> Option<RunMarker> {
    let auto_complete = std::env::var(AUTO_COMPLETE_ENV).ok()?;
    if auto_complete.is_empty() {
        return None;
    }
    let token = std::env::var(TOKEN_ENV).ok()?;
    if token.is_empty() || !is_valid_token(&token) {
        return None;
    }
    let state = drain_state::DrainState::read(project_root)?;
    if state.run_uuid.is_empty() || state.run_uuid != token {
        return None;
    }
    if !process_probe::pid_is_alive(state.orchestrator_pid) {
        return None;
    }
    Some(RunMarker {
        pid: state.orchestrator_pid,
        spec: state.current.unwrap_or_default(),
        started_at: state.started_at,
        zen: state.zen,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- classify (pure) ----------------------------------------------------

    #[test]
    fn classify_no_var_is_interactive() {
        assert_eq!(
            classify(None, None, |_| true),
            OrchestratorContext::Interactive
        );
    }

    #[test]
    fn classify_empty_var_treated_as_unset() {
        // An exported-but-empty `AIDA_AUTO_COMPLETE=` is not orchestrator mode.
        assert_eq!(
            classify(Some(""), Some("tok"), |_| true),
            OrchestratorContext::Interactive
        );
    }

    #[test]
    fn classify_bare_var_no_token_is_uncorroborated_notoken() {
        assert_eq!(
            classify(Some("1"), None, |_| true),
            OrchestratorContext::Uncorroborated(UncorroboratedReason::NoToken)
        );
        // An empty token string is the same as an absent one.
        assert_eq!(
            classify(Some("1"), Some(""), |_| true),
            OrchestratorContext::Uncorroborated(UncorroboratedReason::NoToken)
        );
    }

    #[test]
    fn classify_var_with_token_live_is_orchestrated() {
        assert_eq!(
            classify(Some("1"), Some("tok"), |t| t == "tok"),
            OrchestratorContext::Orchestrated
        );
    }

    #[test]
    fn classify_var_with_token_dead_is_uncorroborated_dead() {
        assert_eq!(
            classify(Some("1"), Some("tok"), |_| false),
            OrchestratorContext::Uncorroborated(UncorroboratedReason::DeadOrchestrator)
        );
    }

    // --- verdict helpers ----------------------------------------------------

    #[test]
    fn status_word_is_orchestrated_only_when_corroborated() {
        assert_eq!(
            OrchestratorContext::Orchestrated.status_word(),
            "orchestrated"
        );
        assert_eq!(
            OrchestratorContext::Interactive.status_word(),
            "interactive"
        );
        assert_eq!(
            OrchestratorContext::Uncorroborated(UncorroboratedReason::NoToken).status_word(),
            "interactive"
        );
    }

    #[test]
    fn note_text_differs_by_reason_and_is_absent_otherwise() {
        let no_token =
            OrchestratorContext::Uncorroborated(UncorroboratedReason::NoToken).informational_note();
        let dead = OrchestratorContext::Uncorroborated(UncorroboratedReason::DeadOrchestrator)
            .informational_note();
        assert!(no_token.is_some());
        assert!(dead.is_some());
        assert_ne!(no_token, dead);
        // The note must never tell the user to chase a non-existent leak.
        for note in [no_token.unwrap(), dead.unwrap()] {
            assert!(!note.to_lowercase().contains("unset"));
            assert!(!note.to_lowercase().contains("leak"));
        }
        assert_eq!(OrchestratorContext::Orchestrated.informational_note(), None);
        assert_eq!(OrchestratorContext::Interactive.informational_note(), None);
    }

    // --- run_is_live (TASK-336: keyed off drain-state.json) -----------------

    /// Helper: write a single-spec drain-state file with `token` as its
    /// run-UUID, owned by `pid`. Returns the token unchanged for convenience.
    fn write_state_with_run(dir: &Path, pid: u32, token: &str, zen: bool) -> String {
        let mut state = drain_state::DrainState::new_single("BUG-233", token, zen);
        state.orchestrator_pid = pid;
        state.write(dir).unwrap();
        token.to_string()
    }

    #[test]
    fn run_is_live_true_when_token_matches_live_drain() {
        let dir = tempfile::tempdir().unwrap();
        let token = uuid::Uuid::now_v7().to_string();
        write_state_with_run(dir.path(), std::process::id(), &token, false);
        assert!(run_is_live(dir.path(), &token));
    }

    #[test]
    fn run_is_live_false_when_no_drain_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let token = uuid::Uuid::now_v7().to_string();
        assert!(!run_is_live(dir.path(), &token));
    }

    #[test]
    fn run_is_live_false_when_token_does_not_match_drain_uuid() {
        // A token that didn't originate from THIS drain — a stale child
        // carrying the previous batch member's UUID, for instance — must not
        // corroborate.
        let dir = tempfile::tempdir().unwrap();
        let actual = uuid::Uuid::now_v7().to_string();
        write_state_with_run(dir.path(), std::process::id(), &actual, false);
        let stale = uuid::Uuid::now_v7().to_string();
        assert_ne!(actual, stale);
        assert!(!run_is_live(dir.path(), &stale));
    }

    // AC4 (TASK-336): a stale drain-state file (dead orchestrator PID) → the
    // UUID does not corroborate.
    #[test]
    fn run_is_live_false_when_orchestrator_pid_is_dead() {
        let dir = tempfile::tempdir().unwrap();
        let token = uuid::Uuid::now_v7().to_string();
        write_state_with_run(dir.path(), u32::MAX - 1, &token, false);
        assert!(!run_is_live(dir.path(), &token));
    }

    #[test]
    fn run_is_live_false_when_drain_state_has_empty_run_uuid() {
        // A drain-state file between batch members (run_uuid cleared) — any
        // would-be child token fails to corroborate until set_run fires.
        let dir = tempfile::tempdir().unwrap();
        let mut state =
            drain_state::DrainState::new_batch("autonomy-modes", &["STORY-1".to_string()]);
        state.orchestrator_pid = std::process::id();
        state.write(dir.path()).unwrap();
        let token = uuid::Uuid::now_v7().to_string();
        assert!(!run_is_live(dir.path(), &token));
    }

    #[test]
    fn run_is_live_false_for_non_uuid_token() {
        let dir = tempfile::tempdir().unwrap();
        // Even with a live drain, a non-UUID token is rejected up front so a
        // crafted `AIDA_AUTO_COMPLETE_TOKEN` cannot piggyback on it.
        write_state_with_run(dir.path(), std::process::id(), "not-a-uuid", false);
        assert!(!run_is_live(dir.path(), "../../etc/passwd"));
        assert!(!run_is_live(dir.path(), "not-a-uuid"));
    }

    // live_run_marker is exercised via integration: it reads `AIDA_AUTO_COMPLETE`
    // + `AIDA_AUTO_COMPLETE_TOKEN` from the process environment, then delegates
    // to the same drain-state-keyed checks `run_is_live` covers above. Avoiding
    // env mutation here keeps the test suite parallel-safe.
}
