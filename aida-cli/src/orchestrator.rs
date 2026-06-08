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
///   state file under `project_root` (resolve it with `find_main_worktree_root`
///   so a child running in a sibling worktree reads the orchestrator's `.aida/`).
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

// ----------------------------------------------------------------------------
// BUG-307: orchestrator auto-release of dormant leases.
//
// The drain reliability arc (BUG-285, BUG-286, BUG-266) closed the failure
// modes that produced phase stalls, leaving *stale-lease state from previous
// stalls* as the dominant friction class — every recovered failure leaves a
// lease behind, and the NEXT drain on the same spec or PR trips on it. When
// the lease's process is gone and the worktree has no uncommitted work, the
// refusal generates manual friction without protecting anything. This module
// classifies a lease-conflict candidate against three independent liveness
// signals and decides whether the orchestrator can safely auto-release it.
//
// The classifier is intentionally pure (takes pre-collected signals, returns
// an enum) so the decision matrix is unit-testable without disk or process
// shenanigans. The wrapper in `main.rs` (`auto_release_decision_for_lease`)
// gathers the signals and calls it.
//
// trace:BUG-307 | ai:claude
// ----------------------------------------------------------------------------

/// `[orchestrator]` section in `.aida/config.toml`. Defaults mirror the BUG-307
/// acceptance criteria — feature on, 10-minute fresh-lease threshold — so a
/// project that has never written the section gets the auto-release behaviour
/// for free. Missing file / section / keys all fall through to defaults; a
/// config error never blocks the orchestrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrchestratorConfig {
    /// Master switch. `false` reverts to the pre-BUG-307 behaviour: refuse on
    /// any same-scope lease conflict without `--steal`.
    pub auto_release_dormant_leases: bool,
    /// Lease-file mtime threshold (minutes). A lease whose mtime is younger
    /// than this is treated as "fresh" and never auto-released even if its
    /// PID is dead — protects the brief window between session_start writing
    /// the lease and the shell wiring up.
    pub stale_lease_threshold_minutes: u64,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            auto_release_dormant_leases: true,
            stale_lease_threshold_minutes: 10,
        }
    }
}

impl OrchestratorConfig {
    /// Load `[orchestrator]` from `<project_root>/.aida/config.toml`. Missing
    /// file / section / keys all fall through to defaults.
    pub(crate) fn load(project_root: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        else {
            return Self::default();
        };
        Self::from_toml_str(&content)
    }

    /// Build from a raw TOML string — used by the tests so they don't have
    /// to touch the filesystem.
    pub(crate) fn from_toml_str(content: &str) -> Self {
        let mut cfg = Self::default();
        for (key, val) in scan_orchestrator_section(content) {
            match key.as_str() {
                "auto_release_dormant_leases" => {
                    if let Some(b) = parse_orch_bool(&val) {
                        cfg.auto_release_dormant_leases = b;
                    }
                }
                "stale_lease_threshold_minutes" => {
                    if let Ok(n) = val.parse::<u64>() {
                        cfg.stale_lease_threshold_minutes = n;
                    }
                }
                _ => {}
            }
        }
        cfg
    }
}

fn parse_orch_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Hand-rolled `[orchestrator]` scanner — mirrors the one in `advisor.rs` so we
/// don't pull a serde TOML dependency for two scalars.
fn scan_orchestrator_section(content: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut in_section = false;
    for raw in content.lines() {
        let line = strip_inline_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_section = stripped.trim_end_matches(']').trim() == "orchestrator";
            continue;
        }
        if in_section {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"').trim_matches('\'').trim();
                pairs.push((k.trim().to_string(), v.to_string()));
            }
        }
    }
    pairs
}

fn strip_inline_comment(s: &str) -> &str {
    let (mut dq, mut sq) = (false, false);
    for (i, c) in s.char_indices() {
        match c {
            '"' if !sq => dq = !dq,
            '\'' if !dq => sq = !sq,
            '#' if !dq && !sq => return &s[..i],
            _ => {}
        }
    }
    s
}

/// The verdict for a single same-scope lease conflict against the BUG-307
/// auto-release gate. The orchestrator's pre-flight loop branches on this:
/// `SafelyDormant` → force-cleanup and continue; `DormantDirty` → refuse with
/// a recover-flavored error; `Live` → fall through to the existing `--steal`
/// path (which itself refuses without `--steal`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutoReleaseDecision {
    /// PID alive, mtime fresh, or a live claude is running inside the
    /// worktree — leave it alone, current refusal applies.
    Live,
    /// Lease's process is gone, mtime is past the staleness threshold, and
    /// the worktree is either missing or holds no uncommitted changes — safe
    /// to release without `--steal`.
    SafelyDormant {
        /// True iff `creator_pid` is no longer in the process table (or the
        /// lease never recorded a creator_pid — pre-STORY-73 leases).
        process_dead: bool,
        /// Age of the lease file in seconds; surfaced in the release log so
        /// the operator sees *how* stale the lease was.
        mtime_age_secs: i64,
        /// True iff the worktree directory no longer exists. Distinguishes
        /// "lease + worktree both leaked" from "lease still references a
        /// clean worktree we're about to remove."
        worktree_missing: bool,
    },
    /// Lease is dormant but its worktree has uncommitted changes — refuse
    /// with a loss-risk-aware message instead of silently discarding work.
    DormantDirty {
        /// Number of uncommitted entries reported by `git status --porcelain`.
        dirty_entries: usize,
    },
}

/// BUG-307: pure classifier for the auto-release gate. Given pre-collected
/// liveness signals + the configured threshold, decide whether a same-scope
/// lease conflict is safe to release without `--steal`.
///
/// The "lease is live" predicate is intentionally generous: ANY of (a) the
/// creator-shell PID is alive, (b) the lease file's mtime is younger than the
/// threshold, (c) a live `claude` process is running inside the worktree
/// suffices. The auto-release path only fires when *all three* fail — the
/// safety case is the one the bug filed against (`◐ dormant`, no live claude,
/// lease minted hours ago).
///
/// trace:BUG-307 | ai:claude
pub(crate) fn classify_for_auto_release(
    pid_alive: bool,
    lease_mtime_age_secs: i64,
    live_claude_in_worktree: bool,
    worktree_exists: bool,
    worktree_dirty_count: usize,
    threshold_minutes: u64,
) -> AutoReleaseDecision {
    // Negative age (clock skew, lease written in the future) is treated as
    // fresh — the operator can't tell whether the lease was just written or
    // arrived from a misconfigured clock, so we fail safe by refusing to
    // auto-release.
    let threshold_secs = (threshold_minutes as i64).saturating_mul(60);
    let mtime_fresh = lease_mtime_age_secs < threshold_secs;
    if pid_alive || mtime_fresh || live_claude_in_worktree {
        return AutoReleaseDecision::Live;
    }
    if worktree_exists && worktree_dirty_count > 0 {
        return AutoReleaseDecision::DormantDirty {
            dirty_entries: worktree_dirty_count,
        };
    }
    AutoReleaseDecision::SafelyDormant {
        process_dead: !pid_alive,
        mtime_age_secs: lease_mtime_age_secs,
        worktree_missing: !worktree_exists,
    }
}

#[cfg(test)]
mod auto_release_tests {
    use super::*;

    // --- config loader ------------------------------------------------------

    #[test]
    fn config_defaults_when_no_section() {
        let cfg = OrchestratorConfig::from_toml_str("");
        assert_eq!(cfg, OrchestratorConfig::default());
        assert!(cfg.auto_release_dormant_leases);
        assert_eq!(cfg.stale_lease_threshold_minutes, 10);
    }

    #[test]
    fn config_reads_explicit_section() {
        let cfg = OrchestratorConfig::from_toml_str(
            "[orchestrator]\n\
             auto_release_dormant_leases = false\n\
             stale_lease_threshold_minutes = 30\n",
        );
        assert!(!cfg.auto_release_dormant_leases);
        assert_eq!(cfg.stale_lease_threshold_minutes, 30);
    }

    #[test]
    fn config_ignores_other_sections() {
        let cfg = OrchestratorConfig::from_toml_str(
            "[advisor]\n\
             auto_release_dormant_leases = false\n\
             [other]\n\
             stale_lease_threshold_minutes = 99\n",
        );
        // Neither key was inside [orchestrator] — defaults stand.
        assert_eq!(cfg, OrchestratorConfig::default());
    }

    #[test]
    fn config_tolerates_inline_comments_and_quotes() {
        let cfg = OrchestratorConfig::from_toml_str(
            "[orchestrator]\n\
             auto_release_dormant_leases = \"true\"  # explicit\n\
             stale_lease_threshold_minutes = 5\n",
        );
        assert!(cfg.auto_release_dormant_leases);
        assert_eq!(cfg.stale_lease_threshold_minutes, 5);
    }

    #[test]
    fn config_unparseable_values_fall_back_to_default() {
        let cfg = OrchestratorConfig::from_toml_str(
            "[orchestrator]\n\
             auto_release_dormant_leases = maybe\n\
             stale_lease_threshold_minutes = forever\n",
        );
        assert_eq!(cfg, OrchestratorConfig::default());
    }

    // --- classifier (pure decision matrix) ---------------------------------

    /// The canonical "auto-release me" case from the BUG-307 report: process
    /// dead, mtime old, no live claude, worktree clean → safe to release.
    #[test]
    fn classify_dormant_clean_is_safely_dormant() {
        let d = classify_for_auto_release(
            /* pid_alive */ false, /* mtime_age_secs */ 7200, // 2h ago
            /* live_claude */ false, /* worktree_exists */ true, /* dirty */ 0,
            /* threshold_minutes */ 10,
        );
        assert!(matches!(
            d,
            AutoReleaseDecision::SafelyDormant {
                process_dead: true,
                mtime_age_secs: 7200,
                worktree_missing: false,
            }
        ));
    }

    /// PID still in the process table → still live, regardless of worktree
    /// state. The user might be coming back to the shell.
    #[test]
    fn classify_pid_alive_is_live_even_if_clean() {
        let d = classify_for_auto_release(true, 99_999, false, true, 0, 10);
        assert_eq!(d, AutoReleaseDecision::Live);
    }

    /// Mtime within the threshold protects a freshly-minted lease whose shell
    /// hasn't wired up yet (PID briefly absent from the table during exec).
    #[test]
    fn classify_mtime_fresh_is_live_even_if_pid_dead() {
        // 5 minutes old, 10-minute threshold → still fresh.
        let d = classify_for_auto_release(false, 300, false, true, 0, 10);
        assert_eq!(d, AutoReleaseDecision::Live);
    }

    /// Mtime exactly at the threshold → no longer fresh. The check is `<`.
    #[test]
    fn classify_mtime_at_threshold_is_not_fresh() {
        let d = classify_for_auto_release(false, 600, false, true, 0, 10);
        assert!(matches!(d, AutoReleaseDecision::SafelyDormant { .. }));
    }

    /// A live `claude` process inside the worktree pins the lease as live
    /// even when our PID + mtime checks both say "gone" (e.g. the lease's
    /// creator shell exited but its claude is still running).
    #[test]
    fn classify_live_claude_pins_as_live() {
        let d = classify_for_auto_release(false, 7200, true, true, 0, 10);
        assert_eq!(d, AutoReleaseDecision::Live);
    }

    /// Dirty worktree is the load-risk gate: refuse with the specific
    /// recover-hint message rather than silently nuking the work.
    #[test]
    fn classify_dormant_dirty_refuses() {
        let d = classify_for_auto_release(false, 7200, false, true, 3, 10);
        assert_eq!(d, AutoReleaseDecision::DormantDirty { dirty_entries: 3 });
    }

    /// Worktree gone → release the leftover lease record. This is the
    /// "session worktree was rm-rf'd out from under us" recovery case.
    #[test]
    fn classify_no_worktree_is_safely_dormant() {
        let d = classify_for_auto_release(false, 7200, false, false, 0, 10);
        assert_eq!(
            d,
            AutoReleaseDecision::SafelyDormant {
                process_dead: true,
                mtime_age_secs: 7200,
                worktree_missing: true,
            }
        );
    }

    /// A negative mtime age (clock skew, lease written in the future) is
    /// treated as fresh — fail safe by refusing to auto-release.
    #[test]
    fn classify_negative_mtime_age_is_treated_as_fresh() {
        let d = classify_for_auto_release(false, -42, false, true, 0, 10);
        assert_eq!(d, AutoReleaseDecision::Live);
    }

    /// A `0`-minute threshold disables the mtime-fresh check entirely —
    /// mtime alone never pins the lease, only PID and live-claude do. Useful
    /// for tests + operators who don't trust the mtime signal at all.
    #[test]
    fn classify_zero_threshold_means_mtime_never_pins() {
        // 0s old → still not fresh because the threshold is 0.
        let d = classify_for_auto_release(false, 0, false, true, 0, 0);
        assert!(matches!(d, AutoReleaseDecision::SafelyDormant { .. }));
    }

    /// BUG-438: the fast-resume case. A crashed implementer's lease is dead-PID
    /// but its mtime is still *fresh* (e.g. 30s) — under the default threshold
    /// that pins it `Live`, so the reviewer phase collides with it. Resume forces
    /// `threshold = 0` so the same dead-PID, clean-worktree lease releases
    /// instead. trace:BUG-438 | ai:claude
    #[test]
    fn classify_fresh_dead_lease_releases_only_at_zero_threshold() {
        // dead pid, 30s-fresh mtime, no live claude, worktree present + clean.
        let live = classify_for_auto_release(false, 30, false, true, 0, 10);
        assert_eq!(
            live,
            AutoReleaseDecision::Live,
            "default threshold keeps a fresh dead lease Live — the bug"
        );
        let released = classify_for_auto_release(false, 30, false, true, 0, 0);
        assert!(
            matches!(released, AutoReleaseDecision::SafelyDormant { .. }),
            "resume forces threshold 0 → the dead clean lease releases — the fix"
        );
        // A dirty worktree is still protected even at threshold 0.
        let dirty = classify_for_auto_release(false, 30, false, true, 2, 0);
        assert!(matches!(dirty, AutoReleaseDecision::DormantDirty { .. }));
    }
}
