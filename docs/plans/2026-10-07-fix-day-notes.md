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

## Checkpoint 2: TASK-1338 (origin TASK-1-229)

Pushed commit `d7aa7bf252eef32c180e50e61a6dcc6a942cfbaf`.
Tests: 3 new reopen, 7 TASK-1600, 36 queue/linkage; all exit 0.
Fresh CLI build and formatting passed; final fast rerun passed 21 gates.
Real-store Approved resets confirmed by CLI JSON, and targeted read-only
reconciliation from main found no eligible flips for either spec.

## Item 3 writer evidence

Canonical review recorder rejects empty recorder identity before writing or
archiving. The shipped review template now uses `aida review record` rather
than raw verdict-file examples. Actual recorder/time metadata is stamped at
write time; historical missing provenance still blocks reconciliation.
`cargo test -p aida-cli-lib --lib review_verdict_tests -- --test-threads=1`:
exit 0, 76 passed. Includes newly authored current/retained metadata,
blank-recorder refusal with unchanged bytes, strict malformed-history refusal,
and template guard. `make check-ci-fast`: exit 0, 21 gates.
Logs: `/tmp/aida-fix-day-evidence/provenance-{tests,fast}.log`.

PR-2435 live head `6fa035c26b735a9e7afc45bb1d2f0bba706144da` has matching
recorded approval, green required checks, and no listed merge hold. Shipping
from fix-day cwd incorrectly applies that checkout's supervised spec gates
(TASK-1603/TASK-1338) to the explicit PR; no gates cleared. Retry uses the
existing source branch in a separate administrative worktree. Its local tip
`9ec89b97b6a9066893618f26fa23c180dc1c912c` differs from the forge head; attempted
fast-forward refused and the local branch was left unchanged. Explicit ship
uses forge metadata/approval-at-head, without rewriting or pushing this branch.
This administrative checkout contains no implementation edits.

## Checkpoint 3: provenance writer

Pushed `8ebc929351873b814ce3ffd5303651ae45fbf60d`; 76 review-verdict tests
and 21 fast gates passed. PR-2435 remains OPEN: source-branch ship also refuses
TASK-1-216's unset/supervised mode. Both old-binary attempts returned exit 0
without merging; logs explicitly show refusal. No mode, hold, approval, or
required check was overridden. Human/advisor ship is a concrete blocker;
TASK-1324 is not claimed Completed. No additional review wait is introduced.

## Item 4: TASK-1606 combined repair

Advisory lease `01a117245df6`. Incorporated source
`431763d3c83d728d63c297633346060c4e16ccd6` into the single fix-day branch,
preserving TASK-1602 ownership protections. Merge conflicts resolved by retaining
both documentation paragraphs and both independent behavior changes. One
`--wait [SECS]` flag remains: bare means 300 seconds; omitted CI is unbounded,
drive ownership refuses immediately; drive seats never self-wait. One deadline
covers ownership and CI registration/settlement. Typed codes preserve fail-closed
holds/review and distinguish timeout/red/not-mergeable/stale definition.

Targeted results so far: 110 ship unit tests, 15 CI-gate unit tests, and 12
label/drive/GitLab integration tests passed (exit 0). The original integration
fixture lacked the new mergeability query; added explicit MERGEABLE response.
GitLab tests now assert classified jobs before merge and no unbounded watcher;
failed/unavailable job rows continue to refuse. Shared-deadline regression verifies
ownership release never grants a second CI allowance. Early compile caught a
duplicate wait field from the automatic merge; removed it before verification.
Logs use `task-1606-unit-final.log`, `task-1606-ci-gate.log`, and
`task-1606-holds-final.log`; preliminary logs are not green evidence.

TASK-1606 final fixture run: all 11 preflight scenarios plus help exit-code
assertions passed (exit 0). The script now checks optional `--wait [<SECS>]`
and the 300-second bare-flag default. Full fast rerun: 21 gates passed.

## Item 5: TASK-1337 (TASK-1-230) pickup preflight

Advisory lease `01a1172cc702`; the canonical spec remains Draft because promotion
requires advisor authority. Explicit operator assignment covers implementation;
no promotion gate was bypassed. Pickup first synthesizes its plan read-only,
reports existing role precedence and shell mismatch, checks occupied explicit /
review branches, and validates interactive child delegation without minting a
grant. Only then does it persist a synthesized queue row and perform setup.
An idle, clean, unlocked, unleased checkout gets a manual removal offer; locked,
dirty, leased, or live-agent checkouts never do. The existing grant issuer repeats
validation at issuance. Any returned error after session setup prints worktree
entry and guided continuation. Successful exec still replaces the launcher.

Verification: 152 queue unit tests, 1 authority test, and all 15 Linux pickup
integration tests passed (exit 0). Black-box tests prove no queue row, lease, or
status mutation on branch/delegation refusal, and a manual command after failed
exec. Initial fixture errors inspected only stderr and allowed missing-interpreter
exec to fall through to the real Codex later on PATH; final fixtures inspect both
streams and exclude real Codex directories. Final logs: `task-1337-queue.log`,
`task-1337-authority.log`, `task-1337-integration-final.log`. All 21 fast gates
passed in `task-1337-fast.log`. Formatting and diff whitespace checks passed.
The large queue-command diff is primarily rustfmt indentation of the existing
post-setup tail within an error-reporting closure (`git diff -w` shows scope).

Full workspace verification has started; no final workspace or sandbox success
is claimed at this checkpoint. PR-2435 is still blocked by supervised/unset-mode
ship policy. TASK-1607 and its lease remain untouched.
