//! Zen-mode provenance corroboration (BUG-237).
//!
//! # The problem
//!
//! `AIDA_ZEN=1` enables zen mode — skills auto-resolve their mechanical
//! `kind:confirmation` prompts, *including the merge confirmation*. It is set
//! by the `--zen` flag and inherited by every child process. On its own it is
//! **unverifiable**: a session that inherits a stale or leaked `AIDA_ZEN=1`
//! cannot tell whether the user authorized zen mode for *it*. A wrong guess is
//! a safety gap, not a papercut — a leaked `AIDA_ZEN=1` auto-resolving a merge
//! confirmation is a silent unauthorized merge.
//!
//! This mirrors BUG-233, which fixed exactly this class of bug for
//! `AIDA_AUTO_COMPLETE`. See [`crate::orchestrator`] for the template.
//!
//! # The corroboration
//!
//! `AIDA_ZEN` has two legitimate origins, so it needs two corroboration paths:
//!
//! 1. **Orchestrator-set** — `aida queue work --auto-complete --zen`. The
//!    orchestrator records `zen=true` on its run marker
//!    ([`crate::orchestrator::RunMarker`]); a phase child trusts its inherited
//!    `AIDA_ZEN` only when [`crate::orchestrator::live_run_marker`] confirms a
//!    live run whose marker has `zen=true`.
//! 2. **Standalone** — `aida queue work --zen`. The `--zen` dispatch mints a
//!    per-invocation token into `AIDA_ZEN_TOKEN` and records it as the
//!    `zen_intent_token` of the session's lease. A session trusts `AIDA_ZEN`
//!    when the lease covering its worktree carries that token.
//!
//! A bare `AIDA_ZEN=1` corroborated by neither is
//! [`ZenContext::Uncorroborated`]: treated *exactly* as interactive, plus a
//! single informational note. Unlike BUG-233 there genuinely *is* a stale
//! value to chase here, so the note carries an `unset AIDA_ZEN` hint.
//!
//! # Why a CLI command for the skills, not the bare env var
//!
//! Skills key their zen behavior off `aida zen status`, which re-runs
//! [`detect`] live. Reading the bare `$AIDA_ZEN` env var is the bug. The
//! command cannot go stale: it re-checks the run marker + lease every call.
//!
//! trace:BUG-237 | ai:claude

use std::path::{Path, PathBuf};

use crate::orchestrator;

/// The env var that *requests* zen mode. Set by the `--zen` flag, inherited by
/// children — and, being inherited, unverifiable on its own.
pub(crate) const ZEN_ENV: &str = "AIDA_ZEN";

/// STORY-564: set by `aida queue work --zen --pause-always` (and inherited by
/// the launched session). Forces the standalone-`--zen` finish checkpoint to
/// pause at grab-next/stop even on a clean finish, restoring the pre-STORY-564
/// always-pause behavior for an operator who wants to drive grab-next by hand.
/// A leak only ever *adds* a pause (the safe direction), so unlike `AIDA_ZEN`
/// it needs no corroboration token. trace:STORY-564 | ai:claude
pub(crate) const ZEN_PAUSE_ALWAYS_ENV: &str = "AIDA_ZEN_PAUSE_ALWAYS";

/// The provenance anchor: a per-invocation UUID minted by the `--zen` dispatch
/// arm and scrubbed by the `Default` / `--no-human` arms. Present iff *this*
/// `aida queue work` was genuinely `--zen`. The session lease copies it; the
/// orchestrator run marker reads its presence. A leaked one is wiped at the
/// dispatch door, so it cannot itself re-create the provenance gap.
pub(crate) const ZEN_TOKEN_ENV: &str = "AIDA_ZEN_TOKEN";

/// Where a corroborated zen session got its provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZenProvenance {
    /// A live `--auto-complete --zen` orchestrator run owns this session.
    Orchestrator,
    /// This session's own lease carries a `--zen` zen-intent token.
    SessionLease,
}

/// Why a session carrying `AIDA_ZEN=1` is *not* trusted as a genuine zen
/// session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UncorroboratedReason {
    /// `AIDA_ZEN=1` with no live orchestrator run and no `--zen` lease token —
    /// the classic stale / leaked-env case.
    NoProvenance,
    /// A live orchestrator run owns this session, but that run was *not*
    /// started with `--zen`. `AIDA_ZEN=1` leaked into the orchestrator.
    OrchestratorNotZen,
}

/// The corroborated verdict on whether this process is in zen mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZenContext {
    /// `AIDA_ZEN=1` corroborated — genuine zen mode. Skills may auto-resolve
    /// `kind:confirmation` prompts.
    Zen(ZenProvenance),
    /// No `AIDA_ZEN=1` — an ordinary interactive session.
    Interactive,
    /// `AIDA_ZEN=1` is set but uncorroborated. Treated *exactly* as
    /// interactive; callers additionally surface [`Self::informational_note`].
    Uncorroborated(UncorroboratedReason),
}

impl ZenContext {
    /// True only for a corroborated zen session. Both other variants are
    /// interactive — skills must behave identically for them apart from the
    /// [`Self::informational_note`]. This is the one predicate the merge
    /// confirmation (and every other `kind:confirmation` prompt) keys off.
    pub(crate) fn is_zen(self) -> bool {
        matches!(self, ZenContext::Zen(_))
    }

    /// The single word `aida zen status` prints — the machine-readable signal
    /// skills branch on instead of the bare `$AIDA_ZEN` env var.
    pub(crate) fn status_word(self) -> &'static str {
        if self.is_zen() {
            "zen"
        } else {
            "interactive"
        }
    }

    /// A stable slug for `--json` consumers describing *why* the verdict
    /// landed where it did.
    pub(crate) fn reason_slug(self) -> &'static str {
        match self {
            ZenContext::Zen(ZenProvenance::Orchestrator) => "orchestrator-run",
            ZenContext::Zen(ZenProvenance::SessionLease) => "session-lease",
            ZenContext::Interactive => "zen-off",
            ZenContext::Uncorroborated(UncorroboratedReason::NoProvenance) => "no-provenance",
            ZenContext::Uncorroborated(UncorroboratedReason::OrchestratorNotZen) => {
                "orchestrator-not-zen"
            }
        }
    }

    /// An informational note for the `Uncorroborated` case, explaining that a
    /// bare `AIDA_ZEN` is being treated as interactive. `None` for the two
    /// unambiguous variants. Unlike BUG-233's note, the `NoProvenance` case
    /// names a real stale value, so it carries an `unset` hint.
    pub(crate) fn informational_note(self) -> Option<&'static str> {
        match self {
            ZenContext::Uncorroborated(UncorroboratedReason::NoProvenance) => Some(
                "AIDA_ZEN=1 is set but its provenance cannot be corroborated — no live \
                 orchestrator run and no --zen session lease own it. Treating this as a \
                 normal interactive session; run `unset AIDA_ZEN` if it is a stale value.",
            ),
            ZenContext::Uncorroborated(UncorroboratedReason::OrchestratorNotZen) => Some(
                "AIDA_ZEN=1 is set but this orchestrator run was not started with --zen. \
                 Treating this as a normal interactive session.",
            ),
            ZenContext::Zen(_) | ZenContext::Interactive => None,
        }
    }
}

/// The corroboration signal from the orchestrator layer — split out so
/// [`classify`] stays a pure function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrchestratorZenSignal {
    /// No live, corroborated orchestrator run owns this process.
    None,
    /// A live orchestrator run owns this process; `zen` is whether that run
    /// was started with `--zen`.
    Live { zen: bool },
}

/// Pure classification: given the `AIDA_ZEN` value, the orchestrator signal,
/// and whether this session's own lease carries a zen-intent token, return the
/// verdict. Split out from [`detect`] so the decision logic is unit-testable
/// without touching the environment or filesystem.
pub(crate) fn classify(
    zen_env: Option<&str>,
    orchestrator: OrchestratorZenSignal,
    lease_zen_token: Option<&str>,
) -> ZenContext {
    // TASK-327: only the exact value `1` enables zen — `AIDA_ZEN=0` /
    // `AIDA_ZEN=false` (and an empty / unset var) are *not* zen.
    if zen_env != Some("1") {
        return ZenContext::Interactive;
    }
    // Orchestrator path: a live run is the authority. A run that owns us but
    // is not `--zen` means `AIDA_ZEN=1` leaked into the orchestrator.
    match orchestrator {
        OrchestratorZenSignal::Live { zen: true } => {
            return ZenContext::Zen(ZenProvenance::Orchestrator);
        }
        OrchestratorZenSignal::Live { zen: false } => {
            return ZenContext::Uncorroborated(UncorroboratedReason::OrchestratorNotZen);
        }
        OrchestratorZenSignal::None => {}
    }
    // Standalone path: the session's own lease records the `--zen` token.
    match lease_zen_token.filter(|t| !t.is_empty()) {
        Some(_) => ZenContext::Zen(ZenProvenance::SessionLease),
        None => ZenContext::Uncorroborated(UncorroboratedReason::NoProvenance),
    }
}

/// The corroborated zen verdict for the current process: reads `AIDA_ZEN` from
/// the environment, the orchestrator run marker under `project_root`, and the
/// session lease covering `cwd`. Resolve `project_root` with
/// `find_main_worktree_root` so a child in a sibling worktree reads the shared
/// `.aida/`. trace:BUG-237 | ai:claude
pub(crate) fn detect(project_root: &Path, cwd: &Path) -> ZenContext {
    let orchestrator = match orchestrator::live_run_marker(project_root) {
        Some(marker) => OrchestratorZenSignal::Live { zen: marker.zen },
        None => OrchestratorZenSignal::None,
    };
    let lease_zen_token =
        crate::active_lease_for_cwd(project_root, cwd).and_then(|lease| lease.zen_intent_token);
    classify(
        std::env::var(ZEN_ENV).ok().as_deref(),
        orchestrator,
        lease_zen_token.as_deref(),
    )
}

// =========================================================================
// STORY-564: clean-vs-human-needed finish decision for the standalone
// `--zen` lane (no orchestrator). `--zen` always opened the PR then *paused*
// at grab-next/stop, so a clean finish with no human in the loop still forced
// a manual `stop` + `aida session end` (the BUG-500 friction). This wires a
// substrate gate the `/aida-pr` finish checkpoint consults: a finish the
// session never needed a human for AUTO-EXITS (the skill runs `aida session
// end` itself), and only a genuinely human-needed finish — the session
// marked itself, an open punt for the spec, or `--pause-always` — pauses.
// =========================================================================

/// The exit decision for a finished standalone `--zen` session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZenFinish {
    /// Clean finish, no human ever needed — the skill runs `aida session end`
    /// itself and exits without an operator round-trip.
    AutoExit,
    /// A human is (or was) in the loop — render the grab-next/stop checkpoint
    /// and pause, exactly as the pre-STORY-564 behavior.
    Pause(FinishPause),
}

/// Why a `--zen` finish pauses instead of auto-exiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinishPause {
    /// Not a corroborated zen session — auto-exit is a zen-only affordance, so
    /// fail safe to pause. (The skill only reaches the gate in the zen branch;
    /// this is the defensive floor.)
    NotZen,
    /// The session marked itself human-needed during the run (`aida zen
    /// needs-human`) — it paused on a design-fork or raised a punt.
    NeedsHuman,
    /// An open punt for the active spec was raised during this session — the
    /// substrate record of a human-needed fork, independent of the marker.
    Punt,
    /// `--pause-always` / `[zen] auto_exit = false` — the operator wants to
    /// drive grab-next by hand.
    PauseAlways,
    /// Operator presence explicitly says the human is at the keyboard. This is
    /// advisory: away/no-opinion clean finishes still auto-exit, and integrity
    /// gates above this still pause.
    OperatorHome,
}

impl ZenFinish {
    /// The bare word the gate prints to stdout for the skill to branch on.
    pub(crate) fn decision_word(self) -> &'static str {
        match self {
            ZenFinish::AutoExit => "auto-exit",
            ZenFinish::Pause(_) => "pause",
        }
    }

    /// A stable slug naming *why* the decision landed, for `--json` consumers
    /// and the human note.
    pub(crate) fn reason_slug(self) -> &'static str {
        match self {
            ZenFinish::AutoExit => "clean",
            ZenFinish::Pause(FinishPause::NotZen) => "not-zen",
            ZenFinish::Pause(FinishPause::NeedsHuman) => "needs-human",
            ZenFinish::Pause(FinishPause::Punt) => "punt",
            ZenFinish::Pause(FinishPause::PauseAlways) => "pause-always",
            ZenFinish::Pause(FinishPause::OperatorHome) => "operator-home",
        }
    }
}

/// Advisory operator-presence input for the finish checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinishPresence {
    /// No presence file or unreadable presence state: preserve pre-presence
    /// clean-finish behavior.
    NoOpinion,
    /// Effective `away`: bias clean finishes toward auto-exit.
    Away,
    /// Effective `home`: a soft reason to pause for operator handoff.
    Home,
}

/// Pure decision: given whether this is a corroborated zen session, whether it
/// marked itself human-needed, whether an open punt for the spec was raised
/// this session, whether pause-always is in force, and the advisory operator
/// presence, return the finish verdict. Split out from [`detect_finish`] so it
/// is unit-testable without touching the environment or filesystem. Order
/// encodes precedence: a non-zen session never auto-exits; integrity signals
/// beat presence; explicit/effective home is only a soft pause.
// trace:STORY-564 | ai:claude
// trace:TASK-758 | ai:codex
pub(crate) fn classify_finish(
    is_zen: bool,
    needs_human_marked: bool,
    has_open_punt: bool,
    pause_always: bool,
    presence: FinishPresence,
) -> ZenFinish {
    if !is_zen {
        return ZenFinish::Pause(FinishPause::NotZen);
    }
    if needs_human_marked {
        return ZenFinish::Pause(FinishPause::NeedsHuman);
    }
    if has_open_punt {
        return ZenFinish::Pause(FinishPause::Punt);
    }
    if pause_always {
        return ZenFinish::Pause(FinishPause::PauseAlways);
    }
    if matches!(presence, FinishPresence::Home) {
        return ZenFinish::Pause(FinishPause::OperatorHome);
    }
    ZenFinish::AutoExit
}

/// The directory holding per-session `--zen` needs-human markers. Under the
/// shared `.aida/` so a child in a sibling worktree and the gate (run in the
/// same worktree) agree on the path.
fn needs_human_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("zen-needs-human")
}

/// The marker file for one session id. Keyed by the session-lease id so two
/// concurrent `--zen` sessions sharing one `.aida/` never read each other's
/// markers.
pub(crate) fn needs_human_marker_path(project_root: &Path, session_id: &str) -> PathBuf {
    needs_human_dir(project_root).join(format!("{session_id}.marker"))
}

/// Record that the current `--zen` session needed a human — called by `aida
/// zen needs-human` when the session pauses on a design-fork or raises a punt.
/// The body is the human-readable reason (for later triage); only the file's
/// *presence* drives the gate. Idempotent: re-marking overwrites.
/// trace:STORY-564 | ai:claude
pub(crate) fn mark_needs_human(
    project_root: &Path,
    session_id: &str,
    reason: &str,
) -> std::io::Result<PathBuf> {
    let dir = needs_human_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let path = needs_human_marker_path(project_root, session_id);
    let stamp = chrono::Utc::now().to_rfc3339();
    std::fs::write(&path, format!("{stamp}\n{reason}\n"))?;
    Ok(path)
}

/// True when a needs-human marker exists for this session id.
pub(crate) fn has_needs_human_marker(project_root: &Path, session_id: &str) -> bool {
    needs_human_marker_path(project_root, session_id).exists()
}

/// Remove a session's needs-human marker — best-effort, called by `aida
/// session end` so a reclaimed session id never inherits a stale marker.
pub(crate) fn clear_needs_human_marker(project_root: &Path, session_id: &str) {
    let _ = std::fs::remove_file(needs_human_marker_path(project_root, session_id));
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- classify (pure) ----------------------------------------------------

    #[test]
    fn classify_no_var_is_interactive() {
        assert_eq!(
            classify(None, OrchestratorZenSignal::None, None),
            ZenContext::Interactive
        );
    }

    #[test]
    fn classify_non_one_value_is_interactive() {
        // TASK-327: `AIDA_ZEN=0` / `=false` / `=` must not enable zen — even
        // when a lease token would otherwise corroborate.
        for v in ["0", "false", "", "yes", "2"] {
            assert_eq!(
                classify(Some(v), OrchestratorZenSignal::None, Some("tok")),
                ZenContext::Interactive,
                "AIDA_ZEN={v:?} must not enable zen"
            );
        }
    }

    #[test]
    fn classify_orchestrator_zen_is_corroborated() {
        // An orchestrator run started with --zen corroborates an inherited
        // AIDA_ZEN=1 regardless of any lease token.
        assert_eq!(
            classify(Some("1"), OrchestratorZenSignal::Live { zen: true }, None),
            ZenContext::Zen(ZenProvenance::Orchestrator)
        );
    }

    #[test]
    fn classify_orchestrator_not_zen_is_uncorroborated() {
        // AIDA_ZEN=1 leaked into a non---zen orchestrator run → uncorroborated.
        assert_eq!(
            classify(
                Some("1"),
                OrchestratorZenSignal::Live { zen: false },
                Some("tok")
            ),
            ZenContext::Uncorroborated(UncorroboratedReason::OrchestratorNotZen)
        );
    }

    #[test]
    fn classify_standalone_lease_token_is_corroborated() {
        // No orchestrator, but the session's own lease records a --zen token.
        assert_eq!(
            classify(Some("1"), OrchestratorZenSignal::None, Some("a-uuid")),
            ZenContext::Zen(ZenProvenance::SessionLease)
        );
        // An empty token string is the same as an absent one.
        assert_eq!(
            classify(Some("1"), OrchestratorZenSignal::None, Some("")),
            ZenContext::Uncorroborated(UncorroboratedReason::NoProvenance)
        );
    }

    #[test]
    fn classify_leaked_zen_no_provenance_is_uncorroborated() {
        // The headline bug: a leaked AIDA_ZEN=1 with no orchestrator and no
        // lease token → uncorroborated, NOT silent zen.
        assert_eq!(
            classify(Some("1"), OrchestratorZenSignal::None, None),
            ZenContext::Uncorroborated(UncorroboratedReason::NoProvenance)
        );
    }

    // --- verdict helpers ----------------------------------------------------

    #[test]
    fn uncorroborated_status_word_is_interactive() {
        // The merge confirmation (and every kind:confirmation prompt) keys its
        // auto-resolve off `status_word() == "zen"`. An uncorroborated
        // AIDA_ZEN must never produce "zen" — that is the silent-merge gap.
        for reason in [
            UncorroboratedReason::NoProvenance,
            UncorroboratedReason::OrchestratorNotZen,
        ] {
            let ctx = ZenContext::Uncorroborated(reason);
            assert_eq!(ctx.status_word(), "interactive");
            assert!(!ctx.is_zen());
        }
        assert_eq!(ZenContext::Interactive.status_word(), "interactive");
        assert_eq!(
            ZenContext::Zen(ZenProvenance::Orchestrator).status_word(),
            "zen"
        );
        assert_eq!(
            ZenContext::Zen(ZenProvenance::SessionLease).status_word(),
            "zen"
        );
    }

    #[test]
    fn note_present_only_when_uncorroborated_and_carries_unset_hint() {
        let no_prov = ZenContext::Uncorroborated(UncorroboratedReason::NoProvenance);
        let not_zen = ZenContext::Uncorroborated(UncorroboratedReason::OrchestratorNotZen);
        // The classic stale-value case names the remedy.
        assert!(no_prov
            .informational_note()
            .unwrap()
            .contains("unset AIDA_ZEN"));
        assert!(not_zen.informational_note().is_some());
        // Corroborated / plain-interactive verdicts say nothing.
        assert_eq!(
            ZenContext::Zen(ZenProvenance::Orchestrator).informational_note(),
            None
        );
        assert_eq!(ZenContext::Interactive.informational_note(), None);
    }

    #[test]
    fn reason_slug_is_stable_per_variant() {
        assert_eq!(
            ZenContext::Zen(ZenProvenance::Orchestrator).reason_slug(),
            "orchestrator-run"
        );
        assert_eq!(
            ZenContext::Zen(ZenProvenance::SessionLease).reason_slug(),
            "session-lease"
        );
        assert_eq!(ZenContext::Interactive.reason_slug(), "zen-off");
        assert_eq!(
            ZenContext::Uncorroborated(UncorroboratedReason::NoProvenance).reason_slug(),
            "no-provenance"
        );
    }

    // --- classify_finish (pure, STORY-564) ----------------------------------

    #[test]
    fn finish_clean_zen_auto_exits() {
        // The headline case: a corroborated zen session that never needed a
        // human, no open punt, no pause-always → AUTO-EXIT. This is what
        // erases the BUG-500 manual-stop friction.
        let f = classify_finish(true, false, false, false, FinishPresence::NoOpinion);
        assert_eq!(f, ZenFinish::AutoExit);
        assert_eq!(f.decision_word(), "auto-exit");
        assert_eq!(f.reason_slug(), "clean");
    }

    #[test]
    fn finish_non_zen_never_auto_exits() {
        // An uncorroborated / interactive session must never be torn down
        // automatically — auto-exit is a zen-only affordance.
        let f = classify_finish(false, false, false, false, FinishPresence::NoOpinion);
        assert_eq!(f, ZenFinish::Pause(FinishPause::NotZen));
        assert_eq!(f.decision_word(), "pause");
        assert_eq!(f.reason_slug(), "not-zen");
    }

    #[test]
    fn finish_needs_human_marker_pauses() {
        let f = classify_finish(true, true, false, false, FinishPresence::Away);
        assert_eq!(f, ZenFinish::Pause(FinishPause::NeedsHuman));
        assert_eq!(f.reason_slug(), "needs-human");
    }

    #[test]
    fn finish_open_punt_pauses() {
        let f = classify_finish(true, false, true, false, FinishPresence::Away);
        assert_eq!(f, ZenFinish::Pause(FinishPause::Punt));
        assert_eq!(f.reason_slug(), "punt");
    }

    #[test]
    fn finish_pause_always_pauses() {
        let f = classify_finish(true, false, false, true, FinishPresence::Away);
        assert_eq!(f, ZenFinish::Pause(FinishPause::PauseAlways));
        assert_eq!(f.reason_slug(), "pause-always");
    }

    #[test]
    fn finish_presence_home_soft_pauses_but_away_auto_exits() {
        let home = classify_finish(true, false, false, false, FinishPresence::Home);
        assert_eq!(home, ZenFinish::Pause(FinishPause::OperatorHome));
        assert_eq!(home.reason_slug(), "operator-home");

        let away = classify_finish(true, false, false, false, FinishPresence::Away);
        assert_eq!(away, ZenFinish::AutoExit);
    }

    #[test]
    fn finish_presence_no_opinion_preserves_clean_auto_exit() {
        let f = classify_finish(true, false, false, false, FinishPresence::NoOpinion);
        assert_eq!(f, ZenFinish::AutoExit);
        assert_eq!(f.reason_slug(), "clean");
    }

    #[test]
    fn finish_precedence_marker_beats_punt_beats_pause_always() {
        // When several human-needed signals fire, the reported reason follows
        // the fixed precedence: marker → punt → pause-always.
        assert_eq!(
            classify_finish(true, true, true, true, FinishPresence::Home),
            ZenFinish::Pause(FinishPause::NeedsHuman)
        );
        assert_eq!(
            classify_finish(true, false, true, true, FinishPresence::Home),
            ZenFinish::Pause(FinishPause::Punt)
        );
        assert_eq!(
            classify_finish(true, false, false, true, FinishPresence::Home),
            ZenFinish::Pause(FinishPause::PauseAlways)
        );
    }

    #[test]
    fn needs_human_marker_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(!has_needs_human_marker(root, "abc123"));
        mark_needs_human(root, "abc123", "design-fork: storage backend").unwrap();
        assert!(has_needs_human_marker(root, "abc123"));
        // Scoped per session id — a sibling session is unaffected.
        assert!(!has_needs_human_marker(root, "def456"));
        // The body carries the reason for later triage.
        let body = std::fs::read_to_string(needs_human_marker_path(root, "abc123")).unwrap();
        assert!(body.contains("design-fork: storage backend"));
        clear_needs_human_marker(root, "abc123");
        assert!(!has_needs_human_marker(root, "abc123"));
        // Clearing an absent marker is a no-op, not an error.
        clear_needs_human_marker(root, "abc123");
    }
}
