# BUG-2621 round 2: incident evidence unavailable

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
