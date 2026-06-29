// trace:STORY-716 | ai:claude
//! `aida worktree` — the EPIC-55 workspace layer.
//!
//! A subcommand namespace that mirrors `git worktree` and houses AIDA's
//! worktree operations. The first slice (STORY-716) is **create-or-enter an
//! epic-scoped workspace**:
//!
//! - `aida worktree add <epic>` — create a git worktree off origin/main
//!   (default path `~/ai/aida-<epic-slug>`, default branch `<epic>-work`),
//!   then write the STORY-706 focus marker INSIDE the new worktree so it is
//!   auto-scoped to that epic. Plain subcommand: prints the path, does NOT cd
//!   (mirrors `git worktree add`). Idempotent — an existing worktree is
//!   re-affirmed, not re-created.
//! - `aida worktree enter <epic>` — the eval-wrapper that creates-if-missing
//!   then **cd's you in**. A subprocess cannot cd its parent shell, so this
//!   follows the existing auto-eval shell-function pattern (`aida role enter`
//!   / `aida dev activate`): the binary emits `cd '<path>'` on stdout and the
//!   `aida()` shell wrapper auto-evals it. MUST be run as bare
//!   `aida worktree enter <epic>` — NOT wrapped in `eval "$(...)"` (the
//!   function auto-evals; the no-double-eval convention, TASK-667).
//! - `aida worktree list` — AIDA-managed worktrees + each one's focus.
//!
//! This module owns the **pure derivation helpers** (path / branch / slug)
//! plus the porcelain parsing, all unit-testable without git. The thin
//! IO/dispatch layer (running `git worktree add`, printing, emitting shell)
//! lives in `main.rs` next to the other command handlers.
//!
//! STORY-714 (warm-pool) will land `aida worktree pool status` + a tiered
//! `aida worktree remove` as sibling subcommands under this same namespace —
//! the `WorktreeCommand` enum in `cli.rs` is shaped to leave room for them.

use std::path::{Path, PathBuf};

/// Normalize a raw epic argument to its canonical uppercase SPEC-ID form, used
/// as the focus label written into the new worktree's `.aida/focus`.
/// `epic-54` / `Epic-54` / `EPIC-54` -> `EPIC-54`. Best-effort (uppercase
/// only): the worktree slice intentionally does NOT hit the cache to validate
/// — the focus marker is written via the low-level `focus::write_focus_marker`,
/// not the validating `aida focus` path (which needs the store the fresh
/// worktree hasn't attached yet).
pub fn normalize_epic_label(raw: &str) -> String {
    raw.trim().to_ascii_uppercase()
}

/// Slug used in the default worktree directory name: lowercased, with every
/// non-alphanumeric character dropped. `EPIC-54` -> `epic54`.
pub fn epic_slug(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Default branch name for an epic worktree: `EPIC-54` -> `epic-54-work`.
/// The canonical id lowercased plus a `-work` suffix.
pub fn default_branch(raw: &str) -> String {
    format!("{}-work", normalize_epic_label(raw).to_ascii_lowercase())
}

/// Default worktree path: `<home>/ai/aida-<slug>`. With `home = ~`,
/// `EPIC-54` -> `~/ai/aida-epic54`. Sibling of the main `~/ai/aida` clone, so
/// epic worktrees land as siblings rather than nesting.
pub fn default_worktree_path(home: &Path, raw: &str) -> PathBuf {
    home.join("ai").join(format!("aida-{}", epic_slug(raw)))
}

/// Parse `git worktree list --porcelain` stdout into the list of registered
/// worktree paths. Each record opens with a `worktree <path>` line; we collect
/// those. Pure so the porcelain handling is unit-testable without git.
pub fn parse_worktree_paths(porcelain: &str) -> Vec<PathBuf> {
    porcelain
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(|p| PathBuf::from(p.trim()))
        .collect()
}

/// Whether `target` is already a registered worktree in `porcelain` output.
/// Compares canonicalized paths when possible (so `~/ai/aida-epic54` and a
/// symlink-resolved form match), falling back to a literal compare.
pub fn is_registered(porcelain: &str, target: &Path) -> bool {
    let want = canonical_or_self(target);
    parse_worktree_paths(porcelain)
        .iter()
        .any(|p| canonical_or_self(p) == want)
}

/// Canonicalize a path, falling back to the path itself when it does not yet
/// exist (so comparisons work for not-yet-created targets too).
fn canonical_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_uppercases_canonical_id() {
        assert_eq!(normalize_epic_label("epic-54"), "EPIC-54");
        assert_eq!(normalize_epic_label("EPIC-54"), "EPIC-54");
        assert_eq!(normalize_epic_label("  Epic-54  "), "EPIC-54");
    }

    #[test]
    fn slug_drops_non_alphanumeric_and_lowercases() {
        assert_eq!(epic_slug("EPIC-54"), "epic54");
        assert_eq!(epic_slug("epic-54"), "epic54");
        assert_eq!(epic_slug("STORY-716"), "story716");
    }

    #[test]
    fn default_branch_is_lowercase_dash_work() {
        assert_eq!(default_branch("EPIC-54"), "epic-54-work");
        assert_eq!(default_branch("epic-54"), "epic-54-work");
        assert_eq!(default_branch("STORY-716"), "story-716-work");
    }

    #[test]
    fn default_path_is_sibling_under_ai() {
        let home = Path::new("/home/joe");
        assert_eq!(
            default_worktree_path(home, "EPIC-54"),
            PathBuf::from("/home/joe/ai/aida-epic54")
        );
        assert_eq!(
            default_worktree_path(home, "epic-54"),
            PathBuf::from("/home/joe/ai/aida-epic54")
        );
    }

    #[test]
    fn parse_porcelain_collects_worktree_paths() {
        let porcelain = "\
worktree /home/joe/ai/aida
HEAD abc123
branch refs/heads/main

worktree /home/joe/ai/aida-epic54
HEAD def456
branch refs/heads/epic-54-work
";
        let paths = parse_worktree_paths(porcelain);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/joe/ai/aida"),
                PathBuf::from("/home/joe/ai/aida-epic54"),
            ]
        );
    }

    #[test]
    fn is_registered_matches_a_listed_path() {
        let porcelain = "worktree /tmp/does-not-exist-aida-epic54\nHEAD abc\n";
        assert!(is_registered(
            porcelain,
            Path::new("/tmp/does-not-exist-aida-epic54")
        ));
        assert!(!is_registered(
            porcelain,
            Path::new("/tmp/does-not-exist-aida-epic99")
        ));
    }
}
