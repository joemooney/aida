//! Commit-or-say-so for setup commands that rewrite the committed
//! `.aida/config.toml` (`aida remote mirror`, the end of `aida init`).
//!
//! A setup command that changes committed config must never leave the file
//! silently dirty: it either commits exactly that file as its own chore
//! commit, or prints one clear "config changed; commit it" line. It only
//! commits when the file was tracked and clean before the command ran, so
//! an operator's unrelated edits to the same file are never swept into an
//! automated commit.
// trace:BUG-1938 | ai:claude

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

/// The committed project config, relative to the project root.
pub(crate) const CONFIG_REL_PATH: &str = ".aida/config.toml";

/// How to settle a config change the command just made.
// trace:BUG-1938 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitMode {
    /// Commit without asking (non-interactive runs).
    Auto,
    /// Ask on the terminal, default yes.
    Prompt,
    /// Never commit; print the hint.
    Never,
}

/// What happened to the config file.
// trace:BUG-1938 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigChangeOutcome {
    /// The file is unchanged (or this is not a git checkout): nothing to say.
    Unchanged,
    /// The change was committed as its own commit.
    Committed,
    /// The change is still in the working tree; the hint was printed.
    LeftUncommitted,
}

/// Interactive terminals are asked; everything else (agents, CI, pipes)
/// commits automatically — the same non-TTY discipline `aida init` uses for
/// its scaffold commit.
// trace:BUG-1938 | ai:claude
pub(crate) fn mode_for_terminal(stdin_is_tty: bool) -> CommitMode {
    if stdin_is_tty {
        CommitMode::Prompt
    } else {
        CommitMode::Auto
    }
}

/// The one line printed whenever the change is left in the working tree.
// trace:BUG-1938 | ai:claude
pub(crate) fn config_change_hint() -> String {
    format!(
        "config changed: {CONFIG_REL_PATH} — commit it with: git add {CONFIG_REL_PATH} && git commit -m \"chore: update AIDA config\""
    )
}

/// `Some(true)` when the config is tracked and has no working-tree or index
/// change; `Some(false)` when it is untracked, missing from the index, or
/// modified; `None` when git cannot answer (not a checkout).
// trace:BUG-1938 | ai:claude
pub(crate) fn config_is_clean(root: &Path) -> Option<bool> {
    let tracked = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--error-unmatch", "--", CONFIG_REL_PATH])
        .output()
        .ok()?;
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--", CONFIG_REL_PATH])
        .output()
        .ok()?;
    if !status.status.success() {
        return None;
    }
    if !tracked.status.success() {
        return Some(false);
    }
    Some(String::from_utf8_lossy(&status.stdout).trim().is_empty())
}

/// Settle a config change: commit exactly `.aida/config.toml` with `subject`
/// when the file was clean before the command (`was_clean`) and `mode`
/// allows it; otherwise print [`config_change_hint`]. A failed commit (hook
/// rejection, missing identity) degrades to the hint, never to an error.
// trace:BUG-1938 | ai:claude
pub(crate) fn finish_config_change(
    root: &Path,
    was_clean: Option<bool>,
    mode: CommitMode,
    subject: &str,
) -> ConfigChangeOutcome {
    let check = crate::glyph(crate::glyphs::Glyph::Check);
    let warn = crate::glyph(crate::glyphs::Glyph::Warning);
    match config_is_clean(root) {
        None | Some(true) => return ConfigChangeOutcome::Unchanged,
        Some(false) => {}
    }
    let proceed = was_clean == Some(true)
        && match mode {
            CommitMode::Auto => true,
            CommitMode::Never => false,
            CommitMode::Prompt => crate::kernel::prompt_yes_no(
                &format!("Commit the {CONFIG_REL_PATH} change now? [Y/n] "),
                true,
            )
            .unwrap_or(false),
        };
    if proceed {
        match aida_core::git_ops::commit_paths(root, subject, &[CONFIG_REL_PATH]) {
            Ok(()) => {
                println!("{check} committed {CONFIG_REL_PATH}: {subject}");
                return ConfigChangeOutcome::Committed;
            }
            Err(e) => eprintln!("{warn} could not commit {CONFIG_REL_PATH}: {e}"),
        }
    }
    println!("{warn} {}", config_change_hint());
    ConfigChangeOutcome::LeftUncommitted
}

/// Set once `aida init` committed its scaffolding in this process, so the
/// end-of-init config refresh may follow it with its own commit. A clone
/// bootstrap (scaffold commit suppressed) or a declined prompt leaves this
/// false and the refresh only prints the hint.
// trace:BUG-1938 | ai:claude
static INIT_SCAFFOLD_COMMITTED: AtomicBool = AtomicBool::new(false);

// trace:BUG-1938 | ai:claude
pub(crate) fn mark_init_scaffold_committed() {
    INIT_SCAFFOLD_COMMITTED.store(true, Ordering::SeqCst);
}

/// Mode for the end-of-init refresh: commit only when the operator already
/// accepted (or the run auto-made) the scaffold commit.
// trace:BUG-1938 | ai:claude
pub(crate) fn init_refresh_mode() -> CommitMode {
    if INIT_SCAFFOLD_COMMITTED.load(Ordering::SeqCst) {
        CommitMode::Auto
    } else {
        CommitMode::Never
    }
}

/// End of `aida init`: settle whatever the post-scaffold steps (forge
/// provider refresh, capabilities record) wrote to the config. Best-effort;
/// never fails init.
// trace:BUG-1938 | ai:claude
pub(crate) fn finish_init_refresh(root: &Path, was_clean: Option<bool>) {
    let outcome = finish_config_change(
        root,
        was_clean,
        init_refresh_mode(),
        "[AI:aida] chore(init): refresh project config",
    );
    if outcome == ConfigChangeOutcome::Committed && aida_core::git_ops::has_remote(root, "origin") {
        println!("  push it so every hub has it: git push");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) {
        let st = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed");
    }

    fn git_out(root: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo with a committed `.aida/config.toml` and a local identity.
    fn repo_with_committed_config(root: &Path) {
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["config", "user.email", "test@example.invalid"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(CONFIG_REL_PATH), "[store]\n").unwrap();
        git(root, &["add", CONFIG_REL_PATH]);
        git(root, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn init_refresh_commits_only_after_the_scaffold_commit() {
        // Process-global flag: assert the transition, not the start state,
        // since another test may already have set it.
        mark_init_scaffold_committed();
        assert_eq!(init_refresh_mode(), CommitMode::Auto);
    }

    #[test]
    fn non_tty_commits_and_tty_prompts() {
        assert_eq!(mode_for_terminal(false), CommitMode::Auto);
        assert_eq!(mode_for_terminal(true), CommitMode::Prompt);
    }

    #[test]
    fn hint_names_the_file_and_the_commit_command() {
        let hint = config_change_hint();
        assert!(
            hint.starts_with("config changed: .aida/config.toml"),
            "{hint}"
        );
        assert!(
            hint.contains("commit it with: git add .aida/config.toml && git commit"),
            "{hint}"
        );
    }

    #[test]
    fn clean_tracked_config_is_clean_and_untracked_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git(root, &["init", "-q", "-b", "main"]);
        // Missing / untracked config is never "clean" — nothing to protect,
        // but also nothing a chore commit should adopt.
        assert_eq!(config_is_clean(root), Some(false));
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(CONFIG_REL_PATH), "[store]\n").unwrap();
        assert_eq!(config_is_clean(root), Some(false));

        let tmp2 = tempfile::tempdir().unwrap();
        repo_with_committed_config(tmp2.path());
        assert_eq!(config_is_clean(tmp2.path()), Some(true));
        std::fs::write(tmp2.path().join(CONFIG_REL_PATH), "[store]\nx = 1\n").unwrap();
        assert_eq!(config_is_clean(tmp2.path()), Some(false));
    }

    #[test]
    fn not_a_checkout_is_unknown_and_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(config_is_clean(tmp.path()), None);
        assert_eq!(
            finish_config_change(tmp.path(), None, CommitMode::Auto, "chore: x"),
            ConfigChangeOutcome::Unchanged
        );
    }

    #[test]
    fn auto_commits_only_the_config_and_leaves_other_staged_work() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo_with_committed_config(root);
        let was_clean = config_is_clean(root);
        // Unrelated staged work that must not ride along.
        std::fs::write(root.join("wip.txt"), "wip\n").unwrap();
        git(root, &["add", "wip.txt"]);
        std::fs::write(root.join(CONFIG_REL_PATH), "[store]\nx = 1\n").unwrap();

        let outcome = finish_config_change(root, was_clean, CommitMode::Auto, "chore: test");
        assert_eq!(outcome, ConfigChangeOutcome::Committed);
        assert_eq!(config_is_clean(root), Some(true));
        assert_eq!(git_out(root, &["log", "-1", "--format=%s"]), "chore: test");
        let files = git_out(root, &["show", "--name-only", "--format=", "HEAD"]);
        assert_eq!(files, CONFIG_REL_PATH);
        assert_eq!(
            git_out(root, &["diff", "--cached", "--name-only"]),
            "wip.txt",
            "unrelated staged work must stay staged, uncommitted"
        );
    }

    #[test]
    fn previously_dirty_config_is_never_auto_committed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo_with_committed_config(root);
        std::fs::write(root.join(CONFIG_REL_PATH), "[store]\n# operator wip\n").unwrap();
        let was_clean = config_is_clean(root);
        assert_eq!(was_clean, Some(false));
        let head = git_out(root, &["rev-parse", "HEAD"]);

        let outcome = finish_config_change(root, was_clean, CommitMode::Auto, "chore: test");
        assert_eq!(outcome, ConfigChangeOutcome::LeftUncommitted);
        assert_eq!(git_out(root, &["rev-parse", "HEAD"]), head);
    }

    #[test]
    fn never_mode_leaves_the_change_and_does_not_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo_with_committed_config(root);
        let was_clean = config_is_clean(root);
        std::fs::write(root.join(CONFIG_REL_PATH), "[store]\nx = 1\n").unwrap();
        let head = git_out(root, &["rev-parse", "HEAD"]);

        let outcome = finish_config_change(root, was_clean, CommitMode::Never, "chore: test");
        assert_eq!(outcome, ConfigChangeOutcome::LeftUncommitted);
        assert_eq!(git_out(root, &["rev-parse", "HEAD"]), head);
    }
}
