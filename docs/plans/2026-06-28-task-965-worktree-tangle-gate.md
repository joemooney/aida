# TASK-965 — Worktree-tangle gate (substrate-as-bouncer)

- **Date:** 2026-06-28
- **Specs:** TASK-965 (child of EPIC-56)
- **Status:** Implemented
- **Complexity:** Small (guard addition, merge-on-green)

## Approach

AIDA's fan-out incident: a bare agent `git checkout -b`s in the MAIN repo, leaving
the primary checkout parked on a feature branch, so the next
`git merge --ff-only origin/main` aborts. We defended it with a memory RULE; the
substrate-as-bouncer principle says ship a GATE. firstmate (kunchenguid) does
exactly this — refuse to launch a crewmate unless the resolved worktree differs
from the primary, and alarm when the primary strands on a feature branch.

Two guards, both backed by pure predicates:

1. **Spawn-time assertion** (`assert_no_worktree_tangle`) — wired into both fan-out
   launch paths (`agent_new_with_config` foreground + `agent_new_bg_dispatch`
   background). A spec-scoped launch whose resolved worktree equals the primary
   checkout root is refused with a clear error. The no-spec launch (which
   deliberately runs in the primary, no worktree resolved) is exempt.
2. **Stranded-primary alarm** (`detect_stranded_primary` +
   `print_stranded_primary_banner`) — `aida status` (fast path) and `aida ps` emit
   a loud banner ABOVE their output when the primary checkout is on a non-default
   branch with in-flight leases. Network-free (git symbolic-ref + lease dir scan),
   so the fast-status no-network/no-full-load contract holds.

## Critical files / symbols

- `aida-cli/src/main.rs`
  - `fn is_worktree_tangle` — pure root-equality predicate (canonicalize + compare).
  - `fn assert_no_worktree_tangle` — the spawn gate (bails on tangle).
  - `fn primary_stranded_on_feature_branch` — pure stranded-detection predicate.
  - `fn local_default_branch_name` — network-free default-branch NAME probe (reuses
    `detect_default_branch_ref`, strips `origin/`).
  - `fn detect_stranded_primary` / `struct StrandedPrimary` / `fn print_stranded_primary_banner`.
  - Call sites: both `prepare_agent_launch` launches; the fast `aida status` block in
    `handle_status_command_distributed`; the `handle_ps` non-json output.

## Tests (`mod task965_worktree_tangle_tests`)

- `worktree_tangle_true_when_launch_cwd_equals_primary`
- `worktree_tangle_false_for_distinct_worktree` (incl. nested-under-primary)
- `stranded_true_on_feature_branch_with_leases`
- `stranded_false_on_clean_default_branch` (no false alarm; case-insensitive)
- `stranded_false_when_no_leases_in_flight`
- `stranded_false_when_branch_or_default_undetectable` (detached HEAD / None)

## Verification

- `cargo build -p aida-cli` — green
- `cargo fmt --all -- --check` — clean
- `cargo clippy -p aida-cli -- -D clippy::correctness` — clean
- `bash scripts/glyph-lint.sh --block` — OK
- `env -u AIDA_SESSION_ROLE cargo test -p aida-cli` — 3094 passed
- `aida ps` on a primary parked on `main` — no false alarm (confirmed)

## Followups

- A future enhancement could count only LIVE leases (process-probe) to suppress the
  alarm when all leases are stale; today any lease-on-record + feature-branch
  primary alarms, which is the conservative/loud choice.
