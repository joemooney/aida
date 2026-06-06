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
use std::path::{Path, PathBuf};
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
    } else if result.stdout.contains("nothing to commit")
        || result.stderr.contains("nothing to commit")
    {
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
///
/// **Prefer `pull_rebase` for the orphan-store flow.** Bare `git pull`
/// fails on divergent branches when the user has neither
/// `pull.rebase` nor `pull.ff` set in their git config (modern git
/// requires explicit reconciliation). `pull_rebase` is deterministic
/// regardless of config and matches the linear-history model the
/// aida-store branch is designed for. Kept for callers that
/// specifically want merge semantics.
/// trace:BUG-1-051 | ai:claude
#[deprecated(
    note = "use pull_rebase — bare `git pull` fails on divergent branches without pull.rebase config"
)]
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

// ---------------------------------------------------------------------------
// Orphan Branch + Worktree
// ---------------------------------------------------------------------------

/// Create an orphan branch and worktree for the AIDA store.
///
/// This is the recommended approach for single-repo projects:
/// - Creates an orphan branch (no shared history with main)
/// - Checks it out as a worktree at the given path
/// - The worktree directory should be added to .gitignore
///
/// Returns the worktree path on success.
pub fn create_store_worktree(
    repo_root: &Path,
    worktree_dir: &str,
    branch_name: &str,
) -> Result<std::path::PathBuf> {
    let worktree_path = repo_root.join(worktree_dir);

    // Check if worktree already exists
    if worktree_path.exists() {
        // Verify it's actually a worktree
        let result = git(repo_root, &["worktree", "list"])?;
        if result.stdout.contains(worktree_dir) {
            return Ok(worktree_path); // already set up
        }
        anyhow::bail!(
            "Directory {} already exists but is not a git worktree",
            worktree_path.display()
        );
    }

    // Check if the orphan branch already exists (e.g., after clone)
    let branch_exists = git(repo_root, &["rev-parse", "--verify", branch_name])
        .map(|r| r.success)
        .unwrap_or(false);

    // BUG-39: drop any stale worktree registrations before trying to add
    // a fresh one. `git worktree prune` is a no-op when nothing's stale,
    // so always-running it is safe and avoids the cryptic "missing but
    // already registered worktree" error users hit after manually
    // deleting the worktree directory. trace:BUG-39 | ai:claude
    let _ = git(repo_root, &["worktree", "prune"]);

    if branch_exists {
        // Branch exists (e.g., from a clone) — just add the worktree
        let result = git(repo_root, &["worktree", "add", worktree_dir, branch_name])?;
        if !result.success {
            anyhow::bail!("Failed to add worktree: {}", result.stderr);
        }
    } else {
        // Create orphan branch via worktree
        // git worktree add --orphan was added in git 2.42+
        // For compatibility, create it manually
        let result = git(repo_root, &["worktree", "add", "--detach", worktree_dir])?;
        if !result.success {
            anyhow::bail!("Failed to create worktree: {}", result.stderr);
        }

        // Create the orphan branch in the worktree
        let result = git(&worktree_path, &["checkout", "--orphan", branch_name])?;
        if !result.success {
            anyhow::bail!("Failed to create orphan branch: {}", result.stderr);
        }

        // Clear the index (orphan branch starts with main's files staged)
        git(&worktree_path, &["rm", "-rf", "--cached", "."])?;
        // Clean working tree
        let _ = git(&worktree_path, &["clean", "-fd"]);
    }

    Ok(worktree_path)
}

/// Remove a store worktree and optionally delete the branch.
pub fn remove_store_worktree(repo_root: &Path, worktree_dir: &str) -> Result<()> {
    let worktree_path = repo_root.join(worktree_dir);
    if worktree_path.exists() {
        git(repo_root, &["worktree", "remove", "--force", worktree_dir])?;
    }
    Ok(())
}

/// Check if a worktree exists for the given directory.
pub fn has_worktree(repo_root: &Path, worktree_dir: &str) -> bool {
    let result = git(repo_root, &["worktree", "list"]).ok();
    result
        .map(|r| r.stdout.contains(worktree_dir))
        .unwrap_or(false)
}

/// Check if the remote is reachable (can we push/pull?).
pub fn is_remote_reachable(repo: &Path, remote: &str) -> bool {
    git(repo, &["ls-remote", "--exit-code", remote])
        .map(|r| r.success)
        .unwrap_or(false)
}

/// True when the repo has the given remote configured. Doesn't check
/// reachability — that's `is_remote_reachable`. Useful for "should we
/// even attempt a push?" decisions where unreachable-but-configured is
/// still a useful signal. trace:BUG-23 | ai:claude
pub fn has_remote(repo: &Path, remote: &str) -> bool {
    git(repo, &["remote", "get-url", remote])
        .map(|r| r.success)
        .unwrap_or(false)
}

/// Check whether `<remote>/<branch>` exists on the remote, without fetching.
/// Returns false on any git error (offline, unreachable remote, etc.) so
/// callers can treat absence and unreachability the same.
/// trace:EPIC-1-052 Phase 4 | ai:claude
pub fn remote_branch_exists(repo: &Path, remote: &str, branch: &str) -> bool {
    git(
        repo,
        &[
            "ls-remote",
            "--exit-code",
            remote,
            &format!("refs/heads/{}", branch),
        ],
    )
    .map(|r| r.success)
    .unwrap_or(false)
}

/// Fetch a single branch from a remote into a local tracking branch.
/// Equivalent to `git fetch <remote> <branch>:<branch>` — creates the
/// local branch if missing, fast-forwards otherwise.
/// trace:EPIC-1-052 Phase 4 | ai:claude
pub fn fetch_branch_into_local(repo: &Path, remote: &str, branch: &str) -> Result<()> {
    let refspec = format!("{}:{}", branch, branch);
    let result = git(repo, &["fetch", remote, &refspec])?;
    if !result.success {
        anyhow::bail!("git fetch {} {} failed: {}", remote, refspec, result.stderr);
    }
    Ok(())
}

/// Get a git config value (checks local, then global).
pub fn git_config_get(key: &str) -> Result<String> {
    let output = Command::new("git").args(["config", key]).output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        anyhow::bail!("git config {} not set", key)
    }
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
pub fn register_node(aida_repo: &Path, user_id: u32, hostname: &str) -> Result<String> {
    register_node_with_email(aida_repo, user_id, hostname, None)
}

/// Register a node and capture the user's email at registration time.
/// trace:EPIC-1-052 | ai:claude
pub fn register_node_with_email(
    aida_repo: &Path,
    user_id: u32,
    hostname: &str,
    email: Option<String>,
) -> Result<String> {
    register_node_full(aida_repo, None, user_id, hostname, email)
}

/// Probe whether a candidate node id is in use, and if so return the next
/// free id formed by suffixing the requested base with `2`, `3`, … This is
/// the read half of STORY-42's auto-suffix UX: callers (e.g., `aida node
/// acquire --id JM`) should call this first and then either accept the
/// suggestion or prompt the user before invoking [`register_node_full`].
///
/// Pulls the latest registry state from `origin` first when the remote is
/// reachable; falls back to the local state otherwise. The returned
/// suggestion is best-effort — between this probe and the CAS push another
/// node may grab the suggested id, in which case `register_node_full` will
/// fail and the caller can retry.
/// trace:STORY-42 | ai:claude
pub fn suggest_free_node_id(aida_repo: &Path, requested: &str) -> Result<NodeIdProbe> {
    use crate::node::{BlockRegistry, NodeRegistry};

    let registry_dir = aida_repo.join("registry");
    let registry_path = registry_dir.join("nodes.toml");
    let blocks_path = registry_dir.join("blocks.yaml");

    if let Ok(branch) = current_branch(aida_repo) {
        let _ = pull_rebase(aida_repo, "origin", &branch);
    }

    let registry = NodeRegistry::load(&registry_path).unwrap_or_default();
    let blocks_in_use: std::collections::HashSet<String> = if blocks_path.exists() {
        BlockRegistry::load(&blocks_path)
            .map(|br| br.blocks.iter().map(|b| b.node_id.clone()).collect())
            .unwrap_or_default()
    } else {
        std::collections::HashSet::new()
    };

    let in_use = |id: &str| registry.is_registered(id) || blocks_in_use.contains(id);

    if !in_use(requested) {
        return Ok(NodeIdProbe::Free);
    }
    for n in 2..u32::MAX {
        let candidate = format!("{}{}", requested, n);
        if !in_use(&candidate) {
            return Ok(NodeIdProbe::Taken {
                suggested: candidate,
            });
        }
    }
    anyhow::bail!("could not find a free suffix for node id '{}'", requested)
}

/// Result of [`suggest_free_node_id`]. trace:STORY-42 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeIdProbe {
    /// The requested id is free — caller can register it directly.
    Free,
    /// The requested id is taken; `suggested` is the lowest-numbered
    /// `<requested><N>` that is currently free.
    Taken { suggested: String },
}

/// Register a node, optionally at a specific id.
///
/// When `requested_id` is Some(id), the CAS loop verifies the id is free;
/// if not, the call fails. When `None`, the next sequential numeric id is
/// claimed (or pre-EPIC-9 legacy ids skipped).
///
/// Node IDs became `String` in EPIC-9 / STORY-41.
/// trace:EPIC-1-052 | ai:claude
/// trace:STORY-41 | ai:claude
pub fn register_node_full(
    aida_repo: &Path,
    requested_id: Option<String>,
    user_id: u32,
    hostname: &str,
    email: Option<String>,
) -> Result<String> {
    use crate::node::{BlockRegistry, NodeConfig, NodeRegistry};

    let registry_dir = aida_repo.join("registry");
    std::fs::create_dir_all(&registry_dir)?;

    let registry_path = registry_dir.join("nodes.toml");
    let blocks_path = registry_dir.join("blocks.yaml");
    let branch = current_branch(aida_repo).unwrap_or_else(|_| "main".to_string());

    // BUG-40: solo users with no origin still want their preferred node id.
    // The CAS push loop is only needed for race-resolution against other
    // clones — when there's no remote, claim locally and let the next
    // `aida push` upload the registration commit. Decided once up front so
    // every retry path agrees on the policy.
    let local_only = !has_remote(aida_repo, "origin");

    for attempt in 0..MAX_CAS_RETRIES {
        // Step 1: Pull latest (skip on first attempt if no remote)
        if attempt > 0 && !local_only {
            if let Err(e) = pull_rebase(aida_repo, "origin", &branch) {
                eprintln!("Warning: pull failed (attempt {}): {}", attempt, e);
                anyhow::bail!(
                    "Cannot complete node registration: remote unreachable after {} attempts. Error: {}",
                    attempt, e
                );
            }
        }

        // Step 2: Load registry and pick the id to claim. A node id is
        // "in use" if it appears in nodes.toml OR if it owns any entry in
        // blocks.yaml — the latter catches legacy clones that operated
        // with implicit (pre-EPIC-1-052) node ids and never registered.
        let mut registry = NodeRegistry::load(&registry_path).unwrap_or_default();
        let blocks_in_use: std::collections::HashSet<String> = if blocks_path.exists() {
            BlockRegistry::load(&blocks_path)
                .map(|br| br.blocks.iter().map(|b| b.node_id.clone()).collect())
                .unwrap_or_default()
        } else {
            std::collections::HashSet::new()
        };
        let id_in_use = |id: &str| registry.is_registered(id) || blocks_in_use.contains(id);

        let node_id: String = match requested_id.clone() {
            Some(id) => {
                if id_in_use(&id) {
                    anyhow::bail!(
                        "Node id {} is already taken (in nodes.toml or blocks.yaml) — \
                         pick a different id",
                        id
                    );
                }
                id
            }
            None => {
                // Walk numeric next-id space, skipping any id that's
                // already implied by blocks.yaml (legacy clones).
                let mut next = registry.next_node_id();
                while blocks_in_use.contains(&next) {
                    let n: u32 = next.parse().unwrap_or(0);
                    next = (n + 1).to_string();
                }
                next
            }
        };

        // Step 3: Register the node, capturing clone_path for hijack-mark-
        // in-place support (STORY-43). The clone path is the parent of the
        // .aida-store worktree (i.e., the project root). trace:STORY-41
        let clone_path = aida_repo.parent().and_then(|p| p.canonicalize().ok());
        registry.register_specific_full(
            node_id.clone(),
            user_id,
            hostname.to_string(),
            email.clone(),
            clone_path,
        );
        registry.save(&registry_path)?;

        // Step 4: Stage, commit, push
        add(aida_repo, &["registry/nodes.toml"])?;

        let msg = format!(
            "chore(registry): register node {} for user {} ({})",
            node_id, user_id, hostname
        );
        commit(aida_repo, &msg)?;

        // Step 5: Push — if rejected, pull and retry. When there's no
        // origin remote (solo first clone), skip the push entirely:
        // the registration commit lives on the local orphan branch and
        // will be uploaded when the user later adds a remote and runs
        // `aida push`. trace:BUG-40 | ai:claude
        if local_only {
            let config = NodeConfig {
                node_id: node_id.clone(),
                user_id,
                hostname: hostname.to_string(),
                email: email.clone(),
                registered_at: chrono::Utc::now(),
            };
            let node_config_path = aida_repo.join(".aida").join("node.toml");
            config.save(&node_config_path)?;
            return Ok(node_id);
        }
        match push(aida_repo, "origin", &branch) {
            Ok(true) => {
                // Success! Save local node config
                let config = NodeConfig {
                    node_id: node_id.clone(),
                    user_id,
                    hostname: hostname.to_string(),
                    email: email.clone(),
                    registered_at: chrono::Utc::now(),
                };
                let node_config_path = aida_repo.join(".aida").join("node.toml");
                config.save(&node_config_path)?;

                return Ok(node_id);
            }
            Ok(false) => {
                // Push rejected — another node registered first. Soft-reset
                // the local commit before pulling so the next iteration's
                // pull --rebase doesn't try to re-apply our now-stale claim
                // (which would conflict on the same nodes.toml lines).
                // Without this reset, the CAS loop wedges on rebase failure.
                // trace:BUG-1-069 | ai:claude
                // Discard the stale local commit AND its staged/working-tree
                // changes so the next iteration's pull --rebase has a clean
                // tree to apply onto. `--soft` alone leaves the index dirty,
                // which pull --rebase rejects. trace:BUG-1-069 | ai:claude
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(aida_repo)
                    .output();
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

/// Backfill a node entry into the shared registry WITHOUT touching the local
/// clone's identity. Unlike [`register_node_full`], this never writes
/// `.aida/node.toml` and never allocates blocks — the entry being backfilled
/// describes some *other* (typically legacy) clone, not the one running the
/// command. Used by `aida node acquire --remote-only` to formalize a clone
/// that has been operating with an implicit pre-EPIC-1-052 node id, from a
/// sibling clone, without hijacking the running clone's own identity.
///
/// `id`, `hostname`, and `email` are all required and explicit — none are
/// inferred from the local environment, since the entry is not about the
/// local clone. The same CAS push loop as [`register_node_full`] applies so
/// concurrent backfills serialize through git. Returns the registered id.
/// trace:FR-265 | ai:claude
pub fn register_node_remote_only(
    aida_repo: &Path,
    id: String,
    user_id: u32,
    hostname: &str,
    email: String,
) -> Result<String> {
    use crate::node::NodeRegistry;

    let registry_dir = aida_repo.join("registry");
    std::fs::create_dir_all(&registry_dir)?;

    let registry_path = registry_dir.join("nodes.toml");
    let branch = current_branch(aida_repo).unwrap_or_else(|_| "main".to_string());

    let local_only = !has_remote(aida_repo, "origin");

    for attempt in 0..MAX_CAS_RETRIES {
        // Step 1: Pull latest (skip on first attempt if no remote)
        if attempt > 0 && !local_only {
            if let Err(e) = pull_rebase(aida_repo, "origin", &branch) {
                eprintln!("Warning: pull failed (attempt {}): {}", attempt, e);
                anyhow::bail!(
                    "Cannot complete remote-only node backfill: remote unreachable after {} attempts. Error: {}",
                    attempt, e
                );
            }
        }

        // Step 2: Load registry and verify the requested id isn't already in
        // nodes.toml. A block-only id (legacy clone with blocks but no
        // registry entry) is the legitimate backfill *target*, so unlike the
        // normal acquire path we do NOT treat block ownership as a collision.
        let mut registry = NodeRegistry::load(&registry_path).unwrap_or_default();
        if registry.is_registered(&id) {
            anyhow::bail!(
                "Node id {} is already in registry/nodes.toml — nothing to backfill",
                id
            );
        }

        // Step 3: Register the entry. clone_path is None: we don't know the
        // legacy clone's path from here, and recording the local clone's path
        // would be wrong (the entry isn't about this clone).
        registry.register_specific_full(
            id.clone(),
            user_id,
            hostname.to_string(),
            Some(email.clone()),
            None,
        );
        registry.save(&registry_path)?;

        // Step 4: Stage, commit
        add(aida_repo, &["registry/nodes.toml"])?;
        let msg = format!(
            "chore(registry): backfill node {} for user {} ({}) [remote-only]",
            id, user_id, hostname
        );
        commit(aida_repo, &msg)?;

        // Step 5: Push (skip when there's no remote — the commit lives on the
        // local orphan branch and uploads on the next `aida push`). Crucially,
        // NO local node.toml is ever written on any path — the running clone
        // keeps its own identity. trace:FR-265 | ai:claude
        if local_only {
            return Ok(id);
        }
        match push(aida_repo, "origin", &branch) {
            Ok(true) => return Ok(id),
            Ok(false) => {
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(aida_repo)
                    .output();
                eprintln!(
                    "Remote-only node backfill: push rejected (attempt {}), retrying...",
                    attempt + 1
                );
                continue;
            }
            Err(e) => anyhow::bail!("Remote-only node backfill failed: {}", e),
        }
    }

    anyhow::bail!(
        "Remote-only node backfill failed after {} attempts — too much contention on the registry",
        MAX_CAS_RETRIES
    );
}

/// Outcome of [`hijack_node`]. Tells the CLI whether a stale-clone marker
/// was successfully dropped or whether we just re-attributed silently
/// (because the old clone is unreachable from this machine).
/// trace:STORY-43 | ai:claude
#[derive(Debug, Clone)]
pub enum HijackOutcome {
    /// Old clone was reachable; HIJACKED.toml was written into its
    /// `.aida-store/.aida/`. The path returned is the marker file location.
    MarkedInPlace { marker_path: PathBuf },
    /// Old clone could not be reached (different host, missing path, no
    /// clone_path recorded). The registry entry was overwritten in place,
    /// so the new clone owns the id; the old clone, if it ever runs `aida`
    /// again, will load a stale node.toml but won't see a marker.
    Reattributed { reason: String },
}

/// Re-claim node `target_id` for the current clone. The registry entry's
/// hostname/email/clone_path/registered are updated in place; if the
/// previous clone is reachable from this machine (same hostname AND its
/// recorded clone_path still exists), we drop a `HIJACKED.toml` into its
/// `.aida-store/.aida/` so the user sees a warning the next time they run
/// `aida` there. The CAS push loop applies — concurrent hijack attempts on
/// different clones serialize through git.
///
/// Returns the [`HijackOutcome`] describing which branch was taken.
/// trace:STORY-43 | ai:claude
pub fn hijack_node(
    aida_repo: &Path,
    target_id: &str,
    user_id: u32,
    hostname: &str,
    email: Option<String>,
) -> Result<HijackOutcome> {
    use crate::node::{HijackMarker, NodeConfig, NodeRegistry};

    let registry_path = aida_repo.join("registry").join("nodes.toml");
    let branch = current_branch(aida_repo).unwrap_or_else(|_| "main".to_string());
    let clone_path = aida_repo.parent().and_then(|p| p.canonicalize().ok());

    for attempt in 0..MAX_CAS_RETRIES {
        if attempt > 0 {
            pull_rebase(aida_repo, "origin", &branch)?;
        }

        let mut registry = NodeRegistry::load(&registry_path).unwrap_or_default();
        let entry_idx = registry
            .nodes
            .iter()
            .position(|n| n.id == target_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Node id '{}' is not registered — nothing to hijack",
                    target_id
                )
            })?;

        // Decide reachability before mutating the registry. "Reachable"
        // means same hostname, the clone_path is recorded and exists on
        // disk, and email matches (proxy for same user). When email is
        // None on either side, fall back to hostname+path.
        let old_entry = registry.nodes[entry_idx].clone();
        let same_host = old_entry.hostname == hostname;
        let same_user = match (old_entry.email.as_deref(), email.as_deref()) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };
        let path_exists = old_entry
            .clone_path
            .as_ref()
            .map(|p| p.exists() && *p != aida_repo.parent().unwrap_or(p))
            .unwrap_or(false);

        let outcome = if same_host && same_user && path_exists {
            // Reachable — write HIJACKED.toml in the old clone.
            let old_clone = old_entry.clone_path.clone().unwrap();
            let marker = HijackMarker {
                node_id: target_id.to_string(),
                new_owner_hostname: hostname.to_string(),
                new_owner_email: email.clone(),
                new_owner_clone_path: clone_path.clone(),
                hijacked_at: chrono::Utc::now(),
            };
            let marker_path = HijackMarker::path_in_store(&old_clone.join(".aida-store"));
            // Best-effort: if the old store path doesn't exist (the clone
            // was deleted but its parent dir lingered), fall through to
            // Reattributed instead of failing the whole hijack.
            if marker_path.parent().map(|p| p.exists()).unwrap_or(false)
                || std::fs::create_dir_all(marker_path.parent().unwrap()).is_ok()
            {
                marker.save(&marker_path)?;
                HijackOutcome::MarkedInPlace { marker_path }
            } else {
                HijackOutcome::Reattributed {
                    reason: format!(
                        "old clone path {} no longer has a writable .aida-store/.aida/",
                        old_clone.display()
                    ),
                }
            }
        } else {
            let reason = if !same_host {
                format!(
                    "old clone was on different host ({} vs {})",
                    old_entry.hostname, hostname
                )
            } else if !same_user {
                "old clone was owned by a different user".to_string()
            } else if old_entry.clone_path.is_none() {
                "old clone path was not recorded (pre-EPIC-9 entry)".to_string()
            } else {
                format!(
                    "old clone path {} no longer exists",
                    old_entry
                        .clone_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                )
            };
            HijackOutcome::Reattributed { reason }
        };

        // Update the registry entry to point at this clone.
        let entry = &mut registry.nodes[entry_idx];
        entry.hostname = hostname.to_string();
        entry.email = email.clone();
        entry.clone_path = clone_path.clone();
        entry.registered = chrono::Utc::now();
        registry.save(&registry_path)?;

        // Stage + commit + push.
        add(aida_repo, &["registry/nodes.toml"])?;
        let msg = format!(
            "chore(registry): hijack node {} for user {} ({})",
            target_id, user_id, hostname
        );
        commit(aida_repo, &msg)?;

        match push(aida_repo, "origin", &branch) {
            Ok(true) => {
                // Save local NodeConfig so this clone now claims the id.
                let config = NodeConfig {
                    node_id: target_id.to_string(),
                    user_id,
                    hostname: hostname.to_string(),
                    email: email.clone(),
                    registered_at: chrono::Utc::now(),
                };
                let node_config_path = aida_repo.join(".aida").join("node.toml");
                config.save(&node_config_path)?;
                // Clear our own HIJACKED.toml (if present) — we're the
                // legitimate owner again and the warning would be stale.
                let local_marker = HijackMarker::path_in_store(aida_repo);
                let _ = std::fs::remove_file(&local_marker);
                return Ok(outcome);
            }
            Ok(false) => {
                // CAS rejected — drop the marker we just wrote (if any),
                // soft-reset, and retry. Without rolling back the marker we
                // could leave a stale HIJACKED.toml when another node won
                // the race and we failed.
                if let HijackOutcome::MarkedInPlace { marker_path } = &outcome {
                    let _ = std::fs::remove_file(marker_path);
                }
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(aida_repo)
                    .output();
                continue;
            }
            Err(e) => anyhow::bail!("Hijack push failed: {}", e),
        }
    }

    anyhow::bail!(
        "Hijack failed after {} attempts — too much contention",
        MAX_CAS_RETRIES
    )
}

/// Remove a node entry from the shared registry via the CAS push loop.
///
/// Note: this does NOT revoke any agreed-IDs already issued by the node,
/// nor does it free up the node id for reuse — the registry is append-only
/// in spirit, but we allow tombstone removal for housekeeping (e.g., a
/// machine that's been decommissioned). Issued IDs remain valid because
/// they live in the object store, not in the registry.
/// trace:EPIC-1-052 | ai:claude
pub fn unregister_node(aida_repo: &Path, node_id: &str) -> Result<bool> {
    use crate::node::NodeRegistry;

    let registry_path = aida_repo.join("registry").join("nodes.toml");
    let branch = current_branch(aida_repo).unwrap_or_else(|_| "main".to_string());

    for attempt in 0..MAX_CAS_RETRIES {
        if attempt > 0 {
            pull_rebase(aida_repo, "origin", &branch)?;
        }

        let mut registry = NodeRegistry::load(&registry_path).unwrap_or_default();
        if !registry.remove(node_id) {
            return Ok(false);
        }
        registry.save(&registry_path)?;
        add(aida_repo, &["registry/nodes.toml"])?;
        commit(
            aida_repo,
            &format!("chore(registry): unregister node {}", node_id),
        )?;

        match push(aida_repo, "origin", &branch) {
            Ok(true) => return Ok(true),
            Ok(false) => {
                // Same soft-reset dance as register_node_full so the next
                // attempt's pull --rebase doesn't conflict with our now-
                // stale local commit. trace:BUG-1-069 | ai:claude
                // Discard the stale local commit AND its staged/working-tree
                // changes so the next iteration's pull --rebase has a clean
                // tree to apply onto. `--soft` alone leaves the index dirty,
                // which pull --rebase rejects. trace:BUG-1-069 | ai:claude
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(aida_repo)
                    .output();
                continue;
            }
            Err(e) => anyhow::bail!("Node unregister failed: {}", e),
        }
    }

    anyhow::bail!(
        "Node unregister failed after {} attempts — too much contention",
        MAX_CAS_RETRIES
    );
}

/// Commit and push all pending object changes in the aida repo.
/// This is the "sync" operation — called when the user wants to share changes.
/// Run the merge gate: assign agreed IDs to all objects that don't have one.
///
/// This is the two-tier ID mechanism: node-namespaced IDs (FR-7-048) get
/// short agreed IDs (FR-423) assigned at merge time via CAS counter.
///
/// Returns the number of agreed IDs assigned.
/// True when `spec_id` is in the short global form `<TYPE>-<SEQ>` (one hyphen,
/// trailing digits). False for node-aware ids like `FR-1-053` or anything
/// malformed. Used to keep merge-gate idempotent against stores that have
/// already had their legacy ids retired (FR-1-071).
/// trace:FR-1-071 | ai:claude
pub fn is_global_id_format(spec_id: &str) -> bool {
    let parts: Vec<&str> = spec_id.split('-').collect();
    parts.len() == 2 && !parts[0].is_empty() && parts[1].chars().all(|c| c.is_ascii_digit())
}

pub fn merge_gate(store_path: &Path) -> Result<Vec<(String, String)>> {
    use crate::node::AgreedCounters;
    use crate::object_store;

    let objects_root = store_path.join("objects");
    let registry_dir = store_path.join("registry");
    std::fs::create_dir_all(&registry_dir)?;

    let counters_path = registry_dir.join("agreed_counters.toml");

    // Load counters. Reader can race a concurrent merge-gate writer
    // mid-`write_atomic`; on Windows that surfaces as a transient
    // PermissionDenied/NotFound from `CreateFile`. Retry through
    // `read_atomic` so the gate never fails to read its own state.
    // trace:TASK-346 | ai:claude
    let mut counters = if counters_path.exists() {
        let content = crate::read_atomic(&counters_path)?;
        toml::from_str::<AgreedCounters>(&content).unwrap_or_default()
    } else {
        AgreedCounters::default()
    };

    // Find all objects without an agreed_id
    let files = object_store::list_objects(&objects_root)?;
    let mut assignments = Vec::new();

    // BUG-82: collision guard. Build a set of every short id the store
    // already resolves to — spec_ids that are themselves in global form
    // (e.g. legacy `FR-140` migrated by FR-1-071), and every existing
    // agreed_id. Candidate agreed_ids that collide with this set get
    // skipped + retried so the counter walks past taken numbers instead
    // of overwriting them. Without this, merge-gate could (and did, in
    // PR-12) assign `TASK-34` to a node-aware req while `TASK-34` already
    // resolved to a different requirement. trace:BUG-82 | ai:claude
    let mut taken_short_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (_spec_id, path) in &files {
        let Ok(existing) = object_store::read_object_from_path(path) else {
            continue;
        };
        if let Some(s) = &existing.spec_id {
            if is_global_id_format(s) {
                taken_short_ids.insert(s.to_ascii_uppercase());
            }
        }
        if let Some(a) = &existing.agreed_id {
            taken_short_ids.insert(a.to_ascii_uppercase());
        }
    }

    for (_spec_id, path) in &files {
        let mut req = match object_store::read_object_from_path(path) {
            Ok(r) => r,
            Err(_) => continue,
        };

        if req.agreed_id.is_some() {
            continue; // already has an agreed ID
        }

        let spec_id = match &req.spec_id {
            Some(s) => s.clone(),
            None => continue,
        };

        // Skip reqs whose spec_id is ALREADY in global form (`<TYPE>-<SEQ>`,
        // e.g., `FR-140`). Without this guard, running merge-gate after
        // `aida db retire-legacy-ids` (FR-1-071) would re-assign new
        // agreed_ids to every previously-migrated req, creating fresh
        // divergence. trace:FR-1-071 | ai:claude
        if is_global_id_format(&spec_id) {
            continue;
        }

        // Use the requirement's type for the agreed ID prefix (FR, BUG, TASK, etc.)
        // This gives short, standard prefixes regardless of the original feature-based prefix
        let type_prefix = match req.req_type {
            crate::models::RequirementType::Functional => "FR",
            crate::models::RequirementType::NonFunctional => "NFR",
            crate::models::RequirementType::System => "SR",
            crate::models::RequirementType::User => "UR",
            crate::models::RequirementType::ChangeRequest => "CR",
            crate::models::RequirementType::Bug => "BUG",
            crate::models::RequirementType::Epic => "EPIC",
            crate::models::RequirementType::Story => "STORY",
            crate::models::RequirementType::Task => "TASK",
            crate::models::RequirementType::Spike => "SPIKE",
            crate::models::RequirementType::Sprint => "SPRINT",
            crate::models::RequirementType::Folder => "FOLDER",
            crate::models::RequirementType::Meta => "META",
            // Docs-layer types — short ASCII prefixes that read naturally
            // in trace comments and refs. trace:FR-1-074 | ai:claude
            crate::models::RequirementType::Principle => "PRIN",
            crate::models::RequirementType::Vision => "VIS",
            crate::models::RequirementType::Constraint => "CON",
            crate::models::RequirementType::Decision => "ADR",
            crate::models::RequirementType::Term => "TERM",
            crate::models::RequirementType::Doc => "DOC",
        };

        // BUG-82: walk past any candidate that already resolves to an
        // existing requirement. Cap the retries so a pathologically
        // dense counter range can't loop forever; 1000 is far above
        // any real-world collision rate. Log skips to stderr so the
        // gap in the counter sequence is visible. trace:BUG-82 | ai:claude
        let mut seq = counters.next(type_prefix);
        let mut agreed = AgreedCounters::format_agreed_id(type_prefix, seq);
        let mut skipped: Vec<String> = Vec::new();
        for _ in 0..1000 {
            if !taken_short_ids.contains(&agreed.to_ascii_uppercase()) {
                break;
            }
            skipped.push(agreed.clone());
            seq = counters.next(type_prefix);
            agreed = AgreedCounters::format_agreed_id(type_prefix, seq);
        }
        if taken_short_ids.contains(&agreed.to_ascii_uppercase()) {
            anyhow::bail!(
                "BUG-82 collision guard: walked 1000 candidates for prefix `{}` \
                 without finding a free agreed-id (last tried: {}); aborting before \
                 assigning a collision",
                type_prefix,
                agreed
            );
        }
        if !skipped.is_empty() {
            eprintln!(
                "  ⚠ BUG-82: skipped {} taken candidate(s) when gating {} → {} ({})",
                skipped.len(),
                spec_id,
                agreed,
                skipped.join(", ")
            );
        }

        // Reserve this id so a later iteration of THIS merge-gate run
        // doesn't double-allocate it.
        taken_short_ids.insert(agreed.to_ascii_uppercase());

        req.agreed_id = Some(agreed.clone());
        object_store::write_object(&objects_root, &req)?;
        assignments.push((spec_id, agreed));
    }

    // Save updated counters
    let content = toml::to_string_pretty(&counters)?;
    // Atomic write: concurrent `aida add` from parallel sessions tears the
    // counter file; a torn counter double-allocates IDs. trace:TASK-331 | ai:claude
    crate::write_atomic(&counters_path, content)?;

    // Stage and commit
    if !assignments.is_empty() {
        add_all(store_path, "objects")?;
        add(store_path, &["registry/agreed_counters.toml"])?;
        let msg = format!(
            "chore(merge-gate): assign {} agreed ID(s)",
            assignments.len()
        );
        commit(store_path, &msg)?;
    }

    Ok(assignments)
}

pub fn sync_objects(aida_repo: &Path, message: &str) -> Result<bool> {
    let branch = current_branch(aida_repo).unwrap_or_else(|_| "main".to_string());

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
        assert_eq!(node_id, "1");

        // Verify registry file exists
        assert!(aida.join("registry/nodes.toml").exists());

        // Verify local node config was saved
        assert!(aida.join(".aida/node.toml").exists());

        // Register another node (simulating a second clone)
        let aida2 = dir.path().join("aida2");
        git(dir.path(), &["clone", bare.to_str().unwrap(), "aida2"]).unwrap();
        configure_user(&aida2, "Alice", "alice@example.com").unwrap();

        let node_id2 = register_node(&aida2, 2, "alice-dev").unwrap();
        assert_eq!(node_id2, "2");
    }

    /// FR-265: `register_node_remote_only` backfills a registry entry for some
    /// other (legacy) clone WITHOUT writing the local clone's `.aida/node.toml`
    /// and WITHOUT allocating blocks. The running clone keeps its own identity.
    /// trace:FR-265 | ai:claude
    #[test]
    fn test_register_node_remote_only_skips_local_identity() {
        let dir = tempfile::tempdir().unwrap();
        let (_bare, aida, _branch) = setup_remote_and_clone(dir.path(), "aida");

        // Sanity: no local node.toml before we start.
        let node_config_path = aida.join(".aida/node.toml");
        assert!(!node_config_path.exists());

        // Backfill node "2" (spock) from this clone.
        let id =
            register_node_remote_only(&aida, "2".into(), 2, "spock", "spock@example.com".into())
                .unwrap();
        assert_eq!(id, "2");

        // The registry entry exists with the backfilled provenance.
        let registry = crate::node::NodeRegistry::load(&aida.join("registry/nodes.toml")).unwrap();
        assert!(registry.is_registered("2"));
        let entry = registry.get("2").unwrap();
        assert_eq!(entry.hostname, "spock");
        assert_eq!(entry.email.as_deref(), Some("spock@example.com"));
        // clone_path must NOT point at the local clone — we don't know the
        // legacy clone's path, so it's left None.
        assert!(entry.clone_path.is_none());

        // Crucially: the local clone's identity file was NOT written.
        assert!(
            !node_config_path.exists(),
            "remote-only backfill must not write local .aida/node.toml"
        );

        // A second backfill of an already-registered id refuses.
        let err =
            register_node_remote_only(&aida, "2".into(), 2, "spock", "spock@example.com".into())
                .unwrap_err();
        assert!(err.to_string().contains("already in registry"));
    }

    #[test]
    fn test_suggest_free_node_id() {
        // trace:STORY-42 | ai:claude
        let dir = tempfile::tempdir().unwrap();
        let (_bare, aida, _branch) = setup_remote_and_clone(dir.path(), "aida");

        // No registry yet — any id is free.
        assert_eq!(
            suggest_free_node_id(&aida, "JM").unwrap(),
            NodeIdProbe::Free
        );

        // Manually seed a NodeRegistry with "JM" taken.
        std::fs::create_dir_all(aida.join("registry")).unwrap();
        let mut registry = crate::node::NodeRegistry::default();
        registry.register_specific("JM".into(), 1, "imac".into(), None);
        registry
            .save(&aida.join("registry").join("nodes.toml"))
            .unwrap();

        match suggest_free_node_id(&aida, "JM").unwrap() {
            NodeIdProbe::Taken { suggested } => assert_eq!(suggested, "JM2"),
            other => panic!("expected Taken, got {:?}", other),
        }

        // After "JM2" is also taken, suggestion advances to "JM3".
        let mut registry =
            crate::node::NodeRegistry::load(&aida.join("registry").join("nodes.toml")).unwrap();
        registry.register_specific("JM2".into(), 1, "spock".into(), None);
        registry
            .save(&aida.join("registry").join("nodes.toml"))
            .unwrap();

        match suggest_free_node_id(&aida, "JM").unwrap() {
            NodeIdProbe::Taken { suggested } => assert_eq!(suggested, "JM3"),
            other => panic!("expected Taken, got {:?}", other),
        }

        // Different prefix is still free.
        assert_eq!(
            suggest_free_node_id(&aida, "AL").unwrap(),
            NodeIdProbe::Free
        );
    }

    /// BUG-82: merge_gate must skip agreed-id candidates that already
    /// resolve to an existing requirement. Seed the store with
    /// `TASK-1` in legacy global form (so it counts as a taken short
    /// id) and a fresh node-aware `TASK-2-001` waiting for gate. With
    /// the counter starting at 0, the naive next() would propose
    /// `TASK-1` — the guard must walk past it and assign `TASK-2`.
    /// trace:BUG-82 | ai:claude
    #[test]
    fn merge_gate_skips_existing_short_id_collisions() {
        use crate::models::{Requirement, RequirementType};
        use crate::object_store;

        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().to_path_buf();
        let objects = store.join("objects");
        std::fs::create_dir_all(&objects).unwrap();

        // Existing requirement with a global-form spec_id `TASK-1`
        // (mimics the FR-1-071 legacy-migration state).
        let mut taken = Requirement::new("TASK-1".into(), String::new());
        taken.spec_id = Some("TASK-1".into());
        taken.req_type = RequirementType::Task;
        object_store::write_object(&objects, &taken).unwrap();

        // New requirement with node-aware spec_id, no agreed_id yet.
        let mut pending = Requirement::new("TASK-2-001".into(), String::new());
        pending.spec_id = Some("TASK-2-001".into());
        pending.req_type = RequirementType::Task;
        object_store::write_object(&objects, &pending).unwrap();

        // Init the store as a git repo so merge_gate's commit path
        // doesn't fail — merge_gate operates on store_path as a
        // workdir.
        init(&store).unwrap();
        configure_user(&store, "Test", "test@example.com").unwrap();

        let assignments = merge_gate(&store).unwrap();

        // Exactly one assignment: TASK-2-001 → TASK-2 (NOT TASK-1).
        assert_eq!(assignments.len(), 1, "exactly one gating expected");
        let (origin, agreed) = &assignments[0];
        assert_eq!(origin, "TASK-2-001");
        assert_eq!(
            agreed, "TASK-2",
            "must skip TASK-1 because it's already taken"
        );

        // Re-running merge_gate is idempotent now that the pending
        // req has been assigned (the legacy short-form TASK-1 is
        // skipped because is_global_id_format catches it before the
        // collision walk fires).
        let second = merge_gate(&store).unwrap();
        assert!(second.is_empty(), "second run should be a no-op");
    }

    /// BUG-82: when an EARLIER candidate from the same merge-gate run
    /// has already been assigned, the next candidate must also walk
    /// past it (within-run reservation). Seed with `TASK-1` taken,
    /// then two pending node-aware tasks; expect `TASK-2` and
    /// `TASK-3` assigned in order (no double-allocation of TASK-2).
    /// trace:BUG-82 | ai:claude
    #[test]
    fn merge_gate_reserves_within_run() {
        use crate::models::{Requirement, RequirementType};
        use crate::object_store;

        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().to_path_buf();
        let objects = store.join("objects");
        std::fs::create_dir_all(&objects).unwrap();

        let mut taken = Requirement::new("TASK-1".into(), String::new());
        taken.spec_id = Some("TASK-1".into());
        taken.req_type = RequirementType::Task;
        object_store::write_object(&objects, &taken).unwrap();

        for s in &["TASK-2-001", "TASK-2-002"] {
            let mut r = Requirement::new(s.to_string(), String::new());
            r.spec_id = Some(s.to_string());
            r.req_type = RequirementType::Task;
            object_store::write_object(&objects, &r).unwrap();
        }

        init(&store).unwrap();
        configure_user(&store, "Test", "test@example.com").unwrap();

        let mut assignments = merge_gate(&store).unwrap();
        assignments.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(assignments.len(), 2);
        let agreed: Vec<&str> = assignments.iter().map(|(_, a)| a.as_str()).collect();
        // Both new ids must be free, and neither may equal TASK-1.
        for a in &agreed {
            assert!(*a != "TASK-1");
        }
        // Distinct → no within-run double-allocation.
        assert_ne!(agreed[0], agreed[1]);
    }
}
