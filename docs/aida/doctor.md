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

### Destructive heals require sign-off in autonomous contexts

`--heal` classifies every fix as **safe** (reversible) or **destructive** (hard to reverse — branch/worktree deletion, re-opening a completed spec, removing a stray ancestor instruction file). Safe fixes always proceed; the safe classification is the bouncer.

Destructive fixes are **fail-closed in unattended contexts**. When `--heal --force --yes` runs where no one can make the check-before-delete judgment — a non-interactive shell (piped/CI, no TTY) **or** inside a live `--auto-complete` orchestrator drain — the destructive category is **gated**: it is *not* applied. The heal report records a `skipped` entry that names what was withheld and prints the exact command to apply it under human sign-off:

```bash
aida doctor --heal --force --yes --category <category>
```

run that at an interactive terminal. The interactive path is unchanged: a human typing `--heal --force --yes` at a TTY is the explicit sign-off and the destructive fix proceeds as before.

## Categories

- `stale-leases`: leases whose worktree is gone, creator PID is dead, or branch has merged.
- `abandoned-leases`: dirty worktrees far behind main, usually after an agent lost context.
- `brief-lease-drift`: pending brief exists while a lease for the same spec is active.
- `brief-spec-drift`: pending brief targets a terminal spec.
- `spec-status-drift`: spec status does not match active lease state.
- `orphan-worktrees`: Git worktree exists but no lease references it.
- `orphan-branches`: local work branch is ahead of main with no open PR.
- `stale-reviewer-leases`: reviewer lease points at a PR that already merged.
- `stale-locks`: stale `.aida/cache.db.lock-info` sidecar from a dead cache writer. See [`cache-locks.md`](cache-locks.md) for retry tuning and override guidance.
- `OBE-briefs`: obsolete briefs for completed or rejected specs.
