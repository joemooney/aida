# Fix-day evidence and observations

Scope and order: [approved plan](2026-10-07-fix-day.md). Sole implementer;
no per-fix PR/review, gate weakening, new specs, or real-store drains.

## Ownership and base

Worktree: `/home/joe/ai/aida-fix-day-2026-10-07`.
Branch: `fix-day-2026-10-07`.
Base: `738653fb3e6de71d0975047adb143c5f31a25d85` (fetched origin/main).
Implementation session lease: `01a11713b830` (TASK-1603).
TASK-1607's lease/worktree is outside scope and untouched.
Native Codex session: `01a11712-4398-7781-a13c-d53e0e6b7e33` (environment evidence,
not inferred from the lease PID).
Plan saved in commit `e2b39a66d6c4181e4f93f3df478e8acbe18bad06`.

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
Writer repair and tests are recorded below.

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

## Pushed code checkpoints

| Item | Full SHA | Targeted evidence |
|---|---|---|
| TASK-1603 | `2722961da71e153c83eda06d96d44f328897209a` | 85 tests; 21 fast gates |
| TASK-1338 | `d7aa7bf252eef32c180e50e61a6dcc6a942cfbaf` | 46 tests; 21 fast gates |
| Provenance writer | `8ebc929351873b814ce3ffd5303651ae45fbf60d` | 76 tests; 21 fast gates |
| TASK-1606 | `226616816cd686c4a5255ad6af09b9c5f6404e73` | 137 Rust tests; 11 Python scenarios + help; 21 fast gates |
| TASK-1337 | `c418c60fe7a58058fbea577b55e85bdbea1e1f65` | 168 tests; 21 fast gates |

These are terminal-reported implementation checkpoints, not advisor approval.
No independent regression/acceptance seat was launched by this implementer.

## End-of-day measurement blockers

Both sandbox attempts used independent throwaway repositories and git-canonical
stores, created with `sandbox create --path <absolute-temp-path> --seed` and
verified to contain three sandbox specs. Fresh build: `cargo build -p aida-cli`
exit 0; `aida --version` reports `c418c60fe7+dirty` (only the evidence notes changed
after that code checkpoint). Each attempt invoked `burndown run --tag sandbox
--max 3 --concurrency 3` with a validated existing sandbox `AIDA_STORE`; each
exited 1 before launch: starting a drain requires dispatch authority. This
implementer has no authorized driver grant. No `--force`, manufactured grant,
role elevation, TTY impersonation, or real-store drain was used.

Artifacts: `/tmp/aida-fix-day-evidence/sandbox-{1,2}.log` and
`sandbox-results.json`. Sandboxes retained under
`/tmp/aida-fix-day-sandboxes-484ycwh8/run-{1,2}/`. Outcomes: shipped 0;
launch-refused **not measured**; other 2 driver-authority refusals before launch.
This is two blocked attempts, not two successful three-spec end-to-end runs.
No valid post-fix success-rate comparison to 17 successful / 20 failed of 37
baseline runs can be made. An authorized driver must perform that measurement.

Whole-branch review/operator merge remain pending; no review verdict inferred.
PR-2435 also requires an authorized human/advisor ship because its unset /
supervised mode is preserved. TASK-1338 / TASK-1337 promotion remains advisor
work. Stretch items were not attempted because item 3 shipping is blocked.

Coordination observation: MCP-created advisory claims recorded branch `main`
from the server cwd while correctly recording the sibling worktree. The CLI
implementation lease records `fix-day-2026-10-07`. Those advisory locks are not
proof of edits on main, and their null PID / `dormant` state is not proof of
this native session's liveness. No other session or harness lease was repaired,
pruned, or relabeled. Actual ownership evidence is the operator's sole-owner
assignment, native session ID, worktree, commit SHAs, and test logs above.

Rebuilt combined-code PR-2435 retry from its administrative source worktree:
`target/debug/aida pr ship 2435 --no-pull --no-cleanup` now exits **23**,
explicitly refusing TASK-1-216 unset/supervised auto-merge. Forge verification:
OPEN, CLEAN, unchanged head `6fa035c26b735a9e7afc45bb1d2f0bba706144da`.
`pr-2435-ship-final.log` is the final refusal evidence; earlier exit-0 attempts
predate TASK-1606. This confirms exit-code repair without loosening that gate.

## Workspace verification environment

Initial `cargo test --workspace` inherited this agent launcher's `AIDA_BIN`
pointing at `/home/joe/ai/aida/target/agent/aida` and native agent identity.
Output-format, signal/queue child-runner guards and inbox identity tests reported
failures. Isolated BUG-1745 reproduction exits 101 with the exact guard:
expected a runner under target/profile/deps, got the foreign agent binary;
unset AIDA_BIN and re-run. Original evidence is retained in `workspace-final.log`
and `workspace-failure-1745.log`; those failures are not counted as green.
A complete `cargo test --workspace` rerun removes inherited `AIDA_*`,
`CODEX_SESSION_ID`, and `CODEX_THREAD_ID` only in that test process. No production
environment, grant, roster, gate, or test expectation is changed. Removed variable
names (no values/secrets) and source SHA are recorded in `workspace-clean-env.json`;
results are in `workspace-clean.log` and `workspace-clean.exit` when complete.
Final fast check already passed 21 gates in `fast-final.log`; final formatting and
whitespace checks passed. The clean workspace result is pending here.

Initial workspace outcome: exit 101; CLI library 7,787 passed, 15 failed,
2 ignored (all earlier integration suites green). The 15 failures are retained
in the original log. Clean-environment reruns of all six affected groups passed
(24 parent tests, exit 0 for every group), including output envelopes, inbox identity,
real SIGTERM propagation, cross-process global queue locks, and piped inbox
watermarks. Logs: `workspace-failures-clean.log` and its JSON exit summary.
The complete clean workspace rerun remains the final broad verification.

Clean workspace CLI library result: 7,802 passed, zero failures, 2 ignored,
448.76 seconds. Remaining core/workspace crates were still running at this
observation. End-of-day read-only real-store JSON confirms TASK-1538 and
STORY-1488 are still Approved (`task-1538-final.json`, `story-1488-final.json`).
For the authorized sandbox driver and independent regression seat, build this
branch and pin `AIDA_BIN` to its own absolute `target/debug/aida`; do not inherit
`/home/joe/ai/aida/target/agent/aida` while claiming this branch's behavior.
For Rust tests, use the recorded clean environment so the runner resolves its
own child test executable rather than an agent-launcher binary.

Clean full-workspace run reached core and exited 101 on one genuine integration
mismatch: `aida_review_embeds_a_parseable_verdict_file_example` still required the
raw JSON write example removed by the provenance repair. Core otherwise had
1,319 passed and 2 ignored. Updated that existing embedded-template regression
to require the canonical recorder command (spec, PR, verdict, exact reviewed
SHA, summary, blocking findings), provenance/read-back instructions, and absence
of direct verdict heredocs. BUG-280's write-before-comment guard remains.
All 23 template tests pass. This is an in-scope provenance integration correction,
not a skipped test or weakened provenance check. A final complete clean-environment
workspace rerun will follow the checkpoint; previous green CLI evidence is retained.
