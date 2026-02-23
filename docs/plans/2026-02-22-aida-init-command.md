# Plan: Bootstrapping Improvements — `aida init` Command

## Context

AIDA's CLAUDE.md and docs reference `aida init`, but this command didn't exist. The actual command was `aida scaffold apply`, which is opaque and gives minimal post-apply feedback. Additionally, `determine_requirements_path()` only checked for `requirements.yaml`, not `requirements.db`, so a fresh SQLite-based project couldn't be auto-detected by subsequent CLI commands.

## Changes Made

### 1. `aida-core/src/project.rs` — Detect `requirements.db`
- Added `requirements.db` detection **before** the existing `requirements.yaml` check in `determine_requirements_path()`
- Updated error message to suggest `aida init` instead of referencing `requirements.yaml`

### 2. `aida-cli/src/cli.rs` — Added `Init` command variant
- `Init { no_skills, no_hooks, force }` variant in the `Command` enum

### 3. `aida-cli/src/main.rs` — Init handler
- Early dispatch before `determine_requirements_path()` (since DB doesn't exist yet)
- `handle_init_command()` function that:
  - Checks for existing `requirements.db` (idempotent without `--force`)
  - Creates SQLite database and seeds META requirements
  - Creates `docs/plans/` directory
  - Runs scaffold with configurable `ScaffoldConfig`
  - Prints colored post-init guidance
- Fixed migration check to skip when path is already `.db` (avoids false "Both YAML and SQLite exist" warning)

## Flags
- `--no-skills`: Skips `.claude/skills/` and `.claude/commands/`
- `--no-hooks`: Skips `.claude/hooks/` and git hooks
- `--force`: Overwrites existing files

## Related Requirements
- TASK-0001 (bootstrapping)

## Status
Completed
