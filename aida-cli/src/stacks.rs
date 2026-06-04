//! Stacked-branch graph (STORY-248).
//!
//! `aida queue work --stack` / `--base <BRANCH>` records the new
//! session's branch + parent-branch + parent-branch SHA in this file:
//!
//!   `.aida/stacks.json`
//!
//! When `aida pull` lands new commits on `main` and one of the recorded
//! parents was just merged (squashed + deleted on origin), the
//! `cascade_rebase_stacked_branches` handler walks the chains bottom-up
//! and runs `git rebase --onto origin/main <parent_sha> <branch>` in each
//! affected worktree. Recording `parent_branch_sha` at fork time is what
//! makes that rebase safe under the project's squash-merge convention —
//! a plain `git rebase origin/main` would re-apply the parent's
//! pre-squash commits and either conflict or duplicate history.
//!
//! Why JSON over TOML: the natural shape is a flat map keyed by branch
//! name, and `serde_json::to_string_pretty` round-trips that cleanly. The
//! file is gitignored under the existing `.aida/*` deny-by-default rule.
//!
//! trace:STORY-248 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One entry in `.aida/stacks.json`: a stacked branch + what it was
/// forked from + the SHA at fork time (needed for `--onto` rebase).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackEntry {
    /// The stacked branch (the one this session was started on).
    pub branch: String,
    /// The branch this one was forked from. May itself be a stack entry
    /// (intermediate link in a chain) or a "base" branch like `main`
    /// (chain root). The cascade looks `parent_branch` up against the
    /// set of branches just deleted on origin to decide whether this
    /// entry needs rebasing.
    pub parent_branch: String,
    /// HEAD SHA of `parent_branch` at fork time. Used as the second
    /// argument of `git rebase --onto origin/main <sha> <branch>` so the
    /// parent's (now-squashed) commits are skipped during the rebase.
    pub parent_branch_sha: String,
    /// The SPEC-ID this session was working, when known. Best-effort —
    /// recorded for human-readable rendering, never load-bearing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<String>,
    /// When the entry was created. Used for ordering in `stack list`
    /// when no chain relationship pins the order.
    pub created_at: DateTime<Utc>,
}

/// On-disk shape of `.aida/stacks.json`. A `BTreeMap` keyed by branch
/// name gives us deterministic JSON ordering, so two saves with the
/// same content are byte-identical (helps tests and human diffs).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StackGraph {
    #[serde(default)]
    pub entries: BTreeMap<String, StackEntry>,
}

impl StackGraph {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    // TASK-552: kept as the direct lookup API for future stack subcommands;
    // current cascade/list code mainly walks the whole graph.
    #[allow(dead_code)]
    pub fn get(&self, branch: &str) -> Option<&StackEntry> {
        self.entries.get(branch)
    }
}

/// Path on disk for the graph file: `<project-root>/.aida/stacks.json`.
pub fn path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("stacks.json")
}

/// Load the graph, or return an empty graph when the file is missing /
/// unparseable. We deliberately swallow parse errors so a corrupted
/// `.aida/stacks.json` cannot block `aida pull` or `aida queue work` —
/// the cascade will then just skip and the next `--stack` add rebuilds.
pub fn load(project_root: &Path) -> StackGraph {
    let p = path(project_root);
    if !p.exists() {
        return StackGraph::default();
    }
    match aida_core::read_atomic(&p) {
        Ok(body) => serde_json::from_str(&body).unwrap_or_default(),
        Err(_) => StackGraph::default(),
    }
}

/// Atomically write the graph. Uses `aida_core::write_atomic` because
/// the file is read by `aida pull` while `aida queue work` may be
/// writing it in another shell.
pub fn save(project_root: &Path, graph: &StackGraph) -> Result<()> {
    let p = path(project_root);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(graph).context("serialize stack graph")?;
    aida_core::write_atomic(&p, body).with_context(|| format!("write {}", p.display()))?;
    Ok(())
}

/// Insert or replace the entry for `entry.branch`.
pub fn add(graph: &mut StackGraph, entry: StackEntry) {
    graph.entries.insert(entry.branch.clone(), entry);
}

/// Remove the entry for `branch`. Idempotent — a missing entry is fine.
pub fn remove(graph: &mut StackGraph, branch: &str) -> Option<StackEntry> {
    graph.entries.remove(branch)
}

/// Rewrite every entry whose `parent_branch == old_parent` to point at
/// `new_parent` (with `new_parent_sha`). Used by the cascade when a
/// chain's bottom rebases onto main — its dependents now hang off the
/// (rebased) branch, but recorded against `main`.
pub fn repoint(graph: &mut StackGraph, old_parent: &str, new_parent: &str, new_parent_sha: &str) {
    for entry in graph.entries.values_mut() {
        if entry.parent_branch == old_parent {
            entry.parent_branch = new_parent.to_string();
            entry.parent_branch_sha = new_parent_sha.to_string();
        }
    }
}

/// Update only the recorded SHA for entries whose parent is `parent` —
/// used after rebasing an intermediate stacked branch (its branch name
/// is unchanged, but its HEAD moved, so dependents' recorded SHA is
/// stale). Caller must pass the new HEAD SHA of `parent`.
pub fn update_parent_sha(graph: &mut StackGraph, parent: &str, new_sha: &str) {
    for entry in graph.entries.values_mut() {
        if entry.parent_branch == parent {
            entry.parent_branch_sha = new_sha.to_string();
        }
    }
}

/// Branch names whose recorded `parent_branch` is `parent` — the stacked
/// children that would be orphaned (and whose PRs GitHub auto-closes) if
/// `parent` were deleted. Sorted for stable output. Used by `aida pr ship`'s
/// `--delete-branch` guard. trace:BUG-434 | ai:claude
pub fn children_of<'a>(graph: &'a StackGraph, parent: &str) -> Vec<&'a str> {
    let mut out: Vec<&str> = graph
        .entries
        .values()
        .filter(|e| e.parent_branch == parent)
        .map(|e| e.branch.as_str())
        .collect();
    out.sort_unstable();
    out
}

/// Derive ordered chains from the graph. Each chain is bottom-of-stack
/// first — i.e. the entry directly forked from a non-stack-entry parent
/// (typically `main`) comes first.
///
/// When the same parent has N stacked children, we emit N separate
/// chains rather than collapsing into a DAG view. Same when an
/// intermediate entry has N children — the prefix repeats but each leaf
/// gets its own chain. This keeps the output a flat list of root-to-leaf
/// paths, which is what `aida stack list` / the cascade walk both want.
pub fn chains(graph: &StackGraph) -> Vec<Vec<&StackEntry>> {
    if graph.entries.is_empty() {
        return Vec::new();
    }
    // adjacency: parent_branch -> sorted-by-branch children
    let mut children: HashMap<&str, Vec<&StackEntry>> = HashMap::new();
    for entry in graph.entries.values() {
        children
            .entry(entry.parent_branch.as_str())
            .or_default()
            .push(entry);
    }
    for kids in children.values_mut() {
        kids.sort_by(|a, b| a.branch.cmp(&b.branch));
    }

    // Roots = parent_branch values NOT themselves stack entries.
    let mut roots: Vec<&str> = children
        .keys()
        .copied()
        .filter(|p| !graph.entries.contains_key(*p))
        .collect();
    roots.sort();

    let mut out: Vec<Vec<&StackEntry>> = Vec::new();
    for root in roots {
        let starters = children.get(root).cloned().unwrap_or_default();
        for start in starters {
            walk_paths(&mut out, Vec::new(), start, &children);
        }
    }
    out
}

fn walk_paths<'a>(
    out: &mut Vec<Vec<&'a StackEntry>>,
    prefix: Vec<&'a StackEntry>,
    node: &'a StackEntry,
    children: &HashMap<&str, Vec<&'a StackEntry>>,
) {
    let mut path = prefix;
    path.push(node);
    let kids = children
        .get(node.branch.as_str())
        .cloned()
        .unwrap_or_default();
    if kids.is_empty() {
        out.push(path);
        return;
    }
    for kid in kids {
        walk_paths(out, path.clone(), kid, children);
    }
}

/// Entries whose `parent_branch` is in `merged_branches` — i.e. whose
/// base just merged into main and needs rebasing onto main. Returned in
/// bottom-up order (chain-root-adjacent first) so the cascade can rebase
/// dependencies before dependents.
// TASK-552: retained as the pure selection helper for stacked-PR cascade
// callers; current implementation computes equivalent state inline.
#[allow(dead_code)]
pub fn parents_merged_in(graph: &StackGraph, merged_branches: &HashSet<String>) -> Vec<StackEntry> {
    let mut hits: Vec<StackEntry> = graph
        .entries
        .values()
        .filter(|e| merged_branches.contains(&e.parent_branch))
        .cloned()
        .collect();
    // Sort by created_at ascending — older first means an entry's parent
    // is processed (if it's also a hit) before the entry itself, which
    // matches the bottom-up cascade order.
    hits.sort_by_key(|e| e.created_at);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(branch: &str, parent: &str, parent_sha: &str) -> StackEntry {
        StackEntry {
            branch: branch.to_string(),
            parent_branch: parent.to_string(),
            parent_branch_sha: parent_sha.to_string(),
            spec_id: None,
            created_at: Utc::now(),
        }
    }

    /// BUG-434: `children_of` returns exactly the branches forked from a
    /// given parent, sorted, and ignores unrelated entries.
    #[test]
    fn children_of_returns_direct_children_sorted() {
        let mut g = StackGraph::default();
        add(&mut g, entry("task-c", "base", "sha"));
        add(&mut g, entry("task-a", "base", "sha"));
        add(&mut g, entry("grandchild", "task-a", "sha")); // not a child of base
        add(&mut g, entry("other", "main", "sha"));

        assert_eq!(children_of(&g, "base"), vec!["task-a", "task-c"]);
        assert_eq!(children_of(&g, "task-a"), vec!["grandchild"]);
        assert!(children_of(&g, "task-c").is_empty());
        assert!(children_of(&g, "nonexistent").is_empty());
    }

    #[test]
    fn stacks_add_then_load_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut g = StackGraph::default();
        add(
            &mut g,
            StackEntry {
                branch: "task-y".into(),
                parent_branch: "task-x".into(),
                parent_branch_sha: "abc123".into(),
                spec_id: Some("TASK-Y".into()),
                created_at: Utc::now(),
            },
        );
        save(root, &g).unwrap();
        let loaded = load(root);
        assert_eq!(loaded.entries.len(), 1);
        let e = loaded.entries.get("task-y").unwrap();
        assert_eq!(e.parent_branch, "task-x");
        assert_eq!(e.parent_branch_sha, "abc123");
        assert_eq!(e.spec_id.as_deref(), Some("TASK-Y"));
    }

    #[test]
    fn stacks_load_missing_file_is_empty_graph() {
        let tmp = tempfile::tempdir().unwrap();
        let g = load(tmp.path());
        assert!(g.is_empty());
    }

    #[test]
    fn stacks_load_corrupt_file_is_empty_graph() {
        // Corrupted JSON must NOT crash `aida pull` / `aida queue work`.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(path(tmp.path()), b"not valid {{{").unwrap();
        let g = load(tmp.path());
        assert!(g.is_empty());
    }

    #[test]
    fn stacks_chains_single_entry() {
        let mut g = StackGraph::default();
        add(&mut g, entry("task-y", "main", "deadbeef"));
        let cs = chains(&g);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].len(), 1);
        assert_eq!(cs[0][0].branch, "task-y");
    }

    #[test]
    fn stacks_chains_multi_link() {
        let mut g = StackGraph::default();
        add(&mut g, entry("task-y", "task-x", "shaX"));
        add(&mut g, entry("task-z", "task-y", "shaY"));
        // task-x is the root (not itself a stack entry).
        let cs = chains(&g);
        assert_eq!(cs.len(), 1);
        let chain: Vec<&str> = cs[0].iter().map(|e| e.branch.as_str()).collect();
        assert_eq!(chain, vec!["task-y", "task-z"]);
    }

    #[test]
    fn stacks_chains_disjoint_returns_two_chains() {
        let mut g = StackGraph::default();
        add(&mut g, entry("a", "main", "1"));
        add(&mut g, entry("b", "feature-zoo", "2"));
        let cs = chains(&g);
        assert_eq!(cs.len(), 2);
    }

    #[test]
    fn stacks_remove_keeps_dependents() {
        let mut g = StackGraph::default();
        add(&mut g, entry("task-y", "task-x", "1"));
        add(&mut g, entry("task-z", "task-y", "2"));
        remove(&mut g, "task-y");
        assert!(g.get("task-y").is_none());
        // task-z still references task-y as parent — that's intentional;
        // the caller is responsible for repoint() if it wants to fix that.
        assert_eq!(g.get("task-z").unwrap().parent_branch, "task-y");
    }

    #[test]
    fn update_parent_sha_touches_only_matching_parent() {
        let mut g = StackGraph::default();
        add(&mut g, entry("task-y", "task-x", "old-x"));
        add(&mut g, entry("task-z", "task-y", "old-y"));
        update_parent_sha(&mut g, "task-y", "new-y");
        assert_eq!(g.get("task-z").unwrap().parent_branch_sha, "new-y");
        // task-y's own parent_branch_sha (which points at task-x) is unchanged.
        assert_eq!(g.get("task-y").unwrap().parent_branch_sha, "old-x");
    }

    #[test]
    fn stacks_repoint_rewrites_chain() {
        let mut g = StackGraph::default();
        add(&mut g, entry("task-y", "task-x", "1"));
        add(&mut g, entry("task-z", "task-y", "2"));
        // task-x just merged → task-y rebased onto main; repoint
        // anything still pointing at task-x.
        repoint(&mut g, "task-x", "main", "newsha");
        assert_eq!(g.get("task-y").unwrap().parent_branch, "main");
        assert_eq!(g.get("task-y").unwrap().parent_branch_sha, "newsha");
        // task-z's parent is task-y, untouched.
        assert_eq!(g.get("task-z").unwrap().parent_branch, "task-y");
    }

    #[test]
    fn parents_merged_in_filters_and_orders() {
        let mut g = StackGraph::default();
        let mut y = entry("task-y", "task-x", "1");
        y.created_at = Utc::now() - chrono::Duration::seconds(10);
        let mut z = entry("task-z", "task-y", "2");
        z.created_at = Utc::now();
        // task-w is NOT affected (its parent isn't in the merged set).
        let mut w = entry("task-w", "feature-zoo", "3");
        w.created_at = Utc::now();
        add(&mut g, y);
        add(&mut g, z);
        add(&mut g, w);
        let mut merged = HashSet::new();
        merged.insert("task-x".to_string());
        merged.insert("task-y".to_string());
        let hits = parents_merged_in(&g, &merged);
        let names: Vec<&str> = hits.iter().map(|e| e.branch.as_str()).collect();
        assert_eq!(names, vec!["task-y", "task-z"]); // bottom-up by created_at
    }

    #[test]
    fn save_writes_deterministic_json() {
        // Two saves of the same graph must be byte-identical (BTreeMap
        // ordering + serde_json::to_string_pretty).
        let tmp = tempfile::tempdir().unwrap();
        let mut g = StackGraph::default();
        add(&mut g, entry("b", "main", "1"));
        add(&mut g, entry("a", "main", "2"));
        save(tmp.path(), &g).unwrap();
        let a = std::fs::read_to_string(path(tmp.path())).unwrap();
        save(tmp.path(), &g).unwrap();
        let b = std::fs::read_to_string(path(tmp.path())).unwrap();
        assert_eq!(a, b);
        // And the order is sorted by key.
        assert!(a.find("\"a\"").unwrap() < a.find("\"b\"").unwrap());
    }
}
