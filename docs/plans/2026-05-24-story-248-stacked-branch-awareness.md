# Plan: STORY-248 — Stacked-branch awareness

Date: 2026-05-24
Specs: STORY-248
Status: In Progress
Complexity: ~600 prod LOC, ~400 test LOC, 3 commits, risk medium

## Context

Today `aida queue work` always forks from `origin/main`. When the user wants
to start TASK-Y while TASK-X is mid-CI/review on a different branch, there is
no way for AIDA to:
  - branch Y from X (so Y can build on shared changes without duplicating
    them), and
  - know that Y depends on X (so when X merges, Y rebases onto the new main).

The rebase classification primitives (`aida-core::rebase::detect` /
`classify`, `aida rebase --auto`, the `/aida-rebase` skill, `aida pr rebase`)
all exist. This story adds the **stack-context layer** that ties them
together for parallel implementation pipelining.

## Approach

Three vertical slices — each commits clean.

**Slice 1 — Base selection (tracer bullet).** Add `--stack` and
`--base <BRANCH>` to `aida queue work`. `--stack` auto-detects the most
recent un-merged implementer lease and uses its branch; `--base BRANCH`
takes the name explicitly with a safety check. Both thread `base` through
the existing `session_start(base: Option<&str>)` parameter (currently
always `None` from queue work). The minted lease records two new fields,
`parent_branch` and `parent_branch_sha`, captured at branch-fork time —
recording the SHA is non-negotiable because the project squash-merges +
deletes branches, so the only safe later rebase is
`git rebase --onto origin/main <recorded-parent-sha> <branch>`.

**Slice 2 — Stack tracking + `aida stack` verb.** Add a new
`aida-cli/src/stacks.rs` module owning `.aida/stacks.json` (atomic
write-through, gitignored under the existing `.aida/*` deny rule). Each
entry records `{ branch, parent_branch, parent_branch_sha, spec_id,
created_at }`. Entries get added by slice 1's queue-work path and removed
when the dependent branch merges (auto-bump callback) or is force-deleted
via `aida session end --force`. Add `aida stack show` (rendered tree) and
`aida stack list` (flat per-chain summary).

**Slice 3 — Auto-rebase cascade on `aida pull`.** After the code-leg pull
lands new commits on main, walk `.aida/stacks.json` chains bottom-up. For
each entry whose `parent_branch` is now merged (= no longer exists locally
+ on origin or matches the auto-bump scan's deleted set), fire
`git rebase --onto origin/main <parent_branch_sha> <branch>`. On success,
remove the stack entry and update any dependent entry's `parent_branch` to
`main`. On classifier-class `diverged-risky` (file overlap), abort the
cascade with a clear pointer to `/aida-rebase`. Gated by `--auto` on
`aida pull`; without it, prompt interactively, refuse non-interactively.

### Diagram

```
 Slice 1 (--stack / --base)
   aida queue work TASK-Y --stack
     │
     ├─ resolve_stack_base() → pick newest un-merged implementer lease
     │     branch = task-x, sha = <fork-point SHA on task-x>
     │
     ├─ session_start(base=Some("task-x"))   ──►  worktree on task-y
     │                                            forked at task-x's HEAD
     │
     └─ lease.parent_branch       = "task-x"
        lease.parent_branch_sha   = <sha>
                       │
                       ▼ (slice 2)
       .aida/stacks.json   { "task-y" → { parent: "task-x", sha: <…> } }

 Slice 3 (aida pull cascade)
   aida pull
     │
     ├─ git pull --ff-only origin main          (main advances)
     ├─ auto_bump_done_to_completed             (existing — already wired)
     │
     └─ cascade_rebase_stacked_branches()       (NEW)
            for chain in stacks.chains():
              for entry in chain (bottom-up):
                if entry.parent_branch ∈ merged-this-pull:
                  rebase --onto origin/main <entry.parent_sha> <entry.branch>
                  on Ok   → drop entry, repoint dependents to main
                  on Conf → abort cascade, point at /aida-rebase
```

## Decisions

- **Lease as parent_branch home, not manifest.** The spec text says
  "Session manifest at `.aida/sessions/<id>.toml`" — that path is the
  `SessionLease` file (the manifest file is `.aida/sessions/<id>.manifest.toml`).
  Putting `parent_branch` on the lease is the right call regardless: the
  lease already owns `branch`, and `aida session leases` / status surfaces
  read the lease, not the manifest.

- **`.aida/stacks.json` as separate state, not derived from leases.** The
  cascade needs to fire **after** the dependent branch merges, which is
  when `aida queue done` already removed its lease. A standalone file
  outlives lease cleanup. JSON over TOML because the natural shape is a
  flat map keyed by branch name and `serde_json::to_string_pretty` round-
  trips that more directly than TOML's table-of-tables.

- **`git rebase --onto origin/main <parent_sha> <branch>`, not plain
  `git rebase origin/main`.** The project squash-merges and deletes
  branches (`gh pr merge --squash --delete-branch`). A plain
  `git rebase origin/main` from a stacked branch would re-apply the
  parent's pre-squash commits on top of the (now-squashed) version of the
  same change → either spurious conflicts or duplicated history. The
  three-argument `--onto` form skips the parent's commits entirely;
  recording `parent_branch_sha` at fork time is what makes this safe.
  Rationale tracked in BUG context on the spec (joe's 2026-05-19 comment).

- **Stack-add is queue-work-only, not session-start-only.** The user-facing
  semantics live in `aida queue work --stack`. `aida session start --base`
  already exists today but is not wired to stack tracking; this story
  leaves it that way (session start is a power-user primitive, queue work
  is the daily driver). A followup TASK can lift it into session start.

- **`--auto` gate on `aida pull` cascade.** Without `--auto`, the cascade
  prompts before each rebase. Non-interactive (`! IsTerminal::stdin`) +
  no `--auto` → log "stacked branches detected; pass --auto to rebase"
  and skip. This matches `aida rebase`'s own gating discipline.

- **`--stack` lease selection: implementer-role, un-merged, freshest.**
  When multiple in-flight implementer leases exist, pick the
  most-recently-started whose branch has not yet been merged. Walk
  `list_leases`, filter `role == "implementer"`, filter
  `!detect_merged_pr_for_branch(branch).is_present()`, sort by
  `started_at` desc, take first. Tie-broken by `started_at` (no random
  order). Detection of "no longer in flight" reuses the existing
  `detect_merged_pr_for_branch` helper.

## Files (in build-order)

### Slice 1 — base selection + lease parent fields

#### `aida-cli/src/cli.rs` — add `--stack` / `--base` to `QueueCommand::Work`

- `QueueCommand::Work`: add two new fields after the `path` override:
  - `stack: bool` (`#[clap(long, conflicts_with = "base")]`) — "auto-pick the most recent un-merged in-flight implementer branch as the base for this session."
  - `base: Option<String>` (`#[clap(long, value_name = "BRANCH", conflicts_with = "stack")]`) — "fork this session's branch from BRANCH instead of origin/main."
  - `force: bool` doesn't exist on Work today; reuse `steal` as the override flag is wrong (different semantics). Add `stack_force: bool` (`#[clap(long = "force-base")]`) for the "base PR closed/merged" override.

#### `aida-cli/src/main.rs` — `SessionLease` + queue-work resolve

- `struct SessionLease`: add two optional fields with `#[serde(default, skip_serializing_if = "Option::is_none")]`:
  - `parent_branch: Option<String>`
  - `parent_branch_sha: Option<String>`
- `fn handle_queue_work` (the `Work` dispatch around line 56587): accept the new `stack`, `base`, and `stack_force` parameters; before the `session_start` call, compute `resolved_base: Option<String>` via a new helper `resolve_stack_base(...)`. Thread it through as `session_start(..., base: resolved_base.as_deref(), ...)` (the parameter already exists at line 17284; queue work currently passes `None`).
- `fn session_start` (around line 17281): after the `else` branch that creates the worktree on a new branch (line 17628), capture the new branch's HEAD SHA via `aida_core::git_ops::head_sha(&worktree_path)`. When `base.is_some()`, that SHA is the `parent_branch_sha` value for the lease. Store both fields on the `SessionLease` literal at line 17818.
- New helper `fn resolve_stack_base(project_root: &Path, stack: bool, base: Option<&str>, force: bool) -> Result<Option<String>>`: returns `None` if neither flag set; with `--stack`, walks `list_leases`, filters un-merged implementer leases, picks freshest by `started_at`; with `--base BRANCH`, validates via `branch_exists_anywhere`, then unless `force` checks `detect_merged_pr_for_branch` and refuses with a "branch already merged — pull and branch from main" message. Place this helper near `branch_exists_anywhere` at line 17088.
- The dispatch site for `Command::Queue(QueueCommand::Work { ... })` (around line 54870) needs the new fields plumbed through to `handle_queue_work`.

### Slice 2 — `.aida/stacks.json` module + `aida stack` verb

#### `aida-cli/src/stacks.rs` (new) — graph storage + queries

- `pub struct StackEntry { branch: String, parent_branch: String, parent_branch_sha: String, spec_id: Option<String>, created_at: DateTime<Utc> }`
- `pub struct StackGraph { entries: BTreeMap<String, StackEntry> }` (key = branch). `BTreeMap` for deterministic JSON ordering.
- `pub fn path(project_root: &Path) -> PathBuf` → `.aida/stacks.json`
- `pub fn load(project_root: &Path) -> StackGraph` — `read_atomic` then `serde_json::from_str`; empty graph on missing file.
- `pub fn save(project_root: &Path, graph: &StackGraph) -> Result<()>` — `serde_json::to_string_pretty` + `aida_core::write_atomic`.
- `pub fn add(graph: &mut StackGraph, entry: StackEntry)` / `pub fn remove(graph: &mut StackGraph, branch: &str)`.
- `pub fn chains(graph: &StackGraph) -> Vec<Vec<&StackEntry>>` — derive ordered chains (bottom of each chain is the entry whose `parent_branch` is NOT itself a stack entry; walk up from there).
- `pub fn repoint(graph: &mut StackGraph, old_parent: &str, new_parent: &str, new_sha: &str)` — rewrite every entry whose `parent_branch == old_parent`. Used by slice 3 when a chain rebases onto main.
- Full test coverage at the bottom of the module.

#### `aida-cli/src/main.rs` — wire stack-add into queue work + add `aida stack` handler

- `mod stacks;` at the top of the file.
- `fn handle_queue_work`: after `session_start` succeeds and the lease is read back (around line 57066), if `resolved_base.is_some()`, call `stacks::add(...)` with the new lease's branch + parent_branch + parent_branch_sha + spec.
- New dispatch arm `Command::Stack(StackCommand::Show { .. })` / `StackCommand::List` calls `handle_stack_show` / `handle_stack_list` — each loads the graph, derives chains, renders.

#### `aida-cli/src/cli.rs` — add `StackCommand` enum + top-level `Stack` variant

- New `pub enum StackCommand { Show { json: bool }, List { json: bool } }` next to `DrainCommand` at line 3391.
- New top-level variant `Stack(StackCommand)` on the `Command` enum, mirroring `Drain(DrainCommand)`.

### Slice 3 — `aida pull` cascade

#### `aida-cli/src/cli.rs` — add `--auto` flag to `Command::Pull`

- `Command::Pull`: add `auto: bool` (`#[clap(long)]`) — "auto-rebase tracked stacked branches whose base merged in this pull; prompt without it (refused non-interactively)."

#### `aida-cli/src/main.rs` — cascade after code pull

- `fn handle_pull_command`: after the code-leg `Ok(s) if s.success()` arm (line 41127), AFTER the `auto_bump_done_to_completed` call (line 41135), invoke a new `fn cascade_rebase_stacked_branches(&project_root, auto)?`.
- New `fn cascade_rebase_stacked_branches(project_root: &Path, auto: bool) -> Result<()>`: loads the graph, derives chains, computes which `parent_branch` values were merged into main during THIS pull (compare the auto-bump's set of "branches deleted on origin" — reuse the result of `auto_bump_done_to_completed` or a sibling helper). For each affected chain bottom-up:
  - Confirm via `aida_core::rebase::detect` against `origin/main` → if `DivergedRisky`, abort cascade, point at `/aida-rebase`.
  - Otherwise run `git rebase --onto origin/main <entry.parent_branch_sha> <entry.branch>` from the **worktree** (need to resolve the worktree path — pull from `list_leases` filtered by branch). On success, `stacks::repoint(graph, &entry.branch_was_parent, "main", new_sha)` and `stacks::remove(graph, &entry.branch)` if it itself merged.
- `Command::Pull` dispatch (line 4017): thread the new `auto: bool` through `handle_pull_command`.

#### `aida-cli/src/stacks.rs` — small helpers used by the cascade

- `pub fn parents_merged_in(graph: &StackGraph, merged_branches: &HashSet<String>) -> Vec<String>` — returns entries whose `parent_branch ∈ merged_branches`, sorted bottom-up.

## Critical Files

- `aida-cli/src/cli.rs` — new flags on `Work` + `Pull`; new `Stack(StackCommand)` enum.
- `aida-cli/src/main.rs` — `SessionLease` fields, `handle_queue_work` base resolution, `session_start` SHA capture, `Command::Pull` cascade, `Command::Stack` dispatch.
- `aida-cli/src/stacks.rs` — new module owning `.aida/stacks.json`.
- `aida-cli/src/lib.rs` (or wherever `mod` declarations live) — `pub mod stacks;`.

## Reusable helpers (do not reimplement)

- `aida_core::git_ops::head_sha` (`aida-core/src/git_ops.rs`) — fork-point SHA capture.
- `aida_core::git_ops::current_branch`, `has_remote`, `remote_branch_exists` — sanity checks.
- `aida_core::rebase::{detect, classify, RebaseClass}` (`aida-core/src/rebase.rs`) — classify the rebase safety BEFORE executing.
- `aida_core::{read_atomic, write_atomic}` (`aida-core/src/fs_atomic.rs`) — race-safe `.aida/stacks.json` I/O (mandatory; see TASK-331 / TASK-346 grep guards on `session_manifest.rs`).
- `branch_exists_anywhere` (`aida-cli/src/main.rs`, line 17088) — `--base BRANCH` existence validation.
- `detect_merged_pr_for_branch` (`aida-cli/src/main.rs`, line 20550) — "is this branch's PR already merged?" check used by both the `--base` safety guard and the slice-3 cascade trigger.
- `list_leases` (`aida-cli/src/main.rs`, line 16010) — `--stack` walks this.
- `session_start` (`aida-cli/src/main.rs`, line 17281) — already accepts `base: Option<&str>`; just thread it.

## Risks + gotchas

1. **Risk**: Squash-merge invalidates a plain rebase. **Mitigation**: Slice 1 captures `parent_branch_sha` at fork time; slice 3 uses
   `git rebase --onto origin/main <sha> <branch>` exclusively. Decision is locked in the "Decisions" section.
2. **Risk**: `.aida/stacks.json` and lease cleanup drift — a lease ends but the stack entry persists. **Mitigation**: slice 2 wires `stacks::remove` into both `aida queue done` AND the auto-bump merge handler (slice 3); both are idempotent so the rare race where both fire is fine. Stale entries (branch missing from `branch_exists_anywhere`) get GC'd at `aida stack show` render time too.
3. **Risk**: User invokes `--stack` from inside a worktree that itself owns the "most recent in-flight implementer lease" → recursive base. **Mitigation**: filter the resolver to exclude the lease whose worktree contains the current cwd (use `active_lease_for_cwd`).
4. **Risk**: A stacked branch's worktree is gone (user deleted it manually) when slice 3 tries to rebase. **Mitigation**: cascade resolver looks the worktree path up via `list_leases`; if missing, log a warning + skip that chain rather than fail the whole pull.
5. **Risk**: Multiple chains exist; one fails. **Mitigation**: cascade is per-chain — failure in one chain doesn't abort others. The pull's exit code reflects whether the code leg + store leg succeeded; cascade failures are warnings (matches the BUG-254 contract that pull's exit only reflects the two legs).
6. **Risk**: Adding fields to `SessionLease` could break old leases. **Mitigation**: both new fields are `#[serde(default, skip_serializing_if = "Option::is_none")]` — matches the pattern other late-added fields already use (e.g. `pr_head_sha`, `zen_intent_token`).
7. **Risk**: `--stack` + `--no-pull` can pick a base whose PR was merged seconds ago. **Mitigation**: this is a known limit — the resolver only sees what the local lease + PR cache knows. `--force-base` is the escape hatch; the post-merge cascade fixes it next `aida pull`.

## Tests (named, not "add tests")

### Slice 1
- `resolve_stack_base_returns_none_without_flags` — happy default.
- `resolve_stack_base_picks_freshest_unmerged_implementer_lease` — multi-lease pick.
- `resolve_stack_base_excludes_current_worktree_lease` — anti-recursion.
- `resolve_stack_base_refuses_merged_base_without_force` — `--base` safety.
- `resolve_stack_base_force_overrides_merged_check` — escape hatch.
- `session_lease_parent_fields_round_trip` — TOML serde for the two new fields, including the missing-field default path.

### Slice 2
- `stacks_add_then_load_round_trip` — JSON serde.
- `stacks_chains_single_entry` / `stacks_chains_multi_link` — chain derivation.
- `stacks_chains_disjoint_returns_two_chains` — independent stacks.
- `stacks_remove_keeps_dependents` — removal is local.
- `stacks_repoint_rewrites_chain` — used by slice 3 cleanup.
- `stack_show_renders_tree` — text rendering smoke.
- `stack_list_json_shape` — JSON-output schema pin.

### Slice 3
- `cascade_skips_when_no_stacked_branches` — no-op on empty graph.
- `cascade_rebases_clean_chain_onto_main` — integration: temp repo, two branches, merge-and-pull, assert rebase landed.
- `cascade_aborts_on_diverged_risky` — overlap → no execute + clear error string.
- `cascade_skips_chain_when_worktree_missing` — robustness.
- `cascade_refuses_non_interactive_without_auto` — gating discipline.

## Verification

Executable smoke test — runs in a temp repo and exercises all three slices end-to-end.

```bash
set -euo pipefail
TMP=$(mktemp -d); cd "$TMP"

# bootstrap repo + aida
git init -q -b main
git config user.email tester@example.com
git config user.name "Tester"
aida init --no-skills --no-hooks --force >/dev/null
echo "base" > base.txt && git add base.txt && git commit -qm "init"
git remote add origin "$TMP/.origin" || true   # local fake remote
git clone --bare . .origin 2>/dev/null || true
git push -u origin main 2>/dev/null || true

# Slice 1 — stack base resolution
aida add --title "TASK-X" --type task --status approved --quiet
aida add --title "TASK-Y" --type task --status approved --quiet
# Simulate a lease for TASK-X (queue work would normally do this) — for the
# smoke test we drive it through queue work --no-launch.
aida queue add TASK-X --for implementer
aida queue work TASK-X --no-launch --no-pull
# Now stack TASK-Y on top:
aida queue add TASK-Y --for implementer
aida queue work TASK-Y --stack --no-launch --no-pull 2>&1 | grep -q "base: task-x" \
  && echo "✓ slice 1: --stack picked task-x"

# Slice 2 — stack tracking
aida stack list | grep -q "task-y → task-x" \
  && echo "✓ slice 2: stack list shows chain"
aida stack show --json | jq -e '.chains[0][0].branch == "task-y"' >/dev/null \
  && echo "✓ slice 2: JSON shape stable"

# Negative: --base on a non-existent branch refuses cleanly
aida queue work TASK-Y --base does-not-exist --no-launch 2>&1 \
  | grep -q "does not exist" && echo "✓ slice 1: --base validates"

# Slice 3 — cascade on pull (simulated: drop task-x from origin to mimic merge)
# This part runs only when origin is reachable; skip otherwise.
git branch -D task-x 2>/dev/null || true
aida pull --auto 2>&1 | grep -qE "(rebased onto main|no stacked branches behind)" \
  && echo "✓ slice 3: cascade executed or no-op'd cleanly"
```

## Followups

- `aida session start --stack` parity with `aida queue work --stack`.
- Multi-stack rebase ordering when two independent chains both need rebasing in one pull.
- `aida stack show` enrichment: per-branch PR + CI + lease state via `gh` (out of scope here — relies on `gh`/network).
- Reviewer-side: surface "PR-Y is stacked on PR-X" in `/aida-review` when reviewing PR-Y while PR-X is unmerged.
- `--auto-complete --pipelined` autonomous flavor (calls into this story's primitives at phase 4 — the bigger surface mentioned in the spec).

## Related

- Builds on: `aida_core::rebase::detect/classify` (TASK-103), `/aida-rebase` skill (TASK-104), `aida pr rebase` (TASK-308).
- Composes with: STORY-246 (`--auto-complete`), TASK-249 (`/aida-drain-queue` skill).
- Successor to: BUG-114 (already completed — was the lease-tracking precondition).
- See also: `docs/autonomous-drain.md` (the workflow story that this acceleration enables).
