//! Migrate pre-pool sibling worktrees into the warm-pool registry (TASK-1009).
//!
//! The warm-pool (STORY-714) only manages worktrees it created. Every worktree
//! that predates the pool — the `../aida-<slug>` siblings the old
//! destroy-and-recreate model minted, plus anything a human made by hand — is
//! invisible to `pool status`, is never reused, and is never swept. `adopt`
//! closes that migration gap: it **registers** existing worktrees into
//! `pool.json` so they become first-class pool members.
//!
//! Three properties make this safe to run against a live workspace:
//!
//! 1. **It never touches a worktree.** `adopt` writes registry entries and
//!    nothing else — no reset, no checkout, no removal, no hook execution. A
//!    mis-adopted tree is undone by dropping its entry.
//! 2. **Preview by default.** Like its sibling `destroy`, an adopt pass is a
//!    dry-run unless the caller opts in, so the classification can be read
//!    before anything becomes acquirable.
//! 3. **Unlanded work is adopted RESERVED, never idle.** A tree with
//!    uncommitted changes or unmerged commits is registered with a durable
//!    lease **and no `leased_at` stamp**, so it is never handed out by
//!    `acquire` (which would hard-reset it) and — because the TASK-1008 TTL
//!    can't judge an unstamped lease — never expires into reclaimability
//!    either. It shows up in `pool status` as `leased`, and a human releases it
//!    deliberately with `pool return` (which salvages the dirty diff first) or
//!    `pool destroy --include-unlanded`.
//!
//! Ambiguous cases are **refused, not guessed**: a path that isn't a registered
//! worktree of this repo, the main checkout itself, a worktree nested inside
//! the project root (`.aida-store`), a missing directory, or a tree a live
//! session lease is holding are all skipped with a reason.
//!
//! trace:TASK-1009 trace:STORY-714 | ai:claude

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::git_ops;
use crate::worktree_pool::{self, PoolEntry};

/// How one adopt candidate was classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptClass {
    /// Clean and already landed — adoptable as an idle, immediately-reusable
    /// pool tree.
    Adoptable,
    /// Uncommitted changes present — adoptable only as a durable reservation.
    Dirty,
    /// HEAD carries commits not yet merged into the default branch —
    /// adoptable only as a durable reservation.
    Unmerged,
    /// A live session lease holds this worktree. Never adopted: the pool would
    /// start accounting for a tree somebody is actively working in.
    InUse,
    /// Already a registered pool member — a no-op (adopt is idempotent).
    AlreadyPooled,
    /// The main checkout, or a worktree nested inside it (e.g. the AIDA store
    /// worktree). Never adopted.
    Protected,
    /// Not a worktree registered against this repository.
    NotAWorktree,
    /// The path does not exist.
    Missing,
}

impl AdoptClass {
    pub fn label(self) -> &'static str {
        match self {
            AdoptClass::Adoptable => "adoptable",
            AdoptClass::Dirty => "dirty",
            AdoptClass::Unmerged => "unmerged",
            AdoptClass::InUse => "in-use",
            AdoptClass::AlreadyPooled => "already-pooled",
            AdoptClass::Protected => "protected",
            AdoptClass::NotAWorktree => "not-a-worktree",
            AdoptClass::Missing => "missing",
        }
    }

    /// True when the class carries work that has not landed on the default
    /// branch — the classes gated behind `--include-unlanded` and adopted as a
    /// permanent reservation rather than an idle tree.
    pub fn is_unlanded(self) -> bool {
        matches!(self, AdoptClass::Dirty | AdoptClass::Unmerged)
    }
}

/// What the pass decided to do with one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptAction {
    /// Dry-run: would be registered if applied.
    WouldAdopt,
    /// Registered into `pool.json`.
    Adopted,
    /// Left unregistered (the `reason` says why).
    Skipped,
}

/// One classified candidate in an adopt report.
#[derive(Debug, Clone)]
pub struct AdoptTarget {
    pub path: PathBuf,
    /// Pool name assigned (or that would be assigned); None for a skip.
    pub name: Option<String>,
    pub class: AdoptClass,
    pub action: AdoptAction,
    /// True when the entry is (or would be) registered as a durable
    /// reservation rather than an idle tree.
    pub reserved: bool,
    pub reason: String,
}

/// The outcome of an adopt pass.
#[derive(Debug, Clone)]
pub struct AdoptReport {
    pub dry_run: bool,
    pub targets: Vec<AdoptTarget>,
}

impl AdoptReport {
    pub fn adopted_count(&self) -> usize {
        self.targets
            .iter()
            .filter(|t| t.action == AdoptAction::Adopted)
            .count()
    }
    pub fn would_adopt_count(&self) -> usize {
        self.targets
            .iter()
            .filter(|t| t.action == AdoptAction::WouldAdopt)
            .count()
    }
    pub fn skipped_count(&self) -> usize {
        self.targets
            .iter()
            .filter(|t| t.action == AdoptAction::Skipped)
            .count()
    }
}

/// Knobs for one adopt pass.
#[derive(Debug, Clone)]
pub struct AdoptOptions {
    /// Preview only — no registry write. Defaults to true.
    pub dry_run: bool,
    /// Adopt dirty / unmerged trees too, as permanent reservations.
    pub include_unlanded: bool,
    /// Pool size cap; adoption stops (with a reason) at the cap so a migration
    /// can't silently blow past `[worktree_pool] max_trees`.
    pub max_trees: Option<usize>,
    /// The lease holder recorded on a reserved adoption.
    pub reserve_holder: String,
}

impl Default for AdoptOptions {
    fn default() -> Self {
        Self {
            dry_run: true,
            include_unlanded: false,
            max_trees: None,
            reserve_holder: DEFAULT_RESERVE_HOLDER.to_string(),
        }
    }
}

/// Lease holder stamped on an adopted tree that still carries unlanded work.
pub const DEFAULT_RESERVE_HOLDER: &str = "adopted";

/// Which worktrees an adopt pass considers.
#[derive(Debug, Clone)]
pub enum AdoptSelector {
    /// Every worktree registered against the repo (minus protected ones).
    All,
    /// Specific paths named by the caller.
    Paths(Vec<PathBuf>),
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The worktree paths currently held by a **live** session lease. Used to
/// refuse adopting a tree somebody is actively working in. Best-effort: the
/// probe is the same one `aida ps` uses, so the two surfaces agree.
// trace:TASK-1009 | ai:claude
#[cfg(feature = "native")]
pub fn live_lease_worktrees(project_root: &Path) -> Vec<PathBuf> {
    let live = crate::liveness::probe_live_claude_sessions();
    let now = chrono::Utc::now();
    crate::liveness::read_session_leases(project_root)
        .into_iter()
        .filter(|l| {
            matches!(
                crate::liveness::lease_state_for(l, &live, now),
                crate::liveness::LeaseState::Live
            )
        })
        .map(|l| canonical(&l.worktree_path))
        .collect()
}

/// Classify one candidate path. Pure with respect to the pool: the caller
/// supplies the already-known registered paths, the live-lease paths, and the
/// default ref, so this decision is unit-testable without a registry.
// trace:TASK-1009 | ai:claude
pub fn classify_for_adopt(
    project_root: &Path,
    path: &Path,
    registered_worktrees: &[PathBuf],
    pooled: &[PathBuf],
    live_worktrees: &[PathBuf],
    default_ref: &str,
) -> AdoptClass {
    let target = canonical(path);
    let root = canonical(project_root);

    // The main checkout and anything nested inside it (the `.aida-store`
    // worktree, a nested experiment) are never pool material.
    if target == root || target.starts_with(&root) {
        return AdoptClass::Protected;
    }
    if !target.exists() {
        return AdoptClass::Missing;
    }
    if pooled.contains(&target) {
        return AdoptClass::AlreadyPooled;
    }
    if !registered_worktrees.contains(&target) {
        return AdoptClass::NotAWorktree;
    }
    if live_worktrees.contains(&target) {
        return AdoptClass::InUse;
    }
    if git_ops::worktree_is_dirty(&target) {
        return AdoptClass::Dirty;
    }
    if !git_ops::worktree_head_is_merged(&target, default_ref) {
        return AdoptClass::Unmerged;
    }
    AdoptClass::Adoptable
}

/// The `--include-*` flag a class needs before it can be adopted, `None` when
/// it is freely adoptable, and a plain refusal string when no flag can unlock
/// it (the ambiguous cases adopt refuses rather than guesses).
fn gate(class: AdoptClass, opts: &AdoptOptions) -> Result<Option<&'static str>, &'static str> {
    match class {
        AdoptClass::Adoptable => Ok(None),
        AdoptClass::Dirty | AdoptClass::Unmerged => {
            if opts.include_unlanded {
                Ok(None)
            } else {
                Ok(Some("--include-unlanded"))
            }
        }
        AdoptClass::InUse => {
            Err("a live session holds this worktree — adopt it after that session ends")
        }
        AdoptClass::AlreadyPooled => Err("already a pool member — nothing to do"),
        AdoptClass::Protected => Err("the main checkout and worktrees inside it are never pooled"),
        AdoptClass::NotAWorktree => Err("not a worktree registered against this repository"),
        AdoptClass::Missing => Err("path does not exist"),
    }
}

/// Build the pool entry an adoption registers. An unlanded tree gets a durable
/// lease with **no `leased_at` stamp**: `acquire` skips leased entries, and the
/// TASK-1008 TTL never judges an unstamped lease stale, so the reservation is
/// permanent until a human releases it. A clean, landed tree is registered idle
/// (no owner, no lease) and is immediately reusable.
// trace:TASK-1009 trace:TASK-1008 | ai:claude
pub fn entry_for_adoption(
    name: String,
    path: PathBuf,
    class: AdoptClass,
    holder: &str,
) -> PoolEntry {
    let mut entry = PoolEntry {
        name,
        path,
        created_at: Some(chrono::Utc::now().timestamp()),
        ..Default::default()
    };
    if class.is_unlanded() {
        entry.leased = true;
        entry.lease_holder = Some(holder.to_string());
        entry.leased_at = None;
    }
    entry
}

/// Next free `aida-pool-<project>-<n>` name given the names already taken.
fn next_free_name(taken: &[String], project_root: &Path) -> String {
    let slug = project_root
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("project");
    let prefix = format!("aida-pool-{slug}");
    let mut n = 0usize;
    loop {
        let name = format!("{prefix}-{n}");
        if !taken.contains(&name) {
            return name;
        }
        n += 1;
    }
}

/// Run an adopt pass. Dry-run unless `opts.dry_run` is false; the only side
/// effect when applied is new entries in `pool.json` — no worktree is touched.
// trace:TASK-1009 | ai:claude
pub fn adopt(
    project_root: &Path,
    selector: &AdoptSelector,
    opts: &AdoptOptions,
    live_worktrees: &[PathBuf],
) -> Result<AdoptReport> {
    let default_ref = git_ops::furthest_ahead_default_ref(project_root)?;
    let registered: Vec<PathBuf> = git_ops::list_worktree_paths(project_root)
        .iter()
        .map(|p| canonical(p))
        .collect();
    let root = canonical(project_root);

    let candidates: Vec<PathBuf> = match selector {
        AdoptSelector::Paths(ps) => ps.iter().map(|p| canonical(p)).collect(),
        AdoptSelector::All => registered
            .iter()
            .filter(|p| **p != root && !p.starts_with(&root))
            .cloned()
            .collect(),
    };

    let max_trees = opts
        .max_trees
        .unwrap_or(worktree_pool::DEFAULT_MAX_TREES)
        .max(1);

    worktree_pool::with_state_lock(project_root, |pool| {
        let pooled: Vec<PathBuf> = pool.entries.iter().map(|e| canonical(&e.path)).collect();
        let mut taken: Vec<String> = pool.entries.iter().map(|e| e.name.clone()).collect();
        let mut size = pool.entries.len();
        let mut targets: Vec<AdoptTarget> = Vec::new();
        let mut seen: Vec<PathBuf> = Vec::new();

        for path in &candidates {
            // A path named twice in one invocation is only considered once.
            if seen.contains(path) {
                continue;
            }
            seen.push(path.clone());

            let class = classify_for_adopt(
                project_root,
                path,
                &registered,
                &pooled,
                live_worktrees,
                &default_ref,
            );

            let missing_flag = match gate(class, opts) {
                Ok(flag) => flag,
                Err(refusal) => {
                    targets.push(AdoptTarget {
                        path: path.clone(),
                        name: None,
                        class,
                        action: AdoptAction::Skipped,
                        reserved: false,
                        reason: format!("{} — {}", class.label(), refusal),
                    });
                    continue;
                }
            };

            if let Some(flag) = missing_flag {
                targets.push(AdoptTarget {
                    path: path.clone(),
                    name: None,
                    class,
                    action: AdoptAction::Skipped,
                    reserved: false,
                    reason: format!("{} — needs {}", class.label(), flag),
                });
                continue;
            }

            // Respect the pool cap: a migration must not silently grow the pool
            // past the configured ceiling.
            if size >= max_trees {
                targets.push(AdoptTarget {
                    path: path.clone(),
                    name: None,
                    class,
                    action: AdoptAction::Skipped,
                    reserved: false,
                    reason: format!(
                        "{} — pool is at its cap of {} (raise [worktree_pool] max_trees)",
                        class.label(),
                        max_trees
                    ),
                });
                continue;
            }

            let name = next_free_name(&taken, project_root);
            let reserved = class.is_unlanded();
            let entry = entry_for_adoption(name.clone(), path.clone(), class, &opts.reserve_holder);

            if opts.dry_run {
                targets.push(AdoptTarget {
                    path: path.clone(),
                    name: Some(name.clone()),
                    class,
                    action: AdoptAction::WouldAdopt,
                    reserved,
                    reason: if reserved {
                        format!(
                            "{} — would adopt as a reservation (never handed out until released)",
                            class.label()
                        )
                    } else {
                        format!("{} — would adopt as an idle pool tree", class.label())
                    },
                });
            } else {
                pool.entries.push(entry);
                targets.push(AdoptTarget {
                    path: path.clone(),
                    name: Some(name.clone()),
                    class,
                    action: AdoptAction::Adopted,
                    reserved,
                    reason: if reserved {
                        format!(
                            "{} — adopted as a reservation (never handed out until released)",
                            class.label()
                        )
                    } else {
                        format!("{} — adopted as an idle pool tree", class.label())
                    },
                });
            }
            // Reserve the name + cap slot for the rest of this pass, whether or
            // not it was written, so a dry-run previews the same names an apply
            // would assign.
            taken.push(name);
            size += 1;
        }

        Ok(AdoptReport {
            dry_run: opts.dry_run,
            targets,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_are_dry_run_and_refuse_unlanded() {
        let o = AdoptOptions::default();
        assert!(o.dry_run, "adopt previews by default");
        assert!(!o.include_unlanded);
        assert_eq!(o.reserve_holder, DEFAULT_RESERVE_HOLDER);
    }

    #[test]
    fn gate_maps_class_to_flag_or_refusal() {
        let none = AdoptOptions::default();
        assert_eq!(gate(AdoptClass::Adoptable, &none), Ok(None));
        assert_eq!(
            gate(AdoptClass::Dirty, &none),
            Ok(Some("--include-unlanded"))
        );
        assert_eq!(
            gate(AdoptClass::Unmerged, &none),
            Ok(Some("--include-unlanded"))
        );
        // The refused classes have no unlocking flag at all.
        for class in [
            AdoptClass::InUse,
            AdoptClass::AlreadyPooled,
            AdoptClass::Protected,
            AdoptClass::NotAWorktree,
            AdoptClass::Missing,
        ] {
            assert!(
                gate(class, &none).is_err(),
                "{} must be refused",
                class.label()
            );
        }
    }

    #[test]
    fn include_unlanded_clears_dirty_and_unmerged_only() {
        let opts = AdoptOptions {
            include_unlanded: true,
            ..Default::default()
        };
        assert_eq!(gate(AdoptClass::Dirty, &opts), Ok(None));
        assert_eq!(gate(AdoptClass::Unmerged, &opts), Ok(None));
        // Still refused — the flag unlocks unlanded work, not ambiguity.
        assert!(gate(AdoptClass::InUse, &opts).is_err());
        assert!(gate(AdoptClass::Protected, &opts).is_err());
    }

    /// An unlanded adoption is a PERMANENT reservation: leased, with no
    /// `leased_at`, so the TASK-1008 TTL can never judge it stale and hand the
    /// tree (plus its uncommitted work) to the next acquirer.
    #[test]
    fn unlanded_adoption_is_a_permanent_unstamped_reservation() {
        for class in [AdoptClass::Dirty, AdoptClass::Unmerged] {
            let e = entry_for_adoption("p-0".into(), PathBuf::from("/tmp/p0"), class, "adopted");
            assert!(e.leased, "{} must be adopted reserved", class.label());
            assert_eq!(e.lease_holder.as_deref(), Some("adopted"));
            assert_eq!(
                e.leased_at, None,
                "an adopted reservation is never TTL-expirable"
            );
            assert!(!worktree_pool::lease_expired(e.leased_at, i64::MAX / 2, 1));
            assert!(e.owner_pid.is_none());
        }
    }

    #[test]
    fn clean_adoption_is_idle_and_immediately_reusable() {
        let e = entry_for_adoption(
            "p-0".into(),
            PathBuf::from("/tmp/p0"),
            AdoptClass::Adoptable,
            "adopted",
        );
        assert!(!e.leased);
        assert_eq!(e.lease_holder, None);
        assert!(e.owner_pid.is_none());
        assert!(e.created_at.is_some());
    }

    #[test]
    fn next_free_name_skips_taken_and_namespaces_by_project() {
        let root = Path::new("/work/myrepo");
        let taken = vec![
            "aida-pool-myrepo-0".to_string(),
            "aida-pool-myrepo-1".to_string(),
        ];
        assert_eq!(next_free_name(&taken, root), "aida-pool-myrepo-2");
        assert_eq!(next_free_name(&[], root), "aida-pool-myrepo-0");
    }

    #[test]
    fn classify_protects_project_root_and_nested_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let nested = root.join(".aida-store");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            classify_for_adopt(root, root, &[], &[], &[], "main"),
            AdoptClass::Protected
        );
        assert_eq!(
            classify_for_adopt(root, &nested, &[], &[], &[], "main"),
            AdoptClass::Protected
        );
    }

    #[test]
    fn classify_reports_missing_pooled_unregistered_and_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let sib = dir.path().join("aida-sibling");
        std::fs::create_dir_all(&sib).unwrap();
        let one = [canonical(&sib)];

        // Missing path.
        assert_eq!(
            classify_for_adopt(&root, &dir.path().join("gone"), &[], &[], &[], "main"),
            AdoptClass::Missing
        );
        // Already pooled wins over everything else that follows.
        assert_eq!(
            classify_for_adopt(&root, &sib, &[], &one, &[], "main"),
            AdoptClass::AlreadyPooled
        );
        // Not registered against the repo.
        assert_eq!(
            classify_for_adopt(&root, &sib, &[], &[], &[], "main"),
            AdoptClass::NotAWorktree
        );
        // Registered but held by a live session lease.
        assert_eq!(
            classify_for_adopt(&root, &sib, &one, &[], &one, "main"),
            AdoptClass::InUse
        );
    }
}

/// Adopt integration tests over real git worktrees in a throwaway repo.
// trace:TASK-1009 | ai:claude
#[cfg(test)]
mod git_integration_tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?} failed in {}", dir.display());
    }

    /// A throwaway repo with one commit on `main`, inside its own tempdir so
    /// sibling worktrees land next to it.
    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.email", "t@t.t"]);
        git(&root, &["config", "user.name", "t"]);
        std::fs::write(root.join("README.md"), "seed").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "seed"]);
        (dir, root)
    }

    /// A pre-pool sibling worktree: created by hand, unknown to the registry.
    fn add_sibling(root: &Path, name: &str) -> PathBuf {
        let path = root.parent().unwrap().join(name);
        git_ops::add_detached_worktree(root, &path, "main").unwrap();
        path
    }

    fn apply() -> AdoptOptions {
        AdoptOptions {
            dry_run: false,
            ..Default::default()
        }
    }

    #[test]
    fn dry_run_is_the_default_and_writes_nothing() {
        let (_d, root) = init_repo();
        let sib = add_sibling(&root, "aida-pre-pool-1");

        let report = adopt(&root, &AdoptSelector::All, &AdoptOptions::default(), &[]).unwrap();
        assert!(report.dry_run);
        assert_eq!(report.would_adopt_count(), 1);
        assert_eq!(report.adopted_count(), 0);
        assert_eq!(report.targets[0].path, canonical(&sib));
        assert!(
            worktree_pool::read_state(&root).unwrap().entries.is_empty(),
            "a preview must not write the registry"
        );
        assert!(sib.exists(), "adopt never touches the worktree itself");
    }

    #[test]
    fn adopts_clean_sibling_as_idle_and_is_idempotent() {
        let (_d, root) = init_repo();
        let sib = add_sibling(&root, "aida-pre-pool-2");

        let report = adopt(&root, &AdoptSelector::All, &apply(), &[]).unwrap();
        assert_eq!(report.adopted_count(), 1);
        let pool = worktree_pool::read_state(&root).unwrap();
        assert_eq!(pool.entries.len(), 1);
        assert_eq!(pool.entries[0].path, canonical(&sib));
        assert!(!pool.entries[0].leased, "a clean landed tree adopts idle");

        // Re-running is a no-op, not a duplicate entry.
        let again = adopt(&root, &AdoptSelector::All, &apply(), &[]).unwrap();
        assert_eq!(again.adopted_count(), 0);
        assert_eq!(again.targets[0].class, AdoptClass::AlreadyPooled);
        assert_eq!(worktree_pool::read_state(&root).unwrap().entries.len(), 1);
    }

    #[test]
    fn an_adopted_clean_tree_is_reusable_by_acquire() {
        let (_d, root) = init_repo();
        let sib = add_sibling(&root, "aida-pre-pool-3");
        adopt(&root, &AdoptSelector::All, &apply(), &[]).unwrap();

        let acquired = worktree_pool::acquire(
            &root,
            &worktree_pool::AcquireOptions {
                max_trees: Some(4),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            canonical(&acquired),
            canonical(&sib),
            "the adopted warm tree should be reused, not a fresh mint"
        );
        assert_eq!(worktree_pool::read_state(&root).unwrap().entries.len(), 1);
    }

    #[test]
    fn dirty_sibling_is_skipped_without_include_unlanded() {
        let (_d, root) = init_repo();
        let sib = add_sibling(&root, "aida-pre-pool-4");
        std::fs::write(sib.join("wip.txt"), "uncommitted").unwrap();

        let report = adopt(&root, &AdoptSelector::All, &apply(), &[]).unwrap();
        assert_eq!(report.adopted_count(), 0);
        assert_eq!(report.targets[0].class, AdoptClass::Dirty);
        assert!(report.targets[0].reason.contains("--include-unlanded"));
        assert!(worktree_pool::read_state(&root).unwrap().entries.is_empty());
    }

    /// The load-bearing safety property: an adopted dirty tree is registered as
    /// a permanent reservation, so the very next `acquire` must NOT hand it out
    /// and hard-reset the uncommitted work away.
    #[test]
    fn dirty_sibling_adopts_reserved_and_acquire_never_resets_it() {
        let (_d, root) = init_repo();
        let sib = add_sibling(&root, "aida-pre-pool-5");
        std::fs::write(sib.join("wip.txt"), "uncommitted").unwrap();

        let opts = AdoptOptions {
            dry_run: false,
            include_unlanded: true,
            ..Default::default()
        };
        let report = adopt(&root, &AdoptSelector::All, &opts, &[]).unwrap();
        assert_eq!(report.adopted_count(), 1);
        assert!(report.targets[0].reserved);

        let pool = worktree_pool::read_state(&root).unwrap();
        assert!(pool.entries[0].leased);
        assert_eq!(pool.entries[0].leased_at, None);

        // Acquire with a cap of 1 and a 1-second TTL: an expirable lease would
        // be reclaimed here. The unstamped reservation must survive instead.
        let err = worktree_pool::acquire(
            &root,
            &worktree_pool::AcquireOptions {
                max_trees: Some(1),
                lease_ttl_secs: Some(1),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("pool is full"),
            "an adopted reservation must never be reclaimed; got: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(sib.join("wip.txt")).unwrap(),
            "uncommitted",
            "the uncommitted work must still be there"
        );
    }

    #[test]
    fn unmerged_sibling_is_gated_then_adopts_reserved() {
        let (_d, root) = init_repo();
        let sib = add_sibling(&root, "aida-pre-pool-6");
        git(&sib, &["checkout", "-q", "-b", "feat-x"]);
        std::fs::write(sib.join("a.txt"), "x").unwrap();
        git(&sib, &["add", "-A"]);
        git(&sib, &["commit", "-qm", "unlanded work"]);

        let report = adopt(&root, &AdoptSelector::All, &apply(), &[]).unwrap();
        assert_eq!(report.targets[0].class, AdoptClass::Unmerged);
        assert_eq!(report.adopted_count(), 0);

        let opts = AdoptOptions {
            dry_run: false,
            include_unlanded: true,
            ..Default::default()
        };
        let report = adopt(&root, &AdoptSelector::All, &opts, &[]).unwrap();
        assert_eq!(report.adopted_count(), 1);
        let pool = worktree_pool::read_state(&root).unwrap();
        assert!(pool.entries[0].leased && pool.entries[0].leased_at.is_none());
        // The branch's commit is untouched — adopt never resets.
        let log = Command::new("git")
            .current_dir(&sib)
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&log.stdout).contains("unlanded work"));
    }

    #[test]
    fn all_never_adopts_the_main_checkout() {
        let (_d, root) = init_repo();
        add_sibling(&root, "aida-pre-pool-7");
        let report = adopt(&root, &AdoptSelector::All, &apply(), &[]).unwrap();
        assert_eq!(report.targets.len(), 1, "only the sibling is a candidate");
        let pool = worktree_pool::read_state(&root).unwrap();
        assert!(pool
            .entries
            .iter()
            .all(|e| canonical(&e.path) != canonical(&root)));
    }

    #[test]
    fn named_path_refuses_the_main_checkout_and_a_non_worktree() {
        let (_d, root) = init_repo();
        let stranger = root.parent().unwrap().join("not-a-worktree");
        std::fs::create_dir_all(&stranger).unwrap();

        let report = adopt(
            &root,
            &AdoptSelector::Paths(vec![root.clone(), stranger.clone()]),
            &apply(),
            &[],
        )
        .unwrap();
        assert_eq!(report.adopted_count(), 0);
        assert_eq!(report.targets[0].class, AdoptClass::Protected);
        assert_eq!(report.targets[1].class, AdoptClass::NotAWorktree);
        assert!(worktree_pool::read_state(&root).unwrap().entries.is_empty());
    }

    #[test]
    fn live_lease_worktree_is_refused() {
        let (_d, root) = init_repo();
        let sib = add_sibling(&root, "aida-pre-pool-8");
        let report = adopt(&root, &AdoptSelector::All, &apply(), &[canonical(&sib)]).unwrap();
        assert_eq!(report.adopted_count(), 0);
        assert_eq!(report.targets[0].class, AdoptClass::InUse);
        assert!(worktree_pool::read_state(&root).unwrap().entries.is_empty());
    }

    #[test]
    fn adoption_stops_at_the_pool_cap() {
        let (_d, root) = init_repo();
        let a = add_sibling(&root, "aida-pre-pool-9a");
        let b = add_sibling(&root, "aida-pre-pool-9b");

        let opts = AdoptOptions {
            dry_run: false,
            max_trees: Some(1),
            ..Default::default()
        };
        let report = adopt(
            &root,
            &AdoptSelector::Paths(vec![a.clone(), b.clone()]),
            &opts,
            &[],
        )
        .unwrap();
        assert_eq!(report.adopted_count(), 1);
        assert_eq!(report.skipped_count(), 1);
        assert!(report.targets[1].reason.contains("cap"));
        assert_eq!(worktree_pool::read_state(&root).unwrap().entries.len(), 1);
    }
}
