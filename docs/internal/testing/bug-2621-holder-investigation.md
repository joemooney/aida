# Admission-lock holder investigation

<!-- trace:TASK-3809 | ai:codex -->

TASK-3809 follows BUG-2621's contention refusal fix. This checkout did not
contain the investigation note referenced by the task when work began.

## Reported host evidence

The TASK-3809 comment from advisor-117 reports sampling `/proc/locks` every
0.2 seconds for 15 minutes, with 613 held-lock samples:

* 454 samples attributed both admission locks to deputy shell supervisor
  PID 4023454 (parent 4023446).
* 148 samples attributed the auto-train lock to train workers.
* 11 samples attributed the review-watch lock to reviewer queue-work children.

These counts are relayed evidence, not measurements repeated in this lane.
The underlying scratchpad log was not attached to the task.

## Source findings

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

## Remaining decision and acceptance

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

## Lane validation

Launcher job `j20261117T020512257-2-0` ran
`cargo test --profile agent -p aida-cli-lib --lib -- task3809`:
3 tests passed, 0 failed. `rustfmt --check` on the changed Rust files and
`git diff --check` also passed. No host re-sampling or production recovery
claim is made. TASK-3809 was punted as a design fork; the edits remain
uncommitted and the publication veto remains in force.
