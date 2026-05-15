//! Quick actions — the status overlay's only interactive surface.
//!
//! The overlay itself is read-only (STORY-133): it *shows* AIDA state,
//! it does not mutate it. The three [`QuickAction`]s are the exception —
//! each shells out to an existing command as a captured subprocess, lands
//! the output in the overlay's activity log, and returns focus to the
//! hosted session when the overlay closes. Every action is a command the
//! user could have typed by hand; nothing here is a new code path.
//!
//! trace:STORY-133 | ai:claude

use std::process::Command;

/// A one-keystroke action invocable from the status overlay.
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
}

impl QuickAction {
    /// Every action, in menu order. The overlay's selector indexes this.
    pub const ALL: [QuickAction; 3] = [Self::QueueNext, Self::SessionEnd, Self::PrView];

    /// Short button label shown in the overlay's action row and used as
    /// the activity-log entry's heading.
    pub fn label(self) -> &'static str {
        match self {
            QuickAction::QueueNext => "Next in queue",
            QuickAction::SessionEnd => "End session",
            QuickAction::PrView => "View PR",
        }
    }

    /// Whether the overlay must arm a confirm before running this — true
    /// for anything that mutates state. Only [`QuickAction::SessionEnd`]
    /// removes a worktree / drops a lease; the other two are read-only.
    pub fn needs_confirm(self) -> bool {
        matches!(self, QuickAction::SessionEnd)
    }
}

/// Build the argv a quick action shells out to. `aida_exe` is the running
/// `aida` binary's path (mirrors [`crate::app::aida_exe`]), so a dev build
/// drives a dev build; `gh` is resolved from `PATH`.
pub fn argv(action: QuickAction, aida_exe: &str) -> Vec<String> {
    let s = |v: &str| v.to_string();
    match action {
        QuickAction::QueueNext => vec![s(aida_exe), s("queue"), s("next")],
        QuickAction::SessionEnd => vec![s(aida_exe), s("session"), s("end"), s("--yes")],
        QuickAction::PrView => vec![s("gh"), s("pr"), s("view")],
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
    fn only_session_end_needs_confirm() {
        // The read-only actions run straight away; the mutating one gates.
        assert!(QuickAction::SessionEnd.needs_confirm());
        assert!(!QuickAction::QueueNext.needs_confirm());
        assert!(!QuickAction::PrView.needs_confirm());
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
