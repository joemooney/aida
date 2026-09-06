# Substrate as bouncer, not passive rules

Rules in prompt text are useful, but AIDA's reliability improvements come
from programmatic gates that intercept unsafe lifecycle states.

## Headless text-question gate (BUG-354 / BUG-374)

In `--no-human=both`, a headless implementer must not ask a plain-text
confirmation question and exit with no PR. A final message like "A or B?
Please confirm" has no human recipient inside the subprocess, so it is a
design-fork punt.

The orchestrator now inspects the terminal headless JSONL `result` event
when phase 1 exits cleanly with no PR. If the final answer contains
decision-fork question wording such as "which path", "should I", or
"confirm and I'll proceed?", it files a design-fork punt instead of
returning a generic phase-1 `NoPr` failure. The existing STORY-306 advisor
tier then resolves or escalates the fork.

The `/aida-pickup` instructions still tell agents to punt explicitly; this
gate is the bouncer for the recurring ceiling-pattern case where prompt
discipline fails.

## Client-side trailer spec-ID validation (STORY-469 Guard 1)

`aida pr ship` and `aida queue done` validate the `(SPEC-ID)` trailers on the
commits the current branch adds over the default branch *before* the work
ships / the spec flips to Done. Every trailer id must resolve to a **live
(non-rejected) spec** in the substrate; a hallucinated, typo'd, or
since-rejected id is refused (exit 1) with the offending sha + id + reason.

This is the client-side twin of the server-side `aida trace gate` (STORY-498,
which runs in CI). Same pure validator (`validate_trailer_references`) and store
resolver (`resolve_spec_in_store`) — only the call site differs. Catching the
dead reference here keeps it out of shared git history entirely, instead of
failing CI after the commit is already pushed.

**Applies when:** committing with a `(SPEC-ID)` trailer and then running
`aida pr ship` or `aida queue done`. Plan commits (`docs(plans): …`) and
commits with no trailer are exempt (nothing to validate).

**Bypass:** `aida pr ship --no-trailer-check`, or `aida queue done --force`.
A store-less checkout (no requirement graph reachable) cannot corroborate, so
the guard soft-warns and proceeds rather than blocking a legitimate ship.

## Local-vs-substrate divergence at status time (STORY-469 Guard 3)

`aida status --cleanup` (and the inline cleanup summary on plain `aida status`)
surfaces a **"Claimed Done but substrate disagrees"** category: a spec whose
status is Done/Completed but whose local reality contradicts the claim. Two
signals fire:

1. an active lease covers the spec **and** its worktree has uncommitted
   modifications (work still on disk despite the Done claim), or
2. no commit references the spec **and** no PR exists (the "I shipped" claim has
   no substrate evidence at all).

This catches an agent's verbal "I shipped" that doesn't match substrate state —
the operator sees the divergence at status time instead of trusting the claim.
Detection is a pure function (`detect_claimed_done_divergence`) fed
filesystem-derived facts, so it stays testable in isolation.

**Bypass:** none needed — it is a read-only surface. Resolve by committing +
shipping the work, or reopening the spec
(`aida edit <SPEC> --status in-progress`).
