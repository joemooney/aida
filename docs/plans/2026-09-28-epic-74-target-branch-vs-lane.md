# Plan: EPIC-74 — replace "feature lane" with target/parent branch

Date: 2026-09-28
Specs: EPIC-74, STORY-1481, STORY-1482, STORY-1483
Status: Draft — needs advisor signoff + ADR before implementation
Complexity: reduces EPIC-74 from heft 14 (8 + 11 + 8 across three guided stories); risk medium (touches PR base selection and Completed semantics)

> Origin: operator, 2026-09-28 — *"I am unsure about the terminology of 'feature
> lane', to me this seems like a branch and calling it a lane is introducing an
> unfamiliar term. I am thinking that we work on an AIDA_TARGET_BRANCH (defaulting
> to main) and an AIDA_PARENT_BRANCH (defaulting to main if AIDA_TARGET_BRANCH is
> not main, else empty)."*

## Approach

EPIC-74 coins "lane" for what is, in every one of its own acceptance criteria, a
**branch**. Drop the coined noun. Express the mechanism as a *target branch* (where
work merges) and a *parent branch* (what it syncs from), and keep exactly one piece
of state that branches cannot express: a store-side declaration, on the epic, that
its subtree is off the main line. Environment variables carry the per-session view;
the store carries the shared guarantee. This collapses three stories' worth of new
vocabulary — lane record, lane seat, lane flag, `lane open/list/close/sync/land` —
into one field on the epic plus two variables, and it makes the Done-vs-Completed
rule definable instead of special-cased.

The naming change is free **right now**: EPIC-74 is still Draft, all three children
are `guided` + `needs-sketch`, STORY-1482 and STORY-1483 are blocked on their
predecessors, and nothing is implemented. After implementation the same rename costs
a deprecation shim for `aida lane *`.

### The two facts, and why only one is a branch

```
  ┌─ per-session, env ────────────┐   ┌─ shared, store ──────────────────┐
  │ AIDA_TARGET_BRANCH  where my  │   │ epic.target_branch               │
  │                     PRs go    │   │   ⇒ "this subtree is off-main"   │
  │ AIDA_PARENT_BRANCH  what I    │   │   read by drains, integrate,     │
  │                     sync from │   │   groom, queue — sessions that   │
  └───────────────────────────────┘   │   never set the env vars         │
                                      └──────────────────────────────────┘

  Completed semantics fall out:

    merged ──► target == default_branch ? ──yes──► Completed
                        │
                        └──no──► Done (in <target-branch>)
                                   │
                        target branch merges to parent ──► Completed
```

## Decisions

These are proposals, not settled calls — EPIC-74's own DIRECTION requires a sketch,
advisor signoff and an ADR first (see **Open questions**).

- **Drop "lane" as a noun.** **Rationale**: the concept never escapes branch
  vocabulary in its own specs. STORY-1481 defines a lane as *"the target branch
  (default `epic-n-work`)"*; STORY-1482's first acceptance criterion is literally
  *"opens its PR with `--base epic-n-work`"*. The term adds a word without adding a
  concept, and the operator — the primary reader — bounced off it on first contact.

- **`AIDA_TARGET_BRANCH` replaces lane-aware PR targeting (STORY-1482's mechanism).**
  **Rationale**: base selection is already centralised. The forge's `default_branch`
  method feeds `base:` at PR-open time, and `--base` is threaded from there. An
  override at that single seam is strictly smaller than teaching drains, `aida pr`,
  `ship`, the reviewer phase and the merge phase what a lane is.

- **`AIDA_PARENT_BRANCH` replaces `lane sync` (STORY-1483's mechanism).**
  **Rationale**: "sync the lane" only ever meant "merge from the branch I came from".
  Naming that branch removes the need for a verb.

- **Landing needs no new command.** **Rationale**: `aida lane land` is an ordinary PR
  from target → parent. It already wants normal review and CI and forbids
  implementer self-merge — which is the standard path. Keep a thin convenience alias
  at most.

- **Keep a store-side declaration for main-line exclusion.** **Rationale**: this is
  the epic's actual safety guarantee and the one thing env vars cannot carry. See
  **Risks #1**.

- **Derive defaults from the repo's default branch, never the literal `main`.**
  **Rationale**: `default_branch_name` (`aida-core/src/git_ops.rs`) and
  `default_branch_of` (`aida-cli-lib/src/forge.rs`) already exist and are what
  Completed is defined against today. Hardcoding `main` would hand a `master`/`trunk`
  repo a parent branch that does not exist.

- **`AIDA_PARENT_BRANCH` should always name a real branch — not empty.** **Rationale**:
  this departs from the operator's sketch, deliberately. An empty default makes
  "am I on a side branch?" a single test, which is appealing, but it puts an empty
  string into every consumer's branch path, and empty branch names reach `git merge`
  arguments. If parent always names a real branch, sync is a harmless no-op when
  `target == parent`, and `target != default_branch` is an equally cheap side-branch
  test. Fewer special cases, no guard needed. **This is the one point in the sketch
  worth overriding, and it is reversible if the no-op sync proves confusing.**

## What changes per existing spec

### EPIC-74 — rewrite, stays Draft

Keep the three problems it names (one-way scoping, no integration target, side-branch
drift) and the TASK-1470/EPIC-72 evidence. Replace the lane framing. Heft drops
because slices 2 and 3 mostly evaporate.

### STORY-1481 — shrinks hard

From "lane record + lane seat + lane flag + `lane open/list/close`" to: **an epic
carries an optional target branch**, and main-line selectors skip specs whose epic
declares one that differs from the default branch. `aida lane open` becomes
`aida edit EPIC-N --target-branch <name>`; `lane list` becomes a filter on existing
list surfaces; `lane close` becomes clearing the field.

The exclusion acceptance criteria survive unchanged in substance — drains
(`queue work`, `burndown`, `shift`), `integrate`, `groom`, `backlog groom` and the
queue views must all skip the subtree and print the visible exclusion line, and the
declaration must survive a clone boundary via the store.

### STORY-1482 — mostly evaporates

PR targeting becomes `--base $AIDA_TARGET_BRANCH`. What *remains* is the genuinely
hard part, and it gets easier to state: Completed keeps meaning "merged to the
default branch", so a spec merged to a non-default target is **Done**, not Completed,
and the pull auto-bump must not promote it. The `lane:EPIC-N` tag is unnecessary —
the target branch is the marker.

### STORY-1483 — becomes small

`lane sync` → merge from parent. Drift warning → commits-behind-parent, with the
existing configurable threshold. `lane land` → an ordinary target → parent PR whose
merge promotes the subtree's Done specs to Completed.

## Critical Files

Indicative blast radius for the design as proposed — enumerated to size the change,
not as a build order. The real file list follows the ADR, since the store-schema
question (epic-only vs per-spec target branch) moves several of these.

- `aida-core/src/models.rs` — the spec/epic record gains the target-branch field.
- `aida-core/src/git_ops.rs` — `default_branch_name`, the derivation both defaults
  key off.
- `aida-cli-lib/src/forge.rs` — the base-selection seam (`default_branch_of`, the
  forge's `default_branch` method feeding `base:` at PR-open). The single highest-
  leverage file: most of STORY-1482 collapses into an override here.
- `aida-cli-lib/src/queue_cmd.rs` — queue views and `queue work` selection.
- `aida-cli-lib/src/burndown.rs`, `aida-cli-lib/src/shift.rs` — drain selection.
- `aida-cli-lib/src/integrate.rs` — main-line integration must skip the subtree.
- `aida-cli-lib/src/git_backend_cmd.rs` — the pull-time status reconcile that must
  not promote an off-main merge to Completed.

## Reusable helpers (do not reimplement)

- `default_branch_name` (`aida-core/src/git_ops.rs`) — repo default branch. Both
  `AIDA_TARGET_BRANCH` and `AIDA_PARENT_BRANCH` default from this; never hardcode
  `main`.
- `default_branch_of` and the forge's `default_branch` method
  (`aida-cli-lib/src/forge.rs`) — existing base selection. Override here rather than
  introducing a second notion of "where does this PR go".
- The forge's declared-base field already carries GitLab's `target_branch` naming,
  so the vocabulary matches the forge layer rather than fighting it.

## Tests (named, not "add tests")

- `off_main_subtree_excluded_from_drain_selection` — the safety guarantee, per
  selector (queue work, burndown, shift, integrate, groom).
- `declaration_survives_clone_boundary` — declared on clone A, excluded on clone B
  after `aida pull`. The guard test for the whole epic; see Risks #1.
- `pr_opens_against_declared_target_branch` — base selection honours the target.
- `merge_to_non_default_target_is_done_not_completed` — the Completed invariant.
- `pull_auto_bump_does_not_promote_off_main_merge` — the negative case that makes
  the invariant real.
- `target_branch_merge_to_parent_promotes_subtree_to_completed` — landing.
- `explicitly_targeting_an_excluded_spec_from_main_line_is_refused` — with
  `--force` override.
- `parent_defaults_to_repo_default_branch_not_literal_main` — proves the
  `master`/`trunk` case; assert it fails against a hardcoded `main`.

## Verification

Not yet executable — the commands below are the shape the smoke test should take
once the ADR settles the surface (`aida edit EPIC-N --target-branch` is proposed,
not implemented). Recorded so the acceptance is concrete at implementation time.

```bash
TMP=$(mktemp -d); cd "$TMP" && git init -b trunk && aida init   # NOT main, on purpose
AIDA_BIN="$(git -C /home/joe/ai/aida rev-parse --show-toplevel)/target/debug/aida"

# An epic declaring a target branch takes its subtree off the main line.
aida add --title "side work" --type epic --status approved      # EPIC-N
aida edit EPIC-N --target-branch feature-x
aida add --title "child" --type story --status approved --parent EPIC-N

aida burndown plan | grep -c STORY-M        # expect: 0 (excluded)
aida burndown plan | grep -i "feature lane\|excluded"   # expect: a visible exclusion line

# Defaults derive from the repo default branch, not the literal "main".
aida status | grep -i parent                # expect: trunk, never main

# Completed semantics: merged off-main is Done, not Completed.
# ... drive a child to merge into feature-x ...
aida show STORY-M | grep -i status          # expect: Done, NOT Completed
# ... merge feature-x -> trunk ...
aida show STORY-M | grep -i status          # expect: Completed
```

## Risks + gotchas

1. **Risk**: someone implements this as env vars *only*, and the epic's core promise
   — "nothing reaches the default branch until deliberately landed" — silently
   evaporates. Env state is per-shell, and the sessions that must honour the
   exclusion are exactly the ones that never set it: a main-line drain, `integrate`,
   `groom`. That is precisely the "isolation is by convention only" defect EPIC-74
   names first. **Mitigation**: the store declaration is non-negotiable; treat the
   cross-clone acceptance criterion (declared on clone A, excluded on clone B after
   `aida pull`) as the guard test for the whole epic.

2. **Risk**: an env var that silently redirects PR base is a foot-gun — a stray
   `AIDA_TARGET_BRANCH` in a shell profile would quietly retarget every PR from that
   terminal. **Mitigation**: derive the session's target from the epic declaration
   where one exists and treat the env var as an explicit override; surface the
   effective target in `aida status` and at PR-open time.

3. **Risk**: Completed semantics are load-bearing across the merge cascade, the pull
   auto-bump and trailer parsing. **Mitigation**: change the *definition* in one
   place (target vs default branch) rather than adding a parallel Done-in-lane state;
   assert the trailer path is untouched.

4. **Risk**: renaming while three approved stories reference "lane" leaves the store
   internally inconsistent. **Mitigation**: do it in one pass while EPIC-74 is still
   Draft and nothing is implemented; no shim is needed today, and one is needed
   forever if this lands first.

## Open questions — advisor + ADR

- EPIC-74's DIRECTION requires a sketch, advisor signoff and an ADR before
  implementation. This rewrites the model across three **approved** stories, so it is
  a design fork: advisor's gate, not product's.
- Does the target branch belong on the **epic** only, or on any spec? Epic-only is
  simpler and matches the "epic-bound side work" framing; per-spec is more flexible
  and probably unnecessary.
- Should `AIDA_TARGET_BRANCH` be settable at all, or should the epic declaration be
  the sole source with the worktree deriving it? The variables are most defensible as
  *derived and overridable*, least defensible as the source of truth.
- Naming: `--target-branch` on the epic reads well; confirm it does not collide with
  the forge's existing `target_branch` field (GitLab's name for the PR base), which
  means the same thing and is a point in favour, not against.

## Followups

- Retire `epic-n-work` as an implicit branch name; make the branch explicit.
- Decide whether `aida lane land` deserves a convenience alias or nothing at all.

## Related

- Builds on: EPIC-74, STORY-1481, STORY-1482, STORY-1483
- References: TASK-1470 (EPIC-72 transplant — the drift evidence EPIC-74 cites)
- Reusable: `default_branch_name` (`aida-core/src/git_ops.rs`), `default_branch_of`
  and the forge's `default_branch` method (`aida-cli-lib/src/forge.rs`) — the
  existing base-selection seam. Do not introduce a second default-branch notion.
