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
