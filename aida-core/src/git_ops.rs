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

/// Outcome of an auto-merging store-leg pull.
#[derive(Debug, Clone)]
pub enum StorePullOutcome {
    /// `git pull --rebase` succeeded with no conflict.
    Clean,
    /// One or more conflicting spec objects were structurally auto-merged
    /// (history unioned, scalars resolved LWW) and the rebase completed.
    /// Carries a one-line note per resolved spec for the caller to surface.
    AutoMerged { notes: Vec<String> },
}

/// Pull-rebase the orphan store, auto-reconciling conflicting spec YAMLs.
///
/// STORY-641 / MU-204: a plain `pull_rebase` stops on a textual conflict in
/// `objects/TYPE/000/SPEC.yaml` whenever two clones edited the SAME spec —
/// even though the spec is structurally union-mergeable (`history:` is
/// append-only by entry id; scalars resolve via LWW). This wrapper attempts
/// the rebase and, if it conflicts AND every unmerged path is a spec object
/// under `objects/**/*.yaml`, runs the pure `conflict::merge_spec_three_way`
/// resolver per spec, stages the result, and continues the rebase.
///
/// SAFETY: if ANY conflicting path is not a spec object, or any structured
/// parse/merge fails, the rebase is aborted and the function returns an Err —
/// the caller falls back to the manual-conflict path. We never force-resolve
/// unknown files and never corrupt the store. trace:STORY-641 | ai:claude
#[cfg(feature = "native")]
pub fn pull_rebase_auto_merge(repo: &Path, remote: &str, branch: &str) -> Result<StorePullOutcome> {
    let result = git(repo, &["pull", "--rebase", remote, branch])?;
    if result.success {
        return Ok(StorePullOutcome::Clean);
    }

    // The rebase may have paused on a conflict. If we're not actually
    // mid-rebase, this was some other failure — surface it.
    if !rebase_in_progress(repo) {
        anyhow::bail!("git pull --rebase failed: {}", result.stderr);
    }

    let mut notes: Vec<String> = Vec::new();

    // Drive the rebase forward one conflicted step at a time. Each
    // `--continue` may surface a fresh batch of conflicts (one rebased
    // commit at a time), so loop until the rebase is no longer in progress.
    loop {
        let conflicted = unmerged_paths(repo)?;
        if conflicted.is_empty() {
            // No conflicts right now but still mid-rebase — let git advance.
            let cont = git(repo, &["-c", "core.editor=true", "rebase", "--continue"])?;
            if !rebase_in_progress(repo) {
                if cont.success || rebase_completed_ok(repo) {
                    return Ok(if notes.is_empty() {
                        StorePullOutcome::Clean
                    } else {
                        StorePullOutcome::AutoMerged { notes }
                    });
                }
                let _ = git(repo, &["rebase", "--abort"]);
                anyhow::bail!("git rebase --continue failed: {}", cont.stderr);
            }
            continue;
        }

        // Every conflicted path must be a structurally-mergeable, append-only
        // artifact: a spec object (history union + scalar LWW) or the oplog
        // (operations union by id + lamport reconcile). Anything else
        // (registries, agreed_counters, blocks) → bail to the manual path; we
        // never force-resolve files without a known union rule.
        for path in &conflicted {
            if !is_spec_object_path(path) && !is_oplog_path(path) {
                let _ = git(repo, &["rebase", "--abort"]);
                anyhow::bail!(
                    "conflict in non-mergeable path `{path}` — cannot auto-merge; falling back to manual resolution"
                );
            }
        }

        for path in &conflicted {
            let resolved = if is_oplog_path(path) {
                resolve_oplog_conflict(repo, path)
            } else {
                resolve_spec_conflict(repo, path)
            };
            match resolved {
                Ok(note) => {
                    add(repo, &[path])
                        .with_context(|| format!("git add {path} after auto-merge"))?;
                    notes.push(note);
                }
                Err(e) => {
                    let _ = git(repo, &["rebase", "--abort"]);
                    anyhow::bail!("structured merge of `{path}` failed: {e}");
                }
            }
        }

        // Continue the rebase with the resolved objects staged. Force a
        // no-op editor so `--continue` never blocks waiting for a commit
        // message on a TTY-less drain.
        let cont = git(repo, &["-c", "core.editor=true", "rebase", "--continue"])?;
        if !rebase_in_progress(repo) {
            if cont.success || rebase_completed_ok(repo) {
                return Ok(StorePullOutcome::AutoMerged { notes });
            }
            let _ = git(repo, &["rebase", "--abort"]);
            anyhow::bail!("git rebase --continue failed: {}", cont.stderr);
        }
        // else: another conflicted commit ahead — loop and resolve it too.
    }
}

/// True when a rebase is currently in progress in `repo`.
#[cfg(feature = "native")]
fn rebase_in_progress(repo: &Path) -> bool {
    let git_dir = match git(repo, &["rev-parse", "--git-dir"]) {
        Ok(r) if r.success => PathBuf::from(r.stdout),
        _ => return false,
    };
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        repo.join(git_dir)
    };
    git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists()
}

/// True when HEAD has advanced and no rebase is in progress (sanity check
/// after a `--continue` that reported a non-zero status but actually finished).
#[cfg(feature = "native")]
fn rebase_completed_ok(repo: &Path) -> bool {
    !rebase_in_progress(repo)
}

/// List the currently-unmerged (conflicted) paths, relative to the repo root.
#[cfg(feature = "native")]
fn unmerged_paths(repo: &Path) -> Result<Vec<String>> {
    let result = git(repo, &["diff", "--name-only", "--diff-filter=U"])?;
    if !result.success {
        anyhow::bail!("git diff --diff-filter=U failed: {}", result.stderr);
    }
    Ok(result
        .stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// True when `path` looks like a spec object: under `objects/` and `.yaml`.
fn is_spec_object_path(path: &str) -> bool {
    let p = path.replace('\\', "/");
    (p.starts_with("objects/") || p.contains("/objects/")) && p.ends_with(".yaml")
}

/// True when `path` is the store's append-only operation log.
fn is_oplog_path(path: &str) -> bool {
    let p = path.replace('\\', "/");
    p == "oplog.yaml" || p.ends_with("/oplog.yaml")
}

/// Resolve a conflicted `oplog.yaml` by unioning the operation logs. The
/// `OpLog` is append-only and id-keyed, so `OpLog::merge` (dedupe by op id +
/// lamport reconcile + deterministic sort) is the exact union we want. A
/// single `aida edit` writes BOTH the spec object and the oplog, so without
/// this the spec auto-merge alone would still leave the oplog conflicted and
/// stop the rebase. trace:STORY-641 | ai:claude
#[cfg(feature = "native")]
fn resolve_oplog_conflict(repo: &Path, path: &str) -> Result<String> {
    use crate::oplog::OpLog;

    let ours_yaml = git_show_stage(repo, 2, path)?;
    let theirs_yaml = git_show_stage(repo, 3, path)?;

    let parse = |label: &str, yaml: &Option<String>| -> Result<Option<OpLog>> {
        match yaml {
            Some(y) => {
                let log: OpLog = serde_yaml::from_str(y)
                    .with_context(|| format!("parse {label} stage of {path}"))?;
                Ok(Some(log))
            }
            None => Ok(None),
        }
    };

    let ours = parse("ours", &ours_yaml)?;
    let theirs = parse("theirs", &theirs_yaml)?;

    let (mut merged, other) = match (ours, theirs) {
        (Some(o), Some(t)) => (o, t),
        (Some(o), None) => (o.clone(), o),
        (None, Some(t)) => (t.clone(), t),
        (None, None) => anyhow::bail!("both sides of {path} are absent"),
    };
    merged.merge(&other);

    let yaml = serde_yaml::to_string(&merged)
        .with_context(|| format!("serialize merged oplog for {path}"))?;
    let abs = repo.join(path);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    crate::write_atomic(&abs, yaml.as_bytes()).with_context(|| format!("write merged {path}"))?;

    Ok(format!(
        "auto-merged oplog: {} operations after union",
        merged.operations.len()
    ))
}

/// Resolve a single conflicted spec object via three-way structured merge.
/// Reads the base/ours/theirs blobs from the index stages and writes the
/// merged YAML back to the working tree. Returns a one-line note.
#[cfg(feature = "native")]
fn resolve_spec_conflict(repo: &Path, path: &str) -> Result<String> {
    use crate::conflict::merge_spec_three_way;
    use crate::models::Requirement;

    // Stage 1 = merge base, 2 = ours (HEAD/onto), 3 = theirs (the commit
    // being rebased). A stage may be absent (add/add or delete/modify); fall
    // back gracefully so a missing base still merges ours+theirs.
    let base_yaml = git_show_stage(repo, 1, path)?;
    let ours_yaml = git_show_stage(repo, 2, path)?;
    let theirs_yaml = git_show_stage(repo, 3, path)?;

    let parse = |label: &str, yaml: &Option<String>| -> Result<Option<Requirement>> {
        match yaml {
            Some(y) => {
                let req: Requirement = serde_yaml::from_str(y)
                    .with_context(|| format!("parse {label} stage of {path}"))?;
                Ok(Some(req))
            }
            None => Ok(None),
        }
    };

    let ours = parse("ours", &ours_yaml)?;
    let theirs = parse("theirs", &theirs_yaml)?;
    let base = parse("base", &base_yaml)?;

    // Both sides must exist to do a 3-way merge. If one side deleted the
    // file, that's not an append-only history conflict — defer to manual.
    let (ours, theirs) = match (ours, theirs) {
        (Some(o), Some(t)) => (o, t),
        _ => anyhow::bail!("one side deleted {path}; not auto-mergeable"),
    };
    // Anchor the union with the merge base when present; otherwise use ours
    // as a harmless base (ours+theirs union still preserves everything).
    let base = base.unwrap_or_else(|| ours.clone());

    let merged = merge_spec_three_way(&base, &ours, &theirs);

    let yaml = serde_yaml::to_string(&merged)
        .with_context(|| format!("serialize merged requirement for {path}"))?;
    let abs = repo.join(path);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    // Use the atomic writer: the store object dir is a known torn-write race
    // path (TASK-331) and a concurrent reader can otherwise see a half-file.
    crate::write_atomic(&abs, yaml.as_bytes()).with_context(|| format!("write merged {path}"))?;

    let spec_label = merged
        .agreed_id
        .clone()
        .or_else(|| merged.spec_id.clone())
        .unwrap_or_else(|| path.to_string());
    let hist_count = merged.history.len();
    let status = merged.effective_status();
    Ok(format!(
        "auto-merged {spec_label}: unioned {hist_count} history entr{}, status LWW={status}",
        if hist_count == 1 { "y" } else { "ies" }
    ))
}

/// `git show :<stage>:<path>` — returns None when the stage is absent.
#[cfg(feature = "native")]
fn git_show_stage(repo: &Path, stage: u8, path: &str) -> Result<Option<String>> {
    let spec = format!(":{stage}:{path}");
    let result = git(repo, &["show", &spec])?;
    if result.success {
        Ok(Some(result.stdout))
    } else {
        // Missing stage (e.g. add/add) is not an error here.
        Ok(None)
    }
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

/// Get the current HEAD commit SHA (full 40-char form).
///
/// Uses the full SHA, not `--short`: this value is the cache-staleness key
/// (compared against the cache's recorded source HEAD). The abbreviated length
/// git picks for `--short` grows with the repo's object count, so a short SHA
/// can change length over a store's lifetime and read as falsely-stale, and two
/// short prefixes can (rarely) collide and read as falsely-fresh. The full SHA
/// is stable and collision-free. trace:TASK-712
pub fn head_sha(repo: &Path) -> Result<String> {
    let result = git(repo, &["rev-parse", "HEAD"])?;
    if !result.success {
        anyhow::bail!("git rev-parse HEAD failed: {}", result.stderr);
    }
    Ok(result.stdout)
}

/// The kind of change a diff reports for a single object file. Renames are
/// decomposed into Deleted+Added (we pass `--no-renames`), which is exactly the
/// cache mutation we want: drop the old spec_id's row, add the new one.
// trace:BUG-636 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectChange {
    Added,
    Modified,
    Deleted,
}

/// Whether `ancestor` is reachable from (an ancestor commit of) `descendant`.
///
/// Used by the incremental cache update to decide whether the recorded cache
/// HEAD is on the linear/merge history leading to the new HEAD (a normal
/// fast-forward / merge advance — incremental is safe) versus a force-push /
/// rebase of the orphan branch that rewrote history (the recorded HEAD is no
/// longer reachable — the `from..to` diff would be meaningless, so the caller
/// must full-rebuild). A commit is its own ancestor, so identical SHAs return
/// true. Empty SHAs (no recorded HEAD / non-git fixture) return false → the
/// caller falls back to a full rebuild. A bad object or any git error maps to
/// false for the same fail-safe reason.
// trace:BUG-636 | ai:claude
pub fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    if ancestor.is_empty() || descendant.is_empty() {
        return Ok(false);
    }
    // `git merge-base --is-ancestor A B` exits 0 when A is an ancestor of B,
    // 1 when it is not, and 128 on a bad object. Our `git` wrapper only exposes
    // success/failure; non-zero (not-ancestor OR error) both mean "don't trust
    // an incremental diff" → false.
    let result = git(repo, &["merge-base", "--is-ancestor", ancestor, descendant])?;
    Ok(result.success)
}

/// List the spec-object YAML files that changed between `from` and `to`,
/// restricted to the `objects/` tree, as `(change, repo-relative path)` pairs.
///
/// Uses `git diff --no-renames --name-status from to -- objects` so a rename is
/// reported as a Deleted+Added pair (the right cache mutation, since the spec_id
/// — and thus the cache row identity — lives in the filename). Status letters:
/// `A`→Added, `M`/`T`→Modified, `D`→Deleted; `C` (copy, only with `--find-copies`
/// which we do not pass) is treated as Added if it ever appears. Only paths under
/// an `objects/` directory ending in `.yaml` are returned — `metadata.yaml` and
/// `oplog.yaml` (store root) are correctly excluded.
// trace:BUG-636 | ai:claude
pub fn changed_object_files(
    repo: &Path,
    from: &str,
    to: &str,
) -> Result<Vec<(ObjectChange, PathBuf)>> {
    let result = git(
        repo,
        &[
            "diff",
            "--no-renames",
            "--name-status",
            from,
            to,
            "--",
            "objects",
        ],
    )?;
    if !result.success {
        anyhow::bail!(
            "git diff --name-status {}..{} failed: {}",
            from,
            to,
            result.stderr
        );
    }
    let mut changes = Vec::new();
    for line in result.stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Format: "<STATUS>\t<path>" (single-letter status with --no-renames).
        let mut parts = line.splitn(2, '\t');
        let status = parts.next().unwrap_or("").trim();
        let path = match parts.next() {
            Some(p) => p.trim(),
            None => continue,
        };
        if !is_spec_object_path(path) {
            continue;
        }
        let kind = match status.chars().next() {
            Some('A') | Some('C') => ObjectChange::Added,
            Some('M') | Some('T') => ObjectChange::Modified,
            Some('D') => ObjectChange::Deleted,
            // Unknown/unexpected (e.g. 'U' unmerged): caller treats an Err as a
            // full-rebuild fallback, but here we just skip the line rather than
            // mis-classify. An unexpected status is rare on the store branch.
            _ => continue,
        };
        changes.push((kind, PathBuf::from(path)));
    }
    Ok(changes)
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

// ── Worktree warm-pool primitives (STORY-714) ───────────────────────────────
//
// The warm-pool keeps a set of long-lived sibling worktrees and recycles them
// (reset-not-delete) instead of the destroy-and-recreate model. These are the
// git verbs the pool registry (`worktree_pool.rs`) drives; keeping them here
// means the pool never reshells `git worktree` itself and the `--detach` add /
// classified remove live next to AIDA's other managed-worktree primitives.

/// Add a new worktree at `path` with a **detached HEAD** pointing at `ref_`.
/// The pool always hands out detached trees — the worker creates its own
/// branch — so a reused tree never stacks one spec's branch on another's
/// (the structural dissolution of BUG-553).
// trace:STORY-714 trace:BUG-553 | ai:claude
pub fn add_detached_worktree(repo_root: &Path, path: &Path, ref_: &str) -> Result<()> {
    // Drop stale registrations first — same rationale as create_store_worktree
    // (a manually-deleted dir leaves a dangling registration). trace:BUG-39
    let _ = git(repo_root, &["worktree", "prune"]);
    let path_str = path.to_string_lossy();
    let result = git(repo_root, &["worktree", "add", "--detach", &path_str, ref_])?;
    if !result.success {
        anyhow::bail!(
            "failed to add pool worktree at {}: {}",
            path.display(),
            result.stderr
        );
    }
    Ok(())
}

/// Reset a worktree to a clean **detached-HEAD** checkout of `ref_`: force a
/// detached checkout, hard-reset, and clean untracked files. This is the
/// shared "acquire / return" cleanup — running it on every acquire makes the
/// base-reset unconditional (no caller can forget it), dissolving BUG-553.
/// Gitignored paths (`target/`, `.aida/cache.db`, …) survive `clean -fd`, so
/// the compiled cache that makes the pool *warm* is preserved.
// trace:STORY-714 trace:BUG-553 | ai:claude
pub fn reset_worktree_to(worktree_path: &Path, ref_: &str) -> Result<()> {
    let co = git(worktree_path, &["checkout", "--detach", "--force", ref_])?;
    if !co.success {
        anyhow::bail!(
            "failed to detach worktree {} onto {}: {}",
            worktree_path.display(),
            ref_,
            co.stderr
        );
    }
    let reset = git(worktree_path, &["reset", "--hard", ref_])?;
    if !reset.success {
        anyhow::bail!(
            "failed to hard-reset worktree {}: {}",
            worktree_path.display(),
            reset.stderr
        );
    }
    // `clean -fd` removes untracked dirs/files but honors .gitignore, so the
    // warm `target/` is kept. We deliberately do NOT pass -x.
    let _ = git(worktree_path, &["clean", "-fd"]);
    Ok(())
}

/// The repo's default branch name (`main` / `master`), detected from
/// `origin/HEAD` when set, else by which local branch exists, else `"main"`.
// trace:STORY-714 | ai:claude
pub fn default_branch_name(repo_root: &Path) -> String {
    if let Ok(r) = git(
        repo_root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        if r.success {
            if let Some(name) = r.stdout.rsplit('/').next() {
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    for cand in ["main", "master"] {
        let exists = git(repo_root, &["rev-parse", "--verify", "--quiet", cand])
            .map(|x| x.success)
            .unwrap_or(false);
        if exists {
            return cand.to_string();
        }
    }
    "main".to_string()
}

/// Resolve the ref a pool worktree should reset to on acquire/return: the
/// **furthest-ahead default ref**. Compares local `<default>` against
/// `origin/<default>` and picks the one strictly ahead; on divergence, a tie,
/// or origin-ahead it prefers `origin/<default>` (the shared base). A fresh
/// clone has only `origin/<default>`; right after a local merge, local is
/// ahead and wins. Errors only when neither ref resolves.
// trace:STORY-714 trace:BUG-553 | ai:claude
pub fn furthest_ahead_default_ref(repo_root: &Path) -> Result<String> {
    let default = default_branch_name(repo_root);
    let local = default.clone();
    let origin = format!("origin/{default}");

    let local_exists = git(repo_root, &["rev-parse", "--verify", "--quiet", &local])
        .map(|r| r.success)
        .unwrap_or(false);
    let origin_exists = git(repo_root, &["rev-parse", "--verify", "--quiet", &origin])
        .map(|r| r.success)
        .unwrap_or(false);

    match (local_exists, origin_exists) {
        (true, true) => {
            let origin_anc_local = is_ancestor(repo_root, &origin, &local).unwrap_or(false);
            let local_anc_origin = is_ancestor(repo_root, &local, &origin).unwrap_or(false);
            // local strictly ahead of origin → reset onto local; otherwise the
            // shared origin ref is the safe base.
            if origin_anc_local && !local_anc_origin {
                Ok(local)
            } else {
                Ok(origin)
            }
        }
        (true, false) => Ok(local),
        (false, true) => Ok(origin),
        (false, false) => {
            anyhow::bail!(
                "no default branch ({local} / {origin}) found to reset pool worktree onto"
            )
        }
    }
}

/// True when a worktree has uncommitted changes (tracked or untracked,
/// gitignored paths excluded). Used to classify a tree as dirty before a
/// reset/return/destroy so unlanded work is never silently discarded.
// trace:STORY-714 | ai:claude
pub fn worktree_is_dirty(worktree_path: &Path) -> bool {
    git(worktree_path, &["status", "--porcelain"])
        .map(|r| !r.stdout.is_empty())
        .unwrap_or(false)
}

/// True when the worktree's current HEAD is already an ancestor of `base_ref`
/// — its commits have landed, so the tree is safe to reset or destroy.
// trace:STORY-714 | ai:claude
pub fn worktree_head_is_merged(worktree_path: &Path, base_ref: &str) -> bool {
    is_ancestor(worktree_path, "HEAD", base_ref).unwrap_or(false)
}

/// The worktree's current HEAD SHA, or None if it can't be resolved.
// trace:STORY-714 | ai:claude
pub fn worktree_head_sha(worktree_path: &Path) -> Option<String> {
    git(worktree_path, &["rev-parse", "HEAD"])
        .ok()
        .filter(|r| r.success)
        .map(|r| r.stdout)
}

/// Remove a worktree by absolute path. `force` adds `--force` (discards a
/// dirty tree). The pool's `destroy` path calls this only after classifying
/// and salvaging; `return` never calls it (the directory persists).
// trace:STORY-714 trace:TASK-0396 | ai:claude
pub fn remove_worktree_at(repo_root: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let path_str = worktree_path.to_string_lossy();
    let mut args: Vec<&str> = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_str);
    let result = git(repo_root, &args)?;
    if !result.success {
        anyhow::bail!(
            "failed to remove worktree {}: {}",
            worktree_path.display(),
            result.stderr
        );
    }
    let _ = git(repo_root, &["worktree", "prune"]);
    Ok(())
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

/// True when the remote has NO branch heads yet — i.e. a brand-new/empty
/// remote, the "push-to-create" state. Implemented via `git ls-remote --heads
/// <remote>`: an empty stdout (with a successful exit) means no heads. Returns
/// false on any git error (offline, unreachable) so callers treat
/// "can't tell" as "not known-empty" and don't take empty-origin-only paths.
/// trace:TASK-844 | ai:claude
pub fn remote_has_no_heads(repo: &Path, remote: &str) -> bool {
    match git(repo, &["ls-remote", "--heads", remote]) {
        Ok(r) if r.success => r.stdout.trim().is_empty(),
        _ => false,
    }
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

/// Resolve `<remote>/<branch>`'s head SHA via a single `git ls-remote` query,
/// WITHOUT fetching any objects. One cheap network round-trip (refs only, no
/// object transfer) — the building block for "is my local store already current
/// with the remote?" decisions that can skip a full `pull --rebase`.
///
/// Returns `Some(sha)` on success, `None` on any git error (offline,
/// unreachable remote, branch absent) so callers can treat "can't tell" as
/// "fall back to the heavier path".
// trace:TASK-857 | ai:claude
pub fn remote_branch_head_sha(repo: &Path, remote: &str, branch: &str) -> Option<String> {
    let r = git(
        repo,
        &["ls-remote", remote, &format!("refs/heads/{}", branch)],
    )
    .ok()?;
    if !r.success {
        return None;
    }
    // Output line is `<sha>\t<ref>`; take the first whitespace-delimited token.
    let sha = r.stdout.split_whitespace().next()?.to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
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

/// True when a local branch with this exact name exists.
/// trace:BUG-559 | ai:claude
pub fn local_branch_exists(repo: &Path, branch: &str) -> bool {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .map(|r| r.success)
    .unwrap_or(false)
}

/// Check out an existing branch (`git checkout <branch>`). Fails if the branch
/// doesn't exist or the working tree can't be switched.
/// trace:BUG-559 | ai:claude
pub fn checkout_branch(repo: &Path, branch: &str) -> Result<()> {
    let result = git(repo, &["checkout", branch])?;
    if !result.success {
        anyhow::bail!("git checkout {} failed: {}", branch, result.stderr);
    }
    Ok(())
}

/// Detach HEAD at the current commit (`git checkout --detach`). Frees whatever
/// branch was checked out so a fetch can write into that branch ref. Used by
/// the fresh-clone auto-attach recovery when `aida-store` is the checked-out
/// branch (the GitLab default-branch quirk) and no code branch is available to
/// switch to. trace:BUG-559 | ai:claude
pub fn detach_head(repo: &Path) -> Result<()> {
    let result = git(repo, &["checkout", "--detach"])?;
    if !result.success {
        anyhow::bail!("git checkout --detach failed: {}", result.stderr);
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

/// Identity captured at node registration time for the STORY-652 friendly
/// name + owner fields. `user` is the registering shell's `current_user_id()`
/// string (threaded down from aida-cli rather than read from the environment
/// here, so aida-core stays free of CLI identity resolution); `name` is the
/// caller-provided friendly name, or None to let the registry compute the
/// `<host>-<user>-<seq>` default. trace:STORY-652 | ai:claude
#[derive(Debug, Clone, Default)]
pub struct NodeIdentity {
    /// Caller-provided friendly node name (None → computed default).
    pub name: Option<String>,
    /// The owner's `current_user_id()` string captured at registration.
    pub user: Option<String>,
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
    register_node_full_identity(
        aida_repo,
        requested_id,
        user_id,
        hostname,
        email,
        NodeIdentity::default(),
    )
}

/// As [`register_node_full`] but also stamps the STORY-652 friendly `name` and
/// owner `user` string onto both the shared registry entry (nodes.toml) and the
/// local `.aida/node.toml`. trace:STORY-652 | ai:claude
pub fn register_node_full_identity(
    aida_repo: &Path,
    requested_id: Option<String>,
    user_id: u32,
    hostname: &str,
    email: Option<String>,
    identity: NodeIdentity,
) -> Result<String> {
    use crate::node::{default_node_name, BlockRegistry, NodeConfig, NodeRegistry};

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
        // STORY-652: compute the friendly name once so both nodes.toml and the
        // local node.toml agree. Owner string prefers the threaded `user`,
        // falling back to the email local-part / integer for the name slug.
        let owner_for_name = identity.user.clone().unwrap_or_else(|| {
            email
                .as_deref()
                .and_then(|e| e.split_once('@').map(|(l, _)| l.to_string()))
                .unwrap_or_else(|| user_id.to_string())
        });
        let node_name = identity
            .name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| default_node_name(hostname, &owner_for_name, &node_id));
        registry.register_specific_full_named(
            node_id.clone(),
            user_id,
            hostname.to_string(),
            email.clone(),
            clone_path,
            Some(node_name.clone()),
            identity.user.clone(),
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
                name: Some(node_name.clone()),
                user: identity.user.clone(),
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
                    name: Some(node_name.clone()),
                    user: identity.user.clone(),
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

/// Which identity field on a node registry entry a backfill writes.
// trace:STORY-654 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeIdentityField {
    /// The owner `$USER` string (`current_user_id`) — the person key the team
    /// roster joins on.
    Owner,
    /// The friendly node name (`<host>-<user>-<seq>`).
    Name,
}

/// Backfill a single identity field (owner `user` or friendly `name`) onto an
/// existing node entry in the shared registry (`registry/nodes.toml`), then
/// push — mirroring the [`register_node_full`] CAS push-wins loop. When `id`
/// matches the running clone's `.aida/node.toml`, the same field is updated on
/// the local node config too, so the legacy clone becomes identity-coherent.
///
/// Errors clearly if `id` is absent from the registry. Solo (no `origin`)
/// writes locally and lets the next `aida push` upload.
// trace:STORY-654 | ai:claude
pub fn set_node_identity_field(
    aida_repo: &Path,
    id: &str,
    field: NodeIdentityField,
    value: &str,
) -> Result<()> {
    use crate::node::{NodeConfig, NodeRegistry};

    let registry_path = aida_repo.join("registry").join("nodes.toml");
    let branch = current_branch(aida_repo).unwrap_or_else(|_| "main".to_string());
    let local_only = !has_remote(aida_repo, "origin");

    // Apply the local node.toml update once, after the registry write succeeds.
    // Returns Ok(true) when the running clone's node.toml was the target.
    let apply_local = |id: &str| -> Result<bool> {
        let node_config_path = aida_repo.join(".aida").join("node.toml");
        if !node_config_path.exists() {
            return Ok(false);
        }
        let mut config = NodeConfig::load(&node_config_path)?;
        if config.node_id != id {
            return Ok(false);
        }
        match field {
            NodeIdentityField::Owner => config.user = Some(value.to_string()),
            NodeIdentityField::Name => config.name = Some(value.to_string()),
        }
        config.save(&node_config_path)?;
        Ok(true)
    };

    for attempt in 0..MAX_CAS_RETRIES {
        // Step 1: pull latest (skip first attempt / solo).
        if attempt > 0 && !local_only {
            if let Err(e) = pull_rebase(aida_repo, "origin", &branch) {
                anyhow::bail!(
                    "Cannot update node identity: remote unreachable after {} attempts. Error: {}",
                    attempt,
                    e
                );
            }
        }

        // Step 2: load → find the entry (error if absent) → set the field.
        let mut registry = NodeRegistry::load(&registry_path).unwrap_or_default();
        let entry = registry
            .nodes
            .iter_mut()
            .find(|n| n.id.as_str() == id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Node id {} is not in the shared registry (registry/nodes.toml) — \
                     nothing to update. Run `aida node list` to see registered nodes.",
                    id
                )
            })?;
        match field {
            NodeIdentityField::Owner => entry.user = Some(value.to_string()),
            NodeIdentityField::Name => entry.name = Some(value.to_string()),
        }
        registry.save(&registry_path)?;

        // Step 3: stage + commit.
        add(aida_repo, &["registry/nodes.toml"])?;
        let field_label = match field {
            NodeIdentityField::Owner => "owner",
            NodeIdentityField::Name => "name",
        };
        let msg = format!(
            "chore(registry): set node {} {} = {}",
            id, field_label, value
        );
        commit(aida_repo, &msg)?;

        // Step 4: push (or stop here when solo).
        if local_only {
            apply_local(id)?;
            return Ok(());
        }
        match push(aida_repo, "origin", &branch) {
            Ok(true) => {
                apply_local(id)?;
                return Ok(());
            }
            Ok(false) => {
                // Push rejected — discard our stale commit + tree so the next
                // pull --rebase applies cleanly. trace:BUG-1-069
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(aida_repo)
                    .output();
                eprintln!(
                    "Node identity update: push rejected (attempt {}), retrying...",
                    attempt + 1
                );
                continue;
            }
            Err(e) => anyhow::bail!("Node identity update failed: {}", e),
        }
    }

    anyhow::bail!(
        "Node identity update failed after {} attempts — too much contention on the registry",
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
        // STORY-652: the node now belongs to a new clone — recompute the
        // friendly name from the new host/owner so the roster reflects the
        // new owner rather than the prior clone's name. trace:STORY-652
        let recomputed_name = entry.display_name();
        entry.name = Some(recomputed_name.clone());
        let recomputed_user = entry.user.clone();
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
                    name: Some(recomputed_name.clone()),
                    user: recomputed_user.clone(),
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
        // trace:TASK-712 — head_sha returns the FULL 40-char SHA (no --short),
        // so the cache-staleness key is abbreviation-length-stable.
        assert_eq!(sha.len(), 40, "head_sha must be the full SHA, got {sha:?}");
        assert!(sha.bytes().all(|b| b.is_ascii_hexdigit()));

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

    // TASK-857: `remote_branch_head_sha` is the lighter `aida add` network
    // floor — it resolves the remote branch head via `ls-remote` (no object
    // transfer) so the add path can SKIP a full `pull --rebase` when the local
    // store is already current. Verifies the three decision outcomes:
    //   * fresh after push → remote head == local HEAD (skip-the-pull case)
    //   * after a local-only commit → remote head != local HEAD (must-pull case)
    //   * unreachable remote → None (offline-tolerant case)
    // trace:TASK-857 | ai:claude
    #[test]
    fn test_remote_branch_head_sha() {
        let dir = tempfile::tempdir().unwrap();
        let (_bare, work, branch) = setup_remote_and_clone(dir.path(), "head-sha-test");

        // Just-pushed: the remote branch head must equal the local HEAD SHA,
        // and must be the full 40-char SHA (the comparison key the add path
        // uses). This is the steady-state "already current → skip pull" case.
        let local = head_sha(&work).unwrap();
        let remote = remote_branch_head_sha(&work, "origin", &branch)
            .expect("ls-remote should resolve the just-pushed branch head");
        assert_eq!(remote.len(), 40, "remote head SHA must be full length");
        assert_eq!(
            remote, local,
            "remote head must match local HEAD after push"
        );

        // Commit locally WITHOUT pushing → local HEAD advances, remote stays
        // put → the SHAs diverge, signalling the add path to do the heavy pull.
        std::fs::write(work.join("local-only.txt"), "ahead").unwrap();
        add(&work, &["local-only.txt"]).unwrap();
        commit(&work, "local only commit").unwrap();
        let local2 = head_sha(&work).unwrap();
        let remote2 = remote_branch_head_sha(&work, "origin", &branch).unwrap();
        assert_ne!(local2, remote2, "local HEAD advanced; remote unchanged");
        assert_eq!(
            remote2, remote,
            "remote head is unchanged until we actually push"
        );

        // Unreachable remote → None, so the add path treats it as "can't tell"
        // and degrades to a local-only file rather than erroring offline.
        let missing = dir.path().join("does-not-exist.git");
        git(&work, &["remote", "add", "void", missing.to_str().unwrap()]).unwrap();
        assert!(
            remote_branch_head_sha(&work, "void", &branch).is_none(),
            "an unreachable remote must yield None, not an error"
        );
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

    /// STORY-654: `set_node_identity_field(Owner)` backfills the owner on an
    /// existing registry entry AND, when the id is the current node, the local
    /// `.aida/node.toml`. trace:STORY-654 | ai:claude
    #[test]
    fn test_set_node_owner_updates_registry_and_current_local() {
        use crate::node::{NodeConfig, NodeRegistry};
        let dir = tempfile::tempdir().unwrap();
        let (_bare, aida, _branch) = setup_remote_and_clone(dir.path(), "aida");

        // Register node "1" for this clone (writes registry + local node.toml).
        register_node(&aida, 1, "test-laptop").unwrap();
        let registry_path = aida.join("registry/nodes.toml");
        let node_config_path = aida.join(".aida/node.toml");
        // Pre: no owner string yet (register_node doesn't set one).
        assert!(NodeRegistry::load(&registry_path)
            .unwrap()
            .get("1")
            .unwrap()
            .user
            .is_none());

        set_node_identity_field(&aida, "1", NodeIdentityField::Owner, "joe").unwrap();

        // Registry entry now carries the owner.
        let entry = NodeRegistry::load(&registry_path).unwrap();
        assert_eq!(entry.get("1").unwrap().user.as_deref(), Some("joe"));
        // And so does the local node.toml (id "1" IS the current node).
        assert_eq!(
            NodeConfig::load(&node_config_path).unwrap().user.as_deref(),
            Some("joe")
        );
    }

    /// STORY-654: `set_node_identity_field(Name)` backfills the friendly name;
    /// and a non-current id leaves the local node.toml's name untouched.
    /// trace:STORY-654 | ai:claude
    #[test]
    fn test_set_node_name_registry_and_noncurrent_skips_local() {
        use crate::node::{NodeConfig, NodeRegistry};
        let dir = tempfile::tempdir().unwrap();
        let (_bare, aida, _branch) = setup_remote_and_clone(dir.path(), "aida");

        // This clone is node "1"; backfill a SEPARATE legacy node "2".
        register_node(&aida, 1, "test-laptop").unwrap();
        register_node_remote_only(&aida, "2".into(), 2, "spock", "spock@example.com".into())
            .unwrap();
        let registry_path = aida.join("registry/nodes.toml");
        let node_config_path = aida.join(".aida/node.toml");
        let local_name_before = NodeConfig::load(&node_config_path).unwrap().name;

        set_node_identity_field(&aida, "2", NodeIdentityField::Name, "spock-box").unwrap();

        // Node "2" entry carries the name.
        let registry = NodeRegistry::load(&registry_path).unwrap();
        assert_eq!(
            registry.get("2").unwrap().name.as_deref(),
            Some("spock-box")
        );
        // Current clone's node.toml (id "1") is untouched.
        assert_eq!(
            NodeConfig::load(&node_config_path).unwrap().name,
            local_name_before
        );
    }

    /// STORY-654: setting a field on an absent id errors clearly.
    /// trace:STORY-654 | ai:claude
    #[test]
    fn test_set_node_owner_absent_id_errors() {
        let dir = tempfile::tempdir().unwrap();
        let (_bare, aida, _branch) = setup_remote_and_clone(dir.path(), "aida");
        register_node(&aida, 1, "test-laptop").unwrap();

        let err = set_node_identity_field(&aida, "99", NodeIdentityField::Owner, "ghost")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not in the shared registry"), "got: {err}");
    }

    // -----------------------------------------------------------------
    // TASK-889: distributed ID-allocation stress tests — the second
    // core distributed-correctness surface. The catastrophic failure
    // mode is an id COLLISION: two distinct specs claiming the same
    // canonical id across clones. These tests lock down the headline
    // invariant — N nodes minting concurrently never collide — through
    // the *real* git CAS push loop, not just the in-process locks the
    // existing dispenser.rs / node.rs tests cover.
    // trace:TASK-889 | ai:claude
    // -----------------------------------------------------------------

    /// Replicate the production block-claim CAS loop (mirrors
    /// `auto_allocate_block_inner` in aida-cli): load blocks.yaml →
    /// claim a range above the current floor → save → commit → push;
    /// on push-rejection hard-reset + pull --rebase and retry with a
    /// freshly-recomputed range. Returns the claimed (start, end).
    ///
    /// This is the single point on which cross-node id uniqueness rests:
    /// only one node's push for a given range can win; the loser recomputes
    /// `next_range_start` (= max(range_end)+1) after pulling the winner's
    /// commit, so the ranges never overlap. trace:TASK-889 | ai:claude
    fn cas_claim_block(
        clone: &Path,
        branch: &str,
        node_id: &str,
        type_prefix: &str,
        size: u32,
    ) -> Result<(u32, u32)> {
        use crate::node::BlockRegistry;
        let blocks_path = clone.join("registry").join("blocks.yaml");
        for attempt in 0..MAX_CAS_RETRIES {
            if attempt > 0 {
                pull_rebase(clone, "origin", branch)?;
            }
            let mut registry = BlockRegistry::load(&blocks_path)?;
            let block = registry.claim_block(
                node_id.to_string(),
                format!("{node_id}@host"),
                "host".into(),
                type_prefix.to_string(),
                size,
            );
            registry.save(&blocks_path)?;
            add(clone, &["registry/blocks.yaml"])?;
            commit(
                clone,
                &format!(
                    "chore(registry): node {node_id} claim {type_prefix}-{}..{}",
                    block.range_start, block.range_end
                ),
            )?;
            match push(clone, "origin", branch)? {
                true => return Ok((block.range_start, block.range_end)),
                false => {
                    // Push lost the race — discard our stale claim commit so
                    // the next pull --rebase has a clean tree to recompute on.
                    let _ = git(clone, &["reset", "--hard", "HEAD~1"]);
                    continue;
                }
            }
        }
        anyhow::bail!("block claim CAS exhausted retries");
    }

    /// SCENARIO 2 (the catastrophic case) + SCENARIO 3 (exhaustion/refill):
    /// three clones (distinct node ids) each claim several blocks of the
    /// same type concurrently against one shared bare remote, then dispense
    /// every id from every block. The invariant: (a) no two claimed ranges
    /// overlap across nodes, and (b) every dispensed canonical id is unique.
    /// A range-overlap or a replayed counter would surface as a duplicate.
    /// trace:TASK-889 | ai:claude
    #[test]
    fn concurrent_multinode_block_claims_never_overlap_or_collide() {
        use crate::node::BlockRegistry;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let dpath = dir.path().to_path_buf();

        // One shared bare remote that all clones push the orphan store to.
        let bare = dpath.join("store.git");
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "--bare"]).unwrap();

        // Seed the store branch with an empty blocks.yaml so every clone
        // starts from the same committed baseline.
        let seed = dpath.join("seed");
        init(&seed).unwrap();
        configure_user(&seed, "Seed", "seed@example.com").unwrap();
        git(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();
        std::fs::create_dir_all(seed.join("registry")).unwrap();
        BlockRegistry::default()
            .save(&seed.join("registry").join("blocks.yaml"))
            .unwrap();
        add(&seed, &["registry/blocks.yaml"]).unwrap();
        commit(&seed, "seed blocks").unwrap();
        let branch = current_branch(&seed).unwrap();
        git(&seed, &["push", "-u", "origin", &branch]).unwrap();

        const NODES: usize = 3;
        const BLOCKS_PER_NODE: usize = 4;
        const BLOCK_SIZE: u32 = 50;

        // Each node = its own clone of the shared remote.
        let branch = Arc::new(branch);
        let bare = Arc::new(bare);
        let handles: Vec<_> = (1..=NODES)
            .map(|n| {
                let dpath = dpath.clone();
                let branch = Arc::clone(&branch);
                let bare = Arc::clone(&bare);
                std::thread::spawn(move || {
                    let node_id = n.to_string();
                    let clone = dpath.join(format!("node{n}"));
                    git(
                        &dpath,
                        &["clone", bare.to_str().unwrap(), clone.to_str().unwrap()],
                    )
                    .unwrap();
                    configure_user(&clone, &format!("n{n}"), &format!("n{n}@e.com")).unwrap();
                    let mut ranges = Vec::new();
                    for _ in 0..BLOCKS_PER_NODE {
                        let r =
                            cas_claim_block(&clone, &branch, &node_id, "FR", BLOCK_SIZE).unwrap();
                        ranges.push((node_id.clone(), r));
                    }
                    ranges
                })
            })
            .collect();

        let all_ranges: Vec<(String, (u32, u32))> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();

        // (a) No two claimed ranges overlap — regardless of owning node.
        let mut ranges: Vec<(u32, u32)> = all_ranges.iter().map(|(_, r)| *r).collect();
        ranges.sort();
        for w in ranges.windows(2) {
            assert!(
                w[0].1 < w[1].0,
                "block ranges overlap across nodes: {:?} then {:?} — \
                 cross-node id collision is possible",
                w[0],
                w[1]
            );
        }
        assert_eq!(ranges.len(), NODES * BLOCKS_PER_NODE);

        // (b) Dispense EVERY id from EVERY claimed range (scenario 3:
        // exhaust each block) and assert global uniqueness. Each range maps
        // 1:1 onto canonical ids `FR-<start>..FR-<end>`; an overlap or an
        // off-by-one at a block boundary would show up as a duplicate or a
        // gap here.
        let mut all_ids: Vec<String> = Vec::new();
        for (_node, (start, end)) in &all_ranges {
            assert_eq!(
                end - start + 1,
                BLOCK_SIZE,
                "block boundary off-by-one: range {start}..{end} is not size {BLOCK_SIZE}"
            );
            for seq in *start..=*end {
                all_ids.push(format!("FR-{seq}"));
            }
        }
        let total = all_ids.len();
        assert_eq!(total, NODES * BLOCKS_PER_NODE * BLOCK_SIZE as usize);
        all_ids.sort();
        all_ids.dedup();
        assert_eq!(
            all_ids.len(),
            total,
            "DUPLICATE canonical id minted across {NODES} concurrent nodes"
        );

        // The remote's final blocks.yaml must agree: all ranges present,
        // contiguous from 1, none lost to a CAS reset.
        let verify = dpath.join("verify");
        git(
            &dpath,
            &["clone", bare.to_str().unwrap(), verify.to_str().unwrap()],
        )
        .unwrap();
        let final_reg = BlockRegistry::load(&verify.join("registry").join("blocks.yaml")).unwrap();
        assert_eq!(
            final_reg.blocks.len(),
            NODES * BLOCKS_PER_NODE,
            "a block claim was lost during the concurrent CAS race"
        );
        // Highest range_end == total id space, i.e. fully contiguous packing.
        let max_end = final_reg.blocks.iter().map(|b| b.range_end).max().unwrap();
        assert_eq!(max_end, (NODES * BLOCKS_PER_NODE) as u32 * BLOCK_SIZE);
    }

    /// SCENARIO 5: interleaved stale-clone claim. Clone A claims and pushes;
    /// clone B — which has NOT pulled A's claim — then claims. B's first push
    /// must be rejected (it computed a range overlapping A's), and the CAS
    /// loop must recover by pulling + recomputing a non-overlapping range.
    /// Proves a stale node cannot mint a colliding range. trace:TASK-889
    #[test]
    fn stale_clone_claim_cannot_overlap_after_cas_recovery() {
        use crate::node::BlockRegistry;

        let dir = tempfile::tempdir().unwrap();
        let dpath = dir.path().to_path_buf();
        let bare = dpath.join("store.git");
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "--bare"]).unwrap();

        // Seed.
        let seed = dpath.join("seed");
        init(&seed).unwrap();
        configure_user(&seed, "Seed", "seed@example.com").unwrap();
        git(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();
        std::fs::create_dir_all(seed.join("registry")).unwrap();
        BlockRegistry::default()
            .save(&seed.join("registry").join("blocks.yaml"))
            .unwrap();
        add(&seed, &["registry/blocks.yaml"]).unwrap();
        commit(&seed, "seed").unwrap();
        let branch = current_branch(&seed).unwrap();
        git(&seed, &["push", "-u", "origin", &branch]).unwrap();

        // Two clones, both at the empty baseline (both stale w.r.t each other).
        let a = dpath.join("a");
        let b = dpath.join("b");
        git(
            &dpath,
            &["clone", bare.to_str().unwrap(), a.to_str().unwrap()],
        )
        .unwrap();
        git(
            &dpath,
            &["clone", bare.to_str().unwrap(), b.to_str().unwrap()],
        )
        .unwrap();
        configure_user(&a, "A", "a@e.com").unwrap();
        configure_user(&b, "B", "b@e.com").unwrap();

        // A claims FR-1..50 and pushes (wins outright).
        let ra = cas_claim_block(&a, &branch, "1", "FR", 50).unwrap();
        assert_eq!(ra, (1, 50));

        // B is still at the empty baseline. Its CAS loop will FIRST compute
        // FR-1..50 (collision!), have its push rejected, then pull A's claim
        // and recompute FR-51..100 — a non-overlapping range.
        let rb = cas_claim_block(&b, &branch, "2", "FR", 50).unwrap();
        assert!(
            rb.0 > ra.1,
            "stale clone B minted an OVERLAPPING range {rb:?} after A's {ra:?}"
        );
        assert_eq!(rb, (51, 100));
    }

    /// SCENARIO 4: merge-gate short-id assignment is collision-free AND
    /// deterministic. Seed many node-aware specs across mixed types, run the
    /// gate, and assert every assigned agreed-id is unique within its type
    /// and densely packed from 1 (FR-1..FR-k, BUG-1..BUG-m, …). A second run
    /// is a no-op. trace:TASK-889 | ai:claude
    #[test]
    fn merge_gate_assigns_unique_dense_short_ids_across_many_specs() {
        use crate::models::{Requirement, RequirementType};
        use crate::object_store;
        use std::collections::HashMap;

        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().to_path_buf();
        let objects = store.join("objects");
        std::fs::create_dir_all(&objects).unwrap();

        // 40 specs each of FR / BUG / TASK with node-aware spec_ids spread
        // across several "nodes" (3, 7, 12) — exactly the post-distributed-
        // minting state the gate must normalize into agreed short-ids.
        let types = [
            (RequirementType::Functional, "FR"),
            (RequirementType::Bug, "BUG"),
            (RequirementType::Task, "TASK"),
        ];
        let mut expected_per_type: HashMap<&str, usize> = HashMap::new();
        for (rt, prefix) in &types {
            for (i, node) in [3u32, 7, 12].iter().enumerate() {
                for seq in 1..=13u32 {
                    let spec_id = format!("{prefix}-{node}-{seq:03}");
                    let mut r = Requirement::new(spec_id.clone(), String::new());
                    r.spec_id = Some(spec_id);
                    r.req_type = rt.clone();
                    object_store::write_object(&objects, &r).unwrap();
                }
                let _ = i;
            }
            expected_per_type.insert(prefix, 3 * 13);
        }

        init(&store).unwrap();
        configure_user(&store, "Test", "test@example.com").unwrap();

        let mut assignments = merge_gate(&store).unwrap();
        assignments.sort_by(|a, b| a.0.cmp(&b.0));

        // Group assigned agreed-ids by type prefix.
        let mut by_prefix: HashMap<String, Vec<u32>> = HashMap::new();
        for (_origin, agreed) in &assignments {
            let (prefix, seq) = agreed.rsplit_once('-').unwrap();
            by_prefix
                .entry(prefix.to_string())
                .or_default()
                .push(seq.parse().unwrap());
        }

        for (prefix, expected_count) in &expected_per_type {
            let mut seqs = by_prefix.get(*prefix).cloned().unwrap_or_default();
            assert_eq!(
                seqs.len(),
                *expected_count,
                "{prefix}: wrong number of agreed-ids assigned"
            );
            seqs.sort();
            // Unique …
            let mut deduped = seqs.clone();
            deduped.dedup();
            assert_eq!(
                deduped.len(),
                seqs.len(),
                "{prefix}: merge-gate assigned a DUPLICATE agreed short-id"
            );
            // … and densely packed from 1 (no off-by-one / no gaps).
            assert_eq!(
                seqs,
                (1..=*expected_count as u32).collect::<Vec<_>>(),
                "{prefix}: agreed-ids not contiguous 1..={expected_count}"
            );
        }

        // Idempotent: re-running gate over the now-assigned store is a no-op.
        let second = merge_gate(&store).unwrap();
        assert!(second.is_empty(), "second merge-gate run must be a no-op");
    }

    /// SCENARIO 6 (edge): two clones accidentally sharing a node id still do
    /// NOT collide on ids, because uniqueness comes from non-overlapping
    /// block RANGES (CAS-serialized), not from node-id distinctness. Two
    /// clones both calling themselves node "1" each get a distinct range, so
    /// their dispensed ids never clash even though attribution is ambiguous.
    /// This documents that the range-allocation invariant is robust to a
    /// duplicate-node-id misconfiguration (the BUG-89-adjacent hazard).
    /// trace:TASK-889 | ai:claude
    #[test]
    fn duplicate_node_id_still_yields_disjoint_ranges() {
        use crate::node::BlockRegistry;

        let dir = tempfile::tempdir().unwrap();
        let dpath = dir.path().to_path_buf();
        let bare = dpath.join("store.git");
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "--bare"]).unwrap();

        let seed = dpath.join("seed");
        init(&seed).unwrap();
        configure_user(&seed, "Seed", "seed@example.com").unwrap();
        git(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();
        std::fs::create_dir_all(seed.join("registry")).unwrap();
        BlockRegistry::default()
            .save(&seed.join("registry").join("blocks.yaml"))
            .unwrap();
        add(&seed, &["registry/blocks.yaml"]).unwrap();
        commit(&seed, "seed").unwrap();
        let branch = current_branch(&seed).unwrap();
        git(&seed, &["push", "-u", "origin", &branch]).unwrap();

        let a = dpath.join("a");
        let b = dpath.join("b");
        git(
            &dpath,
            &["clone", bare.to_str().unwrap(), a.to_str().unwrap()],
        )
        .unwrap();
        git(
            &dpath,
            &["clone", bare.to_str().unwrap(), b.to_str().unwrap()],
        )
        .unwrap();
        configure_user(&a, "A", "a@e.com").unwrap();
        configure_user(&b, "B", "b@e.com").unwrap();

        // BOTH clones (mis)identify as node "1".
        let ra = cas_claim_block(&a, &branch, "1", "FR", 30).unwrap();
        let rb = cas_claim_block(&b, &branch, "1", "FR", 30).unwrap();

        // Ranges are still disjoint — the CAS loop on range_end+1 protects
        // uniqueness even when node ids collide.
        assert!(
            ra.1 < rb.0 || rb.1 < ra.0,
            "duplicate node id produced OVERLAPPING ranges: {ra:?} vs {rb:?}"
        );
    }
}
