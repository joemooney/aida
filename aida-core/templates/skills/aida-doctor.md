---
name: aida-doctor
description: Diagnose and heal multi-agent AIDA state drift: stale leases, obsolete briefs, orphan worktrees, orphan branches, and spec/lease mismatches. Use when multi-agent coordination looks inconsistent or after an interrupted drain/recovery.
disable-model-invocation: true
---

# /aida-doctor

Use this skill when the operator suspects AIDA's coordination substrate has drifted: leases left behind after an agent exits, pending briefs for shipped specs, worktrees with no lease, branches with no PR, stale cache lock-info sidecars, or specs whose status no longer matches active work.

## Workflow

1. Run the read-only diagnostic first:

   ```bash
   aida doctor
   ```

2. If the operator wants a narrower view, focus one category:

   ```bash
   aida doctor check stale-leases
   aida doctor check OBE-briefs
   aida doctor check stale-locks
   ```

3. Heal safe categories only after reading the report:

   ```bash
   aida doctor --heal
   ```

4. Use batch mode only when the operator explicitly asks for cleanup without prompts:

   ```bash
   aida doctor --heal --yes
   ```

5. Branch deletion is intentionally stronger:

   ```bash
   aida doctor heal orphan-branches --yes --force
   ```

## Safety Rules

- Never remove a worktree manually before checking whether `aida doctor` saved a salvage patch.
- Any doctor heal that removes a worktree must first write `.aida/salvage/<spec-id>-<agent>-attempt-<timestamp>.patch` when uncommitted work exists.
- Treat diagnostic-only categories as routing advice, not permission to improvise destructive cleanup.
- If the report says branch deletion is manual, ask the operator before deleting anything.

## PR-state divergence (not automated by `aida doctor`)

`aida doctor` covers lease / worktree / branch / spec-status / lock drift. It does **not** inspect pull-request state — so when a session looks stuck on the PR side, diagnose that surface by hand:

```bash
gh pr list --state open
gh pr view --json state,statusCheckRollup,mergeable,commits,headRefName
```

- **Open PR with a stale base** (behind `main`, conflicts) → `aida pr rebase <N>` (rebases in a temp worktree and force-pushes-with-lease).
- **Green CI but unmerged** (stuck on approval / ceiling rules) → `aida pr ship <N>` (watch CI → squash-merge → pull), or `gh pr merge <N> --squash`.
- **Unpushed local branch** (commits exist, no remote) → `git push -u origin HEAD`.

After a merge lands, if the spec is still `Done`, run `aida db reconcile-status --since <ref>` to replay the `Done → Completed` auto-bump the pull may have missed.

## JSON Mode

For advisor scripting or automation:

```bash
aida doctor --json
aida doctor --heal --yes --json
```

JSON output includes `findings` and, when healing, `healed` action records.
