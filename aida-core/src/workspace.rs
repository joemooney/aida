// trace:ARCH-distributed-workspace | ai:claude
//! Multi-repo workspace support for distributed AIDA.
//!
//! A workspace groups multiple code repos that share a single AIDA store.
//! Each code repo has a `.aida/config.toml` pointing to the shared store.
//!
//! Layout:
//! ```text
//! workspace/
//!   pacgate/              ← code repo 1
//!   pacinet/              ← code repo 2
//!   aida-store/           ← shared requirements store
//!   .aida-workspace       ← workspace config
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Workspace configuration stored in `.aida-workspace`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    /// Human-readable workspace name
    pub name: String,
    /// Path to the shared AIDA store (relative to workspace root)
    #[serde(default = "default_store_path")]
    pub store_path: String,
    /// Code repos in this workspace
    #[serde(default)]
    pub repos: Vec<WorkspaceRepo>,
}

/// A code repo entry in the workspace manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceRepo {
    /// Directory name (relative to workspace root)
    pub path: String,
    /// Optional display name
    #[serde(default)]
    pub name: String,
}

impl WorkspaceRepo {
    /// The repo's canonical slug — the `origin.repo` join key (ADR-12): the
    /// display name when set, else the directory path.
    // trace:STORY-634 | ai:claude
    pub fn slug(&self) -> &str {
        if self.name.is_empty() {
            &self.path
        } else {
            &self.name
        }
    }
}

fn default_store_path() -> String {
    "aida-store".into()
}

impl Default for WorkspaceManifest {
    fn default() -> Self {
        Self {
            name: String::new(),
            store_path: default_store_path(),
            repos: Vec::new(),
        }
    }
}

const WORKSPACE_FILE: &str = ".aida-workspace";

impl WorkspaceManifest {
    /// Discover a workspace by walking up from a directory.
    pub fn discover(from: &Path) -> Option<(PathBuf, Self)> {
        let mut current = from.to_path_buf();
        loop {
            let candidate = current.join(WORKSPACE_FILE);
            if candidate.exists() {
                // Reader can race a concurrent `WorkspaceManifest::save`
                // mid-`write_atomic`; on Windows that surfaces as a
                // transient PermissionDenied/NotFound from `CreateFile`.
                // Retry through `read_atomic`. trace:TASK-346 | ai:claude
                if let Ok(content) = crate::read_atomic(&candidate) {
                    if let Ok(manifest) = toml::from_str::<WorkspaceManifest>(&content) {
                        return Some((current, manifest));
                    }
                }
            }
            if !current.pop() {
                return None;
            }
        }
    }

    /// Get the absolute store path.
    pub fn store_path(&self, workspace_root: &Path) -> PathBuf {
        workspace_root.join(&self.store_path)
    }

    /// Save the workspace manifest.
    pub fn save(&self, workspace_root: &Path) -> Result<()> {
        let path = workspace_root.join(WORKSPACE_FILE);
        let content = toml::to_string_pretty(self)?;
        // Atomic write — uniform with the concurrent-writer paths. trace:TASK-331 | ai:claude
        crate::write_atomic(&path, content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    /// All repo slugs in manifest order — the valid `origin.repo` vocabulary
    /// (ADR-12) a spec's origin must resolve against.
    // trace:STORY-634 | ai:claude
    pub fn repo_slugs(&self) -> Vec<&str> {
        self.repos.iter().map(|r| r.slug()).collect()
    }

    /// Resolve the slug of the workspace repo containing `dir`, if any.
    /// Canonicalizes both sides so a symlinked/relative path still matches.
    /// This is the stamp source for repo-qualified linkage (ADR-12 D5): the
    /// same slug vocabulary as `repo_slugs`.
    // trace:STORY-634 | ai:claude
    pub fn repo_slug_containing(&self, workspace_root: &Path, dir: &Path) -> Option<String> {
        let here = dir.canonicalize().ok()?;
        self.repos.iter().find_map(|r| {
            let repo_abs = workspace_root.join(&r.path).canonicalize().ok()?;
            here.starts_with(&repo_abs).then(|| r.slug().to_string())
        })
    }

    /// Add a repo to the workspace.
    pub fn add_repo(&mut self, path: &str, name: &str) {
        if !self.repos.iter().any(|r| r.path == path) {
            self.repos.push(WorkspaceRepo {
                path: path.to_string(),
                name: name.to_string(),
            });
        }
    }
}

/// Initialize a multi-repo workspace.
///
/// Creates:
/// - `.aida-workspace` manifest
/// - `aida-store/` directory with git init
/// - `.aida/config.toml` in each discovered repo
pub fn init_workspace(
    workspace_root: &Path,
    name: &str,
    store_path: Option<&str>,
    registry_remote: Option<&str>,
) -> Result<WorkspaceManifest> {
    use crate::db::DatabaseBackend;
    use crate::git_ops;

    let store_dir = store_path.unwrap_or("aida-store");
    let store_full = workspace_root.join(store_dir);

    // Create workspace manifest
    let mut manifest = WorkspaceManifest {
        name: name.to_string(),
        store_path: store_dir.to_string(),
        repos: Vec::new(),
    };

    // Discover code repos (directories with .git)
    for entry in std::fs::read_dir(workspace_root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir()
            && path.join(".git").exists()
            && path.file_name().map(|n| n != "aida-store").unwrap_or(false)
        {
            let dir_name = path.file_name().unwrap().to_string_lossy().to_string();
            manifest.add_repo(&dir_name, &dir_name);
        }
    }

    // Create the store
    if !store_full.exists() {
        std::fs::create_dir_all(&store_full)?;
    }

    if !git_ops::is_git_repo(&store_full) {
        git_ops::init(&store_full)?;
        let git_name = git_ops::git_config_get("user.name").unwrap_or_else(|_| "AIDA".to_string());
        let git_email =
            git_ops::git_config_get("user.email").unwrap_or_else(|_| "aida@localhost".to_string());
        git_ops::configure_user(&store_full, &git_name, &git_email)?;
    }

    // Add remote if provided
    if let Some(remote) = registry_remote {
        let has_remote = std::process::Command::new("git")
            .current_dir(&store_full)
            .args(["remote", "get-url", "origin"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !has_remote {
            std::process::Command::new("git")
                .current_dir(&store_full)
                .args(["remote", "add", "origin", remote])
                .output()?;
        }
    }

    // Initialize git backend in the store
    let backend = crate::db::GitBackend::new(&store_full)?;
    let store = crate::models::RequirementsStore::new();
    backend.save(&store)?;

    // Initial commit
    git_ops::add(&store_full, &["metadata.yaml"])?;
    std::fs::create_dir_all(store_full.join("objects"))?;
    std::fs::write(store_full.join("objects/.gitkeep"), "")?;
    git_ops::add(&store_full, &["objects/.gitkeep"])?;

    // The staging-file ignores come from `fs_atomic`, which owns the staging
    // name, so a lock-free `git add -A .` (db sync, auto-push) can never stage
    // one. They live in the STORE's tracked `.gitignore`, never the project's
    // `info/exclude`: a store attached as a linked worktree shares the
    // project's exclude file, so an ignore written there would hide the user's
    // own files too. Existing stores are repaired by TASK-1547.
    // trace:BUG-1677 | ai:claude
    let gitignore = format!(
        "# Node-local state\n.aida/\n*.lock\n{}",
        crate::fs_atomic::store_staging_ignore_block()
    );
    std::fs::write(store_full.join(".gitignore"), &gitignore)?;
    git_ops::add(&store_full, &[".gitignore"])?;
    git_ops::commit(&store_full, "chore: initialize AIDA workspace store")?;

    // Create .aida/config.toml in each repo
    for repo in &manifest.repos {
        let repo_path = workspace_root.join(&repo.path);
        let aida_dir = repo_path.join(".aida");
        std::fs::create_dir_all(&aida_dir)?;

        let relative_store = format!("../{}", store_dir);
        // trace:BUG-1649 | ai:claude
        let config = workspace_repo_config(&relative_store, name);
        std::fs::write(aida_dir.join("config.toml"), config)?;
    }

    // Save workspace manifest
    manifest.save(workspace_root)?;

    Ok(manifest)
}

/// Body of a member repo's `.aida/config.toml` pointing at the shared
/// workspace store. Values are quoted by [`crate::toml_quote::toml_string`]
/// so a Windows path or a name containing `"` still yields valid TOML.
// trace:BUG-1649 | ai:claude
fn workspace_repo_config(relative_store: &str, workspace_name: &str) -> String {
    format!(
        "# AIDA workspace configuration\n\
         [deployment]\n\
         mode = \"distributed\"\n\
         store_path = {}\n\
         store_type = \"sibling\"\n\
         workspace = {}\n",
        crate::toml_quote::toml_string(relative_store),
        crate::toml_quote::toml_string(workspace_name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BUG-1677: the staging-file ignores a freshly created store carries must
    /// actually hide every `write_atomic` staging name the store can produce,
    /// and must hide nothing else. Proven with `git check-ignore` against the
    /// real `init_workspace` output rather than by comparing strings, so a new
    /// store subtree that starts writing atomically fails here.
    // trace:BUG-1677 | ai:claude
    #[test]
    fn bug_1677_store_gitignore_hides_every_staging_name_and_nothing_else() {
        let ws = tempfile::tempdir().unwrap();
        init_workspace(ws.path(), "staging-ignore", None, None).unwrap();
        let store = ws.path().join("aida-store");

        let ignored = |rel: &str| -> bool {
            let path = store.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"x").unwrap();
            std::process::Command::new("git")
                .current_dir(&store)
                .args(["check-ignore", "-q", rel])
                .status()
                .unwrap()
                .success()
        };

        // Every store path an atomic writer targets, named exactly the way
        // `write_atomic` stages it (`with_extension("tmp.<pid>.<seq>")`, which
        // REPLACES the target's extension).
        for staged in [
            "objects/BUG/001/BUG-1677.tmp.4242.0",
            "registry/nodes.tmp.4242.1",
            "registry/agreed_counters.tmp.4242.2",
            "registry/queues/someone.tmp.4242.3",
            "schedule/watchdog.tmp.4242.4",
            "mailbox/0f3a-4b1c.tmp.4242.5",
            "oplog.tmp.4242.6",
        ] {
            assert!(ignored(staged), "staging file must be ignored: {staged}");
        }

        // Real store content is still tracked, and the rule is narrow enough
        // that a legitimately-named file outside those subtrees stays visible —
        // a bare `*.tmp.*` would swallow it.
        for visible in [
            "objects/BUG/001/BUG-1677.yaml",
            "registry/nodes.toml",
            "notes.tmp.txt",
            "docs/handoff.tmp.md",
        ] {
            assert!(!ignored(visible), "must stay visible: {visible}");
        }

        // The written file and the shared constant agree.
        let gitignore = std::fs::read_to_string(store.join(".gitignore")).unwrap();
        for pattern in crate::fs_atomic::STORE_STAGING_IGNORE_PATTERNS {
            assert!(
                gitignore.lines().any(|l| l.trim() == *pattern),
                "missing {pattern} in:\n{gitignore}"
            );
        }
        assert!(
            !gitignore.lines().any(|l| l.trim() == "*.tmp.*"),
            "the staging ignore must stay store-scoped:\n{gitignore}"
        );
    }

    #[test]
    fn test_workspace_manifest_serde() {
        let mut manifest = WorkspaceManifest {
            name: "test-workspace".into(),
            ..Default::default()
        };
        manifest.add_repo("pacgate", "PacGate");
        manifest.add_repo("pacinet", "PacInet");

        let toml_str = toml::to_string_pretty(&manifest).unwrap();
        let back: WorkspaceManifest = toml::from_str(&toml_str).unwrap();
        assert_eq!(back.name, "test-workspace");
        assert_eq!(back.repos.len(), 2);
    }

    #[test]
    fn test_workspace_discover() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = WorkspaceManifest {
            name: "test".into(),
            store_path: "aida-store".into(),
            repos: Vec::new(),
        };
        manifest.save(dir.path()).unwrap();

        // Discover from workspace root
        let (root, found) = WorkspaceManifest::discover(dir.path()).unwrap();
        assert_eq!(root, dir.path());
        assert_eq!(found.name, "test");

        // Discover from subdirectory
        let sub = dir.path().join("subdir");
        std::fs::create_dir_all(&sub).unwrap();
        let (root2, _) = WorkspaceManifest::discover(&sub).unwrap();
        assert_eq!(root2, dir.path());
    }

    /// STORY-634: the repo slug vocabulary (name-else-path) and resolving
    /// which repo contains a directory — the `origin.repo` join key source.
    // trace:STORY-634 | ai:claude
    #[test]
    fn test_repo_slugs_and_containing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("api/src")).unwrap();
        std::fs::create_dir_all(dir.path().join("web-dir")).unwrap();

        let mut manifest = WorkspaceManifest {
            name: "test".into(),
            ..Default::default()
        };
        manifest.add_repo("api", "");
        manifest.add_repo("web-dir", "web");
        assert_eq!(manifest.repo_slugs(), vec!["api", "web"]);

        // Inside a repo (nested) resolves to its slug; the workspace root
        // itself is in no repo.
        assert_eq!(
            manifest.repo_slug_containing(dir.path(), &dir.path().join("api/src")),
            Some("api".to_string())
        );
        assert_eq!(
            manifest.repo_slug_containing(dir.path(), &dir.path().join("web-dir")),
            Some("web".to_string())
        );
        assert_eq!(manifest.repo_slug_containing(dir.path(), dir.path()), None);
    }

    #[test]
    fn test_init_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();

        // Create fake code repos
        for name in &["repo-a", "repo-b"] {
            let repo = ws.join(name);
            std::fs::create_dir_all(repo.join(".git")).unwrap();
        }

        let manifest = init_workspace(ws, "test-ws", None, None).unwrap();

        assert_eq!(manifest.name, "test-ws");
        assert_eq!(manifest.repos.len(), 2);
        assert!(ws.join(".aida-workspace").exists());
        assert!(ws.join("aida-store/metadata.yaml").exists());
        assert!(ws.join("aida-store/.git").exists());

        // Check each repo got a config
        assert!(ws.join("repo-a/.aida/config.toml").exists());
        assert!(ws.join("repo-b/.aida/config.toml").exists());
    }

    // trace:BUG-1649 | ai:claude
    #[test]
    fn bug_1649_workspace_repo_config_round_trips_hostile_values() {
        let plain = workspace_repo_config("../aida-store", "ws");
        assert!(plain.contains("store_path = \"../aida-store\"\n"));
        assert!(plain.contains("workspace = \"ws\"\n"));
        for (store, name) in [
            ("C:\\Users\\RUNNER~1\\x", "plain"),
            ("../has\"quote", "name \"with\" quotes"),
        ] {
            let body = workspace_repo_config(store, name);
            let parsed: toml::Table = toml::from_str(&body).expect("valid TOML");
            let deployment = parsed["deployment"].as_table().unwrap();
            assert_eq!(deployment["store_path"].as_str(), Some(store));
            assert_eq!(deployment["workspace"].as_str(), Some(name));
        }
    }
}
