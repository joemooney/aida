//! Orchestrator-run corroboration (BUG-233).
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
//! The orchestrator mints a per-run UUID and, for the lifetime of the run,
//! holds a [`RunMarkerGuard`] — a marker file `.aida/orchestrator-runs/<uuid>`
//! recording the orchestrator's own PID. It passes `AIDA_AUTO_COMPLETE_TOKEN=
//! <uuid>` to every phase child alongside `AIDA_AUTO_COMPLETE=1`.
//!
//! A child trusts orchestrator-mode ([`OrchestratorContext::Orchestrated`])
//! **only** when all three hold:
//!
//! 1. `AIDA_AUTO_COMPLETE` is set, AND
//! 2. `AIDA_AUTO_COMPLETE_TOKEN` is set, AND
//! 3. the token names a marker file whose recorded PID is still alive.
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
//! marker + PID every call.
//!
//! trace:BUG-233 | ai:claude

use std::path::{Path, PathBuf};

use crate::process_probe;

/// The orchestrator → child signal that a phase subprocess belongs to an
/// `--auto-complete` run. On its own it is **not** trusted — see [`TOKEN_ENV`].
pub(crate) const AUTO_COMPLETE_ENV: &str = "AIDA_AUTO_COMPLETE";

/// The corroboration token: a per-run UUID naming a marker file under
/// `.aida/orchestrator-runs/`. Set by the orchestrator alongside
/// [`AUTO_COMPLETE_ENV`] on every phase child.
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

/// Subdirectory of `.aida/` holding one marker file per *live* orchestrator
/// run. Gitignored by the deny-by-default `.aida/*` rule — pure runtime state.
const RUNS_SUBDIR: &str = "orchestrator-runs";

/// Directory holding the per-run marker files for `project_root`.
pub(crate) fn runs_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(RUNS_SUBDIR)
}

/// Path of the marker file for `token` under `project_root`.
pub(crate) fn marker_path(project_root: &Path, token: &str) -> PathBuf {
    runs_dir(project_root).join(token)
}

/// A token is only ever a bare UUID. Rejecting anything else before it is
/// joined into [`marker_path`] keeps a crafted `AIDA_AUTO_COMPLETE_TOKEN` from
/// escaping `.aida/orchestrator-runs/` (path traversal). trace:BUG-233
fn is_valid_token(token: &str) -> bool {
    uuid::Uuid::parse_str(token).is_ok()
}

/// The contents of an orchestrator-run marker file. Only [`RunMarker::pid`] is
/// load-bearing for orchestrator corroboration; `spec` and `started_at` are
/// diagnostic (and the natural fold-in point when STORY-301's drain-state file
/// lands). `zen` records whether the run was started with `--zen` — a phase
/// child corroborates its inherited `AIDA_ZEN` against it (BUG-237).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunMarker {
    /// PID of the orchestrator process that owns this run.
    pub(crate) pid: u32,
    /// The spec the run is draining (diagnostic).
    pub(crate) spec: String,
    /// RFC-3339 timestamp the run started (diagnostic).
    pub(crate) started_at: String,
    /// BUG-237: whether the orchestrator run was started with `--zen`. A
    /// phase child trusts an inherited `AIDA_ZEN=1` only when this is true.
    pub(crate) zen: bool,
}

impl RunMarker {
    /// Render as the `key=value` line format written to disk.
    fn serialize(&self) -> String {
        format!(
            "pid={}\nspec={}\nstarted_at={}\nzen={}\n",
            self.pid, self.spec, self.started_at, self.zen
        )
    }

    /// Parse a marker file body. Returns `None` when the file is unreadable or
    /// has no parseable `pid=` line — a torn write fails safe (no live run).
    /// A missing `zen=` line defaults to `false` (back-compat with pre-BUG-237
    /// markers, and the safe direction — an un-flagged run is not zen).
    fn parse(body: &str) -> Option<Self> {
        let mut pid: Option<u32> = None;
        let mut spec = String::new();
        let mut started_at = String::new();
        let mut zen = false;
        for line in body.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "pid" => pid = value.trim().parse::<u32>().ok(),
                "spec" => spec = value.trim().to_string(),
                "started_at" => started_at = value.trim().to_string(),
                "zen" => zen = value.trim() == "true",
                _ => {}
            }
        }
        pid.map(|pid| Self {
            pid,
            spec,
            started_at,
            zen,
        })
    }

    /// Read + parse the marker file at `path`, or `None` on any failure.
    fn read(path: &Path) -> Option<Self> {
        Self::parse(&std::fs::read_to_string(path).ok()?)
    }
}

/// RAII handle for an orchestrator-run marker file. The marker exists for as
/// long as the guard is held; [`Drop`] removes it, so a finished — or panicked
/// — run leaves no stale marker for a later child to corroborate against.
///
/// The [`token`](Self::token) is what the orchestrator passes to its children
/// as `AIDA_AUTO_COMPLETE_TOKEN`.
pub(crate) struct RunMarkerGuard {
    path: PathBuf,
    token: String,
}

impl RunMarkerGuard {
    /// Mint a fresh run UUID, write its marker file under `project_root`, and
    /// return the guard. The marker records *this* process's PID — so a child
    /// corroborating the token confirms a live orchestrator. `zen` records
    /// whether the run was started with `--zen` (BUG-237).
    pub(crate) fn register(project_root: &Path, spec: &str, zen: bool) -> std::io::Result<Self> {
        let token = uuid::Uuid::now_v7().to_string();
        let dir = runs_dir(project_root);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(&token);
        let marker = RunMarker {
            pid: std::process::id(),
            spec: spec.to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            zen,
        };
        std::fs::write(&path, marker.serialize())?;
        Ok(Self { path, token })
    }

    /// The run token — passed to phase children as `AIDA_AUTO_COMPLETE_TOKEN`.
    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

impl Drop for RunMarkerGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
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
/// naming a marker file under `project_root` whose recorded PID is alive.
pub(crate) fn run_is_live(project_root: &Path, token: &str) -> bool {
    if !is_valid_token(token) {
        return false;
    }
    match RunMarker::read(&marker_path(project_root, token)) {
        Some(marker) => process_probe::pid_is_alive(marker.pid),
        None => false,
    }
}

/// The corroborated verdict for the current process, reading `AIDA_AUTO_COMPLETE`
/// + `AIDA_AUTO_COMPLETE_TOKEN` from the environment and checking the marker
/// file under `project_root` (resolve it with `find_main_worktree_root` so a
/// child running in a sibling worktree reads the orchestrator's `.aida/`).
pub(crate) fn detect(project_root: &Path) -> OrchestratorContext {
    classify(
        std::env::var(AUTO_COMPLETE_ENV).ok().as_deref(),
        std::env::var(TOKEN_ENV).ok().as_deref(),
        |token| run_is_live(project_root, token),
    )
}

/// The marker file of the *live, corroborated* orchestrator run that owns the
/// current process, or `None`. `Some` exactly when [`detect`] would return
/// [`OrchestratorContext::Orchestrated`] — so a caller that needs a field off
/// the marker (zen-mode corroboration, BUG-237) gets it without re-deriving
/// the corroboration. trace:BUG-237 | ai:claude
pub(crate) fn live_run_marker(project_root: &Path) -> Option<RunMarker> {
    let auto_complete = std::env::var(AUTO_COMPLETE_ENV).ok()?;
    if auto_complete.is_empty() {
        return None;
    }
    let token = std::env::var(TOKEN_ENV).ok()?;
    if token.is_empty() || !is_valid_token(&token) {
        return None;
    }
    let marker = RunMarker::read(&marker_path(project_root, &token))?;
    process_probe::pid_is_alive(marker.pid).then_some(marker)
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

    // --- RunMarker ----------------------------------------------------------

    #[test]
    fn run_marker_round_trips() {
        let marker = RunMarker {
            pid: 4242,
            spec: "BUG-233".to_string(),
            started_at: "2026-05-18T12:00:00+00:00".to_string(),
            zen: false,
        };
        assert_eq!(RunMarker::parse(&marker.serialize()), Some(marker));
    }

    #[test]
    fn run_marker_zen_field_round_trips() {
        // BUG-237: the `zen` flag survives serialize → parse both ways.
        for zen in [true, false] {
            let marker = RunMarker {
                pid: 7,
                spec: "BUG-237".to_string(),
                started_at: "now".to_string(),
                zen,
            };
            assert_eq!(
                RunMarker::parse(&marker.serialize()).map(|m| m.zen),
                Some(zen)
            );
        }
        // A pre-BUG-237 marker with no `zen=` line defaults to false.
        assert_eq!(
            RunMarker::parse("pid=7\nspec=BUG-237\nstarted_at=now\n").map(|m| m.zen),
            Some(false)
        );
    }

    #[test]
    fn run_marker_parse_rejects_body_without_pid() {
        assert_eq!(RunMarker::parse("spec=BUG-233\nstarted_at=now\n"), None);
        assert_eq!(RunMarker::parse(""), None);
        // A torn write that mangled the pid value fails safe.
        assert_eq!(RunMarker::parse("pid=not-a-number\n"), None);
    }

    // --- RunMarkerGuard -----------------------------------------------------

    #[test]
    fn run_marker_guard_writes_then_drop_removes() {
        let dir = tempfile::tempdir().unwrap();
        let path;
        {
            let guard = RunMarkerGuard::register(dir.path(), "BUG-233", true).unwrap();
            path = marker_path(dir.path(), guard.token());
            assert!(path.exists(), "marker should exist while guard is held");
            // The token is a UUID and the marker records this process.
            assert!(is_valid_token(guard.token()));
            let marker = RunMarker::read(&path).unwrap();
            assert_eq!(marker.pid, std::process::id());
            assert_eq!(marker.spec, "BUG-233");
            assert!(marker.zen, "register(.., zen=true) records the zen flag");
        }
        assert!(!path.exists(), "Drop should remove the marker");
    }

    // --- run_is_live --------------------------------------------------------

    #[test]
    fn run_is_live_true_for_live_marker() {
        let dir = tempfile::tempdir().unwrap();
        let guard = RunMarkerGuard::register(dir.path(), "BUG-233", false).unwrap();
        // The marker records this process's PID, which is alive.
        assert!(run_is_live(dir.path(), guard.token()));
    }

    #[test]
    fn run_is_live_false_for_missing_marker() {
        let dir = tempfile::tempdir().unwrap();
        let absent = uuid::Uuid::now_v7().to_string();
        assert!(!run_is_live(dir.path(), &absent));
    }

    #[test]
    fn run_is_live_false_for_dead_pid() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(runs_dir(dir.path())).unwrap();
        let token = uuid::Uuid::now_v7().to_string();
        std::fs::write(
            marker_path(dir.path(), &token),
            RunMarker {
                pid: u32::MAX - 1, // no real process owns this
                spec: "BUG-233".to_string(),
                started_at: "now".to_string(),
                zen: false,
            }
            .serialize(),
        )
        .unwrap();
        assert!(!run_is_live(dir.path(), &token));
    }

    #[test]
    fn run_is_live_false_for_non_uuid_token() {
        let dir = tempfile::tempdir().unwrap();
        // A path-traversal attempt is rejected before the join — even if a file
        // happened to exist at the escaped location.
        assert!(!run_is_live(dir.path(), "../../etc/passwd"));
        assert!(!run_is_live(dir.path(), "not-a-uuid"));
    }
}
