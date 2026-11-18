# Lane publication ownership sketch

Status: direction approved by advisor-117 on 2026-11-16; implementation is split into three separately reviewed slices.

<!-- trace:BUG-2633 | ai:codex -->

An implementer must only publish its leased spec branch. Integration owns
`integration` and `promote/*`, including promotion PR metadata. A promotion
review may generate work for several specs without transferring ownership of
the promotion to those implementers.

## Observed implementation

`queue_cmd/work.rs::resolve_rework_review` selects change-keyed reviews using
ledger bindings as well as the spec's open change. A binding establishes that
a change concerns a spec; it does not establish exclusive branch ownership.
`review_verdict_display.rs::rework_findings_comment` copies all findings,
surviving findings, and fallback review prose into the handoff.
`gate/review_verdict.rs::RecordedVerdict` represents findings as `Vec<String>`;
there is no per-finding canonical spec binding. ID mentions cannot establish
ownership, especially for cross-spec findings and promotion-only findings.

## Proposed boundaries

1. Resolve publication authority from the host-verified live lease and its
   exact branch, never from a branch-name substring or child environment.
   Reserved integration branches remain integration-owned even if a malformed
   implementer lease names them. Unknown or conflicting ownership refuses.
2. Enforce remote writes at a host-controlled boundary. A Git hook or `gh`
   wrapper alone is insufficient: raw Git can bypass hooks and forge API calls
   can bypass `gh pr edit`. Native lanes with ambient host credentials need a
   separate enforcement decision: withhold write credentials and mediate
   publication, or deploy an independently enforced credential/remote policy.
   Read-only Git metadata alone does not prevent a network push.
3. For mediated pushes, validate every destination ref, including explicit
   refspecs, deletion, force pushes, and multi-ref pushes, before sending any
   update. Permit only the exact owned branch. Refusals name the integration
   seat for reserved branches and the recorded owning seat for foreign lanes.
4. For mediated PR title/body edits, resolve the PR's repository and head
   branch through the trusted forge reader; require the exact owned repository
   and branch before mutation. Failed or ambiguous lookup refuses. Integration
   retains promotion editing authority. Caller-supplied PR numbers confer none.
5. Add canonical spec attribution to promotion findings in the durable review
   format, preserving the legacy string format for single-spec reviews.
   Attribution must be recorded by the authorized reviewer, not inferred from
   text. Shared findings explicitly list each affected spec. Unattributed
   promotion findings require advisor triage; do not import the whole verdict.
6. Render spec-scoped handoffs from that attribution, including surviving
   findings. Do not fall back to unfiltered summary or review body. Keep the
   promotion key only as source provenance, explicitly identify the spec lane
   as the work target, and omit commands directing it to edit the promotion.

## Decisions requested

- Approve host-mediated publication with no ambient lane write credentials,
  or identify the existing independently enforced boundary to extend for raw
  Git and forge API writes, including native lanes.
- Approve additive canonical per-finding spec attribution and fail-closed
  advisor triage for legacy unattributed promotion reviews.

## Validation after approval

Exercise real mutation entrypoints with a fake remote/forge and assert zero
remote mutations for reserved branches, foreign branches, explicit refspecs,
multi-ref pushes, deletions, foreign PR edits, and unknown lookup evidence.
Include positive controls for the implementer's exact branch and integration
publication. Exercise alternate raw Git/API routes at the chosen boundary.

Render a promotion verdict containing two spec findings, a shared finding,
promotion-only findings, and surviving findings. Each spec sees only its own
and explicitly shared work, with no promotion edit target. Legacy unscoped
promotion evidence refuses automatic handoff. Existing single-spec review
behavior remains covered.

## Slice boundaries after sign-off

The 2026-11-16 advisor ruling assigns host-mediated publication guards to
slice 1. It reuses this branch and carries a non-completing Refs trailer;
closed PR #4168 stays closed. Host-recorded lease checks fence the ordinary
seat publisher and the alternate rework publisher before import or push.
The host validates forge head branch and repository evidence before repairing
a PR title or annotating its body. Reserved refs remain integration-owned,
including when an implementer lease incorrectly names one.

Slice 2 removes ambient write credentials from native lanes and supplies
a host exercise. Slice 1 does not constrain raw Git or direct child forge
API calls, and does not claim the complete security boundary above.
Slice 3 adds canonical finding attribution and filtered rework handoffs.
Only the final slice may complete the owning bug.

The supplied reviewed codegraph client
`/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph` is absent;
`aida doctor check codegraph` reports it missing. Navigation used `rg`.
