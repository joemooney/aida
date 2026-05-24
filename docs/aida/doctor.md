# AIDA Doctor

`aida doctor` is the operator-facing cleanup command for multi-agent state drift. Run it when AIDA's coordination state looks inconsistent after interrupted drains, abandoned worktrees, stale leases, or obsolete briefs.

## Common Commands

```bash
aida doctor
aida doctor --json
aida doctor --heal
aida doctor --heal --yes
aida doctor check stale-leases
aida doctor heal OBE-briefs --yes
```

The default command is read-only. Healing prompts per category unless `--yes` is supplied.

## Safety Model

Doctor is salvage-first. Before any heal removes a worktree, it saves uncommitted work to:

```text
.aida/salvage/<spec-id>-<agent>-attempt-<timestamp>.patch
```

Branch deletion is not part of normal safe healing. It requires both `--yes` and `--force`.

## Categories

- `stale-leases`: leases whose worktree is gone, creator PID is dead, or branch has merged.
- `abandoned-leases`: dirty worktrees far behind main, usually after an agent lost context.
- `brief-lease-drift`: pending brief exists while a lease for the same spec is active.
- `brief-spec-drift`: pending brief targets a terminal spec.
- `spec-status-drift`: spec status does not match active lease state.
- `orphan-worktrees`: Git worktree exists but no lease references it.
- `orphan-branches`: local work branch is ahead of main with no open PR.
- `stale-reviewer-leases`: reviewer lease points at a PR that already merged.
- `OBE-briefs`: obsolete briefs for completed or rejected specs.
