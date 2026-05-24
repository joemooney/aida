# STORY-248 — Stacked-branch awareness

Date: 2026-05-24 · Specs: STORY-248 · Status: Plan · Complexity: Medium (~3 slices, each independently shippable)

## Summary

Today aida queue work TASK-Y always branches from origin/main. Starting Y while X's PR is still in CI/review/merge means Y can't see X's commits — risks duplicate work or merge conflicts on shared files. Manual `git checkout task-x && git checkout -b task-y` works but AIDA loses the dependency, so when X merges you must remember to rebase Y by hand.

STORY-248 adds stack-context tracking + automatic cascade rebase when parent merges. Eliminates 15-30min "dead time" per spec between work-complete and next pickup.

## Critical design insight (from joe's STORY-248 comment)

AIDA squash-merges + deletes branches (`gh pr merge --squash --delete-branch`). After X merges, X's original commits are NOT on main — only the squashed commit is. Plain `git rebase origin/main` from stacked Y would re-apply X's pre-squash commits onto main and CONFLICT with the squash. Cascade MUST use `git rebase --onto origin/main <recorded-parent-sha> <branch>` to strip everything up to parent + replay only Y's own commits. Recording parent_branch_sha at stack-creation time is MANDATORY.

## Three slices

**Slice 1: --stack/--base flags + lease fields**
- aida queue work --stack (auto-picks most recent in-flight implementer) / --base <branch> (explicit)
- SessionLease gains parent_branch + parent_branch_sha fields (Option, back-compat via serde default)
- Refuses --base on merged-deleted branches (NoOpenPr state)
- session_show prints "stack base: task-x (sha)"
- Pre-pickup banner shows stack base line

**Slice 2: .aida/stacks.json + aida stack subcommand**
- New aida-cli/src/stacks.rs module (Stacks BTreeMap<branch, StackEntry>)
- aida stack show [--branch <name>] — walk chain bottom-up; show PR state, lease state, parent-merged status per row
- aida stack list — list every chain on one line each
- Uses write_atomic for crash-safe writes

**Slice 3: Cascade rebase on aida pull**
- cascade_rebase_stacks after code-leg pull succeeds + auto-bump finishes
- For each branch whose parent_sha is now on main: topo-sort + git rebase --onto origin/main <parent_sha> <branch>
- Success: remove stack entry (branch is on main now)
- Failure: git rebase --abort + leave entry + name unrebased branches + point at /aida-rebase
- print_pull_plan mentions pending cascades

## Key decisions

- D1: Recording parent_branch_sha is mandatory (squash-merge constraint)
- D2: --stack / --base mutually exclusive
- D3: --stack auto-picks most recent in-flight implementer (sort by started_at desc, filter to role=implementer + non-current scope)
- D4: Refuse --base on NoOpenPr (parent merged + branch deleted) with actionable message
- D5: Skip stacks.json write when base resolves to default branch (origin/main / main) — those aren't real stacks
- D6: Cascade removes entries on success (branch is now on main; nothing to track)
- D7: Cascade aborts on conflict + names unrebased branches + points at /aida-rebase
- D8: No --auto flag for MVP — aida pull is operator-typed so opt-in is implicit; interactive prompt if TTY, auto if non-interactive

## Critical Files

- aida-cli/src/cli.rs (--stack / --base flags on QueueCommand::Work; Command::Stack(StackCommand) variant)
- aida-cli/src/main.rs (SessionLease parent_branch+parent_branch_sha; handle_queue_work resolution; session_start capture-parent-sha; session_show "stack base" line; handle_pull_command cascade_rebase_stacks; handle_stack_command)
- aida-cli/src/stacks.rs (NEW — Stacks/StackEntry/load/save/record/remove/chain/all_chains; module-level mod declaration in main.rs top)

## Reusable helpers

branch_exists_anywhere (--base validation), detect_open_pr_for_branch (merged-branch refusal), list_leases (--stack auto-detect + worktree-find), detect_default_branch_ref (skip-stacks-write for default), current_branch_at (aida stack show cwd resolution), write_atomic (crash-safe stacks.json), aida_core::rebase::detect (richer classification on cascade failure if wanted — MVP doesn't require).

## Tests

stacks::tests: chain (linear walk, cycle detection, root-not-in-map termination), all_chains, JSON round-trip
session_start integration: base: Some("task-x") populates parent_branch + parent_branch_sha + stacks.json entry
handle_queue_work: --stack with no in-flight implementer prints fallback note + proceeds (no stacks.json write)
cascade_rebase_stacks: ancestor parent_sha → rebase succeeds + entry removed; failure path leaves entry + names branches
--base on merged-deleted branch: mock NoOpenPr → bails with merged-branch message

## Verification

Multi-step bash: --stack picks most recent implementer; --base task-y explicit; --base merged-deleted refused; aida stack show/list; aida pull cascade rebases when parent on main; conflict path shows /aida-rebase hint.

## Followups (5)

- --auto-complete --pipelined (orchestrator picks next batch member at phase 4 merge instead of phase 6 build) — bigger surface, own STORY
- --force for --base merged-deleted if use case appears
- Multi-stack rebase ordering for multiple independent stacks in one pull (MVP handles each chain independently)
- Stack-aware reviewer view (aida queue work PR-N with PR-N's parent state awareness)
- Surface stacks in aida status + TUI overlay (EPIC-26)

## Composes with

STORY-246 (--auto-complete — natural sibling; --pipelined Followups will call this surface), TASK-103/104/105 (aida rebase — per-branch primitive cascade rides on), /aida-rebase skill (conversational fallback when cascade hits conflict), BUG-114 (precondition — already Completed per joe's comment).

(Full plan generated by web /ultraplan 2026-05-23 evening; canonical text in session chat record. PR-272 +1716/-7 is the implementation.)
