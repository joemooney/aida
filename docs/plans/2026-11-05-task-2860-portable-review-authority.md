# Portable review authority and distributed admission

<!-- trace:TASK-2860 | ai:codex -->

Approved design sketch for TASK-2860, supporting STORY-2492. Documentation only;
this task's 2026-11-05 triage explicitly excludes production implementation.
TASK-2860 comment 3 records the 2026-11-05 12:00 MST advisor signoff: APPROVED.
The publication contract and reservation state table are approved as written,
with the OpenSSH `sshsig` signer. The topology ruling is incorporated below.

## Decisions already recorded

STORY-2492's master sign-off at 2026-11-05 05:19 UTC approved the portable
publication receipt, operator enrollment and synced CAS reservation direction.
Its 05:35:54 UTC amendment (the ruling labeled 22:40 local) specifies OpenSSH
signatures, not the grant-keyring secret-possession API. Use canonical JSON
and detached `sshsig`, namespace `aida-review-receipt`, with an Ed25519 node
key and roster-derived allowed signers whose principal is the node ID.
The timestamp label in the ruling, rather than a new signer design, is the
authority for this choice. Slice 1 is receipts/roster/consumers; slice 2 is
watcher discovery and distributed head reservations. Neither slice by itself
completes the entire story.

Slice 1 landed on `origin/main` in merge commit `0b0f8a6b701` (PR #3411),
and this branch incorporates that mainline work. Its approved sketch is
[Cross-machine review on trusted nodes](2026-11-04-story-2492-cross-machine-review.md);
[second-node setup](../agents/second-review-node.md) describes operation.
`aida-cli-lib/src/gate/review_publication.rs` implements the enrolled node
roster, OpenSSH-signed publication/authorship receipts and consume-side
validation. `gate/review_authority.rs::verified_authority_keys` validates
portable identities through `review_publication::verified_identity`, and
`gate/review_alias.rs` projects synced verdicts through
`review_publication::current`. Slice 2 builds on these landed paths and their
receipt format; it does not introduce a second format.

## Existing trust boundary

`gate/review_authority.rs::verified_authority_keys` validates local grants
and their delegation/roster context or local human receipts, and now validates
portable publication identities through the landed slice-1 consumer.
`author_sessions_at` and `all_author_sessions` supply local authorship evidence;
slice 1 adds signed portable authorship evidence for remote independence checks.
Copying unsigned verdict JSON alone still supplies neither authority nor
portable authorship validation.

`review_watch.rs::Store::history` reads local verdict files/archives and
returns early when the verdict directory is absent. Its runtime reservation
protects local dispatch only. `schedule_ledger.rs` has a useful exact-push CAS
pattern, but deliberately treats an offline local commit as pending success;
that behavior cannot admit a remote reviewer.

## Portable publication contract

An operator explicitly enrolls a node/public-key pair. The host-side
recorder/collector alone signs a publication after its normal local recorder
authority checks succeed. Node trust means trusting that host to make that
validation; a signed document never grants a consumer a remote seat or grant.
Keep keys private on the publishing host; do not export grants, parents,
human receipts or signing secrets. Use the amended slice-1 key lifecycle.

The signed payload binds schema/version, repository and forge identity,
change number, full head SHA, complete bound canonical spec set, publication
ID, publisher node, reviewer actor/session identity, decision, recording time,
blocking finding origin IDs and references to signed authorship evidence.
Sign all security-relevant fields using slice 1's canonicalization. Reject
unknown versions, duplicate/ambiguous fields, invalid signatures and
unresolved finding identities before awarding credit. Publication identity
is `(node, publication ID)`; local ledger sequence and wall-clock time do
not order publications from different nodes.

Authorship evidence must bind the actual change/head and bound spec set,
including multi-spec PRs. Preserve authoring sessions across rework heads;
publish fresh evidence on open, adopt and each relevant push. A receiving
node combines portable evidence with locally known authors. Missing or
incomplete coverage cannot prove independence. Node-qualified process IDs
prevent PID collisions; persistent harness/session identities prevent an
author resumed on another node from becoming an independent reviewer.
Apply existing actor/session independence rules, not a new rule that all
sessions under the same human account are automatically identical.

Verify against the current enrolled, unrevoked roster, exact current change
head and complete spec coverage on every consuming node. Revocation removes
future authority without retroactively undoing merges. Use one checked
projection for ship, train, readiness, aliases and watcher history. Bound
reads by change/head and existing safe-file rules; unrelated malformed
history must not disable the entire repository. A malformed relevant record
cannot yield approval or restore a contradictory cached approval. Distinguish
absent evidence (including stale/revoked-only evidence) from unsafe evidence,
and report the former so an ordinary fresh authorized review can recover.

Synced verdicts govern the derived local verdict view and disagreements are
reported. Preserve local audit and unrelated hold/train events. A reviewer
may explicitly supersede its own prior publication through causal links;
an independent refusal remains blocking until explicitly resolved under the
approved review policy. No timestamp-only winner may erase it. Publication
failure preserves a durable local pending result, reports an actionable retry,
and awards no remote approval. Normal request-changes must carry filed finding
origin IDs through the real recorder and collector without manual JSON edits.

## Approved reservation topology and transitions

The 2026-11-05 12:00 MST advisor ruling selects a single authoritative
admission hub: GitHub origin (`github.com/joemooney/aida`), at
`refs/aida/review-admission`. The hub URL and ref must be explicit configuration
under `[review]`, using `admission_hub` and `admission_ref`; they are never
inferred from a node's `origin` alias. For this topology:

```toml
[review]
admission_hub = "https://github.com/joemooney/aida.git"
admission_ref = "refs/aida/review-admission"
```

This is the slice-2 configuration contract, not an implemented setting in this
documentation task. GitLab is a mirror only: it distributes evidence and never
admits. The hourly reconcile bridge copies evidence and never admits. All
reviewing nodes must reach the configured GitHub authority. If it is unreachable,
admission refuses with a named `admission_hub_unreachable` error and spawns no
reviewer; there is no failover during a partition. A successful CAS on another
writable hub grants no admission. A topology change is an operator migration
with admission stopped and old claims reconciled before admission resumes.

The logical key is `(repository, forge, change, exact head)`; each generation
has an unpredictable claim token, owner node/session, signed transition and
explicit predecessor. Approved transitions are:

| State | Allowed next state | Admission effect |
| --- | --- | --- |
| Absent/terminal | Reserved, with a new generation/token | None until authoritative acceptance |
| Reserved | Running, same owner/token | One dispatcher consumes the accepted token |
| Reserved | Cancelled after proven pre-spawn failure | Retry allowed after accepted cancellation |
| Running | Terminal with publication/exit receipt | Releases that generation |
| Reserved/Running | Recovery after explicit owner/operator reconciliation | Never infer exit from age or a remote PID |

Under the existing store writer lock, fetch/pin the authoritative tip, inspect
current synced verdicts and claims, apply one transition, commit and perform
the exact expected-tip push. On rejection, reload the winner and reconstruct
the transition; do not union-merge conflicting admission records or blindly
rebase a losing claim into a new accepted claim. Preserve unrelated writers.
CAS is enforced by AIDA clients using
`git push --force-with-lease=<ref>:<expected> <hub-url> <new-tip>:<ref>`,
where `<ref>` and `<hub-url>` are the explicit admission configuration and
`<expected>` is the fetched, pinned tip (empty only for initial ref creation).
Each new commit must descend from that tip; the explicit lease rejects a
concurrent update rather than overwriting it. This is client-side CAS, not a
claim of GitHub branch/ruleset protection for the custom `refs/aida/` namespace.
The trust assumption is that only cooperating AIDA writers update this ref:
no operator, bridge, mirror or other credential holder force-pushes, deletes or
rewinds it outside this protocol. GitHub does not enforce that assumption for
this custom ref. Operator recovery uses the stopped-admission migration above.
Bound retries; exhaustion means no spawn.

Require a definite successful push and confirmation of the accepted token
before dispatch. A lost response, offline push or uncertain child spawn keeps
the claim occupied and permits no retry spawn until reconciliation. Local
token consumption retains the existing process/start binding and permanent
runtime lock discipline. A crash between acceptance and spawn leaks capacity
safely. A remote PID, elapsed lease time or clock disagreement never proves
the old child stopped. Recovery records who verified cessation and fences the
old token before a new generation is admitted; it does not terminate processes.

Keep the existing local two-review ceiling. Before admitting a new head of the
same change, resolve any older active generation; a changed head does not
allow overlapping reviewers to bypass that rule. A terminal refusal is review
history, not permission to repeatedly review the same head. Discovery obtains
open changes from the forge after store synchronization, then consults synced
history even when the local verdict directory does not exist. Offline cached
history can explain state but cannot authorize a fresh distributed admission.

## Validation and implementation handoff

Use two isolated clones/homes, real throwaway OpenSSH keys, one bare hub and
fake forge transport. Exercise actual recorder/collector and ship/ready/train
entry points; testing only a pure approval predicate is insufficient.

- Independent B approval is consumed by A at the exact head without B's local
  grant or human receipt; A's ship/train path can land it.
- Ordinary B request-changes publishes findings by origin ID; A sees the
  refusal and findings despite different display aliases. Test multi-spec PRs.
- Stale heads, resumed authors, incomplete authorship, invalid signatures,
  revoked nodes, unsafe paths, unknown schemas and bounded-read failures
  never supply approval. Recovery permits a fresh authorized review.
- A synced refusal overrides a contradictory local approval and the gate
  emits the disagreement report. Local holds remain enforced.
- Simultaneous claims yield one accepted token and at most one spawn. Inject
  rejected pushes, response loss, disconnect, crashes before/after spawn,
  reboot, equal PIDs on different nodes and superseded heads.
- Two writable hubs under partition cannot each admit: only the designated
  authority may accept claims. A loser reloads without resurrecting its claim;
  unrelated store changes survive retry and explicit recovery.

Implementation belongs to STORY-2492 slice 2, under its own acceptance-backed
spec linked to STORY-2492. The authoritative hub/ref decision is already
recorded by the 2026-11-05 12:00 MST APPROVED signoff. Slice-1 PR #3411 is
merged; use its landed roster, signed receipts and consume-side checks as the
foundation. The story remains incomplete until all story acceptance is
satisfied. Do not weaken gates to make the end-to-end test pass.

This sketch incorporates the advisor's approved topology, publication contract
and reservation state table with the OpenSSH `sshsig` signer. No production
gate, schema or reservation is activated by this documentation task.
