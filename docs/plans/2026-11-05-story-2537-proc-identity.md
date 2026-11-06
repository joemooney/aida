# STORY-2537: one process identity and liveness judge

<!-- trace:STORY-2537 | ai:codex -->

Status: proposed sketch, awaiting advisor signoff. This lane changes no
production code. The binding advisor brief recorded on 2026-11-06 at 00:17 UTC
(body labeled 2026-11-05 17:25 MST) limits delivery to this file, spec comments,
and a docs PR titled `(Refs STORY-2537)`. Merging that PR does not complete the
story, authorize implementation, or satisfy BUG-2458's host acceptance.

## Approach — scope and invariant

Consolidate process observation, not ownership policy or grant authority. Every
owner, lease, lock and binding should ask the same question about the same
kernel identity. Namespace invisibility, unreadable evidence and expired
reservations must never silently become proof of process death. An Alive
process does not by itself own a spec, hold a reviewer grant, or authorize mail.

The first implementation slice must include ownership and leases together with
BUG-2458, rather than introducing another ownership-local PID check. Broker
capabilities, grant replacement and a unified human-terminal rule require a
separate later signoff. AIDA remains Linux-only.

## Reusable helpers — existing consumers

Inventory at checkout `71c43691a10`; BUG-2458 PR #3450 is separate, unmerged work
described in the spec's review comments. Its proposed fixes are not assumed to
exist in this checkout. Paths below are relative to the repository root.

| Surface | Existing implementation and evidence | Consolidation requirement |
| --- | --- | --- |
| TASK-1750 primitive | `aida-core/src/liveness.rs`: `pid_is_alive`, `process_start_identity_exact`, `process_identity_is_alive`, `process_identity_matches_strict`, `process_provably_dead`. Exact Linux identity is `linux-starttime:<ticks>`; scheme-aware readers retain bare RFC3339. Lenient lock checks retain unknown identity; strict signal checks reject it. `ProcessSnapshot` couples liveness and start reads. | Reuse these parsers/observation semantics behind one module; remove independent boolean decisions. Ticks alone lack boot and namespace context. |
| Tri-state probe | `aida-core/src/process_probe.rs`: `ProcessIdentity { pid, start }`, `ProbeSource::{capture,facts,probe,ancestry,pid_namespace}`, `Probe::{Alive,Dead,Unknown}`. Start is `linux:<boot>:<ticks>`. Fixtures inject a procfs tree. | This is already a boot-qualified tri-state implementation, not a reason to build a third parser. Add explicit namespace coordinates and validated holder capture; retain injectable facts. |
| Namespace observer | `aida-core/src/pid_observer.rs`: `PidObserver::can_observe`, `PidNamespaceProbe`; re-exported by registry. `aida-cli-lib/src/gate/dispatch_deps.rs` retains a floor copy with parity tests. | Fold observation admission into the judge. Current same-inode/legacy-host admission is not proof of a complete procfs view. Preserve BUG-2212 masked-procfs protection and inventory the floor boundary. |
| Leases | `aida-cli-lib/src/leases.rs`: `lease_recorded_owner`, `lease_has_durable_owner`, `lease_state_for`, `lease_is_live`; `aida-core/src/liveness.rs`: `recorded_lease_owner`, `lease_state_for`, foreign-lease classification. Active and creator PIDs have start strings; a lease also has `pid_namespace`. Foreign leases have a 24-hour heartbeat/mtime backstop. | Distinguish a selected owner from creator/launcher metadata; use the common verdict before lease policy. Migrate write/stamp/adoption paths as well as readers, including `session_start.rs` and registry integration. |
| Ownership | `aida-cli-lib/src/ownership/{types,judge,dual,compare,migrate}.rs`: process witnesses carry host, boot-qualified identity and namespace. `process_witness` currently records only initial-namespace writers; `holder_for_lease` records active and creator witnesses. `judge_owner` lets any Alive win, otherwise any Dead beats Unknown. Heartbeat window is max(3 intervals, 2 hours); future wall times count as fresh. | Remove creator/subreaper masking of worker death; centralize capture/refusal and use one selected holder generation. Keep lifecycle/tree safety in ownership, not in the primitive. Compare and generation-pinned repair must use the same derivation as writes. |
| Merge lock | `aida-cli-lib/src/merge_lock.rs`: `Holder` records host, PID/start, boot, namespace, wall acquisition time and TTL. `probeable_pid` uses `CacheLockInfo::visible_pid`; `holder_alive` uses the lenient helper. `stale_reason` reclaims at TTL even if a PID remains alive. Default TTL is 600 seconds; partial records have a 30-second mtime grace. Acquisition waits use `Instant`. | Replace identity checks in stale detection and `MergeLease::covers`; retain atomic publication and serialization. Explicitly separate lease expiry from process death and review TTL behavior before changing it. |
| Review-watch reservations | `aida-cli-lib/src/review_watch.rs`: local `Identity { pid_namespace, recorded_at, pid, start, boot }`; equality excludes `recorded_at`. `alive` independently parses zombie state and compares ticks/boot; foreign identities retain capacity for four hours, or indefinitely without a timestamp until other evidence. Attempts track dispatcher, worker and reviewer identities, spawning flags and receipts. `review_watch_signal.rs` owns pidfd signaling. | Preserve reservation tokens, two-review capacity, ambiguous receipts and launch handoffs. Use shared process verdicts for each actual participant; retain separate reservation-expiry policy. A shared judge must not replace pidfd signal protection with a check-then-kill race. |
| Mail-wake bindings | `aida-core/src/mailbox_identity.rs`: `SessionBindingFacts::verify`, `MailIdentityFacts::watched`, `row_binds_chain` / `is_self_or_ancestor`, and `observe`'s `pid_start` check; strict PID/start matching backs registry-row authority. `aida-cli-lib/src/agent_registry.rs`: mail-wake updates compare PID/start under the namespace lock; launcher/hook merging uses the same pair. Rows record namespace. `mail_wake.rs` arms them; `mailbox_wake_run.rs` uses `WatchLock::armed_audience`, exact thread IDs and durable delivery ledgers. | Compare the complete identity under the existing row lock; judge before using a row as live evidence. Keep project/handle/recipient/thread/armed-state checks. Queue acceptance remains distinct from acknowledgement. The Python advisor-thread adapter `scripts/codex-mail-wake.py` is not a process-owner attester. |
| Explicit role binding | `aida-cli-lib/src/gate/seat_authority.rs`: `probe_bind_target` obtains single-PID sysinfo name/argv/UID plus exact start; `check_bind_target` rejects PID <= 1, other users, non-harness/non-shell names and ancestors above the terminal leader. `BIND_SHELLS` and harness classification implement the comm/name check. `review_authority.rs` serializes namespace-qualified process tokens. | Shared identity capture/judgment must precede role eligibility checks. Keep same-user, harness/shell and terminal-scope restrictions: matching comm is a mutable label, not identity or authority. Recheck the same identity at mint, not independent PID facts. |
| Existing ratchet | `aida-cli-lib/src/tests/task_1750_liveness_primitive_guard_tests.rs` scans production crates for raw kill-zero probes; process-group probes and test-home cleanup have documented exceptions. | Extend the existing guard, with an inventoried baseline for unmigrated identity decisions. Do not reinterpret group checks or ordinary PID display as owner liveness. |

BUG-2130 already switched writers to exact start ticks after the reader shipped
in a tagged release. Its comments document the mixed-version failure: an old
reader could interpret every new-format live lock as dead. Preserve that lesson
for this schema change. Bare RFC3339 compatibility is not clock-step immunity.

## Proposed type and observation boundary

Proposed module: `aida_core::proc_identity`, consolidating the existing core
implementations. The following is a contract sketch, not implemented Rust:

```text
ProcIdentity { boot_id, pidns_inode, pid, starttime }
ProcessWitness { node_id, identity, holder_generation, capture_evidence }
judge(identity, observer_facts) -> Alive | Dead(reason) | ForeignUnknown(reason)
validate_holder(witness, holder_facts) -> accepted | refusal(reason)
```

`starttime` is the exact unsigned field-22 tick count, never epoch seconds,
RFC3339 or a btime calculation. `pidns_inode` identifies the coordinate system
in which the numeric PID was captured. The tuple is boot-local; namespace
inode reuse cannot identify a process without boot/PID/start. An enrolled node
or trusted local provenance belongs in the witness envelope: a hostname match
or a different machine's boot UUID is not local reboot evidence. `recorded_at`,
heartbeat time, scope and observation generation are not identity equality.

`ForeignUnknown` retains the story's proposed third-result spelling. Its reason
must distinguish foreign namespace/node, missing namespace/boot, incomplete
procfs, permissions and malformed legacy identity. It means unprovable,
including local read failures; it does not mean every unknown process is foreign.
Holder invalidity is an explicit refusal, not a fourth liveness value and not
Dead. Every consumer must retain that diagnostic instead of converting it to
Alive or reclaimable.

Capture the actual process through a trusted launch/stamp handoff or a verified
long-lived session anchor. Bind the write to its holder generation. Do not
recapture today's process at an old PID to upgrade a legacy record: that could
legitimize a recycled PID. Boot, namespace, state and ticks need a consistent
observation, with rechecks/pinned namespace handles where needed. A pidfd can
pin a local process for an operation; it is not a serializable identity.

The procfs mount's PID coordinates matter. Reading a target's innermost
`ns/pid` inode and pairing it with an ancestor-visible numeric PID constructs
an invalid tuple. Capture only when the PID and namespace coordinates are
established together. Verify observer view and target coordinates before
interpreting absence; matching namespace inode alone is insufficient when
procfs is masked, mounted from an ancestor, or otherwise incomplete.

## Decisions — judge decision table

Evaluate node provenance and well-formed identity before probing a numeric PID.
Holder validation applies before accepting Alive for an ownership/binding use.

| Namespace/evidence case | Process result | Consumer implication |
| --- | --- | --- |
| Trusted same node, same boot, same PID coordinates, complete usable procfs; running non-zombie with matching ticks | Alive | Retain owner/lock. Authority still requires the consumer's scope/grant checks. |
| Same observable coordinates; positively absent PID, exited/zombie state, or different exact start ticks | Dead with distinct reason | Only evidence about this recorded process. Tree, lifecycle and release gates still apply. |
| Trusted same node, boot demonstrably changed | Dead: reboot | Applies across that node's namespaces; do not look up the old numeric PID on the new boot. |
| Different node, even with equal PID or namespace inode | ForeignUnknown: other node | Never infer reboot/death from this reader's boot; require an observation from the recorded node. |
| Same boot, reader and recorded PID namespace differ: host to child, child to host, or sibling namespaces | ForeignUnknown: namespace mismatch | No blind `/proc/<pid>` or kill-zero lookup. Numeric collisions prove nothing. |
| Same non-initial namespace and demonstrably matching procfs coordinates | Ordinary Alive/Dead rules above | Do not reserve direct observation solely for the initial namespace. |
| Ancestor can see a descendant process under a different numeric PID | ForeignUnknown until verified translation exists | `NSpid` is diagnostic evidence; no ad hoc translation at callers. A future trusted observer may publish an observation for the original identity. |
| Same inode but masked/incomplete or inconsistent procfs view; inaccessible stat; unreadable boot/namespace | ForeignUnknown with precise reason | Absence cannot prove death. No age-based conversion inside the judge. |
| Missing legacy namespace/boot, bare timestamp, unsupported scheme, or corrupt tuple | ForeignUnknown: insufficient identity | Preserve a safe hold/refusal; repair only from independently established ownership. |
| PID 0/1, namespace init (including host-visible PID > 1 with inner PID 1), known init/user-systemd target, or positively established subreaper holder | Holder refusal: holder identity invalid | Never accept it as Alive ownership, never infer it is Dead, and never let a heartbeat rehabilitate the invalid holder. |
| Arbitrary target whose required holder provenance/subreaper evidence cannot be established | Holder refusal: evidence unavailable | Do not claim a generic remote subreaper detector exists. Retain work pending a valid handoff or operator repair. |

The first slice intentionally supports direct same-coordinate observation and
conservative foreign results. Cross-namespace translation and attested remote
observations are later work; environment-carried PIDs do not attest either.

## BUG-2458: owner capture and init/subreaper refusal

The actual lane worker owns its work, not its launcher, terminal parent,
systemd user manager, or a transient `aida session start` invocation. Reparenting
does not change the worker's identity. Stamp the worker before treating it as
the holder; without a proven worker or validated long-lived interactive anchor,
record no process witness and retain Unknown. Never manufacture Dead when the
setup process exits. On worker replacement, atomically replace the holder
generation; do not keep an old creator witness that can mask worker death.

Centralize refusal in the shared holder-capture/validation boundary, used by
writers, the judge, comparison and migration. A public constructor accepting an
arbitrary unvalidated probe closure would recreate the bypass noted in #3450.
Known init executables are useful refusal evidence, but a name denylist alone
does not establish the absence of an arbitrary subreaper.

Linux's child-subreaper query is self-only; ordinary procfs does not provide a
reliable arbitrary-target subreaper flag. Proposed slice-1 policy: require a
trusted cooperative capture/handoff that checks the actual holder's own
subreaper state, tied to that process identity, plus refusal of PID/namespace
init and known manager processes. Missing such evidence refuses holder
admission rather than pretending a name check proves non-subreaper status.
This launch-time evidence is a lifecycle assurance, not a security capability;
a process can change state later. Arbitrary uncooperative targets cannot be
certified by this slice. Broker-mediated ongoing attestation is separately gated.

Generation-pinned migration must share writer derivation, preserve phase and
tree, and be idempotent. Invalid orphan holders are reported for operator
repair; clearing a bad witness is not releasing its work. BUG-2458 acceptance
still requires operator-run host migration and compare output with residual
UNEXPLAINED rows classified. A foreign-namespace lane's empty view cannot
supply that evidence. Do not waive this criterion in a docs PR.

## Clock steps, heartbeat fallback and action policy

BUG-2130's rule is mandatory: compare boot UUID and raw start ticks. Forward
or backward wall-clock steps, changes to btime, NTP correction and VM resume
must not change identity. A changed trusted boot proves old local processes
ended; a changed display timestamp does not. Legacy timestamps remain explicit
insufficient evidence in the new judge; do not add an equality tolerance or
silently wrap a legacy time in a new identity.

Foreign identity can have a separately validated heartbeat, bound to the exact
node, identity, scope and holder generation. A fresh beat can retain a claim or
show recent activity; it cannot grant process-signal, private-mail or role
authority. Missing/stale beats leave process liveness ForeignUnknown. An
authenticated terminal receipt or same-coordinate observer may establish the
process outcome. Never combine a beat from a new holder with a dead old witness.

For new persisted heartbeat/lease deadlines, propose host-observed
`CLOCK_BOOTTIME` readings with boot and trusted clock-domain identity; local
waits continue to use monotonic deadlines. Boottime includes suspend. Time
namespaces can offset this clock, so readings from unverified clock domains
are not comparable even with equal boot IDs. A local observer can time its own
receipt of advancing beats. After restart, compare persisted readings only
when boot/domain provenance is established; otherwise retain unknown. Wall
times remain useful for display, never proof of death. Clock anomalies in old
wall-time records require conservative handling, not absolute mtime distance.

Existing consumers differ deliberately today: merge locks reclaim after a
600-second TTL, review-watch uses a four-hour foreign backstop, leases use a
24-hour heartbeat backstop, and ownership allows at least two hours for a
heartbeat. These remain named consumer policies until their migration slice
is approved. The proposal is to report `lease expired` or `reservation expired`
separately from Dead, with any consequential recovery under existing lifecycle
and generation checks. In particular, moving merge expiry to a safe deadline
or refusing theft of a positively live merger is a reviewed behavior change,
not a mechanical helper replacement.

| Operation | Required evidence |
| --- | --- |
| Retain work/capacity | Alive, unknown or holder refusal conservatively retains; separately approved terminal/expiry policy can close a reservation. |
| Reclaim ownership | Dead for the current holder generation, then ownership's phase/tree/unpushed/PR checks; invalid identity alone never releases. |
| Signal process | Verified local identity plus pinned pidfd and revalidation; unknown/refusal means no signal. Group semantics stay separately checked. |
| Bind role or read private mail | Positive identity and all existing UID, scope, grant, row, recipient and thread checks; heartbeat alone cannot satisfy them. |

## Proposed slice order and review gates

0. **This docs slice.** Publish the inventory and decision table on STORY-2537.
   Advisor signs the identity coordinate contract, unknown/refusal mapping,
   cooperative holder-admission policy and clock/expiry separation before code.
   Pending signoff is intentional; no child implementation specs are filed yet.
1. **Core plus ownership/leases, including BUG-2458.** Consolidate the two core
   probes and observer admission behind ProcIdentity; add structured refusals and
   fixture facts. Migrate ownership writers/judge/compare/repair and lease owner
   selection/stamp/adoption together. Extend the existing ratchet in this slice
   to prohibit new raw identity decisions; permit only enumerated, temporary
   unmigrated sites. Do not activate STORY-2325 ownership cutover without its
   compare evidence. Host BUG-2458 acceptance remains a separate required check.
2. **Merge serialization.** Migrate capture, stale checks, covers and cleanup
   consumers (including scripts that read lock records). Review the deadline
   and legacy-record policy explicitly; keep atomic publication, bounded waits
   and release fencing. Test that a live merger is not stolen on a clock jump.
3. **Review-watch.** Replace its private Identity/probe with the shared type;
   migrate dispatcher/worker/reviewer receipts and equality. Preserve capacity
   and spawning ambiguity, pidfd safety, exact-head checks and reservation tokens.
   Specify clock-safe backstop migration before changing expiry behavior.
4. **Registry, mail wake and explicit binding.** Upgrade row comparison and
   strict mail identity verification together; migrate role binding's identity
   input while retaining its eligibility checks. Cover launcher/hook races and
   wrong-thread prevention. Expand the ratchet to these callers.
5. **Remaining inventory and ratchet closure.** Enumerate all production
   identity consumers (drain/solo/cache locks, worktree pool, coordination,
   registry/GC, seat occupancy, mandate floor adapters and containment receipts),
   remove temporary baseline exceptions as migrated, and keep explicit group
   probe exceptions. No claim that this first sketch migrates all ~75 files.
6. **Separately signed broker slice.** Build on the peer-credential socket work
   referenced by STORY-2343: per-user authenticated observations and scoped
   capabilities, replay/revocation and namespace translation policy. Only this
   separately reviewed work may propose replacing environment grants or changing
   ship/train/hold-clear human-terminal authority.

Reader/writer rollout is part of each slice. Inventory every reader, including
scripts and rollback binaries; ship readers before writers or introduce an
explicit version boundary that older writers/readers refuse safely. Never add
fields and assume every parser ignores them: review-watch denies unknown
fields. Legacy insufficient identity stays unknown until safely rebound, with
an operator recovery path. No read-only inspection silently upgrades records.

## Tests and verification plan

No production tests are changed or claimed in this lane. Future implementation
acceptance must include these layers:

- **Core fixture tests:** same initial and same non-initial namespace; parent,
  child and sibling mismatch; identical numeric PIDs in distinct namespaces;
  masked/ancestor-mounted procfs; missing boot/namespace; permission errors;
  malformed stat/legacy data; PID reuse; zombies; reboot on the same node versus
  unrelated node boot; comm containing spaces/parentheses; coherent capture
  under disappearance and reuse.
- **Clock fixtures:** step wall time/btime both ways without changing ticks and
  retain Alive; change ticks and get Dead. Test suspend/boottime, time-namespace
  offsets, observer restart, missing clock-domain evidence and old wall-time
  heartbeat records. Expiry must not be reported as process death.
- **Ownership/lease fixtures and command tests:** detached setsid/reparented
  worker remains the holder then reads Dead after exit; no creator/setup PID
  fallback; PID 1, inner init, user systemd and a cooperative custom-named
  subreaper refuse; unavailable subreaper evidence refuses admission. Unstamped
  live sessions retain Unknown. Startup/write/repair agree, repeated repair is
  idempotent, and changed generations are retained.
- **Consumer regressions:** merge lock publication/partial-write/contender
  safety; review capacity, ambiguous launch receipts and PID reuse at signal;
  mail registry replacement races, foreign rows and exact thread/recipient
  checks; explicit binding same-user and harness/shell restrictions, comm
  spoofing without identity match, and target replacement before mint.
- **Host acceptance:** operator captures migrate/compare evidence for BUG-2458;
  classify every residual discrepancy before ownership cutover. Test fixtures
  and this sandbox's views cannot substitute for it.
- **Ratchet and release contract:** negative fixtures prove detection of new
  raw PID/start/kill-zero owner decisions, including alternate syntax; existing
  exceptions stay narrow and reasoned. Test old/new reader and writer matrix
  before enabling the new record format.

For this lane, verify the Markdown diff and repository paths, run the read-only
plan verifier, and confirm the code diff contains only this file. Post the
sketch/validation and PR link to the spec without marking STORY-2537 Done or
Completed. Stop when the docs PR is open; do not ship or record a review.

## Files

This lane adds only `docs/plans/2026-11-05-story-2537-proc-identity.md`.
The requirement discussion is recorded through AIDA comments.

## Critical Files

The only code-branch file changed in this lane is this sketch. Proposed slice-1
production boundaries, for later review, are:

- `aida-core/src/liveness.rs`, `aida-core/src/process_probe.rs` and
  `aida-core/src/pid_observer.rs` (consolidation into the proposed module).
- `aida-cli-lib/src/ownership/judge.rs`,
  `aida-cli-lib/src/ownership/dual.rs`,
  `aida-cli-lib/src/ownership/types.rs`,
  `aida-cli-lib/src/ownership/compare.rs` and
  `aida-cli-lib/src/ownership/migrate.rs`.
- `aida-cli-lib/src/leases.rs`, `aida-cli-lib/src/session_start.rs` and
  `aida-cli-lib/src/agent_registry.rs` for consistent owner capture and stamping.
- The existing TASK-1750 guard named in the inventory, plus focused fixtures
  and regression tests for the first-slice contract.

## Risks + gotchas

The largest risks are mixed-version lock theft, falsely complete procfs views,
subreaper claims that cannot be established remotely, and changing consumer
expiry semantics under the appearance of consolidation. Preserve unknown and
refusal reasons through every adapter. A successful process observation must
never bypass authorization or tree-preservation gates. The proposed
cooperative holder check is explicitly insufficient for arbitrary uncooperative
processes; advisor signoff must accept that admission boundary before code.

## Followups

After signoff, file child specs in the slice order above with their reader/writer
rollout and verification gates. Record the advisor decision on STORY-2537 before
starting slice 1. Keep BUG-2458 blocked on that shared slice and its host
acceptance; retain the separate STORY-2325 cutover gate. Do not file or implement
the broker/grant-replacement slice without its own signoff.

## Related

- STORY-2537: this sketch and the binding advisor brief.
- TASK-1750 and BUG-2130: shared primitive and clock-step-safe writer rollout.
- BUG-2458 and STORY-2325: holder selection, migration and ownership cutover.
- BUG-2425 and BUG-2212: namespace observability and masked procfs.
- BUG-1934 and BUG-2419: explicit role binding and namespace-qualified tokens.
- STORY-2343: peer-credential socket precedent for later broker design.
