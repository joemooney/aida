# Seat recovery playbooks

- Stale/diverged branch: fetch, compare both sides and overlapping files,
  rebase deliberately, then realign the local branch being driven.
- Stranded `*-work` commit: verify its spec and PR state; move it to a still-open
  head, but route remaining work through a fresh branch if the PR is merged.
- Hold-gate shelf: an intentional hold-only red returns to the merge path; any
  real red/unavailable required check gets a concrete rework brief.
- Store divergence: use `aida pull`, reconcile histories without overwriting,
  then run `aida cache verify`; never force-push the canonical store.

Inspect before mutating and preserve commits/audit history.
<!-- trace:STORY-1351 | ai:codex -->
