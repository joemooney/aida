# Untrusted-node review return channel

Status: proposed; advisor signoff required before implementation.
Owning requirement: STORY-2493.
<!-- trace:STORY-2493 | ai:codex -->

## Decision requiring signoff

The intake checksum proves content integrity, not reviewer identity or review
authority. The trusted-node design explicitly excludes untrusted-node envelopes.
A node name in an unsigned envelope cannot safely become an approval accepted by
ship, train or readiness checks. Automatically returning fixed fields must remain
distinct from automatically granting review credit.

Recommended: enroll a narrowly scoped review-return signing key at home, separate
from trusted host-recorder keys. Bind it to a reviewer principal, node and home
repository identity. At home, require authenticated exact-head author-session
evidence and independent reviewer identity before accepting any approval.
This key grants neither store writes nor trusted-node publication authority.
Advisor approval must define how the work-side recorder attests reviewer/session
authority and how home verifies that attestation. A signature alone does not prove
that a review occurred. Until that contract exists, received verdicts are pending
data and provide no positive gate credit.

Alternative: require a home operator to attest each imported verdict through the
existing human-review authority path. This avoids remote reviewer authority but
adds a manual step beyond automatic verdict transport. Select this explicitly
if scoped remote attestation is unsuitable.

## Proposed implementation after approval

1. Preserve observation v1 bytes and digest rules. Add closed, versioned finding
   and review-verdict payloads with canonical ordered hashing, unknown-field
   refusal and bounded regular-file reads. Do not overload observation prose or
   reinterpret existing observation receipts as review authority.
2. Findings carry severity, blocking, title, body and explicit home-relative
   file/line. Validate path traversal, absolute paths, lengths, line bounds and
   enums. Apply reviewed sensitive patterns to every string field, including
   paths. Require explicit work-side operator review before publishing findings;
   never copy local paths, logs or identity implicitly.
3. Verdict payloads carry the PR, full head SHA, closed decision, bounded unique
   blocking digests and enrolled reviewer node. Authentication must bind the
   repository, principal and session as approved above. No free-text diagnostics
   cross this automatic channel. Reject unknown node identifiers rather than
   allowing arbitrary text in the node field.
4. Extend the isolated aida-intent transport and its complete-history validator
   with digest-addressed immutable review/finding paths. Validate both before
   publication and in pull quarantine. Include the new validator in checkpoint
   fingerprints. Preserve branch-only pushes and existing reviewed observation
   behavior; do not give the work credential access to aida-store.
5. Home explicitly processes findings into canonical finding specs using the
   existing durable processing journal/store-lock recovery pattern. Immutable
   receipts bind the full digest to the home origin ID and processing operation.
   A retry recovers the same spec rather than filing duplicates. Imported prose
   remains data, never instructions.
6. Add one checked projection consumed by ship, train and readiness. Resolve all
   blocking digests against authenticated home receipts and current canonical
   finding status. Only canonical `Done` or `Completed` clears a digest by
   status. `Draft`, `Approved`, `InProgress`, `NeedsAttention` and
   `Planned` continue blocking. `Rejected` (including `aida findings dismiss`)
   and `Superseded` do not clear a digest by status alone: rejection need not
   mean remediation, and a replacement may still be open. Missing, pending,
   corrupt or conflicting mappings also cannot clear it. Reopening a resolved
   finding blocks again. Out-of-order verdict/finding arrival stays pending.
   Approval credit is scoped to the verdict's repository, PR and exact head.
   Blocking digest obligations persist for that repository and PR across head
   changes until resolved by status or an authenticated disposition. At head B,
   a verdict for head A grants no approval credit, but its unprocessed or open
   digests still block. Missing an authenticated review for B also blocks merge.
   A fresh authorized independent verdict for B must assess B and enumerate its
   current blocking digests. Omission alone never releases an inherited digest.
   For a false positive rejected at A or a superseded finding, B's verdict must
   explicitly disposition the inherited digest, bind the originating verdict and
   authenticated predecessor chain, and reference the home dismissal/replacement
   audit evidence. Only a checked authorized independent disposition releases
   that obligation; open or unprocessed findings cannot use this exception.
   No same-head successor for A is required to review B. If B retains the old
   digest without a valid disposition, its unresolved status still blocks B.
   For the same-head case, require a new authenticated verdict for the same PR
   and exact head from the authorized independent reviewer, explicitly binding
   and superseding the previous verdict and dispositioning the omitted digest.
   Home must verify that the finding is currently `Rejected` or `Superseded`
   with the corresponding dismissal/replacement audit evidence. A same-head
   successor cannot release an open or unprocessed digest by omission or by
   claiming a false-positive disposition. Retain both verdicts and the home
   audit trail; neither a receipt rewrite nor dismissal alone releases a blocker.
   For both same-head and moved-head dispositions of `Superseded` findings,
   require every replacement to resolve through authenticated home mappings to
   a current `Done` or `Completed` finding before releasing the obligation.
   Missing, conflicting, cyclic or unresolved replacement evidence blocks;
   an open replacement cannot be bypassed by omitting either digest. Retain the
   replacement bindings and recheck their current statuses on every gate entry,
   so reopening a replacement blocks again.
7. Preserve current merge holds, reviewer independence and refusal precedence.
   Do not silently upgrade these envelopes into trusted host publications.
   Define replay, key revocation and authenticated verdict supersession in the
   approved authority contract before enabling approval consumption. The
   superseding verdict must bind its predecessor digest; reject forks and
   concurrent successors as conflicting evidence until explicitly resolved.
   Arrival order must never select the authoritative verdict.

## Critical Files

- `aida-cli-lib/src/observation_envelope.rs`: strict observation schema, canonical checksum,
  sensitive patterns and store-independent generation.
- `aida-cli-lib/src/intent_intake.rs`: isolated transport, history validation/checkpoints,
  publication, receipt and journal-based processing.
- `aida-cli-lib/src/ci_relay.rs`: fixed-field return-channel precedent, with home-derived
  inventory validation; not an authorization mechanism for reviewers.
- `aida-cli-lib/src/gate/review_publication.rs`: enrolled trusted-node signatures and checked
  synced review evidence; its trust scope must remain separate.
- `aida-cli-lib/src/gate/ledger/mod.rs` and `aida-cli-lib/src/gate/review_verdict.rs`: recorder/collector boundary;
  unsigned imports must not bypass existing recorder authority checks.

## Verification

Use two isolated clones and a bare intake hub with fake forge transport. Exercise
actual generation, publish, quarantine pull, home processing and gate entry
points. Cover approved exact-head review; request-changes with a blocking digest;
verdict arriving first; digest-to-origin-ID retry recovery; open/closed/reopened
findings; dismissed (`Rejected`) and `Superseded` findings remaining blocked;
authenticated same-head supersession releasing a false positive with current
`Rejected` status and home dismissal evidence; a same-head successor omitting
an open or unprocessed digest still blocking, including an attempted false-positive
disposition; superseded findings with open replacements blocking under both
same-head and moved-head dispositions, resolved replacements permitting release,
and missing/cyclic replacement mappings or reopened replacements blocking; replayed,
forked and concurrent successors;
head A with a rejected blocking finding followed by head B (no review blocks,
omission alone blocks, an authenticated B disposition with home dismissal evidence
releases, retaining the digest without disposition blocks, and replaying A cannot
change B's result); an open or unprocessed digest at A omitted by an approved B
verdict still blocking B, including an attempted false-positive disposition;
resolved inherited findings reopening and blocking again; stale head;
cross-node self-review; unknown or revoked signer;
tampering; unknown fields; sensitive paths/text; duplicate/conflicting receipts;
and concurrent publication. Assert work operations never mutate canonical store
refs, free-text publication requires review, and unresolved evidence never grants
merge credit. Run executed Rust tests through the lane job runner.

## Followups

Obtain advisor signoff on reviewer/session attestation, home verification and
the supersession contract before implementing this proposal. This document
provides no review authority and does not complete the owning requirement.
