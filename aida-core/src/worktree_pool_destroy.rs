//! Tiered, dry-run-by-default worktree-pool teardown (STORY-714).
//!
//! `return_to_pool` resets-and-keeps; **only** `destroy` deletes a directory.
//! It replaces today's blunt `git worktree remove --force` with a classified,
//! salvage-aware removal: each tree is sorted into a risk class, and a tree is
//! removed only when it is `Disposable` or the caller opted into its class with
//! one `--include-*` flag. The `pre_destroy` hook (`cargo clean`) fires here —
//! the one place a tree is actually deleted — so deliberate teardown can't
//! poison sibling worktrees' cargo caches (TASK-0396).
//!
//! Ported from treehouse `DestroyOptions.missingFlags` / `classifyForDestroy`.
//!
//! trace:STORY-714 trace:TASK-0396 | ai:claude

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::git_ops;
use crate::worktree_pool::{self, PoolEntry};

/// Risk class of a destroy target. Higher-risk classes win when a tree matches
/// several (leased ∧ dirty → `Leased`), mirroring treehouse's precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestroyClass {
    /// Clean, landed, no live owner, no lease — safe to remove.
    Disposable,
    /// Uncommitted changes present.
    Dirty,
    /// HEAD has commits not yet merged into the default branch.
    Unmerged,
    /// A live process owns the tree.
    InUse,
    /// Durably reserved (lease).
    Leased,
    /// Couldn't verify state (path/registration anomaly) — treat as risky.
    Unverified,
}

impl DestroyClass {
    pub fn label(self) -> &'static str {
        match self {
            DestroyClass::Disposable => "disposable",
            DestroyClass::Dirty => "dirty",
            DestroyClass::Unmerged => "unmerged",
            DestroyClass::InUse => "in-use",
            DestroyClass::Leased => "leased",
            DestroyClass::Unverified => "unverified",
        }
    }
}

/// What `destroy` decided to do with one tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestroyAction {
    /// Dry-run: would be removed if applied.
    WouldRemove,
    /// Actually removed.
    Removed,
    /// Left in place (names the missing flag in the target's `reason`).
    Skipped,
}

/// One classified tree in a destroy report.
#[derive(Debug, Clone)]
pub struct DestroyTarget {
    pub entry: PoolEntry,
    pub class: DestroyClass,
    pub action: DestroyAction,
    /// Human-readable reason — for a skip, names the `--include-*` flag needed.
    pub reason: String,
}

/// The outcome of a `destroy` pass.
#[derive(Debug, Clone)]
pub struct DestroyReport {
    pub dry_run: bool,
    pub targets: Vec<DestroyTarget>,
}

impl DestroyReport {
    pub fn removed_count(&self) -> usize {
        self.targets
            .iter()
            .filter(|t| t.action == DestroyAction::Removed)
            .count()
    }
    pub fn would_remove_count(&self) -> usize {
        self.targets
            .iter()
            .filter(|t| t.action == DestroyAction::WouldRemove)
            .count()
    }
}

/// Per-risk-class opt-in flags + dry-run default.
#[derive(Debug, Clone, Default)]
pub struct DestroyOptions {
    pub dry_run: bool,
    /// Allow removal of dirty / unmerged / unverified trees.
    pub include_unlanded: bool,
    /// Allow removal of a tree with a live owner process.
    pub include_in_use: bool,
    /// Allow removal of a durably-leased tree. Honored ONLY when the exact
    /// path is named (never in a bulk `--all` sweep).
    pub include_leased: bool,
    /// `pre_destroy` shell hooks (machine-global config only).
    pub pre_destroy_hooks: Vec<String>,
}

/// Which trees a destroy pass targets.
#[derive(Debug, Clone)]
pub enum DestroySelector {
    /// Every registered pool tree (bulk sweep — `--include-leased` is ignored).
    All,
    /// Specific worktree paths (a named selection — `--include-leased` honored).
    Paths(Vec<PathBuf>),
}

/// Classify a single entry for destruction. `default_ref` is the furthest-ahead
/// default ref to test "merged" against.
pub fn classify_for_destroy(entry: &PoolEntry, default_ref: &str) -> DestroyClass {
    if entry.leased {
        return DestroyClass::Leased;
    }
    if worktree_pool::owner_is_live(entry) {
        return DestroyClass::InUse;
    }
    if !entry.path.exists() {
        return DestroyClass::Unverified;
    }
    if git_ops::worktree_is_dirty(&entry.path) {
        return DestroyClass::Dirty;
    }
    if !git_ops::worktree_head_is_merged(&entry.path, default_ref) {
        return DestroyClass::Unmerged;
    }
    DestroyClass::Disposable
}

/// The `--include-*` flag a class needs, or None when it's freely removable.
/// `leased_named` is true only when the leased tree was named by exact path.
fn missing_flag(
    class: DestroyClass,
    opts: &DestroyOptions,
    leased_named: bool,
) -> Option<&'static str> {
    match class {
        DestroyClass::Disposable => None,
        DestroyClass::Dirty | DestroyClass::Unmerged | DestroyClass::Unverified => {
            if opts.include_unlanded {
                None
            } else {
                Some("--include-unlanded")
            }
        }
        DestroyClass::InUse => {
            if opts.include_in_use {
                None
            } else {
                Some("--include-in-use")
            }
        }
        DestroyClass::Leased => {
            if opts.include_leased && leased_named {
                None
            } else {
                Some("--include-leased")
            }
        }
    }
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Run a destroy pass. Dry-run by default — `opts.dry_run` must be false to
/// actually remove anything. `salvage` is invoked on the worktree path before
/// any unlanded (dirty/unmerged) removal so uncommitted work is patch-saved
/// first (the CLI passes AIDA's `salvage_worktree_patch`).
pub fn destroy(
    project_root: &Path,
    selector: &DestroySelector,
    opts: &DestroyOptions,
    salvage: &mut dyn FnMut(&Path),
) -> Result<DestroyReport> {
    let default_ref = git_ops::furthest_ahead_default_ref(project_root)?;
    // `--include-leased` is honored only for an explicitly-named selection.
    let leased_named = matches!(selector, DestroySelector::Paths(_));

    let selected_canon: Option<Vec<PathBuf>> = match selector {
        DestroySelector::All => None,
        DestroySelector::Paths(ps) => Some(ps.iter().map(|p| canonical(p)).collect()),
    };

    worktree_pool::with_state_lock(project_root, |pool| {
        let mut targets: Vec<DestroyTarget> = Vec::new();
        let mut remove_idx: Vec<usize> = Vec::new();

        for (idx, entry) in pool.entries.iter().enumerate() {
            // Skip entries outside a named selection.
            if let Some(sel) = &selected_canon {
                if !sel.iter().any(|p| *p == canonical(&entry.path)) {
                    continue;
                }
            }

            let class = classify_for_destroy(entry, &default_ref);
            let missing = missing_flag(class, opts, leased_named);

            match (opts.dry_run, missing) {
                (true, None) => targets.push(DestroyTarget {
                    entry: entry.clone(),
                    class,
                    action: DestroyAction::WouldRemove,
                    reason: format!("{} — would remove (dry-run)", class.label()),
                }),
                (true, Some(flag)) => targets.push(DestroyTarget {
                    entry: entry.clone(),
                    class,
                    action: DestroyAction::Skipped,
                    reason: format!("{} — needs {} (dry-run)", class.label(), flag),
                }),
                (false, Some(flag)) => targets.push(DestroyTarget {
                    entry: entry.clone(),
                    class,
                    action: DestroyAction::Skipped,
                    reason: format!("{} — skipped, needs {}", class.label(), flag),
                }),
                (false, None) => {
                    // Salvage unlanded work before the irreversible remove.
                    if matches!(
                        class,
                        DestroyClass::Dirty | DestroyClass::Unmerged | DestroyClass::Unverified
                    ) {
                        salvage(&entry.path);
                    }
                    // pre_destroy hooks (cargo clean) — TASK-0396 swept HERE.
                    crate::worktree_hooks::run_hooks(
                        &opts.pre_destroy_hooks,
                        &entry.path,
                        "pre_destroy",
                    );
                    git_ops::remove_worktree_at(project_root, &entry.path, true)?;
                    remove_idx.push(idx);
                    targets.push(DestroyTarget {
                        entry: entry.clone(),
                        class,
                        action: DestroyAction::Removed,
                        reason: format!("{} — removed", class.label()),
                    });
                }
            }
        }

        // Drop removed entries from the registry (reverse order keeps indices valid).
        for idx in remove_idx.into_iter().rev() {
            pool.entries.remove(idx);
        }

        Ok(DestroyReport {
            dry_run: opts.dry_run,
            targets,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> PoolEntry {
        PoolEntry {
            name: name.into(),
            path: PathBuf::from(format!("/tmp/{name}")),
            ..Default::default()
        }
    }

    #[test]
    fn leased_outranks_other_classes() {
        let mut e = entry("a");
        e.leased = true;
        // Even though the path doesn't exist (would be Unverified), lease wins.
        assert_eq!(classify_for_destroy(&e, "main"), DestroyClass::Leased);
    }

    #[test]
    fn missing_flag_maps_class_to_opt_in() {
        let none = DestroyOptions::default();
        assert_eq!(missing_flag(DestroyClass::Disposable, &none, false), None);
        assert_eq!(
            missing_flag(DestroyClass::Dirty, &none, false),
            Some("--include-unlanded")
        );
        assert_eq!(
            missing_flag(DestroyClass::Unmerged, &none, false),
            Some("--include-unlanded")
        );
        assert_eq!(
            missing_flag(DestroyClass::InUse, &none, false),
            Some("--include-in-use")
        );
        assert_eq!(
            missing_flag(DestroyClass::Leased, &none, true),
            Some("--include-leased")
        );
    }

    #[test]
    fn include_unlanded_clears_dirty_and_unmerged() {
        let opts = DestroyOptions {
            include_unlanded: true,
            ..Default::default()
        };
        assert_eq!(missing_flag(DestroyClass::Dirty, &opts, false), None);
        assert_eq!(missing_flag(DestroyClass::Unmerged, &opts, false), None);
        assert_eq!(missing_flag(DestroyClass::Unverified, &opts, false), None);
    }

    #[test]
    fn leased_removed_only_when_named_and_flagged() {
        let opts = DestroyOptions {
            include_leased: true,
            ..Default::default()
        };
        // Named single path + flag → removable.
        assert_eq!(missing_flag(DestroyClass::Leased, &opts, true), None);
        // Bulk --all (not named) → still blocked even with the flag.
        assert_eq!(
            missing_flag(DestroyClass::Leased, &opts, false),
            Some("--include-leased")
        );
    }

    #[test]
    fn in_use_needs_include_in_use() {
        let only_unlanded = DestroyOptions {
            include_unlanded: true,
            ..Default::default()
        };
        assert_eq!(
            missing_flag(DestroyClass::InUse, &only_unlanded, false),
            Some("--include-in-use")
        );
        let with = DestroyOptions {
            include_in_use: true,
            ..Default::default()
        };
        assert_eq!(missing_flag(DestroyClass::InUse, &with, false), None);
    }
}

/// Destroy integration tests over a real git repo.
// trace:STORY-714 | ai:claude
#[cfg(test)]
mod git_integration_tests {
    use super::*;
    use crate::worktree_pool::{self, AcquireOptions};
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?} failed");
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@t.t"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("README.md"), "seed").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-qm", "seed"]);
        dir
    }

    fn no_salvage() -> impl FnMut(&Path) {
        |_p: &Path| {}
    }

    #[test]
    fn destroy_dry_run_removes_nothing() {
        let repo = init_repo();
        let root = repo.path();
        let p1 = worktree_pool::acquire(
            root,
            &AcquireOptions {
                max_trees: Some(4),
                ..Default::default()
            },
        )
        .unwrap();
        // Return so the entry has no live owner (a clean, disposable tree).
        worktree_pool::return_to_pool(root, &p1).unwrap();

        let opts = DestroyOptions {
            dry_run: true,
            ..Default::default()
        };
        let mut sv = no_salvage();
        let report = destroy(root, &DestroySelector::All, &opts, &mut sv).unwrap();
        assert!(report.dry_run);
        assert_eq!(report.removed_count(), 0);
        assert_eq!(report.would_remove_count(), 1);
        assert!(p1.exists(), "dry-run must not remove the directory");
    }

    #[test]
    fn destroy_disposable_removes_and_runs_pre_destroy_hook() {
        let repo = init_repo();
        let root = repo.path();
        let p1 = worktree_pool::acquire(
            root,
            &AcquireOptions {
                max_trees: Some(4),
                ..Default::default()
            },
        )
        .unwrap();
        worktree_pool::return_to_pool(root, &p1).unwrap();

        // pre_destroy hook writes a marker into the tree BEFORE it is removed —
        // this is where TASK-0396's `cargo clean` fires on the one delete path.
        let marker = root.join("pre-destroy-ran.marker");
        let hook = format!("touch {}", marker.display());
        let opts = DestroyOptions {
            dry_run: false,
            pre_destroy_hooks: vec![hook],
            ..Default::default()
        };
        let mut sv = no_salvage();
        let report = destroy(root, &DestroySelector::All, &opts, &mut sv).unwrap();
        assert_eq!(report.removed_count(), 1);
        assert!(!p1.exists(), "disposable tree should be removed");
        assert!(marker.exists(), "pre_destroy hook must run before removal");
        // Registry no longer lists it.
        assert!(worktree_pool::read_state(root).unwrap().entries.is_empty());
    }

    #[test]
    fn destroy_skips_dirty_without_include_unlanded() {
        let repo = init_repo();
        let root = repo.path();
        let p1 = worktree_pool::acquire(
            root,
            &AcquireOptions {
                max_trees: Some(4),
                ..Default::default()
            },
        )
        .unwrap();
        worktree_pool::return_to_pool(root, &p1).unwrap();
        std::fs::write(p1.join("dirty.txt"), "x").unwrap();

        let opts = DestroyOptions {
            dry_run: false,
            ..Default::default()
        };
        let mut sv = no_salvage();
        let report = destroy(root, &DestroySelector::All, &opts, &mut sv).unwrap();
        assert_eq!(report.removed_count(), 0);
        assert!(
            p1.exists(),
            "dirty tree must be kept without --include-unlanded"
        );
        assert!(report.targets[0].reason.contains("--include-unlanded"));
    }

    #[test]
    fn destroy_all_never_removes_leased_tree() {
        let repo = init_repo();
        let root = repo.path();
        let p1 = worktree_pool::acquire(
            root,
            &AcquireOptions {
                lease_holder: Some("drain".into()),
                max_trees: Some(4),
                ..Default::default()
            },
        )
        .unwrap();
        // Leave it leased (don't return). Bulk --all must refuse even with the flag.
        let opts = DestroyOptions {
            dry_run: false,
            include_leased: true,
            ..Default::default()
        };
        let mut sv = no_salvage();
        let report = destroy(root, &DestroySelector::All, &opts, &mut sv).unwrap();
        assert_eq!(report.removed_count(), 0);
        assert!(
            p1.exists(),
            "a leased tree is never removed by a bulk --all sweep"
        );
        assert!(report.targets[0].reason.contains("--include-leased"));
    }
}
