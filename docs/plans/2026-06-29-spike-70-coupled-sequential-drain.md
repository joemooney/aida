# Plan: SPIKE-70 — coupled-sequential / single-branch drain mode

Date: 2026-06-29
Specs: SPIKE-70, EPIC-54, BUG-650, EPIC-28, STORY-248, BUG-554, STORY-711
Status: Draft
Complexity: ~350 prod LOC, ~250 test LOC, ~5 commits, risk medium (touches the reliability-critical drain)

<!-- Design spike. Read-only investigation → plan the operator reviews before anyone
implements. Symbol refs throughout; the drain is keystone. trace:SPIKE-70 -->

## Approach

AIDA's drain assumes ONE work shape: independent ready specs, each its own
worktree+branch, fanned out in PARALLEL (`/aida-burndown`) or driven one-PR-at-a-time
SEQUENTIALLY (`aida queue work --batch --auto-complete` → `auto_complete.rs::drain_batch`).
Neither fits COUPLED work that shares files and accumulates (EPIC-54's TUI children:
each increment touches `mod.rs`/`state.rs` and builds on the prior). Parallel fan-out
collides at merge; the sequential batch drain merges each member to main as its own
PR/review, which is wrong for increments that should ship together and be validated
step-by-step. Last night the advisor hand-drove the sequence. This plan turns that hand
sequence into two named drain modes and recommends when each applies:

- **`--sequential`** — BLESS the existing per-spec-PR sequential-merge (`drain_batch`)
  as a first-class, named mode (concurrency forced to 1, each member forks off the
  freshly-pulled main, optional per-member checkpoint). For coupled-but-independently-
  shippable work: ordered, but each increment is still a reviewable PR to main.
- **`--single-branch`** (a.k.a. `--branch-accumulate`) — NEW. All members commit to ONE
  shared feature branch in ONE worktree, commit-per-member, NO per-member merge-to-main,
  ONE cluster PR at the end. For tightly-coupled work that ships together (EPIC-54).

### Diagram

    PARALLEL (today, /aida-burndown):   A┐
                                        B├─ N worktrees, N PRs → main  (independent only)
                                        C┘

    --sequential (today's drain_batch, just NAMED):
        A → branch off main → CI → review → MERGE → pull
                                                     └─► B → branch off main' → … MERGE → pull → C
        (N PRs, ordered; each forks off the prior's merge; EPIC-28 shelve-and-continue on failure)

    --single-branch (NEW):
        feat-branch off main ─ commit(A) ─ CI ─ [checkpoint] ─ commit(B) ─ CI ─ [checkpoint] ─ commit(C) ─ CI
                                                                                              └─► ONE cluster PR → main
        (1 worktree, 1 PR; NO reset between members; HALT-on-failure — later commits build on earlier)

## Decisions

- **Decision: ship `--single-branch` first; `--sequential` is a naming/guard pass over
  existing code.** Rationale: `drain_batch` already IS sequential-merge (verified: phase-5
  Pull advances main before `next_head()`, and the implementer bases on `origin/main`).
  Single-branch is the real gap — there is no one-PR/one-branch path anywhere.
- **Decision: single-branch HALTS on a member failure; sequential SHELVES-and-continues
  (EPIC-28).** Rationale: on one accumulating branch, later increments build on earlier
  commits, so "skip the broken one and continue" would build on broken code. Halt, park
  the branch with prior members' commits intact, alert the operator. EPIC-28's
  shelve→skip-dependents→continue-independents is correct ONLY when members are
  independent (the `--sequential` / parallel case).
- **Decision: `--single-branch` disables the BUG-554 "reset between members" rule.**
  Rationale: accumulation is the whole point — members are intended to stack on one
  branch. The BUG-554 reset is the safety rule for ACCIDENTAL reuse; here reuse is
  deliberate, so the mode replaces "reset" with "accumulate" and never opens a per-member
  PR (the two hazards BUG-554 names — polluted PRs, double-reachable commits — only bite
  when each member is meant to be its own PR).
- **Decision: per-increment human-validation checkpoint = `--zen`'s pause-after-each.**
  Rationale: `--zen` already pauses at the grab-next/stop checkpoint (`cli.rs` ~4619,
  `[zen] auto_exit`). Single-branch reuses it as the "validate this increment before the
  next" gate. Under `--no-human=both` the checkpoint auto-continues (fully headless).
- **Decision: ONE cluster PR linking every member SPEC-ID; all members Done→Completed on
  merge.** Rationale: the digest already models multi-spec PRs (`digest.rs::collapse_cluster_prs`);
  reuse it. The merge-driven Done→Completed auto-bump must credit every SPEC-ID in the PR.
- **Decision: single-branch spawns ONE isolated worktree, not the host checkout.**
  Rationale: shrinks BUG-650's HARNESS-WORKTREE contention surface from N implementer
  worktrees to 1, and lets the cluster drive run from a checkout shared with an advisor
  session. Does NOT by itself fix BUG-650 — see Risks.
- **Decision: invocation is a flag on `aida queue work --batch`, not a new subcommand.**
  Rationale: `--batch`/`--batches` already define an ordered member set; `--single-branch`
  /`--sequential` just pick how that set is driven. Keeps composition with
  `--auto-complete`/`--zen`/`--no-human`/`--max`/`--max-failures` free.

## Files (in build-order)

### `aida-cli/src/cli.rs` — Work command flags

- `Work { … }` (line 4188): add `--single-branch: bool` and `--sequential: bool`.
  `--single-branch` `requires = "autonomous"`, `requires = "batch"|"batches"`,
  `conflicts_with = "concurrency"` (>1 makes no sense), `conflicts_with = "sequential"`.
  `--sequential` is the named guard over the existing per-spec-PR drain (forces
  concurrency=1; mostly a doc/UX surface).

### `aida-cli/src/auto_complete.rs` — the orchestration engine

- New `drain_batch_single_branch` (sibling to `drain_batch`, line 2866): create ONE
  worktree+branch up front (reuse session-start helpers), loop `next_head()` members in
  queue order, per member run Implementer + CI ONLY (no Reviewer/Merge/Pull between
  members), commit-per-member in place, mark the spec Done, NO reset. Halt on the first
  member failure (park branch, leave prior commits). After the last member: run the
  Reviewer/Merge phases ONCE for the cluster and open ONE PR.
- New `AutoCompleteVariant` arm or a `SingleBranch` flag threaded through
  `orchestrate_with_resume` (line 2376): per-member runs cap at phase 2 (CI); the
  Merge/Pull/PR phases run once at cluster end, not per member.
- Per-member checkpoint hook: between members, honor the `--zen` pause / `--no-human=both`
  auto-continue (reuse the existing zen-finish decision path).

### `aida-cli/src/main.rs` — Work dispatch

- The `queue work` handler that builds the `BatchDriver` and calls `drain_batch*`
  (around the batch-name resolution, lines 905-996 / 16134-16161): route `--single-branch`
  to `drain_batch_single_branch`, `--sequential` to the existing `drain_batch` with
  concurrency pinned to 1.

### `aida-cli/src/digest.rs` — cluster PR (reuse, light touch)

- `collapse_cluster_prs` (line 343) already groups multi-spec PRs; confirm the
  single-branch cluster PR (multiple `(SPEC)` subjects, one PR) folds in correctly.

### `aida-core/templates/skills/aida-burndown.md` — routing doc

- Lines 100-125 (the BUG-554 manual reset note + serialize-group note): replace the
  "reserve sequential-through-one-worktree … reset between each" hand-driving guidance
  with "route coupled file-sharing sets to `aida queue work --batch … --single-branch`
  (one PR) or `--sequential` (ordered PRs); do NOT fan them out."

## Critical Files

- `aida-cli/src/auto_complete.rs`
- `aida-cli/src/cli.rs`
- `aida-cli/src/main.rs`
- `aida-cli/src/digest.rs`
- `aida-core/templates/skills/aida-burndown.md`

## Reusable helpers (do not reimplement)

- `auto_complete.rs::drain_batch` / `drain_batch_with_caps` — the sequential loop;
  `--sequential` reuses it verbatim. `--single-branch` mirrors its `BatchDriver`/
  `next_head()` shape.
- `auto_complete.rs::orchestrate_with_resume` + `AutoCompleteVariant::last_phase` —
  the phase gating; single-branch caps per-member runs at CI by reusing the variant cap.
- STORY-248 `--stack` / `--base` (`cli.rs` 4294-4310) — branch-accumulation +
  auto-rebase-on-merge substrate; the single shared branch can reuse the base/stack
  machinery rather than re-inventing worktree creation.
- `digest.rs::collapse_cluster_prs` — multi-spec cluster-PR modeling.
- The Done→Completed merge auto-bump (subject `(SPEC-ID)` parsing) — extend to credit
  every member ID on the one cluster PR rather than re-implement.
- The `--zen` pause / `aida zen finish` decision path — the per-increment checkpoint.

## Risks + gotchas

1. **Risk: BUG-650 HARNESS-WORKTREE contention.** Single-branch shrinks the surface
   (1 worktree, not N) but still contends if it tries to drive in the host checkout.
   **Mitigation:** single-branch MUST spawn its own isolated worktree (BUG-650 fix option
   (a)); treat BUG-650 as a soft dependency, not a blocker — document that until BUG-650
   lands, run the cluster drive from a separate clone if an advisor shares the checkout.
2. **Risk: halt-on-failure leaves a half-built branch.** **Mitigation:** park the branch
   with prior members' commits intact, mark the failed member NeedsAttention, alert the
   operator with the branch name + the member that failed; never auto-discard accumulated
   work.
3. **Risk: failure-rule divergence (halt vs EPIC-28 shelve) confuses operators.**
   **Mitigation:** the mode picks the rule — `--single-branch` ⇒ halt; `--sequential`/
   parallel ⇒ EPIC-28 shelve-and-continue — and the banner states which is active.
4. **Risk: cluster PR's Done→Completed bump misses members.** **Mitigation:** the bump
   must parse ALL `(SPEC-ID)` subjects on the merged PR; add a test asserting every member
   reaches Completed on one cluster merge.
5. **Risk: this touches the reliability-critical drain.** **Mitigation:** per the burndown
   skill's own guardrail, ship changes to the autonomy machinery SUPERVISED (`--zen`), not
   through an unattended drain; land `--single-branch` behind explicit opt-in (default
   behavior of the existing drain unchanged).
6. **Risk: STORY-711 advisor-lock interaction.** A single shared branch driven for a
   cluster wants exactly the advisor-authorized branch/worktree lock STORY-711 proposes.
   **Mitigation:** design the worktree creation so a future STORY-711 lock binds cleanly;
   don't pre-build the lock here.

## Tests (named, not "add tests")

- `single_branch_commits_each_member_no_intermediate_merge` — N members → N commits on
  one branch, zero merges-to-main until the end.
- `single_branch_halts_on_member_failure_keeps_prior_commits` — member 2 fails → branch
  retains member 1's commit, member 2 NeedsAttention, drain stops.
- `single_branch_opens_one_cluster_pr_linking_all_members` — one PR, all SPEC-IDs linked.
- `single_branch_cluster_merge_bumps_all_members_completed` — every member Done→Completed
  on the one merge.
- `single_branch_no_reset_between_members` — asserts the BUG-554 reset is suppressed.
- `sequential_forces_concurrency_one_and_forks_off_fresh_main` — `--sequential` pins
  concurrency=1; member N+1's base == prior member's merged main.
- `sequential_shelves_and_continues_on_failure` — EPIC-28 rule still applies in
  `--sequential` (the divergence from single-branch's halt).
- `single_branch_zen_pauses_each_member_no_human_both_continues` — checkpoint behavior.

## Verification

```bash
TMP=$(mktemp -d); cd "$TMP" && git init && aida init
# Three coupled specs sharing one file, tagged batch:tui
for n in 1 2 3; do aida add --title "increment $n" --type task --status approved --tag batch:tui; done
aida queue add TASK-1 TASK-2 TASK-3
AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"
# Positive: single-branch → ONE branch, ONE PR
$AIDA_BIN queue work --batch tui --auto-complete --single-branch --no-human=both --no-launch --dry-run
git branch --list | wc -l        # expect: 1 feature branch (+ main)
# Negative: a member failing halts, leaving prior commits
# (drive a mock failure on TASK-2 → assert TASK-1 commit present, TASK-2 NeedsAttention, no PR)
```

## Followups (PROPOSED implementation TASKs — operator files them)

- **TASK (cli):** add `--single-branch` + `--sequential` flags to `cli.rs` `Work` with the
  conflict/requires guards. ~40 LOC.
- **TASK (engine):** `drain_batch_single_branch` in `auto_complete.rs` — one worktree/
  branch, per-member Implementer+CI, no intermediate merge, halt-on-failure. ~180 LOC.
- **TASK (cluster PR):** one PR linking all member SPEC-IDs + Done→Completed bump for all
  members on the single merge; reconcile with `digest.rs::collapse_cluster_prs`. ~80 LOC.
- **TASK (checkpoint):** per-member `--zen` pause / `--no-human=both` auto-continue hook. ~40 LOC.
- **TASK (bless --sequential):** name + guard the existing `drain_batch` sequential-merge
  (concurrency=1, fork-off-fresh-main assertion, doc the EPIC-28 shelve rule). ~30 LOC.
- **TASK (docs):** rewrite `aida-burndown.md` lines 100-125 to route coupled sets to the
  new modes instead of the BUG-554 manual reset dance; add a discipline note on
  halt-vs-shelve. ~doc only.
- **Relationship to file:** BlockedBy/RelatesTo **BUG-650** (isolated worktree so single-
  branch doesn't contend); RelatesTo **EPIC-28** (failure-rule divergence); RelatesTo
  **STORY-248** (reuse `--stack`/`--base` substrate); RelatesTo **STORY-711** (advisor-lock
  on the shared branch).

## Related

- Builds on: STORY-248 (--stack/--base), TASK-285/TASK-310 (batch drain), EPIC-28 (shelving)
- Surfaced by: EPIC-54 (TUI redesign — the coupled work that exposed the gap), SPIKE-70
- See also: BUG-554 (the manual reset hazard this replaces), BUG-650 (contention),
  `aida-core/templates/skills/aida-burndown.md`
