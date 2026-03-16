// trace:ARCH-distributed-git-ops | ai:claude
//! Git operations for distributed AIDA.
//!
//! Provides wrappers around git commands for:
//! - Node registration CAS (compare-and-swap) loop
//! - Committing object changes
//! - Push/pull synchronization
//!
//! These operations shell out to the `git` CLI rather than using libgit2,
//! keeping the dependency light and behavior identical to what users see
//! when they run git manually.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Result of a git command execution.
#[derive(Debug)]
pub struct GitResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run a git command in the given working directory.
fn git(cwd: &Path, args: &[&str]) -> Result<GitResult> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run: git {}", args.join(" ")))?;

    Ok(GitResult {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

/// Check if a directory is a git repository.
pub fn is_git_repo(path: &Path) -> bool {
    git(path, &["rev-parse", "--git-dir"])
        .map(|r| r.success)
        .unwrap_or(false)
}

/// Initialize a new git repository.
pub fn init(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    let result = git(path, &["init"])?;
    if !result.success {
        anyhow::bail!("git init failed: {}", result.stderr);
    }
    Ok(())
}

/// Stage specific files.
pub fn add(repo: &Path, paths: &[&str]) -> Result<()> {
    let mut args = vec!["add"];
    args.extend(paths);
    let result = git(repo, &args)?;
    if !result.success {
        anyhow::bail!("git add failed: {}", result.stderr);
    }
    Ok(())
}

/// Stage all changes (tracked and untracked) in a subdirectory.
pub fn add_all(repo: &Path, subdir: &str) -> Result<()> {
    let result = git(repo, &["add", "-A", subdir])?;
    if !result.success {
        anyhow::bail!("git add -A {} failed: {}", subdir, result.stderr);
    }
    Ok(())
}

/// Commit staged changes.
pub fn commit(repo: &Path, message: &str) -> Result<bool> {
    let result = git(repo, &["commit", "-m", message])?;
    if result.success {
        Ok(true)
    } else if result.stdout.contains("nothing to commit") || result.stderr.contains("nothing to commit") {
        Ok(false) // nothing to commit — not an error
    } else {
        anyhow::bail!("git commit failed: {}", result.stderr);
    }
}

/// Push to the remote. Returns true on success, false if rejected (non-fast-forward).
pub fn push(repo: &Path, remote: &str, branch: &str) -> Result<bool> {
    let result = git(repo, &["push", remote, branch])?;
    if result.success {
        Ok(true)
    } else if result.stderr.contains("non-fast-forward")
        || result.stderr.contains("rejected")
        || result.stderr.contains("fetch first")
    {
        Ok(false) // push rejected — caller should pull and retry
    } else {
        anyhow::bail!("git push failed: {}", result.stderr);
    }
}

/// Pull with rebase from remote.
pub fn pull_rebase(repo: &Path, remote: &str, branch: &str) -> Result<()> {
    let result = git(repo, &["pull", "--rebase", remote, branch])?;
    if !result.success {
        anyhow::bail!("git pull --rebase failed: {}", result.stderr);
    }
    Ok(())
}

/// Pull (merge) from remote.
pub fn pull(repo: &Path, remote: &str, branch: &str) -> Result<()> {
    let result = git(repo, &["pull", remote, branch])?;
    if !result.success {
        anyhow::bail!("git pull failed: {}", result.stderr);
    }
    Ok(())
}

/// Get the current HEAD commit SHA (short form).
pub fn head_sha(repo: &Path) -> Result<String> {
    let result = git(repo, &["rev-parse", "--short", "HEAD"])?;
    if !result.success {
        anyhow::bail!("git rev-parse HEAD failed: {}", result.stderr);
    }
    Ok(result.stdout)
}

/// Get the current branch name.
pub fn current_branch(repo: &Path) -> Result<String> {
    let result = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if !result.success {
        anyhow::bail!("git branch detection failed: {}", result.stderr);
    }
    Ok(result.stdout)
}

/// Check if the working tree has uncommitted changes.
pub fn has_changes(repo: &Path) -> Result<bool> {
    let result = git(repo, &["status", "--porcelain"])?;
    Ok(!result.stdout.is_empty())
}

/// Check if the remote is reachable (can we push/pull?).
pub fn is_remote_reachable(repo: &Path, remote: &str) -> bool {
    git(repo, &["ls-remote", "--exit-code", remote])
        .map(|r| r.success)
        .unwrap_or(false)
}

/// Configure user name and email for commits (repo-local, not global).
pub fn configure_user(repo: &Path, name: &str, email: &str) -> Result<()> {
    git(repo, &["config", "user.name", name])?;
    git(repo, &["config", "user.email", email])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Node Registration CAS Loop
// ---------------------------------------------------------------------------

/// Maximum retries for the CAS push loop.
const MAX_CAS_RETRIES: u32 = 10;

/// Register a new node in the aida registry via git CAS push loop.
///
/// This is the core distributed identity mechanism:
/// 1. Pull latest from remote
/// 2. Read node_counter, claim the next ID
/// 3. Write updated counter + registry entry
/// 4. Commit and push
/// 5. If push rejected (someone else registered first), pull and retry
///
/// Returns the assigned node_id on success.
pub fn register_node(
    aida_repo: &Path,
    user_id: u32,
    hostname: &str,
) -> Result<u32> {
    use crate::node::{NodeConfig, NodeRegistry};

    let registry_dir = aida_repo.join("registry");
    std::fs::create_dir_all(&registry_dir)?;

    let registry_path = registry_dir.join("nodes.toml");
    let branch = current_branch(aida_repo)
        .unwrap_or_else(|_| "main".to_string());

    for attempt in 0..MAX_CAS_RETRIES {
        // Step 1: Pull latest (skip on first attempt if no remote)
        if attempt > 0 {
            if let Err(e) = pull_rebase(aida_repo, "origin", &branch) {
                eprintln!("Warning: pull failed (attempt {}): {}", attempt, e);
                anyhow::bail!(
                    "Cannot complete node registration: remote unreachable after {} attempts. Error: {}",
                    attempt, e
                );
            }
        }

        // Step 2: Load registry and claim next ID
        let mut registry = NodeRegistry::load(&registry_path)
            .unwrap_or_default();
        let node_id = registry.next_node_id();

        // Step 3: Register the node
        registry.register(user_id, hostname.to_string());
        registry.save(&registry_path)?;

        // Step 4: Stage, commit, push
        add(aida_repo, &["registry/nodes.toml"])?;

        let msg = format!(
            "chore(registry): register node {} for user {} ({})",
            node_id, user_id, hostname
        );
        commit(aida_repo, &msg)?;

        // Step 5: Push — if rejected, pull and retry
        match push(aida_repo, "origin", &branch) {
            Ok(true) => {
                // Success! Save local node config
                let config = NodeConfig {
                    node_id,
                    user_id,
                    hostname: hostname.to_string(),
                    registered_at: chrono::Utc::now(),
                };
                let node_config_path = aida_repo.join(".aida").join("node.toml");
                config.save(&node_config_path)?;

                return Ok(node_id);
            }
            Ok(false) => {
                // Push rejected — another node registered first. Retry.
                eprintln!(
                    "Node registration: push rejected (attempt {}), retrying...",
                    attempt + 1
                );
                continue;
            }
            Err(e) => {
                anyhow::bail!("Node registration failed: {}", e);
            }
        }
    }

    anyhow::bail!(
        "Node registration failed after {} attempts — too much contention on the registry",
        MAX_CAS_RETRIES
    );
}

/// Commit and push all pending object changes in the aida repo.
/// This is the "sync" operation — called when the user wants to share changes.
pub fn sync_objects(aida_repo: &Path, message: &str) -> Result<bool> {
    let branch = current_branch(aida_repo)
        .unwrap_or_else(|_| "main".to_string());

    // Stage all changes in objects/ and metadata.yaml
    add_all(aida_repo, "objects")?;
    if aida_repo.join("metadata.yaml").exists() {
        add(aida_repo, &["metadata.yaml"])?;
    }

    // Commit
    let committed = commit(aida_repo, message)?;
    if !committed {
        return Ok(false); // nothing to sync
    }

    // Push — retry with pull on rejection
    for _attempt in 0..MAX_CAS_RETRIES {
        match push(aida_repo, "origin", &branch)? {
            true => return Ok(true),
            false => {
                pull_rebase(aida_repo, "origin", &branch)?;
            }
        }
    }

    anyhow::bail!("Sync failed after {} push attempts", MAX_CAS_RETRIES);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_init_and_is_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("test-repo");

        assert!(!is_git_repo(&repo));
        init(&repo).unwrap();
        assert!(is_git_repo(&repo));
    }

    #[test]
    fn test_add_commit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("test-repo");
        init(&repo).unwrap();
        configure_user(&repo, "Test User", "test@example.com").unwrap();

        // Create a file and commit
        std::fs::write(repo.join("test.txt"), "hello").unwrap();
        add(&repo, &["test.txt"]).unwrap();
        let committed = commit(&repo, "initial commit").unwrap();
        assert!(committed);

        // Nothing to commit now
        let committed2 = commit(&repo, "empty").unwrap();
        assert!(!committed2);
    }

    #[test]
    fn test_has_changes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("test-repo");
        init(&repo).unwrap();
        configure_user(&repo, "Test User", "test@example.com").unwrap();

        // Initially no changes (empty repo)
        // Create and commit a file first
        std::fs::write(repo.join("test.txt"), "hello").unwrap();
        add(&repo, &["test.txt"]).unwrap();
        commit(&repo, "initial").unwrap();

        assert!(!has_changes(&repo).unwrap());

        // Modify the file
        std::fs::write(repo.join("test.txt"), "modified").unwrap();
        assert!(has_changes(&repo).unwrap());
    }

    #[test]
    fn test_head_sha_and_branch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("test-repo");
        init(&repo).unwrap();
        configure_user(&repo, "Test User", "test@example.com").unwrap();

        std::fs::write(repo.join("test.txt"), "hello").unwrap();
        add(&repo, &["test.txt"]).unwrap();
        commit(&repo, "initial").unwrap();

        let sha = head_sha(&repo).unwrap();
        assert!(!sha.is_empty());
        assert!(sha.len() >= 7);

        let branch = current_branch(&repo).unwrap();
        // Could be "main" or "master" depending on git config
        assert!(!branch.is_empty());
    }

    /// Helper: create a bare remote and a working clone with an initial commit pushed.
    /// Returns (bare_path, work_path, branch_name).
    fn setup_remote_and_clone(dir: &Path, name: &str) -> (PathBuf, PathBuf, String) {
        let bare = dir.join(format!("{}.git", name));
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "--bare"]).unwrap();

        let work = dir.join(name);
        init(&work).unwrap();
        configure_user(&work, "Test User", "test@example.com").unwrap();
        git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();

        // Initial commit so the branch exists
        std::fs::write(work.join("README.md"), "# Test").unwrap();
        add(&work, &["README.md"]).unwrap();
        commit(&work, "initial").unwrap();
        let branch = current_branch(&work).unwrap();
        git(&work, &["push", "-u", "origin", &branch]).unwrap();

        (bare, work, branch)
    }

    #[test]
    fn test_push_to_local_bare_repo() {
        let dir = tempfile::tempdir().unwrap();
        let (_bare, work, branch) = setup_remote_and_clone(dir.path(), "push-test");

        // Add another file and push
        std::fs::write(work.join("second.txt"), "hello").unwrap();
        add(&work, &["second.txt"]).unwrap();
        commit(&work, "second commit").unwrap();
        let pushed = push(&work, "origin", &branch).unwrap();
        assert!(pushed);
    }

    #[test]
    fn test_register_node_local() {
        let dir = tempfile::tempdir().unwrap();
        let (bare, aida, _branch) = setup_remote_and_clone(dir.path(), "aida");

        // Register first node
        let node_id = register_node(&aida, 1, "test-laptop").unwrap();
        assert_eq!(node_id, 1);

        // Verify registry file exists
        assert!(aida.join("registry/nodes.toml").exists());

        // Verify local node config was saved
        assert!(aida.join(".aida/node.toml").exists());

        // Register another node (simulating a second clone)
        let aida2 = dir.path().join("aida2");
        git(dir.path(), &["clone", bare.to_str().unwrap(), "aida2"]).unwrap();
        configure_user(&aida2, "Alice", "alice@example.com").unwrap();

        let node_id2 = register_node(&aida2, 2, "alice-dev").unwrap();
        assert_eq!(node_id2, 2);
    }
}
