//! Distributed AIDA store location — resolves the git-canonical orphan-store
//! worktree path (and, from it, the project root) from a starting directory.
//!
//! This is the CANONICAL resolver: it's what `aida-cli`'s command dispatch
//! (and every `aida mailbox send`) ultimately routes through to find the
//! store, and thus the project root (`store_path.parent()`). It lives here in
//! `aida-core`, not `aida-cli`, so non-CLI callers that can't depend on the
//! bin-only `aida-cli` crate — e.g. `aida-tui`'s mail scope — resolve to the
//! SAME root the CLI does, instead of maintaining their own ad-hoc walk that
//! can silently diverge in a nested-git-worktree layout (BUG-331 territory).
// trace:TASK-1141 | ai:claude

use std::path::{Path, PathBuf};

/// Classification of a candidate store path: either it resolves to a usable
/// store, or it's set-but-unusable with a specific reason. Lets the
/// resolution core stay pure/unit-testable while a caller-side env wrapper
/// (e.g. `aida-cli`'s `AIDA_STORE` handling) decides whether to print a
/// fall-through notice.
// trace:BUG-567 | ai:claude
pub enum StoreOverride {
    /// The path points at a valid git-canonical store (canonicalized).
    Usable(PathBuf),
    /// The path was set but unusable; carries a human-readable reason so a
    /// caller can name WHY it fell through.
    Unusable { reason: String },
}

impl StoreOverride {
    pub fn is_some(&self) -> bool {
        matches!(self, StoreOverride::Usable(_))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, StoreOverride::Unusable { .. })
    }

    pub fn expect(self, msg: &str) -> PathBuf {
        match self {
            StoreOverride::Usable(p) => p,
            StoreOverride::Unusable { reason } => panic!("{msg}: unusable ({reason})"),
        }
    }
}

/// Classify a candidate store path: `Usable` (canonicalized) when it exists
/// and holds an `objects/` subdirectory (the per-spec YAML tree GitBackend
/// manages), else `Unusable` with a specific reason. Validation is
/// deliberately strict — a typo'd or not-yet-created path is `Unusable`
/// rather than erroring, so a stale override never silently misdirects
/// writes.
// trace:SPIKE-48 trace:BUG-567 | ai:claude
pub fn classify_store_path(path: &Path) -> StoreOverride {
    if !path.is_dir() {
        return StoreOverride::Unusable {
            reason: "not a directory".to_string(),
        };
    }
    if !path.join("objects").is_dir() {
        return StoreOverride::Unusable {
            reason: "missing an `objects/` subdirectory".to_string(),
        };
    }
    StoreOverride::Usable(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
}

/// Is a git-canonical store physically attached at `<project_root>/.aida-store`?
/// True when `.aida-store/objects/` is a directory — the hallmark of an
/// attached orphan-store worktree, resolved through a symlink too.
// trace:BUG-433 | ai:claude
pub fn is_store_attached(project_root: &Path) -> bool {
    project_root.join(".aida-store").join("objects").is_dir()
}

/// BUG-331: resolve `<main-worktree>/<rel_store>` from inside a linked git
/// worktree.
///
/// `git rev-parse --git-common-dir` yields the SHARED `.git` directory (e.g.
/// `/main/.git`) regardless of which worktree we're in; its parent is the main
/// worktree. The `.aida-store/` orphan-branch worktree is created (and
/// gitignored) only there, so a linked/nested worktree (a sibling created via
/// `git worktree add`, or a nested one under e.g. `.claude/worktrees/`) must
/// look here instead of falling back to centralized mode — or, for a caller
/// like `aida-tui`'s mail scope, silently reading/writing the wrong project
/// root's `.aida/mailbox/`.
///
/// Returns `None` when not in a git repo, git is unavailable/old, or the
/// store is genuinely absent — callers then fall through to their existing
/// resolution.
// trace:BUG-331 | ai:claude
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

/// Walk up from `start` and return the store path declared by the nearest
/// ancestor's `.aida/config.toml` (its `store_path` key), resolved relative to
/// the directory holding that config — not to `start`, so calling from a
/// nested subdir still resolves correctly.
///
/// BUG-331: when the locally-resolved `store_path` doesn't exist (the
/// hallmark of a linked/nested worktree, which has the tracked
/// `.aida/config.toml` but not the gitignored `.aida-store/` worktree — that
/// one only lives in the MAIN worktree), fall back to resolving it at the main
/// worktree via `git rev-parse --git-common-dir` instead of giving up.
// trace:BUG-57 trace:BUG-331 | ai:claude
pub fn detect_distributed_store_from(start: &Path) -> Option<PathBuf> {
    let mut current = start;
    loop {
        let config_path = current.join(".aida").join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            // store_path is relative to the directory containing config.toml,
            // not to the original start dir — otherwise a nested-dir caller
            // would resolve the store against the wrong base.
            for line in content.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("store_path") {
                    if let Some(val) = rest.split('=').nth(1) {
                        let val = val.trim().trim_matches('"').trim_matches('\'');
                        let store_path = current.join(val);
                        if store_path.exists() && store_path.is_dir() {
                            return Some(store_path);
                        }
                        if let Some(main_store) = main_worktree_store(current, val) {
                            return Some(main_store);
                        }
                    }
                }
            }
        }
        match current.parent() {
            Some(p) => current = p,
            None => return None,
        }
    }
}

/// Detect the distributed store from the process's current directory,
/// honoring the `AIDA_STORE` env override when it points at a usable store.
///
/// This mirrors `aida-cli`'s `detect_distributed_store()` resolver — minus
/// the diagnostic stderr notice for a set-but-unusable `AIDA_STORE`, which
/// stays a CLI-side concern (a background/TUI caller shouldn't print to a
/// terminal it doesn't own). A caller that wants that notice should classify
/// the env var itself via [`classify_store_path`].
// trace:SPIKE-48 | ai:claude
pub fn detect_distributed_store() -> Option<PathBuf> {
    if let Some(p) = env_store_override() {
        return Some(p);
    }
    let cwd = std::env::current_dir().ok()?;
    detect_distributed_store_from(&cwd)
}

/// `AIDA_STORE`, classified and unwrapped to `Some(path)` only when usable —
/// the env-override half of [`detect_distributed_store`], factored out so a
/// `start`-parameterized caller (like [`resolve_project_root_from`]) can
/// combine it with an explicit walk-up dir instead of the process cwd.
fn env_store_override() -> Option<PathBuf> {
    let raw = std::env::var("AIDA_STORE").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    match classify_store_path(Path::new(raw)) {
        StoreOverride::Usable(p) => Some(p),
        StoreOverride::Unusable { .. } => None,
    }
}

/// Resolve the AIDA project root the same way `aida-cli`'s
/// `handle_mailbox_command` does: find the attached distributed store (see
/// [`detect_distributed_store`]), then take its parent — the store worktree
/// lives at `<project_root>/.aida-store`, so its parent IS the project root
/// whose `.aida/mailbox/` (and everything else project-root-relative) the CLI
/// reads and writes.
///
/// `None` when no distributed store is resolvable (not an AIDA project here,
/// or a distributed project whose store isn't attached in this working copy)
/// — a caller with a legacy/non-distributed fallback should use it here.
// trace:TASK-1141 | ai:claude
pub fn resolve_project_root() -> Option<PathBuf> {
    detect_distributed_store()?
        .parent()
        .map(|p| p.to_path_buf())
}

/// Same as [`resolve_project_root`], but parameterized on the starting
/// directory instead of the process cwd — lets a caller resolve for a
/// specific `cwd` it already captured (testability) without a global env
/// mutation dance. `AIDA_STORE` is still honored first (it isn't
/// cwd-dependent), then the walk starts from `start`.
// trace:TASK-1141 | ai:claude
pub fn resolve_project_root_from(start: &Path) -> Option<PathBuf> {
    if let Some(p) = env_store_override() {
        return p.parent().map(|p| p.to_path_buf());
    }
    detect_distributed_store_from(start)?
        .parent()
        .map(|p| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn git(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git on PATH");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn classify_store_path_accepts_dir_with_objects() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().join("sandbox");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        let resolved = classify_store_path(&store).expect("should accept store");
        assert_eq!(resolved, store.canonicalize().unwrap());
    }

    #[test]
    fn classify_store_path_rejects_non_store_paths() {
        let tmp = TempDir::new().unwrap();

        assert!(classify_store_path(&tmp.path().join("nope")).is_none());

        let bare = tmp.path().join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        assert!(classify_store_path(&bare).is_none());

        let file = tmp.path().join("afile");
        std::fs::write(&file, "x").unwrap();
        assert!(classify_store_path(&file).is_none());
    }

    #[test]
    fn detect_distributed_store_walks_up_from_subdir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[deployment]\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".aida-store")).unwrap();

        let nested = root.join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();

        let resolved = detect_distributed_store_from(&nested).expect("should walk up");
        assert_eq!(resolved, root.join(".aida-store"));
    }

    // BUG-331 / TASK-1141: from a linked (sibling or nested) git worktree,
    // detection resolves the canonical store at the MAIN worktree (via
    // git-common-dir) instead of failing. The linked worktree has the
    // tracked `.aida/config.toml` but no local `.aida-store/`. This is the
    // exact case the TUI mail path must now agree with the CLI on.
    #[test]
    fn detect_distributed_store_resolves_from_linked_worktree() {
        let tmp = TempDir::new().unwrap();
        let main_root = tmp.path().join("main");
        std::fs::create_dir_all(&main_root).unwrap();
        git(&main_root, &["init", "-q", "-b", "main"]);
        git(&main_root, &["config", "user.email", "t@t.t"]);
        git(&main_root, &["config", "user.name", "t"]);
        std::fs::create_dir_all(main_root.join(".aida")).unwrap();
        std::fs::write(
            main_root.join(".aida/config.toml"),
            "[deployment]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        git(&main_root, &["add", "."]);
        git(&main_root, &["commit", "-q", "-m", "init"]);
        // The attached store worktree lives ONLY in the main worktree.
        std::fs::create_dir_all(main_root.join(".aida-store")).unwrap();

        let linked = tmp.path().join("linked");
        git(
            &main_root,
            &["worktree", "add", "--detach", linked.to_str().unwrap()],
        );

        // The linked worktree has NO `.aida-store/` of its own.
        assert!(!linked.join(".aida-store").exists());

        let resolved =
            detect_distributed_store_from(&linked).expect("should resolve via the main worktree");
        assert_eq!(
            resolved.canonicalize().unwrap(),
            main_root.join(".aida-store").canonicalize().unwrap(),
            "must resolve to the MAIN worktree's store, not fail"
        );

        // And the project root derived from it (store_path.parent()) is the
        // MAIN worktree's root, NOT the linked worktree's root — the exact
        // divergence TASK-1141 closes for the TUI mail path.
        assert_eq!(
            resolved.parent().unwrap().canonicalize().unwrap(),
            main_root.canonicalize().unwrap(),
        );
    }

    #[test]
    fn detect_distributed_store_returns_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(detect_distributed_store_from(&nested).is_none());
    }

    #[test]
    fn is_store_attached_detects_objects_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        assert!(!is_store_attached(root)); // bare project — no store
        std::fs::create_dir_all(root.join(".aida-store")).unwrap();
        assert!(!is_store_attached(root)); // .aida-store but not git-canonical
        std::fs::create_dir_all(root.join(".aida-store/objects")).unwrap();
        assert!(is_store_attached(root)); // objects/ present -> attached
    }
}
