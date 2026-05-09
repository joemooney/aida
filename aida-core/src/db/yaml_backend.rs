//! **ARCHIVED — legacy backend, do not use in new code.**
//!
//! Pre-EPIC-1-001 single-YAML-file storage. Kept compilable so the
//! archived `aida db migrate` command can still run one-shot
//! migrations off the legacy path. The kernel's canonical store is the
//! orphan-branch git backend (`db::cached_git_backend::CachedGitBackend`).
//! Eventual full removal is tracked by FR-1-076.
//!
//! trace:FR-1-076 | ai:claude

use anyhow::Result;
use std::path::{Path, PathBuf};

use super::traits::{BackendType, DatabaseBackend};
use crate::models::RequirementsStore;
use crate::storage::Storage;

/// YAML file backend implementation
///
/// This wraps the existing Storage class to implement the DatabaseBackend trait,
/// providing compatibility with the existing codebase while enabling the new
/// abstraction layer.
pub struct YamlBackend {
    storage: Storage,
    path: PathBuf,
}

impl YamlBackend {
    /// Creates a new YAML backend for the given file path
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref().to_path_buf();
        Self {
            storage: Storage::new(&path),
            path,
        }
    }

    /// Gets a reference to the underlying Storage
    pub fn storage(&self) -> &Storage {
        &self.storage
    }
}

impl DatabaseBackend for YamlBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::Yaml
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> Result<RequirementsStore> {
        self.storage.load()
    }

    fn save(&self, store: &RequirementsStore) -> Result<()> {
        self.storage.save(store)
    }

    fn update_atomically<F>(&self, update_fn: F) -> Result<RequirementsStore>
    where
        F: FnOnce(&mut RequirementsStore),
    {
        self.storage.update_atomically(update_fn)
    }

    /// Creates a baseline with git tagging support for YAML backend
    fn create_baseline(
        &self,
        name: String,
        description: Option<String>,
        created_by: String,
    ) -> Result<crate::models::Baseline> {
        let mut store = self.load()?;
        let mut baseline = store.create_baseline(name, description, created_by).clone();

        // Try to create a git tag for this baseline
        if let Some(git_tag) = self.create_git_tag_for_baseline(&baseline) {
            // Update the baseline with the git tag
            if let Some(b) = store.baselines.iter_mut().find(|b| b.id == baseline.id) {
                b.git_tag = Some(git_tag.clone());
                baseline.git_tag = Some(git_tag);
            }
        }

        self.save(&store)?;
        Ok(baseline)
    }
}

impl YamlBackend {
    /// Attempts to create a git tag for a baseline
    /// Returns the tag name if successful, None if git is not available or fails
    fn create_git_tag_for_baseline(&self, baseline: &crate::models::Baseline) -> Option<String> {
        use std::process::Command;

        // Get the directory containing the YAML file
        let dir = self.path.parent()?;

        // Check if we're in a git repository
        let git_check = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(dir)
            .output()
            .ok()?;

        if !git_check.status.success() {
            return None; // Not a git repo
        }

        let tag_name = baseline.git_tag_name();
        let message = baseline.description.as_deref().unwrap_or(&baseline.name);

        // Create an annotated tag
        let result = Command::new("git")
            .args(["tag", "-a", &tag_name, "-m", message])
            .current_dir(dir)
            .output()
            .ok()?;

        if result.status.success() {
            Some(tag_name)
        } else {
            // Tag might already exist or other error
            None
        }
    }

    /// Lists git tags that match the baseline pattern
    #[allow(dead_code)]
    pub fn list_git_baseline_tags(&self) -> Vec<String> {
        use std::process::Command;

        let Some(dir) = self.path.parent() else {
            return Vec::new();
        };

        let output = Command::new("git")
            .args(["tag", "-l", "baseline-*"])
            .current_dir(dir)
            .output()
            .ok();

        match output {
            Some(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|s| s.to_string())
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn test_yaml_backend_create_and_load() {
        // Use a path that doesn't exist yet
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.yaml");
        let backend = YamlBackend::new(&file_path);

        // Should create file with empty store
        backend.create_if_not_exists().unwrap();

        let store = backend.load().unwrap();
        assert!(store.requirements.is_empty());
        assert!(store.users.is_empty());
    }

    #[test]
    fn test_yaml_backend_save_and_load() {
        let temp_file = NamedTempFile::new().unwrap();
        let backend = YamlBackend::new(temp_file.path());

        let mut store = RequirementsStore::new();
        store.name = "Test DB".to_string();
        store.title = "Test Database".to_string();

        backend.save(&store).unwrap();

        let loaded = backend.load().unwrap();
        assert_eq!(loaded.name, "Test DB");
        assert_eq!(loaded.title, "Test Database");
    }
}
