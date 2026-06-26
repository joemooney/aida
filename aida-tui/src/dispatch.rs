//! In-process intent dispatch — the STORY-681 self-sufficiency slice.
//!
//! Background: STORY-244 split `aida tui` into a launcher that paints a
//! board, exits emitting one [`crate::intent::Intent`] line on fd 3, and a
//! bash `aida-tui` wrapper that read that line, dispatched the action
//! (`eval` the command / `claude --resume <id>`), reset the terminal, and
//! re-entered the launcher. BUG-612 then routed bare `aida tui` through
//! that wrapper so it Just Worked — but it still *required* the shell
//! function.
//!
//! STORY-681 moves the dispatch + re-entry loop **into the `aida tui`
//! process itself** ([`crate::launcher::run`]). When the event loop returns
//! an Intent, the process drops the terminal guard (restoring cooked mode),
//! runs the dispatched command as a child that inherits the real terminal,
//! waits for it, and re-enters the launcher. No fd 3, no `aida-tui` shell
//! function, no `aida dev shell-init` prerequisite — `aida tui` is
//! self-sufficient from any shell.
//!
//! Why direct `Command` spawn and not `sh -c`: every Intent payload has
//! already passed [`crate::intent::is_safe_payload`] (alphanumerics plus a
//! tiny punctuation allow-list — no shell metacharacters), so the command
//! is a plain whitespace-tokenised argv. Spawning the program directly with
//! its args inherits stdio onto the real terminal and sidesteps the shell
//! entirely, which is strictly safer than the wrapper's `eval`.
//!
//! trace:STORY-681 | ai:claude

use crate::intent::Intent;
use anyhow::{Context, Result};
use std::process::Command;

/// What an Intent dispatches to, after the loop has restored cooked mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatch {
    /// Stop the re-entry loop and return to the shell.
    Quit,
    /// Run a child process (program + args) that inherits the terminal,
    /// then re-enter the launcher when it exits. Covers `launch:` /
    /// `shell:` (the typed command) and `resume:` (`claude --resume <id>`).
    Run { program: String, args: Vec<String> },
}

/// Translate an [`Intent`] into the [`Dispatch`] the in-process loop runs.
///
/// This is the Rust equivalent of the bash wrapper's `case "$_intent"`:
///   - `Quit`            → [`Dispatch::Quit`]
///   - `Launch(cmd)`     → run the tokenised `cmd`
///   - `Shell(cmd)`      → run the tokenised `cmd`
///   - `Resume(id)`      → run `claude --resume <id>`
///
/// Returns `Err` when a `Launch`/`Shell` payload is empty (no program to
/// run) — the caller surfaces it as a transient notice and re-enters rather
/// than crashing the loop.
//
// trace:STORY-681 | ai:claude
pub fn plan(intent: &Intent) -> Result<Dispatch> {
    match intent {
        Intent::Quit => Ok(Dispatch::Quit),
        Intent::Launch(cmd) | Intent::Shell(cmd) => {
            let mut parts = cmd.split_whitespace();
            let program = parts
                .next()
                .context("launch/shell intent had an empty command")?
                .to_string();
            let args = parts.map(str::to_string).collect();
            Ok(Dispatch::Run { program, args })
        }
        Intent::Resume(id) => {
            anyhow::ensure!(
                !id.trim().is_empty(),
                "resume intent had an empty session id"
            );
            Ok(Dispatch::Run {
                program: "claude".to_string(),
                args: vec!["--resume".to_string(), id.clone()],
            })
        }
    }
}

/// Run a [`Dispatch::Run`] child to completion, inheriting the real
/// terminal (stdin/stdout/stderr), and return its exit status. The caller
/// must already have dropped the [`crate::term::TermGuard`] so cooked mode
/// and the main screen are restored before the child paints.
///
/// On Unix a missing program (e.g. `claude` not on PATH) returns `Err`
/// rather than silently looping; the launcher surfaces it as a notice.
//
// trace:STORY-681 | ai:claude
pub fn run_child(program: &str, args: &[String]) -> Result<std::process::ExitStatus> {
    Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to spawn `{program}` (is it on PATH?)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_plans_to_quit() {
        assert_eq!(plan(&Intent::Quit).unwrap(), Dispatch::Quit);
    }

    #[test]
    fn launch_tokenises_program_and_args() {
        let d = plan(&Intent::Launch("aida queue work STORY-244".into())).unwrap();
        assert_eq!(
            d,
            Dispatch::Run {
                program: "aida".into(),
                args: vec!["queue".into(), "work".into(), "STORY-244".into()],
            }
        );
    }

    #[test]
    fn shell_tokenises_like_launch() {
        let d = plan(&Intent::Shell("gh pr view 42".into())).unwrap();
        assert_eq!(
            d,
            Dispatch::Run {
                program: "gh".into(),
                args: vec!["pr".into(), "view".into(), "42".into()],
            }
        );
    }

    #[test]
    fn resume_becomes_claude_resume() {
        let d = plan(&Intent::Resume("019e2d4f-7777-7abc".into())).unwrap();
        assert_eq!(
            d,
            Dispatch::Run {
                program: "claude".into(),
                args: vec!["--resume".into(), "019e2d4f-7777-7abc".into()],
            }
        );
    }

    #[test]
    fn launch_with_flags_keeps_all_args() {
        // Multi-flag commands the board emits (drain, role-scoped work)
        // must round-trip every token.
        let d = plan(&Intent::Launch("aida queue work --auto-complete".into())).unwrap();
        assert_eq!(
            d,
            Dispatch::Run {
                program: "aida".into(),
                args: vec!["queue".into(), "work".into(), "--auto-complete".into()],
            }
        );
    }

    #[test]
    fn empty_launch_payload_is_err() {
        assert!(plan(&Intent::Launch(String::new())).is_err());
        assert!(plan(&Intent::Launch("   ".into())).is_err());
    }

    #[test]
    fn empty_resume_id_is_err() {
        assert!(plan(&Intent::Resume(String::new())).is_err());
        assert!(plan(&Intent::Resume("  ".into())).is_err());
    }

    #[test]
    fn run_child_executes_and_returns_status() {
        // `true` exits 0, `false` exits non-zero — a portable way to prove
        // run_child actually spawns + waits + reports status on Unix CI.
        let ok = run_child("true", &[]).unwrap();
        assert!(ok.success());
        let bad = run_child("false", &[]).unwrap();
        assert!(!bad.success());
    }

    #[test]
    fn run_child_missing_program_is_err() {
        assert!(run_child("aida-no-such-binary-xyz", &[]).is_err());
    }
}
