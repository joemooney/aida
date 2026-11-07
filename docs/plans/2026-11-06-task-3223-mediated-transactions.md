# TASK-3223 mediated-transaction sketch v1 — UNAPPROVED

<!-- trace:TASK-3223 | ai:codex -->

This is a proposal, not an implemented persistence contract or permission cutover.
TASK-3225's first comment records the binding advisor amendment: S2 cannot
enable mutation verbs until a separate recoverable-transaction sketch is approved.
The parent operator ruling approves synchronous mediation and the shared bwrap
boundary; it does not select this transaction design. TASK-3223 remains guided.

**Status: unapproved v1 design proposal, documentation only.** No independent
advisor approval is recorded on TASK-3223. A code-review verdict or merging this
document does not supply that architecture approval. The full S2 contract stays
open; this sketch implements none of its acceptance criteria.

## Implementation scope

The following acceptance work remains unimplemented by this sketch:

| TASK-3223 contract | Current evidence / remaining work |
| --- | --- |
| Route every inventoried CLI/MCP writer through mediation | Service accepts only `session_probe` / `request_status`; typed mutation routing is pending. |
| Deny raw store, authority metadata and shared-cache writes | Existing store/cache write grants remain; protected ancestor, symlink, primary-checkout and resume layouts need implementation and tests. |
| Provide lane-private or read-only derived caches | No cache cutover is implemented here. |
| Gate the mediated profile and preserve Claude/native compatibility | No gated mutation profile or workflow validation is implemented here. |
| Prove stale-base, concurrent-mutation and persist-before-reply recovery | These tests remain required after transaction approval and implementation. |

Independent advisor signoff must select the redo-transaction design (or an
alternative), canonical reader/writer recovery admission, and the multi-root
claim/queue participant protocol. Until then this document remains an unapproved
proposal, even if retained in repository history as a Refs-only sketch.

## Observed implementation

- `store_mutation/service.py` permits only `session_probe` and `request_status`.
  Its accepted-request map is memory-only, appropriate for those read-only verbs.
- `store_mutation/mod.rs::Session::prepare` binds the host authority owner to
  project, store, worktree, role, grant and owned spec. Its authority exchange
  answers a live check; it cannot carry a typed mutation or durable outcome.
- `db/git_backend.rs::update_atomically_tracked` acquires the store lock,
  validates preconditions, and then writes metadata and objects separately.
  `object_store::write_object` atomically replaces one file. Neither provides
  an atomic mutation-plus-outcome record across files and external queues.
- `contained_boundary.rs::contained_store_write_paths` grants the store and
  shared cache directories. Removing these grants alone breaks legitimate CLI
  and MCP writers; ancestor workspace grants also require explicit exclusions.
- `scripts/store-writers.toml` and its security guide retain unresolved writer
  families. The static ratchet is review evidence, not an authorization table.

## Proposed transaction choice

Prefer a host-owned durable redo transaction integrated with canonical backend
persistence, rather than a separate success sidecar after an ordinary CLI call.
An alternative is a new transactional canonical representation; that would
change the storage architecture more broadly. Both require independent review.

The proposed redo design needs these invariants before enabling any verb:

1. Under `with_store_write_lock`, revalidate the launch binding and operation
   authority, recover outstanding transactions, reload canonical inputs, and
   compare explicit expected revisions. Lock admission is bounded at five
   seconds without changing process-global environment or the ordinary default.
2. Construct a bounded host-generated transaction containing the fixed binding
   identity, epoch, request UUID, typed-payload digest, canonical preimage hashes,
   complete postimages, and exact bounded result. Clients supply neither paths
   nor postimages. Preserve hypothesis locks, provenance and existing policy
   callbacks while constructing the proposed changes.
3. Persist and fsync the complete transaction and its directory before installing
   any postimage. The durable transaction is the mutation decision AND dedupe
   outcome; its presence requires recovery before canonical data is served.
   Install postimages by atomic replacement and sync touched directories. Publish
   success only after all required effects are durable. An interrupted caller
   receives no success merely because a prepared record exists.
4. Recovery holds the same lock. Each destination must match its recorded
   preimage or postimage. Complete installation from recorded bytes without
   repeating business logic, generating IDs or appending another comment.
   Unexpected bytes, corrupt evidence or unavailable participants quarantine the
   transaction and return `OutcomeUnknown`; never blindly replay the operation.
5. Same binding/epoch/request UUID and digest returns the durable result after
   current authorization. Changed payload is `Conflict`. Unknown status is
   unknown. Keep S1 bounds and capacity; never evict unresolved retry evidence.
   Ended epochs cannot be reused by a resumed lane. Host recovery of an accepted
   decision is distinct from granting a new request after revocation.

This is NOT achieved by adding a journal only inside the service. Every canonical
reader/writer must honor recovery/admission so it cannot observe or overwrite a
partial installation. Old binaries, sync, import, history rewrites and transport
must refuse incompatible transaction state. Define a versioned host-managed
format and upgrade/admission rule before implementation. Cache is derived:
invalidate or rebuild after recovered installation, never use cache freshness as
the mutation commit point. Git publication failure must not erase a durable
mutation or cause business logic to run again.

Claims, global role queues, delivery ledgers and canonical objects have different
locks and roots. A canonical redo record alone cannot make those effects atomic.
Before supporting `own_claim` or `queue_done`, enumerate participants, choose a
fixed lock order and recoverable participant protocol, and retain existing
ownership/PR/override gates. Until that design is approved and implemented,
those verbs must refuse. No claim of full S2 completion from comment/edit alone.

## Routing and boundary cutover after approval

- Add a closed Rust host operation enum and bounded field schemas. Extend the
  private helper-to-Rust channel with framed, correlated messages; the Python
  broker continues checking kernel sender identity. No generic argv/MCP deputy.
- Extract policy-preserving operation handlers used by CLI, MCP and the host
  service with explicit authority context. Intercept mediated requests before
  store/cache setup or alias/deferred-child dispatch. Unsupported writers refuse
  before effects; never fall back to direct persistence. Selectors in lane env
  locate a channel but confer no authority. Native compatibility stays unchanged.
- Reconcile every inventory operation and raw Git/AIDA spawn, including the named
  `git_ops::fetch_rebase_target` blocker. Classify each as typed, host-only,
  genuinely lane-private or refused. Account for cache-on-read/migration paths.
- Gate the mediated profile separately. Preserve read-only canonical store,
  metadata, Git authority and shared cache across canonical paths and aliases,
  ancestor grants, primary checkout and resume. Use lane-private derived caches
  where required. Protect directory entries against rename/replacement as well
  as file contents; deny mounts/grants beneath protected roots. Refuse launch
  before tools if service readiness or these protections cannot be established.
- Preserve Claude's legitimate workflows through equivalent typed operations;
  retain existing compatibility paths until the gated profile passes validation.
  Do not change Codex-native labels or claim it has AIDA containment.

## Required validation after implementation

Fault-inject at journal durability, each installation, final durability and reply.
Restart the host and verify one comment, one mutation and an identical durable
result. Include corrupt/prepared evidence, revoked authority during recovery,
stale bases, concurrent duplicate and competing requests, changed-payload retries,
capacity/lock timeout, old-binary admission and mixed queue/claim participants.
Independent readers must never return partial success or stale cache as success.

Run existing protocol/owner/lifecycle tests plus operation-policy regressions and
the writer ratchet. Add same-profile raw create/append/rename negatives through
terminal and file tools with parent-observed unchanged sentinels, and host-read
mediated claim/comment/edit/queue-done positives. Exercise symlink, ancestor,
primary-checkout and resume layouts. Real-vendor C2 evidence belongs to TASK-3224;
synthetic tests alone do not close BUG-2446.

## Decision and handoff

Request independent signoff on the redo transaction and reader/writer admission
model, including the multi-root claim/queue participant design, before enabling
verbs. If that scope is too broad, approve a separately named partial transaction
slice with a Refs trailer; do not silently narrow TASK-3223's full contract.
No runtime edits, permission changes or conformance claims accompany this sketch.
