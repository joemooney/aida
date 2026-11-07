# TASK-3223 mediated-transaction sketch v1 — APPROVED DESIGN

<!-- trace:TASK-3223 | ai:codex -->

**Status: design approved; implementation pending.** The independent advisor
ruling recorded on TASK-3223 at 2026-11-07 04:55:39 UTC selects the host-owned
durable redo transaction, with the admission and scope amendments below. That
ruling resolves TASK-3225 comment 0 amendment 2 for canonical-object mutations.
Approval of this design does not establish implementation, permission cutover,
conformance, or completion of TASK-3223.

TASK-3223 covers S2a: canonical-object redo transactions, CLI/MCP routing,
raw-write denial, and lane-private caches. Multi-root `own_claim` and `queue_done`
move to TASK-3326 (S2b), blocked by TASK-3223. They continue to refuse with
`UnsupportedOperation` until their participant protocol is approved and
implemented. TASK-3223 is in drive mode; its remaining implementation contract
is unchanged.

## Implementation scope

The following acceptance work remains unimplemented by this sketch:

| TASK-3223 contract | Current evidence / remaining work |
| --- | --- |
| Route every inventoried CLI/MCP writer through mediation | Service accepts only `session_probe` / `request_status`; typed mutation routing is pending. |
| Deny raw store, authority metadata and shared-cache writes | Existing store/cache write grants remain; protected ancestor, symlink, primary-checkout and resume layouts need implementation and tests. |
| Provide lane-private or read-only derived caches | No cache cutover is implemented here. |
| Gate the mediated profile and preserve Claude/native compatibility | No gated mutation profile or workflow validation is implemented here. |
| Prove stale-base, concurrent-mutation and persist-before-reply recovery | These tests remain required after implementation. |

The advisor selected redo transactions for canonical objects. Approval of a
multi-root claim/queue participant protocol remains outside this slice and is
required on TASK-3326 before enabling those operations.

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

## Approved transaction choice

Use a host-owned durable redo transaction integrated with canonical backend
persistence. The advisor selected this design over a new transactional canonical
representation. A success sidecar after an ordinary CLI call cannot provide the
required durable mutation-plus-outcome guarantee.

The redo design needs these invariants before enabling any mutation verb:

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
   outcome; its presence requires recovery before another canonical writer acts.
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

## Advisor admission amendments

The advisor ruling narrows the original sketch's recovery admission:

- **Writers:** integrate recovery at `with_store_write_lock` acquisition. The
  first canonical writer holding that lock completes or quarantines outstanding
  transactions before doing its own work. No separate admission protocol is
  introduced. Writer inventory coverage must verify this shared boundary.
- **Readers:** retain today's lock-free, per-file atomic-replacement guarantee.
  Multi-file views can already be momentarily inconsistent under
  `update_atomically_tracked`; this design adds no reader admission or blocking
  requirement. A reader finding an outstanding transaction may report the store
  as recovering. This is not a multi-file snapshot guarantee.
- **Binary compatibility:** the approved operational assumption is one user and
  one live binary. Place the transaction directory behind a format-version
  marker; a newer binary refuses an unknown version. No compatibility shim for
  older binaries is required. Concurrent old writers are outside that approved
  assumption and must not be represented as supported.

A journal only inside the service is insufficient: all canonical writers must
reach recovery through the shared writer lock, including sync, import, history
rewrites and transport. Cache is derived: invalidate or rebuild after recovered
installation, never use cache freshness as the mutation commit point. Git
publication failure must not erase a durable mutation or cause business logic
to run again.

Claims, global role queues, delivery ledgers and canonical objects have different
locks and roots. A canonical redo record alone cannot make those effects atomic.
Before supporting `own_claim` or `queue_done`, enumerate participants, choose a
fixed lock order and recoverable participant protocol, and retain existing
ownership/PR/override gates. Until that design is approved and implemented,
those verbs must refuse with `UnsupportedOperation`. TASK-3326 owns this
participant protocol; canonical-object completion alone does not complete S2b.

## Required routing and boundary cutover

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
capacity/lock timeout, and unknown transaction-format versions. Verify writer
recovery admission and the approved lock-free reader behavior separately: readers
have per-file atomicity, not a multi-file snapshot guarantee. Service replies must
not report mutation success before durable installation or use stale cache as
success. Test `own_claim` and `queue_done` refusal in S2a; mixed-root recovery
validation belongs to TASK-3326.

Run existing protocol/owner/lifecycle tests plus operation-policy regressions and
the writer ratchet. Add same-profile raw create/append/rename negatives through
terminal and file tools with parent-observed unchanged sentinels, and host-read
mediated canonical-object comment/edit positives. Exercise symlink, ancestor,
primary-checkout and resume layouts. Real-vendor C2 evidence belongs to TASK-3224;
synthetic tests alone do not close BUG-2446.

## Decision and remaining work

The advisor ruling approves invariants 1–5 with the writer-lock, lock-free-reader
and single-live-binary amendments above. Implement and validate the S2a routing,
transaction and boundary changes before claiming TASK-3223 completion. Obtain a
separate participant-protocol approval for TASK-3326 before enabling claim/queue
verbs. No runtime edits, permission changes or conformance claims accompany
this design document.
