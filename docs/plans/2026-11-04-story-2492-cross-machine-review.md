# Cross-machine review on trusted nodes

Status: proposed; master sign-off required before implementation.
Owning requirement: STORY-2492.
<!-- trace:STORY-2492 | ai:codex -->

## Decision requiring sign-off

Choose the authority carried by a synced review. Verdict JSON alone is
insufficient: the shipping node currently revalidates its recorder against
local seat-grant files or a local human receipt. Treating presence in the
shared store as approval authority would change that trust boundary.

Recommended: a versioned, immutable review publication receipt, issued by
the trusted host-side recorder/collector only after ordinary local authority
validation. The receiving node trusts the publishing node's validation for
that exact publication; it does not acquire a usable remote role grant.
The publisher's trust enrollment, allowed publication writers and receipt
validation rules need explicit approval. This sketch does not implement or
implicitly enable them. Untrusted work-node envelopes remain deferred.

Alternative: export the grant/parent and human-receipt evidence and rerun
validation on each consumer. This would also require defined remote
revocation, roster and process-binding semantics. Copying raw grant records
into the shared store is not an adequate implementation of that alternative.

## Existing implementation and gaps

- `aida-cli-lib/src/gate/ledger/mod.rs`: `record_verdict_at` and
  `record_collected_verdict` are the ordinary and host-collector entry points.
  Contained callers are deliberately refused ledger writes.
- `gate/ledger/event.rs`: `GateEvent` already binds a change, head, optional
  canonical spec key, actor and complete verdict body. `seq` is local to a
  change ledger; it cannot order independent clones' publications.
- `gate/ledger/checked.rs`: cross-alias readers require bounded, strict,
  no-follow reads. Remote evidence must enter these readers too, rather
  than only the lossy single-key projection in `gate/ledger/project.rs`.
- `gate/review_authority.rs`: `verified_authority_keys` revalidates against
  local grants or human receipts. `author_sessions_at` and
  `all_author_sessions` read local leases and `review-authorship.jsonl`.
  Both reviewer authority and authorship evidence need a portable form.
- `gate/seat_authority.rs`: `grant_authorized_at` checks grant validity at
  recording time, binding, roster ceiling and delegation parent. A remote
  publication must preserve this validation rather than bypass it.
- `review_watch.rs`: `Store::history` reads local legacy verdict files and
  archives, and early-returns when that directory is absent. Admission and
  `probe_unneeded` use that history and machine-local reservation state.
- `schedule_ledger.rs`: useful store-lock and exact-push CAS pattern, but
  its offline-success and last-writer-wins behavior cannot authorize a
  distributed review reservation.

## Proposed implementation after approval

1. Store immutable review publications under a versioned review namespace
   in `aida-store`, keyed by repository identity, forge/change, exact full
   head SHA and publication UUID. Include canonical spec identities,
   reviewer handle, clone/node identity, stable harness session identity,
   decision, recorded time, validated receipt and blocking finding origin
   IDs. Reject unresolved finding identities at publication rather than
   persisting machine-local display IDs as links. Persist enough authored
   session evidence to check independence on every receiving node.
2. Keep process identities node-qualified. Equal PIDs/start ticks on two
   hosts are not equal people or sessions; a session resumed on a different
   node is not independent merely because its node changed. Preserve the
   current session/author overlap rules. Define how author evidence is
   complete for the change; an absent author set must not prove independence.
3. Publish through the existing store writer lock and exact-push CAS loop.
   Immutable publications preserve distinct reviews without file-level
   last-writer-wins. Failed publication reports local pending state and
   grants no remote credit. Do not roll back unrelated store writers.
4. Add one checked read projection for synced publications, consumed by
   ship, train, readiness, alias selection and review-watch history. Validate
   schema, repository/change/head, receipt binding and finding references.
   Keep existing stale-head and carry-over policies explicit; syncing alone
   never blesses a changed head. Unsupported or unreadable remote evidence
   refuses approval rather than falling back to a cached approval.
5. Make the local ledger a derived verdict view when synced evidence
   exists. Report disagreement and select the synced verdict, including a
   synced refusal opposed by a newer local approval. Preserve local history
   for audit. Define conflicting concurrent publications conservatively:
   no timestamp-only ordering that erases an independent refusal. Keep
   unrelated local hold/train events under their existing lifecycle rules.
6. Reserve `(repository, change, head)` through a synced CAS claim before
   spawning. A rejected push re-reads the winner; an offline push never
   starts a reviewer. An ambiguous remote owner retains its reservation
   until explicit recovery or a terminal receipt. Never use local PID
   liveness to retire a different machine's claim. Checking verdicts alone
   does not prevent simultaneous reviews before either verdict exists.
7. Forge discovery supplies open PRs on either node; store sync supplies
   canonical specs, publications and claims. Document required sync ordering,
   identity setup, pending publications, disagreement diagnostics and claim
   recovery in a short second-node operator guide.

## Validation required

Use two isolated clones, distinct AIDA homes, independent sessions and one
temporary bare hub. Run the actual recorder/collector and gate entry points;
fake only the forge transport as needed for a deterministic test.

- A publishes a change, B reserves and reviews, A syncs and its train accepts
  the independent exact-head approval and lands the change.
- B requests changes with origin-ID findings; A syncs and sees both the
  refusal and the same finding objects despite different display aliases.
- A changed head, self-approval across nodes, missing authorship evidence,
  malformed publication and invalid authority all refuse approval.
- Synced refusal beats a contradictory newer local approval and reports it.
- Simultaneous watcher claims permit exactly one spawn. Offline, rejected
  and ambiguous pushes never produce a second admitted reviewer.
- Unsupported schema, read-budget exhaustion and unsafe paths cannot yield
  positive credit. Existing local verdict and hold tests remain valid.

## Current outcome

Source inspection only; no production behavior changed and no acceptance
criterion is claimed complete. Punt the story for the authority decision and
sketch approval; resume implementation after sign-off is recorded on the spec.
