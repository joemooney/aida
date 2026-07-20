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

    let gitignore = "# Node-local state\n.aida/\n*.lock\n";
    std::fs::write(store_full.join(".gitignore"), gitignore)?;
    git_ops::add(&store_full, &[".gitignore"])?;
    git_ops::commit(&store_full, "chore: initialize AIDA workspace store")?;

    // Create .aida/config.toml in each repo
    for repo in &manifest.repos {
        let repo_path = workspace_root.join(&repo.path);
        let aida_dir = repo_path.join(".aida");
        std::fs::create_dir_all(&aida_dir)?;

        let relative_store = format!("../{}", store_dir);
        let config = format!(
            "# AIDA workspace configuration\n\
             [deployment]\n\
             mode = \"distributed\"\n\
             store_path = \"{}\"\n\
             store_type = \"sibling\"\n\
             workspace = \"{}\"\n",
            relative_store, name
        );
        std::fs::write(aida_dir.join("config.toml"), config)?;
    }

    // Save workspace manifest
    manifest.save(workspace_root)?;

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
