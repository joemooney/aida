//! In-process spec/list reads for the redesign prototype (STORY-693).
//!
//! The prototype used to shell out to a fresh ~287MB `aida` subprocess for
//! every read — the scope item lists (`aida list … --json`) and the show
//! modal (`aida show <id> --no-git`). Each call cold-started the whole AIDA
//! runtime (config load, store attach, cache open + freshness check), which
//! is the source of the show/preview lag.
//!
//! This module opens the cache-backed git backend ONCE
//! ([`aida_core::CachedGitBackend`], the same read path the CLI `list` / `show`
//! use) and serves the scope lists + the show modal from it in-process —
//! microseconds per read, no subprocess. The backend is held on the app state
//! for the lifetime of the TUI session.
//!
//! `why` is deliberately NOT moved here: its classifier lives in
//! `aida-cli/burndown.rs`, not in `aida-core`, so factoring it in-process is a
//! separate task (see the `TODO(why in-process)` in `mod.rs`).
//!
//! trace:STORY-693 | ai:claude

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use aida_core::{
    ArchiveFilter, CachedGitBackend, DatabaseBackend, DeferFilter, ListFilter, RelationshipType,
    RequirementSummary,
};

use super::state::{Scope, TargetItem};

/// A loaded spec for the show modal: structured fields + the description body,
/// read in-process. Rendered natively by the modal (replacing the captured
/// `aida show` stdout). trace:STORY-693 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSpec {
    pub id: String,
    pub title: String,
    pub req_type: String,
    pub status: String,
    pub priority: String,
    pub tags: Vec<String>,
    /// The raw description (markdown) body.
    pub description: String,
}

/// The in-process read handle: a cache-backed git backend opened once from the
/// project root. All redesign reads (scope lists + show modal) go through it,
/// so there is no per-read subprocess cold-start. trace:STORY-693 | ai:claude
pub struct SpecStore {
    backend: CachedGitBackend,
}

impl SpecStore {
    /// Open the cache-backed backend for the project rooted at `project_root`
    /// (the directory holding `.aida/config.toml`). Resolves the orphan-branch
    /// store worktree — including the sibling-worktree case where `.aida-store`
    /// only lives in the main worktree (BUG-331) — and opens (or rebuilds) the
    /// SQLite cache beside it. Returns `None` when no distributed store can be
    /// found (the prototype then falls back to empty lists rather than
    /// crashing). trace:STORY-693 | ai:claude
    pub fn open(project_root: &Path) -> Option<Self> {
        let store_path = resolve_store_path(project_root)?;
        let cache_path = CachedGitBackend::default_cache_path(&store_path);
        // Reads need neither the dispenser (id allocation is a write concern)
        // nor any forge feature — `open` builds a plain GitBackend + cache.
        let backend = CachedGitBackend::open(&store_path, &cache_path).ok()?;
        Some(SpecStore { backend })
    }

    /// The target set for a functional scope, read from the cache in-process.
    ///
    /// * [`Scope::Backlog`] → the approved + planned slice.
    /// * [`Scope::Open`]    → every non-terminal spec (not Completed/Rejected),
    ///   mirroring `aida list open`.
    ///
    /// Non-functional scopes return an empty set.
    ///
    /// `focus` is the optional EPIC focus lens (STORY-695): when `Some`, the
    /// scope's item set is narrowed to specs in that set (the focus epic + its
    /// transitive children, as computed by [`Self::descendants_of`]). When
    /// `None`, behavior is unchanged (every scope-matching spec).
    /// trace:STORY-693 trace:STORY-695 | ai:claude
    pub fn scope_items(&self, scope: Scope, focus: Option<&HashSet<String>>) -> Vec<TargetItem> {
        let filter = ListFilter {
            // Backlog/Open both default to the active view (archived + deferred
            // rows hidden), matching the CLI `list` defaults.
            archive: ArchiveFilter::NonArchivedOnly,
            defer: DeferFilter::NonDeferredOnly,
            ..Default::default()
        };
        let summaries = match self.backend.list_summaries(&filter) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let items = summaries
            .into_iter()
            .filter(|s| scope_includes(scope, &s.status))
            .map(summary_to_item);
        // Apply the EPIC focus lens (if set) — a PURE narrow over the produced
        // rows. trace:STORY-695 | ai:claude
        match focus {
            Some(set) => items.filter(|it| focus_includes(set, it)).collect(),
            None => items.collect(),
        }
    }

    /// The transitive descendant closure of `epic_id` (STORY-695): the epic
    /// itself plus every spec whose parent chain reaches it. Returned as the
    /// set of the descendants' display ids (spec_id, or agreed_id when no
    /// spec_id), so the focus lens can be applied to the [`TargetItem`] rows
    /// (which carry the same display id).
    ///
    /// Loads every requirement once (each carries its `Parent`-type
    /// relationships — a parent → child edge) and builds a parent→children
    /// index from them, then BFS-walks the index from the epic. All in-process
    /// via the open backend (no subprocess). Returns an empty set when the epic
    /// can't be resolved or the store can't be read. trace:STORY-695 | ai:claude
    pub fn descendants_of(&self, epic_id: &str) -> HashSet<String> {
        // Resolve the focus epic's UUID (it may be given as a spec_id or an
        // agreed id) so the closure walk keys off the canonical uuid edges.
        let root = match self.backend.get_requirement_by_spec_id(epic_id) {
            Ok(Some(req)) => req.id,
            _ => return HashSet::new(),
        };
        let all = match self.backend.list_requirements(true) {
            Ok(reqs) => reqs,
            Err(_) => return HashSet::new(),
        };
        let nodes: Vec<ClosureNode> = all
            .into_iter()
            .map(|req| ClosureNode {
                id: req.id,
                display_id: req
                    .spec_id
                    .clone()
                    .or_else(|| req.agreed_id.clone())
                    .unwrap_or_default(),
                children: req
                    .relationships
                    .iter()
                    .filter(|r| r.rel_type == RelationshipType::Parent)
                    .map(|r| r.target_id)
                    .collect(),
            })
            .collect();
        compute_descendants(&nodes, root)
    }

    /// Load one spec's full record (structured fields + description body) for
    /// the show modal, in-process. `id` is a spec_id (e.g. `STORY-693`) or an
    /// agreed id. Returns `None` when the spec can't be found. The cache is
    /// stale-checked first so a just-edited spec reads fresh.
    /// trace:STORY-693 | ai:claude
    pub fn load_spec(&self, id: &str) -> Option<LoadedSpec> {
        let req = self.backend.get_requirement_by_spec_id(id).ok()??;
        let mut tags: Vec<String> = req.tags.iter().cloned().collect();
        tags.sort();
        Some(LoadedSpec {
            id: req
                .spec_id
                .clone()
                .or_else(|| req.agreed_id.clone())
                .unwrap_or_else(|| id.to_string()),
            title: req.title.clone(),
            req_type: format!("{:?}", req.req_type),
            status: format!("{:?}", req.status),
            priority: format!("{:?}", req.priority),
            tags,
            description: req.description.clone(),
        })
    }
}

/// Does `scope` include a spec whose cache status string is `status`?
/// Backlog = Approved + Planned; Open = any non-terminal (not Completed /
/// Rejected). Matched case-insensitively against the cache's stored status
/// strings. trace:STORY-693 | ai:claude
fn scope_includes(scope: Scope, status: &str) -> bool {
    match scope {
        Scope::Backlog => {
            status.eq_ignore_ascii_case("approved") || status.eq_ignore_ascii_case("planned")
        }
        Scope::Open => !is_terminal_status(status),
        // Other scopes have no in-process target set yet.
        _ => false,
    }
}

/// A status string is terminal when the spec is Completed or Rejected. Mirrors
/// `aida-cli`'s `is_terminal_status_str` (STORY-86: "Done" is NOT terminal —
/// work finished on a branch, auto-bumps to Completed once merged).
/// trace:STORY-693 | ai:claude
fn is_terminal_status(status: &str) -> bool {
    let t = status.trim();
    t.eq_ignore_ascii_case("completed") || t.eq_ignore_ascii_case("rejected")
}

/// Project a cache summary into the bottom-panel target row. Priority now flows
/// through (the old `aida list --json` path dropped it; the cache carries it).
/// The body stays empty here — the show modal loads the full record on open.
/// trace:STORY-693 | ai:claude
fn summary_to_item(s: RequirementSummary) -> TargetItem {
    TargetItem {
        id: s.spec_id.or(s.agreed_id).unwrap_or_default(),
        title: s.title,
        req_type: s.req_type,
        status: s.status,
        priority: s.priority,
        body: String::new(),
    }
}

/// One spec reduced to what the descendant-closure walk needs: its uuid, its
/// display id (for the returned set), and the uuids of its direct children
/// (its `Parent`-type relationship targets). Pure input to
/// [`compute_descendants`] so the closure logic is unit-testable without a
/// store. trace:STORY-695 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureNode {
    pub id: uuid::Uuid,
    pub display_id: String,
    pub children: Vec<uuid::Uuid>,
}

/// Compute the transitive descendant closure of `root` over the parent→child
/// graph described by `nodes`: BFS from the root, following each node's
/// `children` edges, collecting every reached node's `display_id` (including
/// the root's). A pure function of its inputs — no store, no IO — so the
/// closure logic (epic + direct + grandchild in, unrelated specs out, cycle
/// safety) is unit-testable. Empty display ids are dropped. trace:STORY-695 | ai:claude
pub fn compute_descendants(nodes: &[ClosureNode], root: uuid::Uuid) -> HashSet<String> {
    let by_id: HashMap<uuid::Uuid, &ClosureNode> = nodes.iter().map(|n| (n.id, n)).collect();
    let mut result: HashSet<String> = HashSet::new();
    let mut visited: HashSet<uuid::Uuid> = HashSet::new();
    let mut queue: Vec<uuid::Uuid> = vec![root];
    while let Some(uuid) = queue.pop() {
        if !visited.insert(uuid) {
            continue; // already walked — guards against cycles.
        }
        if let Some(node) = by_id.get(&uuid) {
            if !node.display_id.is_empty() {
                result.insert(node.display_id.clone());
            }
            for &child in &node.children {
                if !visited.contains(&child) {
                    queue.push(child);
                }
            }
        }
    }
    result
}

/// Does the EPIC focus lens `set` include this target row? The lens matches on
/// the row's display id (the same id [`ClosureNode::display_id`] carries). Pure
/// so the filter narrowing is unit-testable. trace:STORY-695 | ai:claude
fn focus_includes(set: &HashSet<String>, item: &TargetItem) -> bool {
    set.contains(&item.id)
}

/// A coarse lifecycle-bucket tally of a focus set's specs, for the status-line
/// progress summary (e.g. "6 done · 1 queued · 2 approved · 3 draft"). Buckets:
///
/// * `done`     — Done or Completed (finished work).
/// * `in_progress` — InProgress or NeedsAttention (active / parked work).
/// * `approved` — Approved or Planned (groomed, ready to pick up).
/// * `draft`    — Draft (ungroomed).
///
/// Other statuses (e.g. Rejected) fall in none. Pure over the (status-string)
/// inputs so it is unit-testable. trace:STORY-695 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FocusProgress {
    pub done: usize,
    pub in_progress: usize,
    pub approved: usize,
    pub draft: usize,
}

impl FocusProgress {
    /// Tally a focus set's specs into the lifecycle buckets, given an iterator
    /// of their status strings (cache Debug form — "Draft", "InProgress", …).
    /// Matched case-insensitively. trace:STORY-695 | ai:claude
    pub fn tally<'a, I: IntoIterator<Item = &'a str>>(statuses: I) -> Self {
        let mut p = FocusProgress::default();
        for status in statuses {
            let s = status.trim();
            if s.eq_ignore_ascii_case("done") || s.eq_ignore_ascii_case("completed") {
                p.done += 1;
            } else if s.eq_ignore_ascii_case("inprogress")
                || s.eq_ignore_ascii_case("in-progress")
                || s.eq_ignore_ascii_case("needsattention")
                || s.eq_ignore_ascii_case("needs-attention")
            {
                p.in_progress += 1;
            } else if s.eq_ignore_ascii_case("approved") || s.eq_ignore_ascii_case("planned") {
                p.approved += 1;
            } else if s.eq_ignore_ascii_case("draft") {
                p.draft += 1;
            }
        }
        p
    }

    /// A concise one-line summary of the non-zero buckets, e.g.
    /// "6 done · 1 in-progress · 2 approved · 3 draft". Empty buckets are
    /// dropped; an all-empty tally renders "no specs". trace:STORY-695 | ai:claude
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.done > 0 {
            parts.push(format!("{} done", self.done));
        }
        if self.in_progress > 0 {
            parts.push(format!("{} in-progress", self.in_progress));
        }
        if self.approved > 0 {
            parts.push(format!("{} approved", self.approved));
        }
        if self.draft > 0 {
            parts.push(format!("{} draft", self.draft));
        }
        if parts.is_empty() {
            "no specs".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

/// The lifecycle-bucket progress summary for an EPIC focus set, read in-process
/// from the cache. Loads the active (non-archived, non-deferred) spec summaries
/// once, keeps those in `focus`, and tallies them via [`FocusProgress::tally`].
/// Returns the `(FocusProgress, total)` so the caller can render
/// "<EPIC>: <summary>". trace:STORY-695 | ai:claude
impl SpecStore {
    pub fn focus_progress(&self, focus: &HashSet<String>) -> (FocusProgress, usize) {
        let filter = ListFilter {
            archive: ArchiveFilter::NonArchivedOnly,
            defer: DeferFilter::NonDeferredOnly,
            ..Default::default()
        };
        let summaries = match self.backend.list_summaries(&filter) {
            Ok(s) => s,
            Err(_) => return (FocusProgress::default(), 0),
        };
        let in_focus: Vec<String> = summaries
            .into_iter()
            .filter(|s| {
                let id = s
                    .spec_id
                    .clone()
                    .or_else(|| s.agreed_id.clone())
                    .unwrap_or_default();
                focus.contains(&id)
            })
            .map(|s| s.status)
            .collect();
        let progress = FocusProgress::tally(in_focus.iter().map(|s| s.as_str()));
        (progress, in_focus.len())
    }
}

/// Resolve the orphan-branch store worktree for the project rooted at
/// `project_root`. Reads `store_path` from `.aida/config.toml`, tries the
/// local path, then falls back to the main worktree (BUG-331: a sibling
/// `git worktree` has the tracked config but the gitignored `.aida-store/`
/// only lives in the main worktree). Mirrors aida-cli's
/// `detect_distributed_store_from` + `main_worktree_store` for the read path.
/// trace:STORY-693 | ai:claude
fn resolve_store_path(project_root: &Path) -> Option<PathBuf> {
    let mut current = Some(project_root);
    while let Some(dir) = current {
        let config_path = dir.join(".aida").join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Some(rel) = store_path_value(&content) {
                let local = dir.join(&rel);
                if local.exists() && local.is_dir() {
                    return Some(local);
                }
                if let Some(main_store) = main_worktree_store(dir, &rel) {
                    return Some(main_store);
                }
            }
        }
        current = dir.parent();
    }
    None
}

/// Extract `store_path = "<value>"` from a `config.toml` body (a focused
/// line-scan rather than a full TOML parse, matching aida-cli's reader).
/// trace:STORY-693 | ai:claude
fn store_path_value(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("store_path") {
            if let Some(val) = rest.split('=').nth(1) {
                let val = val.trim().trim_matches('"').trim_matches('\'');
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// BUG-331: resolve `<main-worktree>/<rel_store>` from inside a git worktree
/// via `git rev-parse --git-common-dir` (the shared `.git`; its parent is the
/// main worktree, where the gitignored `.aida-store/` lives). This is the one
/// remaining `git` shell-out, fired ONCE at backend-open (not per read) and
/// only on the sibling-worktree fallback path. trace:STORY-693 | ai:claude
fn main_worktree_store(current: &Path, rel_store: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(current)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let common_dir = Path::new(raw);
    let common_dir = if common_dir.is_absolute() {
        common_dir.to_path_buf()
    } else {
        current.join(common_dir)
    };
    let common_dir = common_dir.canonicalize().ok()?;
    let main_worktree = common_dir.parent()?;
    let store = main_worktree.join(rel_store);
    if store.exists() && store.is_dir() {
        Some(store)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_includes_backlog_is_approved_and_planned() {
        assert!(scope_includes(Scope::Backlog, "Approved"));
        assert!(scope_includes(Scope::Backlog, "approved"));
        assert!(scope_includes(Scope::Backlog, "Planned"));
        assert!(!scope_includes(Scope::Backlog, "Draft"));
        assert!(!scope_includes(Scope::Backlog, "InProgress"));
        assert!(!scope_includes(Scope::Backlog, "Completed"));
    }

    #[test]
    fn scope_includes_open_is_every_non_terminal() {
        for s in [
            "Draft",
            "Approved",
            "Planned",
            "InProgress",
            "Done",
            "NeedsAttention",
        ] {
            assert!(scope_includes(Scope::Open, s), "{s} should be open");
        }
        assert!(!scope_includes(Scope::Open, "Completed"));
        assert!(!scope_includes(Scope::Open, "Rejected"));
        // STORY-86: Done is NOT terminal — it stays in the Open view.
        assert!(scope_includes(Scope::Open, "Done"));
    }

    #[test]
    fn non_functional_scopes_have_no_in_process_set() {
        assert!(!scope_includes(Scope::Queue, "Approved"));
        assert!(!scope_includes(Scope::Prs, "Open"));
    }

    // --- EPIC focus lens (STORY-695) -------------------------------------

    fn node(id: u128, display: &str, children: &[u128]) -> ClosureNode {
        ClosureNode {
            id: uuid::Uuid::from_u128(id),
            display_id: display.to_string(),
            children: children.iter().map(|&c| uuid::Uuid::from_u128(c)).collect(),
        }
    }

    fn target(id: &str, status: &str) -> TargetItem {
        TargetItem {
            id: id.to_string(),
            title: String::new(),
            req_type: "Task".into(),
            status: status.to_string(),
            priority: String::new(),
            body: String::new(),
        }
    }

    #[test]
    fn closure_includes_epic_direct_and_grandchild_excludes_unrelated() {
        // EPIC(1) → STORY(2) → TASK(3); plus an unrelated STORY(9).
        let nodes = vec![
            node(1, "EPIC-54", &[2]),
            node(2, "STORY-695", &[3]),
            node(3, "TASK-913", &[]),
            node(9, "STORY-999", &[]),
        ];
        let set = compute_descendants(&nodes, uuid::Uuid::from_u128(1));
        assert!(set.contains("EPIC-54"), "epic itself is in the closure");
        assert!(set.contains("STORY-695"), "direct child is in");
        assert!(set.contains("TASK-913"), "transitive grandchild is in");
        assert!(!set.contains("STORY-999"), "unrelated spec is out");
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn closure_is_cycle_safe() {
        // A pathological parent cycle 1→2→1 must terminate.
        let nodes = vec![node(1, "EPIC-1", &[2]), node(2, "STORY-2", &[1])];
        let set = compute_descendants(&nodes, uuid::Uuid::from_u128(1));
        assert!(set.contains("EPIC-1"));
        assert!(set.contains("STORY-2"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn closure_of_missing_root_is_empty() {
        let nodes = vec![node(1, "EPIC-1", &[])];
        let set = compute_descendants(&nodes, uuid::Uuid::from_u128(42));
        assert!(set.is_empty());
    }

    #[test]
    fn focus_filter_narrows_a_scope_list() {
        let mut set = HashSet::new();
        set.insert("STORY-695".to_string());
        set.insert("TASK-913".to_string());
        let items = [
            target("STORY-695", "Approved"),
            target("TASK-913", "Draft"),
            target("STORY-999", "Approved"), // not in focus
        ];
        let kept: Vec<&str> = items
            .iter()
            .filter(|it| focus_includes(&set, it))
            .map(|it| it.id.as_str())
            .collect();
        assert_eq!(kept, vec!["STORY-695", "TASK-913"]);
    }

    #[test]
    fn progress_summary_counts_buckets() {
        let statuses = [
            "Done",
            "Completed",
            "InProgress",
            "Approved",
            "Planned",
            "Draft",
            "Draft",
            "Rejected", // counted in no bucket
        ];
        let p = FocusProgress::tally(statuses.iter().copied());
        assert_eq!(p.done, 2, "Done + Completed");
        assert_eq!(p.in_progress, 1);
        assert_eq!(p.approved, 2, "Approved + Planned");
        assert_eq!(p.draft, 2);
        let s = p.summary();
        assert!(s.contains("2 done"));
        assert!(s.contains("1 in-progress"));
        assert!(s.contains("2 approved"));
        assert!(s.contains("2 draft"));
    }

    #[test]
    fn progress_summary_drops_empty_buckets_and_handles_none() {
        let p = FocusProgress::tally(["Draft", "Draft"].iter().copied());
        assert_eq!(p.summary(), "2 draft");
        let empty = FocusProgress::tally(std::iter::empty::<&str>());
        assert_eq!(empty.summary(), "no specs");
    }

    #[test]
    fn store_path_value_parses_the_config_line() {
        let cfg =
            "store_type = \"worktree\"\nstore_path = \".aida-store\"\nbranch = \"aida-store\"\n";
        assert_eq!(store_path_value(cfg), Some(".aida-store".to_string()));
        assert_eq!(store_path_value("# nothing here\n"), None);
    }
}
