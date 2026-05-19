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

use std::path::Path;

use crate::orchestrator;

/// The env var that *requests* zen mode. Set by the `--zen` flag, inherited by
/// children — and, being inherited, unverifiable on its own.
pub(crate) const ZEN_ENV: &str = "AIDA_ZEN";

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
}
