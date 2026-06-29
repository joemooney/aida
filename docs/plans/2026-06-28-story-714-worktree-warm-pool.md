# Plan: Worktree warm-pool — return-to-pool reset instead of destroy-and-recreate

Date: 2026-06-28
Specs: STORY-714, EPIC-56, TASK-0396, BUG-553
Status: Slice 1 implemented (pool primitives + CLI surface + `session end --return`); lifecycle wiring + default-flip remain as followups
Complexity: ~900 prod LOC, ~500 test LOC, ~6 commits, risk medium

## Implementation note (slice 1, 2026-06-28)

Shipped: the core pool primitives (`aida-core/src/worktree_pool.rs`,
`worktree_pool_destroy.rs`, `worktree_hooks.rs`), the git verbs in
`git_ops.rs` (`add_detached_worktree`, `reset_worktree_to`,
`furthest_ahead_default_ref`, `worktree_is_dirty`, `worktree_head_is_merged`,
`remove_worktree_at`), the `aida worktree pool {status,acquire,return,destroy}`
CLI surface, and the `aida session end --return` wiring. 22 new core tests
(pure logic + git-fixture integration) plus the help-leak regression.

**Slice boundary.** The DoD recipe references `aida session end --return`, but
exercising that end-to-end needs a session lease whose worktree is a pool tree
— i.e. `aida agent new` / `aida queue work` / the orchestrator must *acquire
from the pool on start*, which is the next slice. Until then the directly-
testable surface is `aida worktree pool acquire/return/status/destroy`; the
Verification below was run with `aida worktree pool return <path>` in place of
`session end --return` and all four checks pass. Remaining (see Followups):
acquire-on-start integration, replacing the `run_compete_arm` /
`heal_doctor_orphan_worktree` `--force` removals with the tiered `destroy`, and
the opt-in→default flip once the pool is proven by dogfood.

<!--
  DESIGN SKETCH for operator / master-advisor sign-off BEFORE the build.
  This is architectural (changes the worktree lifecycle that every fan-out
  implementer, `aida agent new`, and the orchestrator depend on), so it is
  sketched first per AIDA discipline (sketch-first sign-off is empirically
  cheaper than implement-then-revise).

  Prefer SYMBOL refs over LINE refs. trace:TASK-92
  Prior art studied: treehouse (github.com/kunchenguid/treehouse),
  internal/pool/{pool,state,destroy,prune}.go + internal/git/git.go +
  internal/hooks/hooks.go.
-->

## Approach

Today AIDA treats a per-spec worktree as **disposable**: `aida queue work` / `aida agent new` / the orchestrator create a sibling worktree (`../aida-<slug>`), the implementer works in it, and `aida session end` (and the doctor heal paths) **`git worktree remove --force`** it. Treehouse's insight is that the worktree is the *expensive, warm* thing — its `target/` is a compiled cache — and the branch is the cheap, throwaway thing. So treehouse keeps a fixed **pool** of worktrees per repo and, on hand-back, **resets the worktree to a clean detached-HEAD base instead of deleting it** (`acquire` / `return`). The directory persists; only its contents are reset.

This plan ports that model to AIDA as a **warm-pool** backed by a registry under `.aida/worktree-pool/`. `acquire` prefers an idle pooled worktree (reset-not-create), creating a new one only up to a cap; `return` (`aida session end --return`) resets-and-marks-idle instead of removing; **every acquire does a detached-HEAD hard-reset to the furthest-ahead default ref** before handing the tree out; durable **reservations** (a lease that survives with zero live processes) are kept distinct from **PID-liveness**; and the only path that actually deletes a directory is a **tiered, dry-run-by-default `destroy`** with one `--include-*` opt-in flag per risk class, carrying `post_create` / `pre_destroy` hooks. The `pre_destroy` hook is where the TASK-0396 `cargo clean` finally runs — at the one moment a worktree's `target/.fingerprint` paths are about to dangle.

The payoff is two whole bug classes *dissolved* (not patched): they exist only because the current model deletes-and-recreates, and the warm-pool removes the delete and standardizes the recreate.

### Diagram

```
                    ┌──────────────── .aida/worktree-pool/ (registry + flock) ────────────────┐
                    │  pool.json: [ {name, path, owner_pid, owner_started_at,                  │
                    │                leased, lease_holder, leased_at, destroying} ... ]        │
                    └────────────────────────────────────────────────────────────────────────┘
   acquire ─────────────────────────────────────────────────────────────►
     prefer IDLE (clean ∧ unleased ∧ owner-dead ∧ not-dirty)
        └─ found? ── reset --hard <furthest-ahead default> + clean -fd ── detached HEAD ── hand out (warm target/)
        └─ none?  ── create up to max_trees ── git worktree add --detach ── post_create hook ── hand out
   work in worktree (branch created by the worker, not the pool) ...
   return (`aida session end --return`) ───────────────────────────────►
        reset --hard <furthest-ahead default> + clean -fd ── mark IDLE ── DIRECTORY PERSISTS (cache stays warm)
   destroy (`aida worktree pool destroy`, dry-run default) ────────────►
        classify each: disposable | dirty | unmerged | in-use | leased | unverified
        remove only disposable unless --include-{unlanded,in-use,leased}
        pre_destroy hook (cargo clean -p <members>)  ← TASK-0396 swept HERE, the one place a tree is deleted
```

## Decisions

- **Pool registry lives under `.aida/worktree-pool/` (per-clone runtime state).** **Rationale**: `.gitignore` is already deny-by-default for `.aida/*` (BUG-73), so a new `pool.json` + lock file need **zero** gitignore change and never get committed. Mirrors treehouse's `~/.treehouse/<repo>/treehouse-state.json`, but kept *inside* the repo's `.aida/` (consistent with `.aida/cache.db`, `.aida/agent-briefs/`) rather than a `~/.aida/` global, because a pool is per-repository.
- **Worktree directories stay siblings of the project root** (`../aida-pool-<n>/`), not nested under `.aida/`. **Rationale**: AIDA's existing worktrees are siblings (`../aida-<slug>`); nesting a worktree *inside* `.aida/` (itself gitignored) invites confusing self-reference and breaks the `target/`-sibling layout cargo already depends on. The registry records the path; the directory location is unchanged from today's convention.
- **Reservation (lease) is a distinct field from PID-liveness.** **Rationale**: treehouse's `Leased`/`LeaseHolder`/`LeasedAt` survive with zero processes inside the tree, whereas `OwnerPID`/`OwnerStartedAt` self-heal when the owner dies (`ownerAlive`, `healState`). AIDA already has *both* notions split across `aida session leases` (durable) and `aida ps` PID-liveness (`agent_registry`). The pool unifies them onto one entry so a headless drain that parks `NeedsAttention` keeps its tree reserved even though no process is live, while a crashed implementer's tree self-heals back to idle.
- **`acquire` always detached-HEAD hard-resets to the furthest-ahead default ref before handing out.** **Rationale**: this is the structural dissolution of BUG-553 — the reset is unconditional and lives in the *acquire* primitive, so no caller can "reuse a worktree and forget to base-reset." Furthest-ahead resolution (treehouse `branchRef`: local-vs-`origin/main`, prefer the strictly-ahead one, prefer origin on divergence) means an acquire right after a local merge resets to local `main`, and a fresh clone resets to `origin/main`.
- **`return` resets but does not delete; only `destroy` deletes.** **Rationale**: keeping the directory is the structural dissolution of TASK-0396 — a tree that is never removed never leaves a dangling `target/.fingerprint` absolute path behind. The single delete path (`destroy`) runs the `cargo clean` `pre_destroy` hook, so even deliberate teardown can't poison siblings.
- **Destroy is dry-run-by-default with per-risk-class opt-in flags.** **Rationale**: directly ports treehouse `DestroyOptions.missingFlags` — `--include-unlanded` (dirty/unmerged/unverified), `--include-in-use` (live owner/process), `--include-leased` (durable lease, and only when the exact path is named, never in a bulk sweep). Replaces today's blunt `git worktree remove --force` (in `run_compete_arm`, `heal_doctor_orphan_worktree`) with a classified, salvage-aware removal. Reuses AIDA's existing `salvage_worktree_patch` before any unlanded removal.
- **Migration is opt-in then default-flip.** **Rationale**: `aida session end` keeps removing by default in the first slices; `--return` opts into the pool. Once the pool is proven on this repo's own dogfood, the default flips (`--remove` becomes the opt-out). Avoids a big-bang lifecycle change to the autonomy keystone.
- **No thin scaffold shipped in this PR.** **Rationale**: this is a sketch-first sign-off artifact; the design touches the autonomy keystone's worktree lifecycle, so the operator/master should bless the *shape* before any Rust lands. The PR is a pure plan doc — fast CI, easy review, nothing to revert if the design changes.

## Files (in build-order)

### `aida-core/src/worktree_pool.rs` (new) — the pool registry + primitives

- `struct PoolEntry`: `name: String`, `path: PathBuf`, `created_at`, `owner_pid: Option<i32>`, `owner_started_at: Option<i64>`, `leased: bool`, `lease_holder: Option<String>`, `leased_at: Option<...>`, `destroying: bool`. Serde with `skip_serializing_if` so pre-pool/empty fields round-trip (mirrors treehouse `WorktreeEntry` json tags).
- `struct Pool { entries: Vec<PoolEntry> }` + `read_state` / `write_state` / `with_state_lock` (advisory file lock on `.aida/worktree-pool/pool.lock`; port of treehouse `WithStateLock` flock).
- `fn heal_state`: drop entries whose path is gone; clear `owner_*` when the owner pid is dead (port of `healState` + `ownerAlive`).
- `fn acquire(opts) -> PoolPath`: prefer idle (clean ∧ unleased ∧ owner-dead ∧ not-dirty ∧ not-destroying) → `reset_worktree`; else create up to `max_trees`; stamp owner-reservation or durable lease (`markAcquired`/`reserveOwner`).
- `fn return_to_pool(path)`: `reset_worktree` + clear owner/lease + mark idle (port of `Release`).
- `fn reset_worktree(path, ref)`: `checkout --detach --force <ref>` + `reset --hard <ref>` + `clean -fd` (port of git `ResetWorktree`).
- `fn furthest_ahead_default_ref(repo_root) -> String`: local-`main`-vs-`origin/main`, prefer strictly-ahead, prefer origin on divergence (port of git `branchRef`).
- `fn list(pool_dir) -> Vec<PoolStatus>`: classify each entry available/in-use/leased/dirty/here (port of `List`).

### `aida-core/src/worktree_pool_destroy.rs` (new) — tiered dry-run teardown

- `enum DestroyClass { Disposable, Dirty, Unmerged, InUse, Leased, Unverified }` (port of treehouse `DestroyClass`).
- `fn classify_for_destroy(entry, default_ref) -> DestroyTarget`: leased? live? backing-repo-missing? dirty? merged-into-default? (port of `classifyForDestroy`, reuse AIDA `git_ops::is_head_merged` if present else add).
- `struct DestroyOptions { dry_run, include_unlanded, include_in_use, include_leased, pre_destroy }` + `fn missing_flags` (port of `DestroyOptions.missingFlags`; `--include-leased` honored only on a named single path, never bulk `--all`).
- `fn destroy_one` / `fn destroy_pool`: two-phase reserve→hook→remove with re-check (port of `executeDestroy`); call `salvage_worktree_patch` before any unlanded removal.

### `aida-core/src/worktree_hooks.rs` (new) — lifecycle hooks

- `fn run_hooks(commands, work_dir)`: sequential shell, failures logged-not-fatal (port of `hooks.Run`). `post_create` (after a fresh `git worktree add`) and `pre_destroy` (before each delete). Config keys under `[worktree_pool]` in `.aida/config.toml`; executable hooks honored only from the machine-global config (treehouse's repo-config-can't-run-hooks safety stance).

### `aida-cli/src/cli.rs` — surface the pool

- `aida worktree pool status` (read-only list), `aida worktree pool destroy [--all|<path>] [--dry-run default] [--include-*]`, `--return` flag on `aida session end`.

### `aida-cli/src/session.rs` + `aida-cli/src/main.rs` — wire acquire/return into the lifecycle

- Worktree-creation sites (`aida agent new`, queue-work, orchestrator) call `worktree_pool::acquire` instead of a bare `git worktree add`.
- `aida session end --return` calls `return_to_pool` instead of `git worktree remove`.
- Replace the `--force` removals in `run_compete_arm` and `heal_doctor_orphan_worktree` with the tiered `destroy_one`.

### `aida-core/src/git_ops.rs` — extend the managed-worktree primitives

- Extend `create_store_worktree` / `remove_store_worktree` patterns with a `--detach` add + a classified remove the pool can call (rather than the pool reshelling `git worktree` itself).

### `aida-core/templates/docs/aida/discipline/` + `docs/session-lifecycle.md` — doctrine

- Update the TASK-0396 "Gotcha: cargo incremental cache" section: warm-pool dissolves it; `cargo clean` now runs as the `pre_destroy` hook on the one delete path.
- Update the BUG-553 reset-before-each-spec rule: the acquire primitive base-resets, so the manual rule becomes a structural guarantee.

## Critical Files

- `aida-core/src/worktree_pool.rs` (new)
- `aida-core/src/worktree_pool_destroy.rs` (new)
- `aida-core/src/worktree_hooks.rs` (new)
- `aida-cli/src/cli.rs`
- `aida-cli/src/session.rs`
- `aida-cli/src/main.rs`
- `aida-core/src/git_ops.rs`
- `docs/session-lifecycle.md`
- `aida-core/templates/docs/aida/discipline/` (machinery-glossary, session-discipline)

## Reusable helpers (do not reimplement)

- `git_ops::create_store_worktree`, `git_ops::remove_store_worktree`, `git_ops::has_worktree` (`aida-core/src/git_ops.rs`) — AIDA's existing managed-worktree primitives; the pool's `git worktree add/remove` shells should route through these, extended with `--detach`.
- `salvage_worktree_patch` (`aida-cli/src/main.rs`) — already saves a patch of dirty worktree state before removal; call it in `destroy` before any `--include-unlanded` deletion (reuse, don't reinvent the salvage path).
- `partition_by_project` (`aida-cli/src/claude_agents.rs`) — lease-worktree-vs-elsewhere scoping; reuse to decide which sibling dirs the pool owns.
- `agent_registry` liveness (`covers`, `classify_status`, live-agents-covering-cwd in `aida-cli/src/agent_registry.rs`) — existing PID-liveness source; feed `owner_alive` rather than re-detecting processes.
- `aida ps` / `aida session leases` plumbing — existing durable-reservation vs PID-liveness split; the pool entry consolidates these two onto one record.
- `temp_worktree_path` (`aida-cli/src/pr_rebase.rs`) — existing sibling-worktree path-naming convention to keep pool dirs consistent.
- treehouse `internal/pool/*.go` + `internal/git/git.go` (cloned under scratchpad) — direct line-for-line reference for `acquire`/`Release`/`healState`/`branchRef`/`DestroyOptions`/`hooks.Run`.

## Risks + gotchas

1. **Risk: migration from the destroy-recreate model.** Existing sibling worktrees + live leases predate the registry; a flag-flip could orphan them. **Mitigation**: `acquire`/`return` are opt-in (`--return`) for the first slices; ship `aida worktree pool adopt` to register pre-existing sibling worktrees into `pool.json` before the default flips; `heal_state` tolerates a missing/empty registry (decodes to empty, today's behavior).
2. **Risk: concurrent-acquire races.** Parallel fan-out implementers can race for the same idle tree. **Mitigation**: every state mutation runs under `with_state_lock` (advisory flock on `pool.lock`), exactly as treehouse serializes with `WithStateLock`; the reservation is stamped *inside* the lock before the path is returned.
3. **Risk: the cargo cross-worktree fingerprint subtlety (TASK-0396) is more subtle than "don't delete."** Even with a persistent pool, a `reset --hard` + `clean -fd` to a *different* default base can leave `target/.fingerprint` referencing source that the new base doesn't have — a soft replay of the same poison within one reused tree. **Mitigation**: the reset is to the furthest-ahead default (same code the next build compiles against), so the fingerprints match the checked-out source; and the `pre_destroy` `cargo clean -p <members>` hook is the belt-and-suspenders for the genuine teardown path. Validate empirically (Verification below) that a reset-reuse does **not** reproduce the non-exhaustive-match symptom.
4. **Risk: reservation leak.** A durable lease (`leased=true`, no live pid) that is never released pins a tree out of the pool forever. **Mitigation**: `--include-leased` destroy on a *named* path can reclaim it; `aida worktree pool status` surfaces the holder + age; a future TTL on `leased_at` can auto-expire (out of scope for slice 1).
5. **Risk: hook execution is a code-exec surface.** `pre_destroy = ["rm -rf ..."]` from a checked-in repo config is dangerous. **Mitigation**: port treehouse's stance — executable hooks honored only from machine-global config (`~/.aida/`), never repo-level `.aida/config.toml`; document the split.
6. **Risk: `max_trees` cap stalls a wide fan-out.** A burndown wider than the cap blocks on "all worktrees in use." **Mitigation**: default cap generous (e.g. 16, treehouse's default); clear error naming the cap + the knob; fan-out already self-limits by agent budget.

## Tests (named, not "add tests")

- `acquire_prefers_idle_over_create` — an idle clean entry is reset and reused, not a new `worktree add`.
- `acquire_resets_to_furthest_ahead_default` — after a local merge, acquire resets to local `main`; on a fresh clone, to `origin/main`.
- `acquire_base_reset_dissolves_branch_stacking` — sequential acquire→commit→return→acquire leaves the second tree at the base, not stacked on the first branch (BUG-553 regression).
- `return_resets_and_keeps_directory` — `return_to_pool` leaves the dir on disk and marks idle (no `worktree remove`).
- `heal_state_clears_dead_owner` — an entry whose `owner_pid` is dead self-heals to idle; a `leased` entry with no pid stays reserved.
- `destroy_dry_run_removes_nothing` — default is preview-only.
- `destroy_skips_dirty_without_include_unlanded` — and names the missing flag.
- `destroy_leased_only_by_named_path` — bulk `--all` never removes a leased tree; named path + `--include-leased` does.
- `pre_destroy_hook_runs_cargo_clean_before_remove` — the TASK-0396 sweep fires on the delete path (hook invoked, then remove).
- `concurrent_acquire_serializes_under_lock` — two acquires don't hand out the same idle path.

## Verification

```bash
# Build the pool primitives + drive acquire/return/destroy on a throwaway repo.
TMP=$(mktemp -d); cd "$TMP" && git init -b main && aida init
AIDA_BIN="$(git -C /home/joe/ai/aida rev-parse --show-toplevel)/target/debug/aida"

# 1. acquire reuses an idle tree (warm), does not recreate
P1=$("$AIDA_BIN" worktree pool acquire --json | jq -r .path)
"$AIDA_BIN" worktree pool status | grep -q "$P1"
# work + return
( cd "$P1" && git checkout -b feat-a && echo x > a && git add -A && git commit -m "a" )
"$AIDA_BIN" session end --return
"$AIDA_BIN" worktree pool status | grep -qi available     # directory persists, idle

# 2. BUG-553 dissolution: next acquire base-resets, no branch stacking
P2=$("$AIDA_BIN" worktree pool acquire --json | jq -r .path)
test "$P2" = "$P1"                                          # same warm tree reused
( cd "$P2" && git rev-parse --abbrev-ref HEAD | grep -qi HEAD )   # detached, reset to base
( cd "$P2" && git log --oneline | grep -qv "feat-a" )      # prior branch's commit not stacked

# 3. TASK-0396 dissolution: tree never deleted on return → no dangling fingerprint
# (negative: a sibling cargo build after a return does NOT cite a removed path)

# 4. tiered destroy is dry-run by default and refuses unlanded without the flag
"$AIDA_BIN" worktree pool destroy --all | grep -qi "dry-run"
( cd "$P2" && echo dirty > d.txt )
"$AIDA_BIN" worktree pool destroy --all --no-dry-run | grep -qi -- "--include-unlanded"
```

**Worktree-aware binary path** (TASK-388): the recipe resolves `AIDA_BIN` via `git rev-parse --show-toplevel` of the AIDA repo, never bare `target/debug/aida`.

## Followups

- TTL auto-expiry on `leased_at` (reservation-leak mitigation, risk 4).
- `aida worktree pool adopt` for migrating pre-pool sibling worktrees (risk 1).
- `post_create` hook recipe to pre-warm `cargo build` on a freshly-created pool tree (turn cold-create into warm-on-first-use).
- Cross-platform file-lock parity (treehouse has `lock_unix.go` / `lock_windows.go`); AIDA must match for the nightly cross-platform matrix.
- Telemetry: pool hit-rate (reuse vs create) to prove the warm-cache payoff empirically (substrate learning loop).

## Related

- STORY-714 (this), child of EPIC-56 (Apply AXI agentic-ergonomics lessons — treehouse lane).
- TASK-0396 — cargo incremental-cache poison across worktree lifetimes (dissolved by return-not-delete + `pre_destroy` cargo-clean).
- BUG-553 — branch-stacking on worktree reuse (dissolved by unconditional base-reset in `acquire`).
- `docs/session-lifecycle.md` — the worktree lifecycle this reshapes.
- treehouse `internal/pool/`, `internal/git/git.go`, `internal/hooks/hooks.go` — the studied prior art.
