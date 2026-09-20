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
