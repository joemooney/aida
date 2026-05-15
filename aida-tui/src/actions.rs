//! Quick actions — the status overlay's interactive surface.
//!
//! The overlay itself is read-only (STORY-133): it *shows* AIDA state.
//! The [`QuickAction`]s are the exception, in two flavours:
//!
//!   * **subprocess** actions (`QueueNext`, `SessionEnd`, `PrView`) shell
//!     out to an existing command, capture the output into the activity
//!     log, and return focus to the hosted session on overlay close;
//!   * **injection** actions (`DrainToReview`, `DrainToMerge` — STORY-136)
//!     type a `/aida-drain-queue` slash command into the focused Claude
//!     session and close the overlay so it runs there.
//!
//! Every action is something the user could have typed by hand; nothing
//! here is a new code path.
//!
//! trace:STORY-133 STORY-136 | ai:claude

use std::process::Command;

/// A one-keystroke action invocable from the status overlay.
///
/// Two kinds: **subprocess** actions run a captured command and land the
/// output in the activity log; **injection** actions (the autonomous
/// drains, STORY-136) type a slash command straight into the focused
/// Claude session. [`QuickAction::injection`] distinguishes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickAction {
    /// Preview the head of the active role's queue (`aida queue next`) —
    /// the read-only "what would `queue work` pick up next?" check.
    QueueNext,
    /// End the session covering the TUI's cwd (`aida session end --yes`).
    /// Mutating, so the overlay arms a `y`/cancel confirm first (plan
    /// risk #7 — quick action vs. live lease).
    SessionEnd,
    /// Show the current branch's pull request (`gh pr view`).
    PrView,
    /// Start an autonomous queue drain with the reviewer in the loop —
    /// injects `/aida-drain-queue --mode review` into the focused Claude
    /// session. trace:STORY-136 | ai:claude
    DrainToReview,
    /// Start an autonomous queue drain that merges each PR itself, with
    /// no reviewer checkpoint — injects `/aida-drain-queue --mode
    /// merge`. trace:STORY-136 | ai:claude
    DrainToMerge,
}

impl QuickAction {
    /// Every action, in menu order. The overlay's selector indexes this.
    pub const ALL: [QuickAction; 5] = [
        Self::QueueNext,
        Self::SessionEnd,
        Self::PrView,
        Self::DrainToReview,
        Self::DrainToMerge,
    ];

    /// Short button label shown in the overlay's action row and used as
    /// the activity-log entry's heading.
    pub fn label(self) -> &'static str {
        match self {
            QuickAction::QueueNext => "Next in queue",
            QuickAction::SessionEnd => "End session",
            QuickAction::PrView => "View PR",
            QuickAction::DrainToReview => "Drain → review",
            QuickAction::DrainToMerge => "Drain → merge (no review)",
        }
    }

    /// Whether the overlay must arm a confirm before running this — true
    /// for anything that mutates state or starts an autonomous loop. The
    /// read-only previews (`QueueNext`, `PrView`) run straight away.
    pub fn needs_confirm(self) -> bool {
        matches!(
            self,
            QuickAction::SessionEnd | QuickAction::DrainToReview | QuickAction::DrainToMerge
        )
    }

    /// For an action that drives the focused Claude session by typing a
    /// slash command into it (rather than running a captured
    /// subprocess), the text to inject — `None` for the subprocess
    /// actions.
    ///
    /// The drain actions never hand-write `/goal` text. They invoke the
    /// `/aida-drain-queue` skill (TASK-249), which assembles the `/goal`
    /// prompt with real command flags and the mechanism clause that
    /// matches the mode — the structural fix for the 2026-05-15
    /// phrasing trap (`aida queue work --next` was a non-existent flag;
    /// a hand-picked mechanism clause silently chose the workflow).
    /// trace:STORY-136 | ai:claude
    pub fn injection(self) -> Option<&'static str> {
        match self {
            QuickAction::DrainToReview => Some("/aida-drain-queue --mode review"),
            QuickAction::DrainToMerge => Some("/aida-drain-queue --mode merge"),
            QuickAction::QueueNext | QuickAction::SessionEnd | QuickAction::PrView => None,
        }
    }
}

/// Build the argv a quick action shells out to. `aida_exe` is the running
/// `aida` binary's path (mirrors [`crate::app::aida_exe`]), so a dev build
/// drives a dev build; `gh` is resolved from `PATH`.
///
/// Only meaningful for subprocess actions — the drain actions are
/// injected into the focused PTY ([`QuickAction::injection`]) and never
/// reach this, so they map to an empty argv.
pub fn argv(action: QuickAction, aida_exe: &str) -> Vec<String> {
    let s = |v: &str| v.to_string();
    match action {
        QuickAction::QueueNext => vec![s(aida_exe), s("queue"), s("next")],
        QuickAction::SessionEnd => vec![s(aida_exe), s("session"), s("end"), s("--yes")],
        QuickAction::PrView => vec![s("gh"), s("pr"), s("view")],
        QuickAction::DrainToReview | QuickAction::DrainToMerge => Vec::new(),
    }
}

/// One entry in the overlay's activity log — the result of running a
/// quick action, or a TUI-internal note.
#[derive(Debug, Clone)]
pub struct ActivityEntry {
    /// Local wall-clock time the entry was recorded (`HH:MM:SS`). The log
    /// is a within-session timeline, so time-of-day is the useful stamp.
    pub when: String,
    /// Human label — the action name, or an event source for a note.
    pub label: String,
    /// The command line actually run (joined argv); empty for a note.
    pub command: String,
    /// Whether the subprocess exited successfully.
    pub ok: bool,
    /// Captured output (stdout then stderr), already split into lines.
    pub lines: Vec<String>,
}

impl ActivityEntry {
    /// A TUI-internal note — no subprocess, used for e.g. a fetch failure.
    pub fn note(label: &str, line: &str) -> Self {
        ActivityEntry {
            when: now_hms(),
            label: label.to_string(),
            command: String::new(),
            ok: false,
            lines: vec![line.to_string()],
        }
    }
}

/// Run a quick action to completion, capturing stdout + stderr into an
/// [`ActivityEntry`].
pub fn run(action: QuickAction, aida_exe: &str) -> ActivityEntry {
    run_argv(action.label(), &argv(action, aida_exe))
}

/// Run an explicit argv and fold the result into an [`ActivityEntry`].
/// Never panics — a spawn failure (e.g. `gh` not on `PATH`) becomes a
/// failed entry rather than taking the TUI down. Split out from [`run`]
/// so the capture path is testable without a real `QuickAction`.
pub fn run_argv(label: &str, args: &[String]) -> ActivityEntry {
    let when = now_hms();
    let label = label.to_string();
    let command = args.join(" ");
    let Some((program, rest)) = args.split_first() else {
        return ActivityEntry {
            when,
            label,
            command,
            ok: false,
            lines: vec!["empty command".to_string()],
        };
    };

    let mut cmd = Command::new(program);
    cmd.args(rest);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(out) => {
            let ok = out.status.success();
            let mut lines: Vec<String> = Vec::new();
            for stream in [out.stdout.as_slice(), out.stderr.as_slice()] {
                for line in String::from_utf8_lossy(stream).lines() {
                    lines.push(line.to_string());
                }
            }
            if lines.is_empty() {
                lines.push(if ok {
                    "(completed — no output)".to_string()
                } else {
                    format!("(no output — exit {})", out.status)
                });
            }
            ActivityEntry {
                when,
                label,
                command,
                ok,
                lines,
            }
        }
        Err(e) => ActivityEntry {
            when,
            label,
            command,
            ok: false,
            lines: vec![format!("could not run `{}`: {}", program, e)],
        },
    }
}

/// Local `HH:MM:SS`.
fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_action_argv_shapes() {
        assert_eq!(
            argv(QuickAction::QueueNext, "/opt/aida"),
            vec!["/opt/aida", "queue", "next"]
        );
        assert_eq!(
            argv(QuickAction::SessionEnd, "/opt/aida"),
            vec!["/opt/aida", "session", "end", "--yes"]
        );
        // `gh` is resolved from PATH, not the running aida binary.
        assert_eq!(
            argv(QuickAction::PrView, "/opt/aida"),
            vec!["gh", "pr", "view"]
        );
    }

    #[test]
    fn mutating_and_drain_actions_need_confirm() {
        // Read-only previews run straight away…
        assert!(!QuickAction::QueueNext.needs_confirm());
        assert!(!QuickAction::PrView.needs_confirm());
        // …state-changing / autonomous-loop actions gate behind a confirm.
        assert!(QuickAction::SessionEnd.needs_confirm());
        assert!(QuickAction::DrainToReview.needs_confirm());
        assert!(QuickAction::DrainToMerge.needs_confirm());
    }

    #[test]
    fn drain_actions_inject_the_drain_skill_not_freetext_goal() {
        // STORY-136: the drain buttons must call the `/aida-drain-queue`
        // skill (TASK-249), never hand-write `/goal` text — and the
        // mode flag must match the button.
        assert_eq!(
            QuickAction::DrainToReview.injection(),
            Some("/aida-drain-queue --mode review")
        );
        assert_eq!(
            QuickAction::DrainToMerge.injection(),
            Some("/aida-drain-queue --mode merge")
        );
        // Subprocess actions are not injected.
        assert!(QuickAction::QueueNext.injection().is_none());
        assert!(QuickAction::SessionEnd.injection().is_none());
        // No drain action hand-rolls a literal `/goal`.
        for action in QuickAction::ALL {
            if let Some(text) = action.injection() {
                assert!(
                    !text.starts_with("/goal"),
                    "drain must not hand-write /goal"
                );
            }
        }
    }

    #[test]
    fn run_argv_folds_spawn_failure_into_failed_entry() {
        // A binary that cannot exist must not panic the supervisor.
        let entry = run_argv(
            "bogus",
            &["aida-tui-no-such-binary-zzz".to_string(), "--x".to_string()],
        );
        assert!(!entry.ok);
        assert_eq!(entry.label, "bogus");
        assert!(entry.lines[0].contains("could not run"));
    }
}
