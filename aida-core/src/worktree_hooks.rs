//! Worktree-pool lifecycle hooks (STORY-714).
//!
//! `post_create` runs after a fresh `git worktree add`; `pre_destroy` runs
//! before each delete — the one place TASK-0396's `cargo clean` finally fires,
//! at the exact moment a tree's `target/.fingerprint` paths are about to dangle.
//!
//! SECURITY (treehouse's stance, ported): executable hook commands are a
//! code-exec surface, so they are honored **only from machine-global config**
//! (`~/.aida/`), never a checked-in repo-level `.aida/config.toml` — cloning a
//! repo must not be able to run arbitrary shell on your machine. The CLI layer
//! owns that sourcing rule; this module only runs what it is handed. Failures
//! are logged, never fatal.
//!
//! trace:STORY-714 trace:TASK-0396 | ai:claude

use std::path::Path;
use std::process::Command;

/// Canonical `post_create` pre-warm command (TASK-1010): kick off a
/// `cargo build` so a freshly-created pool worktree's `target/` is warm before
/// the first fanned agent builds in it — turning cold-create into
/// warm-on-first-use.
///
/// It is **backgrounded** (`nohup … &`): `run_hooks` runs each hook via
/// `sh -c`, and the trailing `&` lets that shell return immediately, so the
/// pre-warm never delays the tree handout (non-blocking). `nohup` detaches the
/// build from the short-lived acquire process so it survives that process
/// exiting; output is discarded because a pre-warm is advisory — a broken or
/// incomplete build just means the first real build is colder, never an error.
///
/// This is the command the opt-in `[worktree_pool] prewarm_build = true` knob
/// (honored **only** from the machine-global `~/.aida/config.toml`, like every
/// other hook — see the module docs) appends to the `post_create` hooks. Users
/// who want a different pre-warm (a non-Rust project, a narrower `-p` build)
/// write their own `post_create` hook line instead.
// trace:TASK-1010 | ai:claude
pub const PREWARM_BUILD_COMMAND: &str = "nohup cargo build >/dev/null 2>&1 &";

/// Run each hook command (a shell line) sequentially in `work_dir`. A failing
/// hook is logged to stderr and skipped — hooks are best-effort, never fatal:
/// a broken pre-warm must not block a worktree handout, and a broken
/// `pre_destroy` must not wedge teardown. `phase` is log context only.
pub fn run_hooks(commands: &[String], work_dir: &Path, phase: &str) {
    for cmd in commands {
        if cmd.trim().is_empty() {
            continue;
        }
        let status = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(work_dir)
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => eprintln!(
                "aida: {phase} hook exited {} in {}: {}",
                s.code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into()),
                work_dir.display(),
                cmd
            ),
            Err(e) => eprintln!(
                "aida: {phase} hook failed to spawn in {} ({e}): {}",
                work_dir.display(),
                cmd
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_hooks_executes_in_work_dir() {
        let dir = tempfile::tempdir().unwrap();
        run_hooks(&["touch ran.marker".to_string()], dir.path(), "post_create");
        assert!(dir.path().join("ran.marker").exists());
    }

    #[test]
    fn run_hooks_failure_is_non_fatal() {
        let dir = tempfile::tempdir().unwrap();
        // A failing command must not panic and must not stop later hooks.
        run_hooks(
            &["false".to_string(), "touch after.marker".to_string()],
            dir.path(),
            "pre_destroy",
        );
        assert!(dir.path().join("after.marker").exists());
    }

    // TASK-1010: the pre-warm command must be backgrounded so it never blocks
    // the tree handout, and detached so it survives the acquire process exiting.
    #[test]
    fn prewarm_build_command_is_backgrounded_and_detached() {
        assert!(PREWARM_BUILD_COMMAND.trim_end().ends_with('&'));
        assert!(PREWARM_BUILD_COMMAND.contains("cargo build"));
        assert!(PREWARM_BUILD_COMMAND.contains("nohup"));
    }

    // TASK-1010: running a backgrounded post_create hook returns promptly and
    // is non-fatal, matching how the pre-warm command is dispatched.
    #[test]
    fn run_hooks_backgrounded_command_is_non_blocking() {
        let dir = tempfile::tempdir().unwrap();
        // Sleep-then-touch, backgrounded: run_hooks must return before the
        // marker appears (the shell backgrounds it and exits immediately).
        run_hooks(
            &["nohup sh -c 'sleep 5; touch late.marker' >/dev/null 2>&1 &".to_string()],
            dir.path(),
            "post_create",
        );
        assert!(
            !dir.path().join("late.marker").exists(),
            "backgrounded hook must not block run_hooks"
        );
    }

    #[test]
    fn run_hooks_skips_blank_commands() {
        let dir = tempfile::tempdir().unwrap();
        run_hooks(
            &["".to_string(), "   ".to_string()],
            dir.path(),
            "post_create",
        );
        // Nothing to assert beyond "did not panic"; no marker created.
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }
}
