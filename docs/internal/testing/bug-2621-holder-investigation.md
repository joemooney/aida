# BUG-2621 admission-lock investigation

This note preserves the BUG-2621 round-2/round-3 record and the later
TASK-3809 investigation and repair record. Historical validation applies to
the revisions named in each section; it is not validation of this merged tree.
The relayed host sampling below does not identify the holder and purpose
during the original 3122-failure episode or close the outstanding receipt and
recovery-contract findings. Existing punts and merge holds remain in force.

## BUG-2621 round 2: incident evidence unavailable

<!-- trace:BUG-2621 | ai:codex -->

Investigation on 2026-11-16 in the native round-2 seat did not establish the
actual admission-lock holder during the reported 3122-failure episode.
The binding advisor ruling requires that identification before implementation.
No holder identity or purpose is inferred from the error text.

Observed evidence:

- `aida show BUG-2621 --full --comments` contains the failure count and
  `blocked-retired-controllers` report, but no holder PID, start identity,
  descriptor inventory, or lock inode identifying that episode's holder.
- Neither `/home/dirtlab/ai/aida/.aida/review-watch` nor its `auto-train`
  counterpart exists in this seat's primary checkout view.
- `/proc/locks` returned no entries in this execution environment. This does
  not establish the historical host's lock ownership.
- `/home/joe` and the prescribed host codegraph executable are absent.
  `aida doctor check codegraph` reports a missing client. Source inspection
  used `rg`; no index was created or modified.
- `Directory::lock` in `aida-cli-lib/src/review_watch.rs` opens through
  `Directory::file` / `open_file` and returns a kernel flock guard. The source
  explicitly describes transient fork inheritance until exec. That source
  comment is not evidence of which process held the incident lock. Rust
  `OpenOptions` is used; no claim of an observed descriptor's close-on-exec
  flags can be made without its host fdinfo.

Finding 2 remains blocked on host incident evidence: capture the affected
lock's device/inode, kernel lock holder PID, process start identity and command,
and matching descriptor/fdinfo evidence for the controller and descendants.
This must distinguish a live controller performing lock-held work from a child
retaining the open file description. If the episode has ended, retained host
diagnostics or an authorized reproduction are needed; a present-day snapshot
cannot establish its historical holder.

Findings 1 and 3 remain open. No source implementation or tests were changed,
because the required investigation-first boundary is unresolved. No lock was
unlinked, stolen, or used as grounds to kill a process. Publication must remain
vetoed pending resolution of the punt.

## Round 3: remove the unapproved skip, retain the investigation punt

The round-3 seat rechecked the host paths and `/proc/locks`: both service
runtime directories and the prescribed `/home/joe/.../codegraph` executable
remain absent, and the kernel lock listing remains empty. The available spec
comments still do not identify the historical holder. These observations do
not identify the incident process or establish descriptor inheritance.

Following finding 2's explicit fallback, the unconditional WouldBlock success
path has now been removed. Busy admission again propagates its original lock
error into service failure reporting. The shared contention regression now
requires that refusal and its WouldBlock cause, while retaining the unsafe-file
regression. This is a rollback of the unapproved semantic skip, not completion
of the holder-receipt design or its recovery test contract.

Findings 1 and 3 remain open behind finding 2's investigation-first prerequisite;
none is disputed. The same-process guard test verifies refusal only and is not
claimed as held-then-abandoned child or close-on-exec evidence. Existing lock
opening and descriptor flags are unchanged. Host incident evidence or an
authorized incident reproduction is still required before implementing holder
validation, receipt refresh, holder-aware alerts, and their recovery regressions.
The punt remains a publication veto; do not mark BUG-2621 complete.

Round-3 validation:

- Launcher job `j20261117T011658219-2-0`: requested
  `cargo test --profile agent -p aida-cli-lib --lib -- advisor_loop_service`
  compiled successfully but executed zero tests (shared source filename is not
  the inclusion module name); this is not counted as tested.
- Launcher job `j20261117T012311936-2-0` with filter `bug2621`: four tests
  passed, covering contention refusal and unsafe-file refusal in both inclusions.
- `make check-ci-source`: all 48 fast-tier gates passed, including formatting.
  This wrapper also compiled/executed the structural ratchets; it was not purely
  source-only as assumed when invoked. No further direct Cargo runs were made.
- `git diff --check`: passed. Full `make check-ci-fast` remains a host-launcher
  pre-publication requirement; this punt grants no publication authority.

## Resumed round 3: narrowed ruling and rollback alert coverage

The spec's advisor-117 ruling recorded at 2026-11-17 01:30 UTC narrows this
round to finding 2 and assigns findings 1 and 3 to a follow-up carrying the
advisor's holder evidence. The current branch already contains the rollback
in commit `94a1e3d5ec`; this resumed seat does not claim it as a new change.
The additional contention regression assertions verify that the preserved
WouldBlock diagnostic increments the failure episode and appears in its
threshold alert in both service inclusions. These are rollback assertions,
not holder-aware alert or receipt-contract coverage.

Rechecking the available spec comments and host paths still yields no actual
incident-holder evidence: the two primary-checkout runtime directories and
the prescribed host codegraph executable are absent, `/proc/locks` is empty,
and `aida doctor check codegraph` reports the client missing. No holder or
inherited-descriptor cause can be inferred. Findings 1 and 3 remain incomplete;
none of the review findings is disputed. No receipt design or descriptor
semantics are changed. This is a partial slice requiring a `Refs BUG-2621`
publication declaration, not completion credit.

Resumed validation: launcher job `j20261117T015427969-2-0` executed all four
`bug2621` tests successfully, including the new alert assertions in both
inclusions. `rustfmt --check --edition 2021` on the shared source and
`git diff --check` passed. The complete `make check-ci-fast` pre-push gate
remains for the host launcher; it invokes Cargo directly and is outside this
seat's permitted test execution path. A new missing-context punt explicitly
records the incomplete findings and requires host reconciliation with the
narrowed advisor ruling before publication.

## TASK-3809: later holder investigation

<!-- trace:TASK-3809 | ai:codex -->

TASK-3809 follows BUG-2621's contention refusal fix. The TASK-3809 checkout did not
contain the investigation note referenced by the task when that work began.

### Reported host evidence

The TASK-3809 comment from advisor-117 reports sampling `/proc/locks` every
0.2 seconds for 15 minutes, with 613 held-lock samples:

* 454 samples attributed both admission locks to deputy shell supervisor
  PID 4023454 (parent 4023446).
* 148 samples attributed the auto-train lock to train workers.
* 11 samples attributed the review-watch lock to reviewer queue-work children.

These counts are relayed evidence, not measurements repeated in this lane.
The underlying scratchpad log was not attached to the task.

### Source findings

Both services include `advisor_loop_service.rs`. `tick_with_launcher` takes
`admission.lock` before reading the service receipt and retains it across
authority validation, review reconciliation, retired grant revocation, grant
minting, controller spawn and receipt persistence. A supervisor appearing as
the holder is therefore consistent with the source; it is not by itself proof
of a leaked descriptor. Workers and controller checks use the same lock.

`Directory::open_file` uses Rust `OpenOptions`, whose descriptors are already
close-on-exec. Adding only `O_CLOEXEC` would not address the interval between
fork and exec. The previous lock owner was a plain `File`: closing it does
not release flock while a duplicate inherited by a child remains open.

The narrow repair uses an owning `RuntimeLock` guard that explicitly unlocks
at scope exit. Its regressions retain a duplicated open file description,
verify contention while the guard lives, verify acquisition after guard drop,
and verify the old duplicate cannot release a subsequent owner's lock. Error
return recovery and the close-on-exec descriptor flag are also covered.
This changes neither state serialization nor admission refusal policy.

### Remaining decision and acceptance

This repair does not establish that the reported reviewer holders were
pre-exec descendants, nor eliminate the supervisor's legitimate critical
section. Do not claim findings 1 and 3 are closed on this evidence alone.

Moving supervision to a separate lock requires an accepted design for the
shared review state and controller binding boundary: `reconcile_reviews`
mutates review reservations, while `check_controller_locked` deliberately
compares the binding under the admission lock. Merely renaming the lock in
the supervisor would remove that serialization and is unsafe.

Before completing the task, obtain the raw holder log and the original review
findings, correlate process identities and lock duration, and approve a lock
ordering and receipt revalidation design. Then exercise supervisor/worker
contention, durable alert delivery and recovery, restart, and live/ambiguous
descendant refusal with both services. Re-sample the host after deployment;
synthetic duplicate-descriptor tests cannot establish production recovery.

### Lane validation

Launcher job `j20261117T020512257-2-0` ran
`cargo test --profile agent -p aida-cli-lib --lib -- task3809`:
3 tests passed, 0 failed. `rustfmt --check` on the changed Rust files and
`git diff --check` also passed. No host re-sampling or production recovery
claim is made. TASK-3809 was punted as a design fork; the edits remain
uncommitted and the publication veto remains in force.

### PR-4188 recovery-test rework

The advisor subsequently authorized the narrow lifetime repair with a Refs
trailer, leaving the broader acceptance above open. The later CI failure at
the killed-merge test's `Open` versus `Merged` assertion has a separate
fixture race: `crash_ready` created the readiness file before writing the forge
receipt. The parent watches file existence and immediately kills the child.
If killed between creation and write, the empty receipt selects `Negative`
confirmation instead of `Positive`. Normal recovery can then merge successfully,
but the fake forge still reports `Open` at the unchanged final assertion.

The rework writes the receipt to a temporary file in the same directory and
renames it into place only after the complete write. The kill/read handshake
therefore observes the complete receipt. All assertions, recovery decisions,
and production code remain unchanged. This addresses fixture synchronization;
it does not establish an admission-lock cause or close findings 1 and 3.

Launcher jobs `j20261117T055127094-2-0` and `j20261117T055203833-2-0`
each executed all 18 `pr_train_journal` tests successfully, including the
killed-merge/eject test. Job `j20261117T055317789-2-0` passed all 90
application-library ratchets; `j20261117T055317983-2-0` passed all 51 shared
ratchets. `make check-ci-source` passed all 48 gates, including workspace
formatting. That target unexpectedly also invoked Cargo for its structural
ratchet; the shared tests were independently verified through the launcher.
Job `j20261117T055318182-2-0` passed both CLI audit tests, matching the
checked ratchet manifest. Job `j20261117T055325967-3-0` built the CLI, but the
dynamic manual checker could not execute it from this seat: the binary is
inside the launcher's target overlay and absent from the seat-visible target
path. That last pre-push check remains for the host. The 86 Python
ratchet/manual/structural/writer-inventory fixtures and the writer inventory
check passed separately. Edits remain uncommitted for host publication with
a Refs trailer; the existing merge hold is not cleared by this validation.
