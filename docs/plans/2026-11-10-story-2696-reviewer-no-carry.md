# Authenticated reviewer carry-over policy

Status: approved by advisor Joe on 2026-11-10; implementation follows this sketch.
Owning requirement: STORY-2696.
<!-- trace:STORY-2696 | ai:codex -->

The advisor ruling of 2026-11-10 selects an extension of the existing signed
publication receipt, with `carry_eligible: bool`, missing meaning true. There
is no second receipt type, migration, expiration or removal mechanism. The
marker is permanent schema. This sketch implements that selection; it does
not reopen the design fork. Advisor signoff is recorded on the owning spec.

## Publication schema v2

Advance newly emitted signed review publications from version 1 to version 2
in `aida-cli-lib/src/gate/review_publication.rs`. Continue reading v1 and v2.
Add a boolean `carry_eligible` with a true deserialization default and omit it
when true during canonical serialization. That preserves the exact canonical
bytes of existing v1 publications, whose signature verification currently
reserializes the parsed receipt. New false values are part of the signed
payload. Keep the OpenSSH namespace, trust roster, publication identity,
supersession rules and `verified_authority_keys` authority boundary.

Project the signed value into the local verdict's recorder attestation and
include it in parsed verdict equality. `verified_identity` already compares
the entire parsed verdict to the signed projection: changing a projected
false to true must refuse authority, including after a proof was cached.
Malformed boolean values and unsupported schema versions must refuse.
Authority-map keys distinguish false policy from true, so a valid sibling
projection cannot lend authority to a tampered policy on the same receipt.
Legacy true authority keys retain their encoding.

## Local recording

Expose `aida review record --no-carry` for approvals; reject the flag on other
verdicts before mutation. An approval without the flag records eligibility
as true. Changing policy is a fresh reviewer recording, preserving the old
round and the publication's existing supersession behavior.

Carry the value through the shared record handler, spec record and PR
handshake, `RecorderAttestation`, and the existing human recording receipt
in `review_authority.rs`. Missing values default to true on legacy local
evidence. For human approvals, require the attestation policy to match the
durable receipt during `verified_authority_keys` validation. For granted
reviewers, retain the existing grant/time/binding checks; the marker has the
same local trust boundary as the existing recording attestation. Do not
introduce another policy receipt or claim protection against arbitrary
same-user rewriting of local grant/verdict evidence.

## Carry-over selection and proof-cache key

Keep an ineligible approval valid for its exact reviewed head. Only refuse
its extension to a later head. Bind eligibility to the effective selected
approval, using the existing append/supersession and authority rules; never
infer it from branch metadata, prose or an unrelated recording. Conflicting
effective review evidence continues to refuse under the existing policy.

Thread that selected policy through the shared moved-head/carry-over path
used by ship, drain, merge-hold, mandate, status and watcher readiness.
Check it before consulting cached proofs or doing protected-base/Git work.
An authenticated false yields an explicit reviewer-policy refusal requiring
a fresh review of the new head. It grants no override or merge authority.

Bump `review_carryover::PROOF_VERSION` from 1 to 2 and explicitly add
`carry_eligible` to both carry-over proof and refusal ledger events. The
logical cache key becomes `(change, approved_sha, head_sha, protected_base,
base_tip, proof_version, carry_eligible)`. Extend the lookup/match in
`ledger/project.rs` and `review_carryover::stored_answer` accordingly, with
a true default for old event fields. Old proof-version-1 events are readable
but cannot satisfy a v2 lookup. Eligibility is revalidated before every
lookup; a cached true proof cannot authorize a newly false recording.
Keep policy refusals separate from commit-only deterministic refusals so
one recording's policy cannot poison a later eligible recording's proof.

## Targeted acceptance evidence after implementation

- Existing canonical v1 signed bytes and legacy local receipts still verify;
  missing policy is true. Version-2 true/false round trips preserve policy.
- Signed payload/projection tampering, malformed values, and local human
  receipt policy mismatch refuse through the existing authority boundary.
- A reviewer can record no-carry; implementer/unbound recorders remain
  refused. Spec and PR handshake copies agree, including retained rounds.
- No-carry approves the exact head, but refuses a clean base merge and a
  moved head for which an eligible proof was already cached.
- A later eligible authenticated recording uses the eligible cache key;
  old-version or different-policy events cannot satisfy it.
- Shared ship/drain/merge-hold/mandate/watcher consumers agree, and
  unrelated or superseded records cannot supply or clear the marker.

Run executed-test filters through `aida lane job run -- cargo test --profile
agent -p aida-cli-lib --lib -- <filter>` (CLI-local modules include private
fixtures), plus the applicable formatting and schema/CLI guards. The legacy
regression keeps a stored OpenSSH-signed v1 payload and public key and checks
canonical bytes as well as the signature. Version-1 proofs are recomputed,
never converted into refusals merely because the version changed.

## Handoff

The advisor approved this concrete sketch in the owning spec comments. The
schema and cache changes are implemented together so policy cannot be
recorded without enforcement. The reviewed
host codegraph executable configured in AGENTS.md was unavailable in this
seat; source relationships above were inspected with `rg` and direct reads.
