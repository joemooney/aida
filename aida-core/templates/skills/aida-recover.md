---
name: aida-recover
description: Scrapes session, orchestrator, worktree, PR, and spec graphs to detect state divergence and walks the operator through a detailed diagnostic playbook. Always run when a session is stuck, a PR is unmerged despite passing checks, or orchestrator queues/leases get out of sync.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Edit
---

# AIDA Recovery & Diagnostic Playbook Skill

## Purpose

Walk the operator or AI advisor through a structured diagnostic battery to detect and repair state divergences between:
1. **Session Leases** (stale, misregistered, or orphaned leases)
2. **Git Worktrees** (wrong branch checkouts, missing/dangling paths, deleted branches)
3. **GitHub Pull Requests** (stale bases, green but unmerged PRs, unpushed branches)
4. **AIDA Spec Graph** (Done/Completed state mismatch, missed auto-bumps)
5. **Orchestrator/Queue** (stale drain locks, queued items for completed specs)

---

## When to Use

Invoke `/aida-recover` when:
- "My PR is merged but AIDA still says the spec is In Progress / Done."
- "AIDA says a directory is already locked/leased but no one is working there."
- "Claude keeps modifying the main worktree on a feature branch."
- "The orchestrator or queue is stuck and won't accept new work."
- You observe dual-compilation errors, stale git-stores, or lost synchronization between sessions.

---

## Diagnostics Playbook

Run the following step-by-step diagnostic queries, evaluate the symptoms, and execute the matching recovery recommendations.

---

### Surface 1: Lease State Divergence

#### 1. Detection Command
```bash
# List all active session leases
aida session leases 2>/dev/null || cargo run --bin aida -- session leases

# Inspect the raw TOML lease files
ls -la .aida/sessions/*.toml 2>/dev/null
```

#### 2. Symptoms & Diagnosis
* **Stale Lease**: A lease exists, but the process ID listed in its `creator_pid` field is dead.
  * *Diagnosis test*: Run `kill -0 <creator_pid>` or `ps -p <creator_pid>`. If the process is dead, the lease is stale.
* **Misregistered Lease**: Sibling worktree branch is active, but the lease's `worktree_path` is erroneously set to the parent repository `/home/joe/ai/aida`.
  * *Diagnosis test*: Look for `worktree_path = "/home/joe/ai/aida"` in leases that cover active feature specs.
* **Orphan Lease**: Sibling worktree path has been deleted, but the lease `.toml` file remains behind.
  * *Diagnosis test*: `[ ! -d "$worktree_path" ]` for a listed lease path.

#### 3. Recommendation & Recovery
> [!IMPORTANT]
> Await operator confirmation before deleting lease files or closing active sessions.

* **For Stale/Orphan Leases**:
  Run `aida session end <id>` to clean it up. If that fails or warns:
  ```bash
  rm -f .aida/sessions/<id>.toml
  ```
* **For Misregistered Leases**:
  Terminate the incorrect lease, and start a fresh session inside the correct worktree:
  ```bash
  aida session end <id>
  # In the correct sibling worktree directory:
  aida session start --owns <scope> --branch <branch> --reuse-branch
  ```

---

### Surface 2: Git Worktree State Divergence

#### 1. Detection Command
```bash
# List all registered git worktrees
git worktree list

# List local branch tracking status
git branch -vv
```

#### 2. Symptoms & Diagnosis
* **Main Worktree on Feature Branch**: The main parent worktree (`/home/joe/ai/aida`) has checked out a feature branch (e.g. `story-316` or `task-479`) instead of `main` or `master`.
  * *Diagnosis*: Leads to co-mingled commits and forces peer sessions to wait.
* **Missing-but-Registered Worktree**: `git worktree list` lists a sibling path, but the directory has been deleted manually on disk without running `git worktree remove`.
* **Registered-but-Missing Worktree**: A worktree directory exists on disk but is not registered in `git worktree list`, or lacks a corresponding active session lease.

#### 3. Recommendation & Recovery
* **For Main Worktree on wrong branch**:
  ```bash
  cd /home/joe/ai/aida
  git checkout main
  ```
* **For Missing-but-Registered Worktrees**:
  Clean up git's internal metadata for deleted worktrees:
  ```bash
  git worktree prune
  ```
* **For Registered-but-Missing Worktree Directories**:
  Prune or clean up manually deleted workspaces:
  ```bash
  git worktree remove --force <path>
  ```

---

### Surface 3: Pull Request State Divergence

#### 1. Detection Command
```bash
# List active open pull requests
gh pr list --state open

# Check detailed status of the active branch PR
gh pr view --json state,statusCheckRollup,mergeable,commits,headRefName
```

#### 2. Symptoms & Diagnosis
* **Open PR with Stale Base**: The PR is out of sync with `main`. AIDA or reviewers will refuse to merge.
  * *Diagnosis*: `gh pr view` reports merge conflicts or behind commits.
* **PR with Green CI but Unmerged**: All CI checks are green (passed), but the PR remains open (stuck on reviewer approval or automated ceiling rules).
* **PR with No Remote Branch**: Local commits have been created but not pushed to the upstream repository.

#### 3. Recommendation & Recovery
* **For Open PR with Stale Base**:
  Fetch and rebase the feature branch against the current `main` branch:
  ```bash
  aida pr rebase 2>/dev/null || (git fetch origin main && git rebase origin/main && git push -f origin HEAD)
  ```
* **For Green CI but Unmerged PR**:
  Re-trigger the shipping automation or perform a clean squash-merge:
  ```bash
  aida pr ship
  # Or manual fallback:
  gh pr merge --squash --delete-branch
  ```
* **For Unpushed Local Branch**:
  Set the upstream and push:
  ```bash
  git push -u origin HEAD
  ```

---

### Surface 4: AIDA Spec State Divergence

#### 1. Detection Command
```bash
# Check status of the specific spec in AIDA database
aida show <spec-id>

# Reconcile status from git logs
aida db reconcile-status --dry-run
```

#### 2. Symptoms & Diagnosis
* **Done/Completed State Mismatch**: The PR has already landed on `main`, but the spec is still listed as `draft`, `approved`, or `in_progress`.
  * *Diagnosis*: Squash commit was landed but missed the AIDA trailing-parens bump scanner (e.g. `(STORY-316)` was missing from the commit header).
* **Spec Stuck in Approved with Active Commits**: Commits already exist on `main` referencing the spec, but its database status remains `approved`.

#### 3. Recommendation & Recovery
* **For Done/Completed State Mismatch**:
  Run AIDA's built-in reconciliation engine:
  ```bash
  aida db reconcile-status
  ```
  If that does not resolve the state, manually advance the requirement:
  ```bash
  aida db set-status <spec-id> completed
  # or
  aida db set-status <spec-id> released
  ```

---

### Surface 5: Orchestrator & Queue State Divergence

#### 1. Detection Command
```bash
# List items in AIDA queue
aida queue list

# Inspect lock and exit sentinel files
ls -la .aida/sessions/*.exit-requested 2>/dev/null
```

#### 2. Symptoms & Diagnosis
* **Stale Exit Sentinel / Lock File**: Active lock or exit-requested files exist, blocking new worker drains from launching.
  * *Diagnosis*: The worker drain exited, but didn't clean up its sentinel files.
* **Queue Directives referencing Shipped Specs**: The queue contains directives (`worker.cmd`) that reference specs that have already been shipped and completed.

#### 3. Recommendation & Recovery
* **For Stale Sentinel Files**:
  Remove the sentinel to release the worker block:
  ```bash
  rm -f .aida/sessions/*.exit-requested
  rm -f .aida/lock
  ```
* **For Stuck Queue Directives**:
  Prune or clean up the task queue:
  ```bash
  aida queue prune
  ```
