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

use std::path::{Path, PathBuf};

use aida_core::{
    ArchiveFilter, CachedGitBackend, DatabaseBackend, DeferFilter, ListFilter, RequirementSummary,
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
    /// Non-functional scopes return an empty set. trace:STORY-693 | ai:claude
    pub fn scope_items(&self, scope: Scope) -> Vec<TargetItem> {
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
        summaries
            .into_iter()
            .filter(|s| scope_includes(scope, &s.status))
            .map(summary_to_item)
            .collect()
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

    #[test]
    fn store_path_value_parses_the_config_line() {
        let cfg =
            "store_type = \"worktree\"\nstore_path = \".aida-store\"\nbranch = \"aida-store\"\n";
        assert_eq!(store_path_value(cfg), Some(".aida-store".to_string()));
        assert_eq!(store_path_value("# nothing here\n"), None);
    }
}
