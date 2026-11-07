# Work CI relay and restricted result intake

Status: sketch and request-admission fork approved by advisor on 2026-11-05.
The binding ruling below supersedes the earlier proposed ref spelling.
Owner: STORY-2480. trace:STORY-2480 | ai:codex

## Existing decisions and implementation gap

The operator's 2026-11-04 ruling chooses explicit `ci/*` request branches and
answers the work-runner policy question. Neither decision needs reopening.
The STORY-2479 advisor ruling approves a separately enrolled clone-local
intake policy and pinned remote, independent of store transport.

At base `5602eae5bf1`, TASK-3077 supplies only local observation
note/validate/export. Its parent comments explicitly defer intake enrollment,
orphan publication/pull, and processing. `observation_envelope.rs` admits only
`kind=observation`; no production `ci-results` transport or processor exists.
The parent being Completed does not establish that dependency's availability.

This story therefore needs both the missing shared transport and a new receipt
wire contract. AGENTS.md's Sketch-First Protocol requires master sign-off for
file formats and transport boundaries before an implementation PR.

## Proposed boundary and sequence

1. Deliver the independently enrolled intake transport approved on STORY-2479
   as its own prerequisite. It must validate incoming history and isolate its
   index, refs, lock and worktree from product/store operations. Preserve the
   existing store-only work transport contract.
2. Add a strict local CI receipt codec and pinned-commit inventory validator.
   Keep observation v1 unchanged; dispatch the CI schema only in its own
   `ci-results` namespace. Neither parser interprets imported data as commands.
3. Add the work-side bounded polling adapter and user service. Mirror only
   explicit `refs/heads/ci/*` requests; reject arbitrary refspecs and tags.
   Poll outward to home and work APIs; expose no listener at work.
4. Add the home-only intake processor and native commit-status publisher.
   Home credentials with status/API authority never go to work. Expose one
   stable check context that operators can require in merge/train policy.

## Proposed receipt contract for approval

Exactly these top-level keys: `version`, `kind`, `sha`, `attempt`, `status`,
`jobs`, `failing_tests`, `digest`. Version is integer 1 and kind is the literal
`ci-result`. SHA is a full lowercase object ID matching an admitted home
request commit. An attempt is a home-assigned bounded positive integer; work
pipeline IDs remain local and never become filenames or receipt fields.

Jobs contain exactly `name`, `status`, `duration_seconds`. Names must belong to
the expected home job inventory for that SHA. Status is a closed enum:
`success`, `failed`, `canceled`, `skipped`. Duration is an integer in
0..86400; reject floats, booleans, negatives and overflow. Failing test names
must belong to the home test inventory. Sort jobs and test names, refuse
duplicates, and reject missing expected jobs rather than treating a subset
of successes as a passing pipeline. Success requires all required jobs to
succeed and no failing tests. Unknown/incomplete pipeline states stay local
and never publish success.

Proposed limits: 128 KiB serialized receipt, 256 jobs, 2048 failing tests,
512 UTF-8 bytes per identifier. Reject unknown/duplicate/missing fields,
trailing data, unsupported versions and invalid types at every level. Hash
compact UTF-8 JSON with the first eight fields in the order above, excluding
`digest`, using the observation codec's serde_json escaping convention. Digest
is full lowercase SHA-256; it proves byte integrity, not work provenance.

Inventory must be derived from the pinned home commit, never supplied by a
work response. Define the supported GitLab/GitHub job syntax and test discovery
before coding; includes, matrices and dynamic tests must either resolve from
that commit through a bounded data parser or refuse as unsupported. Never run
work-returned discovery code. Work validates against the fetched home commit;
home independently repeats validation. Durations/statuses remain claims by
the relay; allow-list validation cannot certify their truth or eliminate covert
channels. No logs, URLs, paths, runner identity or diagnostic prose is exported.

## Transport, enforcement and crash recovery

Publish immutable `ci-results/<sha>/<attempt>.json` entries on `aida-intent`.
Use the shared intake lock, tree/history validation and explicit single-branch
push; never force-push. Existing observations and processing receipts must
remain byte-identical. An identical receipt is a no-op; different bytes at the
same identity refuse. A divergent push retains the pending local receipt and
requires validated refresh/recovery, without an automatic overwrite.

The home hub must enforce the work credential's write restriction server-side:
only intake updates, no other branches, tags, deletes or product ancestry.
Do not assume a local refspec or client config constrains a stolen credential.
Admission requires a deliberate rejected push to another branch and tag using
that credential. Exact deployment enforcement depends on the hub's available
controls; fail closed if it cannot enforce the restriction. Separate home read
and intake write credentials; keep work-project API credentials local to work.

Persist the request SHA, mirrored ref, pipeline association, deadline and attempt
before external side effects. A restart reconciles exact remote facts before
retrying. Deduplicate by SHA and attempt across renamed request refs. Bound
subprocess/API calls and response bodies, total pipeline time and retry count;
redact diagnostics rather than returning remote text. A user-service restart
must preserve the original deadline instead of replenishing the retry budget.

Home quarantines refused inputs locally without forwarding their contents.
Publish statuses only for admitted home requests after validating the complete
receipt and authoritative inventory. Persist processing identity so replay is
idempotent; superseded attempts cannot overwrite a newer result. Receipt text
does not select a status endpoint, credential, context or destination repository.

## Verification after sign-off

Use two local bare repositories, a local fake pipeline/status API and isolated
HOME/config/state directories. Cover request push, exact-SHA mirror, pass/fail
pipeline, orphan receipt return and native status publication. Repeat each phase
across restarts to prove no duplicate work and bounded retries. Snapshot product
and store refs/indexes throughout. A deliberate forbidden branch/tag push must
fail at the hub, independently of the relay client.

Reject tampering with a recomputed digest, unknown/duplicate/nested fields,
unknown identifiers, missing jobs, inconsistent success, oversized input,
hostile paths/modes/ancestry, conflicting immutable receipts and older-attempt
replay. Test concurrent intake publishers and uncertain API/push outcomes.

## Decision requested

Approve the separate CI receipt schema, home-assigned attempt identity and
fail-closed inventory support policy. Confirm sequencing: finish shared intake
transport first, or explicitly authorize its implementation in this story.
No runtime implementation or rollout is claimed by this sketch.


## Request admission addendum (2026-11-05 implementer pickup)

The original receipt schema and transport sequence have advisor sign-off in
STORY-2480's comments. TASK-3096 merged as PR #3415, commit `158725d7dd9`, and
its independent intake transport is present in this checkout. The former
prerequisite block is resolved. The operator rulings on explicit `ci/*`
triggers and work-runner policy remain settled.

A separate request-authority decision is still needed before wiring runtime
publication. The approved receipt requires a **home-assigned attempt**, an
**admitted home request commit**, and independent revalidation at home. The
existing trigger contract is only `refs/heads/ci/*`. It does not define how
an attempt is assigned or transported, whether request refs can change, or
what preserves admission after a ref disappears. TASK-3096 supplies no CI
request registry: its closed namespaces are `observations/` and `receipts/`.

This affects correctness at a concrete existing boundary:

- `intent_intake::Intake::history` revalidates the complete accepted live tree
  against the current validator and policy, including when reusing a validated
  checkpoint. Extending that tree with immutable CI receipts requires durable
  admission evidence for each receipt's SHA and attempt.
- Using whatever `ci/*` refs happen to exist now loses old admission evidence
  when a home request is renamed, moved or deleted. Then ordinary observation
  pulls and publications can refuse because a historical CI receipt can no
  longer be admitted.
- Taking `attempt` from the work pipeline ID or trusting the receipt's own
  attempt violates the approved home-assigned identity rule. A correct digest
  does not establish that the home operator requested that attempt.
- Pinning a local request snapshot on first encounter is insufficient: a new
  home validator or a second enrolled clone must independently recover the
  same authority, rather than rely on the work relay's local journal.

### Recommended concrete profile for approval

Use immutable `refs/heads/ci/<attempt>` request refs. `<attempt>` is the
canonical decimal spelling of a positive `u32` assigned at home, unique across
the repository. The branch's direct commit target is its only SHA binding.
Admission requires both that exact ref spelling and that exact target. Generic
names such as `ci/feature` are ignored, never assigned an attempt at work.

The home operator creates the ref before the work poller can act. Request refs
must remain unchanged and retained for the lifetime of intake history. The hub
must refuse their replacement and deletion by the request-creation identity,
in addition to denying all non-intake writes by the work return credential.
The home read credential can fetch retained requests; work never creates them.
Both validators resolve and pin these home refs before validating receipts.
The work pipeline association and retry journal remain local and cannot grant
admission. A retry for one admitted request preserves the same attempt and
original deadline. A fresh attempt requires a newly created home request ref.

This avoids a second request wire format, but imposes permanent branch
retention and a narrower request naming convention. Those are new lifecycle
requirements, not properties already supplied by TASK-3096 or the receipt
codec approval. Do not introduce them implicitly while implementing the relay.

### Alternative requiring a separate contract

Retain generic mutable `ci/*` trigger refs, but add a durable, home-authored
request registry binding SHA, home-assigned attempt and lifecycle. Specify its
schema, namespace, authentication, retention and recovery rules before using
it. A work-writable intake branch alone cannot prove which entries were
assigned at home. This option needs explicit authority separation and must not
be inferred from observation receipts or successful pipeline responses.

### Verification to add after the decision

In addition to the approved end-to-end plan, cover a second fresh home clone,
request ref rename/move/deletion, duplicate attempts, noncanonical attempt
spellings, work-selected attempts with valid digests, and observation-only
intake traffic after CI receipt publication. For the recommended profile,
exercise server-side request replacement/deletion refusals independently of
the relay client. For the registry alternative, exercise forged home admission
and recovery without any work-side local state.

No runtime implementation, service deployment, PR, CI result, or completion is
claimed by this addendum. STORY-2480 is punted as a design fork for this specific
request-admission choice, not for the previously approved schema or prerequisite.


## Binding request-admission ruling (2026-11-05 20:50 MST)

The advisor approved immutable retained attempts and rejected a separate
home-authored request registry. Home assigns the attempt and pushes its exact
SHA to `refs/aida/ci/<attempt>` on work GitLab; it never rewrites or deletes an
attempt ref. Results are keyed by `(attempt, SHA)`; pruning is out of scope.
This ruling supersedes the addendum's proposed `refs/heads/ci/<attempt>`
admission spelling. The implementation retains the same custom refs on the home
hub so a fresh home clone can independently recover admission. Work's ordinary
`refs/heads/ci/<attempt>` is only the exact-SHA GitLab pipeline trigger.

Implementation uses the existing intake transport and shared lock, a strict
CI codec, pinned static inventory, a bounded work timer, and home-only native
status publication. See [operation, support limits and recovery](../agents/work-ci-relay.md).
Runtime rollout requires server-enforced per-credential ref restrictions; no
production credential or runner is enrolled by the implementation session.

## Round 2 inventory follow-up (2026-11-06)

This implementation slice supports bounded flat static inventories, not AIDA's
own CI deployment. AIDA has about 48 MiB of tracked Rust sources versus the
16 MiB inventory cap, and its GitLab config uses workflow/default/include,
hidden templates and rules. A separate flat pipeline alone cannot address the
source cap. Finding 3 explicitly permits this rescope plus a follow-up spec.

Requested follow-up title: **Admit AIDA repository size and GitLab templates to
work CI relay inventory** (draft task, parent STORY-2480).

Filed during round 3 as **TASK-3232** (canonical
`TASK-b6b1278890774ef993d2ebc327862470-886`), a `from-review` draft child of
STORY-2480. Advisor promotion remains pending; this slice claims no deployment.

Acceptance for that task:

- A fixture at least as large as AIDA (48 MiB) with several distinct source
  SHAs validates within a documented bounded budget, using bulk object reads
  and/or explicit test-bearing crate scope.
- AIDA's committed GitLab configuration is admitted with deterministic selected
  job names from local includes/templates. Remote includes, commit escapes and
  dynamic expansion refuse; work API claims never define inventory.
- A retained request launches the intended AIDA jobs, returns qualified test
  failures safely, and gates native AIDA PR/train checks end to end.

No AIDA runner deployment or story completion is claimed by this slice.

## TASK-3232 implementation profile (2026-11-06)

The advisor approved this follow-up on 2026-11-06 (LIGHT gate). The inventory
now preflights committed blob sizes/modes and bulk-reads up to 64 MiB of
Rust/YAML, with a 60-second per-inventory budget. The newest-32 plus on-demand
ruling is retained, with a separate 300-second source-validation snapshot budget.
Intake ancestry/metadata keeps its existing 30-second budget exclusive of source
validation. Exhaustion refuses; it never promotes partial evidence.

Committed local includes and hidden `extends` templates resolve with bounded
GitLab merge precedence. The supported closed rule vocabulary models canonical
relay push branches and rejects dynamic/remote expansion and commit/path escapes.
AIDA's `.merge-gate` admits those branches explicitly; its relay inventory selects
`verify`, `test` and `bwrap-confinement`. MR-only holds and release/Pages jobs stay
outside this push profile. Native merge holds remain independently enforced.

The regression fixture uses real AIDA Rust sources padded above 48 MiB at three
source commits, immutable retained requests and their exact-SHA execution branches,
qualified test failures, returned receipts, and home HTTP status publication. Its
captured native statuses feed the production commit reader and PR/train required
check judges. See [the operational bounds](../agents/work-ci-relay.md).
No runner or credential deployment, remote test execution, or STORY-2480 completion
is claimed by this inventory follow-up.

<!-- trace:TASK-3232 | ai:codex -->
