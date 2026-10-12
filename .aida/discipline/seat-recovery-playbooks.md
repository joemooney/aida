# Seat recovery playbooks

These are bounded recovery procedures for product and advisor sessions. Inspect
before mutating; preserve commits and the audit trail. <!-- trace:STORY-1351 | ai:codex -->

## Stale or diverged local branch

Fetch, compare both sides of the divergence, and inspect overlapping files.
Rebase when safe, resolve deliberately when files overlap, then realign the
local branch to the branch actually being driven. Never start new work on a
stale premise. See `git-sync-and-review.md` for the exact inspection commands.

## Stranded commit on a `*-work` branch

Confirm the commit, its intended spec, and the current PR state. If the PR is
still open, move the commit onto its live head with a normal cherry-pick or
rebase and verify the remote head. If the PR is merged or closed, do not push
to it: route the remaining change through a fresh spec/branch so it is reviewed.

## Misclassified hold-gate shelf

Inspect the CI rows and the merge-hold marker. If the only red row is the
intentional hold gate, restore the item to the merge path; do not treat it as a
product failure. If any required build/test row is red or unavailable, keep it
shelved and route a concrete rework brief.

## Store divergence

Use `aida pull` first. Inspect both store histories, reconcile rather than
overwriting, and validate with `aida cache verify` after the rebase. Never hard
reset or force-push the canonical store to make the warning disappear.

## Stacked PR after its base is squash-merged

A squash merge rewrites the base PR's commits into one new commit on `main`, so
a PR stacked on that base still carries the originals and now conflicts.
<!-- trace:TASK-1726 | ai:claude -->

1. Before merging the base, record its last head SHA (`OLD_BASE`, the base PR's
   final head commit) and the stacked PR's current head (`OLD_HEAD`). Merge the
   base WITHOUT deleting its branch: deleting it first can close the stacked PR
   instead of retargeting it (depending on how the branch is deleted).
   `OLD_BASE` must be an ancestor of the stacked head.
2. Retarget the stacked PR's base to `main` (for example
   `gh pr edit <stacked-pr> --base main`).
3. Only now delete the base branch.
4. Replay just the stacked commits onto the new `main`, then push with the lease
   pinned to the head you inspected:
   `git fetch origin`,
   `git rebase --onto origin/main "$OLD_BASE" <stacked-branch>`,
   `git push --force-with-lease=<stacked-branch>:"$OLD_HEAD" origin <stacked-branch>`.
   Re-run the gates (for example `make check-ci-fast`) and let CI go green.
5. Verify with `git range-diff "$OLD_BASE".."$OLD_HEAD" origin/main..<stacked-branch>`
   against the approved head before merging. The head changed, so any approval
   on the old head no longer covers it: the reviewer must re-record the verdict
   at the new head. If range-diff shows every commit unchanged (`=`), that is a
   re-check of the rebase; a changed patch needs a fresh review.
