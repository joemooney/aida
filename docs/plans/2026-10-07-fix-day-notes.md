# Fix-day evidence and observations

Scope and order: [approved plan](2026-10-07-fix-day.md). Sole implementer;
no per-fix PR/review, gate weakening, new specs, or real-store drains.

## Ownership and base

Worktree: `/home/joe/ai/aida-fix-day-2026-10-07`.
Branch: `fix-day-2026-10-07`.
Base: `738653fb3e6de71d0975047adb143c5f31a25d85` (fetched origin/main).
Implementation session lease: `01a11713b830` (TASK-1603).
TASK-1607's lease/worktree is outside scope and untouched.
Plan saved in commit `e2b39a66d6`.

## Checkpoint 1: TASK-1603

Combined commit: `2722961da71e153c83eda06d96d44f328897209a`, pushed to origin.
Incorporates source `eec6fca71808f3d9f67179f1b54fb51260fd90ec` with both
OVERVIEW paragraphs retained. Evidence newly run on the combined tree:

- `cargo test -p aida-cli-lib --lib drain_state::tests:: -- --test-threads=1`: exit 0, 69 passed.
- `cargo test -p aida-cli-lib --lib orchestrator::tests:: -- --test-threads=1`: exit 0, 14 passed.
- `cargo test -p aida-cli-lib --lib pipelined_delayed_child_does_not_exhaust_batch_or_next_n -- --test-threads=1`: exit 0, 1 passed.
- `cargo test -p aida-cli-lib --lib phase1_registration_failure_leaves_status_unstarted -- --test-threads=1`: exit 0, 1 passed.
- `cargo fmt --all -- --check`: exit 0.
- `make check-ci-fast`: exit 0, 21 gates passed.

Logs: `/tmp/aida-fix-day-evidence/task-1603-{drain-state,orchestrator,delayed,phase1,fmt,fast}.log`.
This is targeted local evidence, not workspace/CI/vendor-drain verification.
Independent UUID records retain simultaneous member authority through sibling
registration/cleanup. Telemetry writes cannot resurrect removed tokens. The
delayed-head regression covers both batch and nextN selection. Registration
failure precedes status mutation. Actual simultaneous process stress and
sandbox/vendor end-to-end remain for final verification.

## Item 2 observations

TASK-1-229 resolves to TASK-1338; TASK-1-230 resolves to TASK-1337.
Both are Draft. CLI promotion of TASK-1338 refused advisor-authority gate;
no gate bypass attempted. Explicit operator assignment permits work and MCP
advisory claim succeeded: `01a11719a179`. Approval/status transition requires
an authorized advisor; this does not block editing under the advisory claim.

TASK-1538 and STORY-1488 initially read Done despite repeated human reopens.
Pre-change CLI JSON saved in `/tmp/aida-fix-day-evidence/{task-1538,story-1488}-before.json`.
Main already has TASK-1600's SHA fence; legacy status history and the queue
classifier still need coverage. New code treats the latest non-automated status
decision as authoritative, fencing old commit timestamps on a Done/Completed
to Approved reopen. A newer deliberate decision or later commit permits progress.

## Item 3 inherited evidence

PR-2435 duplicate round removal was already done by the advisor with an audit
backup at `.aida/handoff/advisor/audit/PR-2435-20261007T062120Z/` in the main
checkout. Do not repair it again or infer provenance. The review skill template
still demonstrates raw verdict-file writes without recorder/time metadata;
these become unattributed archived rounds on subsequent canonical recording.
Writer repair and tests are pending.

Item 2 targeted results on the working tree: three new TASK-1338 regressions,
seven TASK-1600 regressions, and 36 queue/linkage tests passed, all exit 0.
`cargo build -p aida-cli` passed. The original test expectation incorrectly
expected Completed despite `closure:pending`; corrected to expect Done while
preserving that gate. A fast-gate formatting check ran during that edit and
failed; a clean full rerun follows. Both requested real-store status resets
succeeded using the fresh branch binary. Targeted reconciliation dry-runs
require default-branch cwd and refused from the feature worktree; repeated
read-only from main without switching or editing main's checkout.
