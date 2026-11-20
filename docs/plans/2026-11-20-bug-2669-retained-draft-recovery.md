# Retained reviewer-draft recovery after the launcher exits

Status: PROPOSED 2026-11-20. Awaiting advisor signoff; nothing here is
implemented. Decisions D1 to D3 below need a ruling before any code.

<!-- trace:BUG-2669 | ai:claude -->

## Problem

A headless reviewer writes a data-only draft
(`<review worktree>/.aida/review-verdicts/draft-<uuid>/draft.json`) and the
launcher that spawned it records the verdict
(`docs/plans/2026-11-13-bug-2564-reviewer-verdict-handoff.md`). Everything that
authenticates the draft lives in the launcher process:

- the directory descriptor opened before the spawn (`LaunchDraft`);
- the reviewer child grant, bound to the launcher's process and, when
  available, to a secret in its session keyring (`VerifiedReviewer`).

When the launcher dies before recording, the draft is still on disk but no
process can record it. Teardown keeps the checkout because it holds an
unrecorded draft, the next launch creates a new draft directory and never
looks at the old one, and a full review runs again. On 2026-11-20 an APPROVED
draft for PR #4347 was stranded this way and recorded by hand.

The regression
`bug_2669_retained_draft_after_launcher_restart_is_not_recorded_and_stays_retained`
(`aida-cli-lib/src/review_draft.rs`) pins that behaviour: nothing reaches the
ledger, the old grant is refused, and the draft stays unconsumed.

## Constraints

1. No credential enters the vendor sandbox (unchanged from the handoff design).
2. No impersonation. A later process never presents the first launch's
   reviewer grant, never receives its key and never rebinds it. The grant
   binding rules stay as they are.
3. The ship gate accepts a grant-attested approval only when the attesting
   grant was authorized at the record's `recorded_at`
   (`grant_authorized_at`, `gate/review_authority.rs`). The first launch's grant
   may be expired by recovery time, so a recovered record needs a grant that
   is live when it is written.
4. Every missing or inconsistent piece of evidence refuses. A refusal leaves
   the draft where it is.

## Proposal

Recovery is a new act by the recovering launcher under its own authority, with
durable evidence that an authorized launcher registered the draft slot before
the reviewer ran. It does not continue the dead launch.

### 1. Launch receipt

Before spawning the reviewer, the launcher writes a receipt to
`~/.aida/review-handoffs/<draft-uuid>.json`, beside `session-grants/`
(exclusive create, mode 0600, same lock and atomic-write helpers as grants).
A lane sandbox cannot write there.

Fields: version, draft launch id, project root, canonical review worktree,
the draft directory's device and inode, PR number, full head SHA, routed
reviewer session id, launcher grant id, principal, creation time, state
(`open`, `consumed`, `superseded`, `discarded`). The reviewer child grant id is
added when the spawn path can surface it before waiting; it is evidence, not
a precondition.

The receipt holds no secret and is not a capability: reading or copying it
grants nothing. It is the record that a launcher with reviewer delegation
registered this slot for this PR, head and reviewer session.

### 2. Seal

When the reviewer exits and the launcher has read and validated the draft, it
writes the draft's SHA-256 into the receipt before the first recording
attempt. A draft whose recording or publication then fails terminally is
sealed. A draft whose launcher died while the reviewer was still running is
unsealed.

### 3. Recovery

Runs only in a process whose own grant delegates `reviewer`, the same
`validate_child_delegation` check that gates launching a review. In order:

1. Find the worktree's unconsumed drafts and load each receipt by launch id.
   No receipt: not recoverable.
2. Receipt matches the project root, worktree, PR and current principal, and
   its launcher grant delegated `reviewer` when the receipt was created.
3. Open the draft directory with the existing no-follow helpers; its device
   and inode equal the receipt's. `draft.json` passes the existing checks
   (bounded, regular, singly linked, closed schema) and names the receipt's
   PR and head.
4. Sealed: the digest matches. Unsealed: decision D1.
5. The forge head still equals the receipt's head. Otherwise a fresh review
   is required and the draft stays as a record of its findings.
6. No newer verdict exists for the change (the existing replay check). If the
   ledger already carries this launch's stamp with the same digest, only the
   consumption marker is completed.
7. Mint a new reviewer child grant under the recovering launcher and build the
   existing `VerifiedReviewer` from it, with the receipt's reviewer session as
   the recorder identity. The existing checks then run unchanged: the reviewer
   identity must not overlap the launcher's, and an approval is refused when
   that session authored the work.
8. Record through `record_reviewer_draft`, verify with `verify_recorded_draft`,
   rename the draft to `recorded.json`, mark the receipt consumed and revoke
   the recovery grant.

The record's `reviewer_draft` stamp gains
`recovered: {receipt_sha256, original_launcher, recovered_by}`, so the ledger
and `aida review verdict` show that the verdict was recovered and by whom. A
lock on the receipt plus the existing launch-and-digest replay check make
recovery exactly-once across concurrent launchers.

### 4. Supersession

Before a launcher registers a new receipt for a worktree, it either recovers
each earlier draft there or marks its receipt `superseded`. A superseded
unsealed draft is never recoverable. This closes the one path by which a
non-reviewer could alter an unsealed draft: a later vendor session launched in
the same worktree (for instance a resumed implementer thread, whose own draft
is refused) editing the earlier draft so that it is recorded under the earlier
independent session.

### 5. Triggers

- Reviewer launch for a PR: recover first. A recovered verdict at the current
  head replaces the vendor run.
- Teardown and reap, where an unrecorded draft pins the checkout today:
  attempt recovery when the process holds reviewer delegation; otherwise keep
  the checkout and name the command.
- Operator commands `aida review draft list | recover | discard`, registered
  from their own module. `discard` is the deliberate removal the retention
  message already asks for.

## Authority analysis

| Path | Who can reach it | What stops a seat without the grant |
|---|---|---|
| Write a receipt | A launcher with reviewer delegation | Receipts live in the user's AIDA home, outside every lane sandbox |
| Record a recovered draft | A process whose live grant delegates `reviewer` | `validate_child_delegation`, then the unchanged record-time checks |
| Alter draft content | Whoever can write the review worktree | Seal digest; supersession; head pin; D1 for unsealed drafts |
| Discard a draft | Operator command | Same delegation check; the findings file is renamed, not deleted |

A recovering launcher could instead launch a new reviewer and record its
draft, so recovery gives no seat a power it lacks, provided the draft is
authentic. Authenticity rests on the receipt (not writable from a sandbox),
the pinned directory identity and the seal.

Residual exposure: an unsealed draft can be rewritten between the launcher's
death and recovery by a process that can write the review worktree. The same
writers can already do so during a live launch; the exposure grows in time
only. Supersession removes later vendor sessions from that set; a host process
running as the user is outside the model, as it is for the grant store.

No path fails open: a missing receipt, an unreadable directory, a failed
forge lookup, a lock timeout or an expired recovering grant all leave the
draft unrecorded and retained.

## Alternatives rejected

- Store the first grant's key so a later process can present it: a durable
  bearer credential that undoes the process binding.
- A long-lived supervisor holding the grant: a new always-on authority holder.
- Record under the recovering launcher's own seat grant: the verdict would no
  longer be a reviewer-seat record independent of the launcher.
- Keep discarding: costs a review round and strands the checkout.

## Decisions needed

- **D1, unsealed drafts.** (a) Recover any verdict automatically, relying on
  the receipt, directory identity, head pin and supersession. (b) Recover
  unsealed request-changes, rejected and waiting drafts automatically, and
  require the operator command at a terminal for an unsealed approval.
  (c) Never recover unsealed drafts. Recommended: (a). An approval stranded by a
  launcher that died before collecting, as reported for PR #4347, is unsealed,
  so (b) would still need a human for that case, though with one command.
- **D2, receipt location.** The user's AIDA home (recommended: the same trust
  root as grants, host-local like the worktree) or the project store (visible
  to other hosts, but writable from lane sandboxes).
- **D3, age cap.** Automatic recovery only within 24 hours of the receipt
  (recommended, matching the maximum grant age); older drafts need the
  operator command.

## Slices after signoff

1. Receipt, seal and `aida review draft list`. No authority change.
2. Recovery core, supersession and the launch trigger. Strict-tier review.
3. Teardown trigger, `recover` and `discard` commands, `docs/lifecycle.md` and
   the review skill.

Drafts written before slice 1 have no receipt and stay a manual case.

## Tests required

- After a launcher restart, a sealed draft and (per D1) an unsealed draft are
  recorded exactly once at the pinned head under the recovery grant, with the
  original reviewer session as recorder identity and the `recovered` stamp.
  This replaces the expectation of the current regression.
- Refusals: no receipt; receipt for another PR, worktree, root or principal;
  replaced draft directory; digest mismatch; moved head; newer verdict;
  superseded receipt; recovering grant without reviewer delegation, expired or
  revoked; reviewer session that authored the work; age over the cap.
- Two concurrent recoveries produce one ledger record.
- A recovered request-changes draft keeps its findings and arms the hold.
- An interrupted recovery (recorded, not yet marked consumed) completes
  without a second round.
- Supersession: a new launch in the worktree makes the earlier unsealed draft
  unrecoverable.
