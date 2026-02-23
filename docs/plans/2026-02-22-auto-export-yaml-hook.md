# Plan: Auto-export requirements.yaml via pre-commit hook

**Date**: 2026-02-22

## Context

`requirements.db` (SQLite binary) is tracked in git, causing binary bloat and undiffable history. Solution: auto-export to `requirements.yaml` (text, diffable) before each commit via a git pre-commit hook, then stop tracking the binary.

## Steps

### 1. Create `.git/hooks/pre-commit`

Bash script that:
- Checks if `requirements.db` exists
- Runs `aida db migrate --from sqlite --to yaml --output requirements.yaml --force`
- Stages `requirements.yaml` with `git add requirements.yaml`
- Skips gracefully with a warning if `aida` binary is not available
- Checks both `.db` and `.db-wal` timestamps (SQLite WAL mode writes to the WAL file, not the main DB)

### 2. Update `.gitignore`

- **Remove** the `requirements.yaml` line (un-ignore it so it gets tracked)
- Keep `*.requirements.yaml`, `requirements.db*`, etc. ignored

### 3. Initial export and untrack binary

- Run `aida db migrate --from sqlite --to yaml --output requirements.yaml --force`
- Run `git rm --cached requirements.db` to stop tracking the binary
- Commit: new `requirements.yaml` + updated `.gitignore` + untracked `.db`

## Files

| File | Action |
|------|--------|
| `.git/hooks/pre-commit` | **Create** — hook script |
| `.gitignore` | **Modify** — un-ignore `requirements.yaml` |
| `requirements.yaml` | **Generated** — initial export from DB |
| `requirements.db` | **Untrack** — `git rm --cached` |

## Verification

1. Make a small change via `aida edit`
2. `git commit` — hook auto-exports and stages `requirements.yaml`
3. `git diff --cached requirements.yaml` — shows only the actual change
4. `git status` — `requirements.db` no longer tracked

## Related Requirements

- General infrastructure/tooling improvement for git-friendly requirements tracking

## Status

Completed
