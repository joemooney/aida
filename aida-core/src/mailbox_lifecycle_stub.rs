//! Non-Unix stand-in for the mail-source lifecycle: the retention barrier
//! needs flock, `O_NOFOLLOW` and `/proc`, so on other platforms every
//! destructive entry proceeds as it did before STORY-1843 C2, with nothing
//! retained.
//!
//! trace:STORY-1843 | ai:claude

/// Named by pool and session-end refusals; never reached off Unix.
pub const ACTIVATION_COMMAND: &str = "aida mailbox retention activate --dry-run";

pub mod guard {
    use std::path::Path;

    /// Never constructed off Unix.
    #[derive(Debug)]
    pub enum Retirement {}

    impl Retirement {
        pub fn complete(self) -> std::io::Result<()> {
            match self {}
        }
        pub fn complete_reuse(self) -> std::io::Result<()> {
            match self {}
        }
        pub fn reopen(self) -> std::io::Result<()> {
            match self {}
        }
    }

    /// Never constructed off Unix.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RetentionRefusal {
        pub checkout: std::path::PathBuf,
        pub reason: String,
        pub needs_activation: bool,
        pub retained_dir: Option<std::path::PathBuf>,
    }

    impl std::fmt::Display for RetentionRefusal {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.reason)
        }
    }

    impl std::error::Error for RetentionRefusal {}
    pub type Gate = Result<Option<Retirement>, RetentionRefusal>;

    pub fn retire_checkout(_checkout: &Path, _reason: &str) -> Gate {
        Ok(None)
    }

    pub fn retire_clone_checkout(_main: Option<&Path>, _clone_root: &Path, _reason: &str) -> Gate {
        Ok(None)
    }

    pub fn remove_worktree_retained(
        repo_root: &Path,
        worktree_path: &Path,
        force: bool,
        _reason: &str,
    ) -> anyhow::Result<()> {
        crate::git_ops::remove_worktree_unguarded(repo_root, worktree_path, force)
    }

    pub fn remove_worktree_with(
        repo_root: &Path,
        worktree_path: &Path,
        force: bool,
        _retirement: Option<Retirement>,
    ) -> anyhow::Result<()> {
        crate::git_ops::remove_worktree_unguarded(repo_root, worktree_path, force)
    }

    pub fn settle<T>(
        step: anyhow::Result<T>,
        _retirement: Option<Retirement>,
        _on_success: impl FnOnce(Retirement) -> std::io::Result<()>,
    ) -> anyhow::Result<T> {
        step
    }
}
