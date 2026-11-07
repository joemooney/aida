# STORY-2401 slice 3: confirmation before lifecycle mutation

Implements PR B of the advisor-approved sketch on STORY-2401 (signoff
2026-11-04 04:59 UTC). Base: `origin/main` at `6454b6e602a`.
The log contains slice 1 (#3324 / BUG-2433) and slice 2 (#3427).
This slice uses a **Refs STORY-2401** trailer. It does not complete the story;
the train journal and CI workflow digest remain slices 4 and 5.

## Confirmation boundary

- `forge_observed::confirm_negative`: two consecutive known negatives, bounded
  to three reads with injected backoff, plus an independent known negative.
  Pending/transient reads reset the negative sequence. Contradictory evidence
  is unconfirmed. An exact positive forge answer takes the positive path;
  Git alone proves a positive only when the forge supplied no contrary answer.
- `forge_observed::confirm_snapshot`: two identical complete snapshots, bounded
  to three reads, followed by an independent agreement check. Changed heads,
  bases, authors and states do not combine into authority.
- `forge_confirmation`: the production Git port uses a private ref and
  `--no-write-fetch-head`, validates the base/head, and times each Git command
  out after 30 seconds. It checks pinned-head ancestry, tree equality and
  conventional merge-subject receipts on the freshly fetched base. It never
  substitutes a stale tracking ref after a failed fetch. Cleanup is best effort.
- `Forge::observe_change_ground_truth` supplies that port; the in-memory forge
  scripts its independent answer for publication-boundary fixtures. Production
  providers retain the real Git implementation.
- GitHub observed metadata rejects incomplete JSON or unknown state, with a
  15-second command deadline. A denied/failed metadata read supplies no snapshot
  and cannot become permission to close. Legacy `Result` methods remain for
  consumers outside this slice.

## Mutation and status inventory

The inventory was obtained with `rg` over `close_change`, `change_metadata`,
`Merged`, completion/status writes and the train retirement/ejection methods.

| Consumer | Evidence and behavior |
| --- | --- |
| `pr_train::ForgeRails::test` | Base-read failures and ambiguous opening results suspend (with the exact branch reported); the journal/resume protocol remains slice 4. Re-read red CI before retirement. A CI-read failure reconciles landing or suspends. Missing-aggregate and pending-ceiling exhaustion suspend. |
| `pr_train::ForgeRails::retire`, `abort` | One pinned train gate checks repeated forge state plus fresh Git truth. Unconfirmed retains all pending members and live PRs. Positive evidence runs member landing. |
| `pr_train::ForgeRails::land` | Member head/refusal negatives are re-read; retirement additionally passes the pinned train gate. Unclassifiable legacy live member-gate errors suspend. Merge-error reconciliation uses the same train gate. |
| `pr_train::close_abandoned_train` | Confirm metadata/Git and re-read the dead owner's body identity. Never delete the branch after a failed close. Unconfirmed cleanup stops new admission rather than assuming the old train retired. |
| `pr_train::member_landed` | Closure follows successful pinned merge or reconciled positive landing; no error is promoted to landing. |
| `phase_driver::retract_publication_with_notice` | Confirm state/Git and author ownership before closing. Unconfirmed leaves the PR open under the existing protective hold behavior. |
| `pr_lookup::pr_is_merged_with_sink` | Stable metadata plus Git evidence; `None` when unconfirmed. Shared by queue reconciliation and phase reconciliation. |
| `phase_driver::detect_merged_pr` | Branch search discovers a number; confirm that exact number before granting completion credit. Credit and reopen checks remain required. |
| `queue_cmd::review_pr_is_merged_or_closed` | Review queue GC/unrouting requires the stable metadata/Git check. Unknown retains the row. Deliberate operator `rework` status changes are user decisions, not forge-derived flips. |
| `db_reconcile::collect_stranded_review_pr_resolutions` | Forge-only fallback for merged-before-review completion or closed-unmerged rejection requires the stable metadata/Git check. Unknown is a reported lookup failure, with no status flip. |
| `db_reconcile` git-log/covers completion | Existing immutable, pinned Git completion evidence; no negative forge read is used as completion evidence. Required open-change search needs two clear reads separated by backoff; unavailable or newly open evidence blocks. The writer still revalidates the independent pinned Git completion evidence. |
| `phase_gates` | Pure CI/reconciliation decisions consume the checked facts above; no direct forge-driven spec writer. |
| `dispatch_flow` | Records the outcome of an attempted dispatch, not a forge-driven spec/story status flip. Its legacy command readers remain ratcheted. |
| `review_watch` | Three fetch attempts with backoff. A typed exhausted fetch before reviewer spawn marks only its exact worker-bound reservation retryable. Parent completion releases it only after the queue worker exits and descendants are known gone. An actual/ambiguous reviewer retains ordinary attempt/capacity rules. No prose diagnostic is parsed to decide retryability. |

## Changed files

All Rust paths below are under `aida-cli-lib/src/`:

- `forge.rs`, `forge_observed.rs`, new `forge_confirmation.rs`, `lib.rs`
- `pr_train.rs`, `phase_driver.rs`, `pr_lookup.rs`, `queue_cmd.rs`,
  `db_reconcile.rs`, `reconcile_evidence.rs`, `session_reap.rs`
- `review_watch.rs`, `review_watch_forge.rs`
- `tests/story_1690_train_tests.rs`, `tests/story_2401_forge_observed_tests.rs`,
  `tests/task_1993_review_watch_tests.rs`, `tests/auto_bump_done_tests.rs`
- Documentation: `OVERVIEW.md` and this inventory.

## Validation

LIGHT gate only: `cargo test --profile agent -p aida-cli-lib --lib story_2401`
and `make check-ci-fast`. The focused tests cover negative/positive/unknown
sequences, conflicting or changing snapshots, private Git fetch/cleanup, train
suspension and landing, metadata validation, review fetch retry bounds and durable
same-head retry eligibility. The stranded-review fixture uses real local Git
base evidence alongside its fake forge; it never accesses the network.

Final results (2026-11-06):

- `cargo test --profile agent -p aida-cli-lib --lib story_2401`: **29 passed**,
  zero failures (12,564 unrelated tests filtered out).
- `make check-ci-fast`: **38 gates passed**. An earlier formatting-only failure
  was corrected; subsequent runs passed.
- `git diff --check`: passed.
- Logs: `/tmp/story-2401-slice3-verified.log` and
  `/tmp/story-2401-slice3-fast-verified.log`.

Native-seat handoff: edits remain uncommitted. No commit, push, PR creation,
queue completion, ship or review recording was performed. Host publication must
use a non-completing title such as:

`[AI:codex] fix(forge): confirm lifecycle negatives before cleanup (Refs STORY-2401)`

The story remains In Progress for the approved journal and workflow-digest slices.
No implementation or validation blocker remains; Git metadata/publication belongs
to the host launcher. Existing host-side punt/hold vetoes retain their authority.

<!-- trace:STORY-2401 | ai:codex -->

## Round 3 review corrections

Closed train members are consistent terminal observations. An exact subject
`(#N)` and parsed `Aida-Train-Member: #N at <pinned head>` trailer on the
freshly fetched base upgrades that observation to landed for credit consumers.
A squash subject alone never upgrades a Closed PR to Merged. Missing local head
objects skip ancestry/tree checks and still permit squash-receipt lookup.

Keep/no-op observations return after one metadata read. Open PRs do not sleep
or fetch during review GC, session reaping, stranded-review reconciliation or
merge-credit checks. Closed PRs remain possible train-credit candidates. Sweeps
share one fetched tip per repository/base; no fetched result survives the sweep.
Metadata refusals are `Known(Err)` and stop immediately; bounded transport
retries emit the caller's BUG-286 sink events.

Round 5 supersedes the earlier suspension decision per the advisor's F5 ruling.
The live member gate now separates settled policy refusals from unavailable
metadata, labels, head fetches, approval base reads, corpus artifacts and message
reads. Confirmed settled refusals eject members; failed evidence reads suspend.
The live-path test retires a negatively confirmed train after a draft refusal,
ejects its member and preserves the member's hold. Corpus decisions retain their
ordinary hold mirrors. A missing approval and a newly placed hold are covered by
the typed gate regression.

Publication must use the non-completing subject above. This native seat cannot
rewrite historical commit d726665ca8f or create the new round's commit; the host
must publish the declared Refs subject. Per round-5 finding 7, reword the
historical subject only if rebuilding the branch. AC3 and AC4 remain unimplemented.

Round-3 changed files (left uncommitted for the host):

- `aida-cli-lib/src/forge.rs`
- `aida-cli-lib/src/forge_confirmation.rs`
- `aida-cli-lib/src/pr_lookup.rs`
- `aida-cli-lib/src/queue_cmd.rs`
- `aida-cli-lib/src/db_reconcile.rs`
- `aida-cli-lib/src/session_reap.rs`
- `aida-cli-lib/src/pr_train.rs`
- `aida-cli-lib/src/tests/story_2401_forge_observed_tests.rs`
- `aida-cli-lib/src/tests/story_1690_train_tests.rs`
- This plan and handoff inventory.

The native publication declaration was successfully recorded with the exact Refs
subject above; it creates no commit or PR. Existing merge-hold authority remains.

Round-3 validation logs:

- Targeted command: `cargo test --profile agent -p aida-cli-lib --lib story_2401`.
  `/tmp/story-2401-round3-tests-final.log` is the corrected rerun; the initial
  compile found two missing test-fixture fields, which were fixed. The rerun
  passed: **33 tests, zero failures**, 12,583 unrelated tests filtered out.
- `make check-ci-fast`: **38 gates passed**, including formatting, the
  feature-disabled CLI check, and standalone TUI check. Log:
  `/tmp/story-2401-round3-fast.log`. Some guards attempted a fetch that the
  native seat refused as read-only; their local-evidence checks still passed.
- `git diff --check`: passed.

Round-3 validation is complete. No source/test blocker remains. The host owns
commit creation, historical subject repair and publication; the existing merge
hold still requires its ordinary authorized clearance. No queue completion,
merge, review recording, commit or push was performed by this seat.

## Round 5 review repairs (2026-11-06)

- Finding 1: downloaded `test-timings-lib-0` from run 42224787380. The only
  unbudgeted test above 10 seconds was
  `auto_bump_done_tests::stranded_review_pr_forge_failure_is_preserved_for_reporting`
  at 15.59s. This is an in-process missing-gh fixture, rather than a spawned
  binary. Its isolated config now sets `retry_base_delay_ms = 0`, retaining
  retries and failure assertions. The test joins the `story_2401` filter.
- Finding 2: settled live member-gate refusals are `Known(Err)`; unavailable
  secondary reads remain transient. Confirmation and hold release gates remain
  in place, and live ejection retains the member hold.
- Finding 3: required-check registration and explicit settlement deadlines again
  produce bounded deterministic failures. After negative landing confirmation,
  the train retires and members receive ejection reasons, instead of repeatedly
  suspending the same member set. Transport failures still suspend.
- Finding 4: a defaulted per-head fetch-failure counter survives attempt replacement.
  Three fetch-unavailable dispatches stop automatic launches; a new head or the
  existing operator release path can retry. The regression exercises exhaustion,
  setup-retry preservation, freed capacity and a fresh head.
- Finding 5: filed BUG-2463 (canonical BUG-HOME-229) under STORY-2401 for independent
  confirmation of rebase merges when the original PR head is absent locally.
- Finding 6: publication retraction and abandoned-train cleanup confirm only Open
  observations. Terminal observations return after one read without a pause/fetch.
- Finding 7: branch history was not rebuilt; historical d726665ca8f is unchanged.
  Host publication must retain the descriptive, non-completing Refs subject.

The active merge hold remains in force. This native seat leaves edits uncommitted
for host publication and does not publish, clear the hold, or mark the queue done.

Native publication declaration recorded successfully:
`[AI:codex] fix(forge): restore settled train refusals and bound retries (Refs STORY-2401)`.
This records host publication intent only; no commit or push occurred in this seat.

Round-5 changed files:

- `aida-cli-lib/src/forge.rs`
- `aida-cli-lib/src/gate/approval.rs`
- `aida-cli-lib/src/phase_driver.rs`
- `aida-cli-lib/src/pr_train.rs`
- `aida-cli-lib/src/review_watch.rs`
- `aida-cli-lib/src/tests/auto_bump_done_tests.rs`
- `aida-cli-lib/src/tests/story_1690_train_tests.rs`
- `aida-cli-lib/src/tests/story_2401_forge_observed_tests.rs`
- `aida-cli-lib/src/tests/task_1993_review_watch_tests.rs`
- This plan.

Validation notes: the first targeted run compiled successfully and passed 35/36
checks in 2.00s. The new live fixture alone failed because its opened train was
indexed under its base rather than tip; the fixture now indexes by tip. The
first fast-gate run found one formatting difference in an included test file;
that was corrected and the extra `rustfmt --edition 2021 --check
 aida-cli-lib/src/lib_part*.rs` check passed. Both full requested checks are
being rerun after these corrections; final results follow below.

Final targeted validation: `cargo test -p aida-cli-lib story_2401 --lib --profile
agent` passed **36 tests, zero failures**, 12,626 unrelated tests filtered out;
runtime **1.84s**. Log: `/tmp/story2401-round5-targeted-final.log`. This includes
the corrected live ejection/hold fixture, zero-delay timing regression,
registration-deadline ejection and per-head fetch cap. No broader Rust test run
was performed.

Final fast validation: `make check-ci-fast` passed **all 38 gates**. Log:
`/tmp/story2401-round5-fast-rerun.log`. Formatting includes both ordinary Cargo
modules and the extra `lib_part*.rs` inclusion graph. `git diff --check` passed.
Some read-only guards printed refused `.git/FETCH_HEAD` writes and used their
local-evidence path; every gate still returned success. No implementation or
validation blocker remains. The host must create the new round's commit and
publish the recorded Refs subject; existing merge-hold clearance remains a
separate authorized review action. No commit, push, merge, review record or
queue completion was performed by this seat.

## Round 6 review repairs (2026-11-06)

- Finding 1: the launcher identified the lib-1 failure as
  `gate_orchestrators_reach_outside_only_through_their_frozen_inventory`.
  Round 5 added direct `forge_observed::Observed` type/variant references in
  `gate/approval.rs` without listing them in `gate/orchestrator_leaves.rs`.
  The inventory now explicitly lists that typed read dependency. The ratchet
  remains unchanged; this protected inventory change retains human merge review.
- Finding 2: OVERVIEW now distinguishes unavailable CI reads (suspend)
  from required-check registration/settlement deadlines and settled
  member-gate refusals (eject/retire after negative landing confirmation).
- Finding 3: approval base-read diagnostics render the inner reason as prose
  for known refusals, pending observations and transient failures, preserving
  the unavailable classification without exposing Debug wrappers.

The initial targeted and fast commands stalled in the Cargo wrapper's slot
queue while `cargo ps --long` reported both slots free. Both were interrupted;
validation was restarted with the wrapper's documented `AIDA_CARGO_NO_SLOT=1`
option. No queue lock or other build process was modified. Results follow below.

This native seat leaves changes uncommitted for the host launcher. Publication
must use a new, non-completing Refs commit; the active merge hold remains in force.

Round-6 fast validation: `AIDA_CARGO_NO_SLOT=1 make check-ci-fast` passed
**all 38 gates**, including feature-disabled CLI, TUI, formatting and guard
fixtures. Log: `/tmp/story2401-round6-fast-final.log`. `git diff --check` passed.

Native publication declaration accepted:
`[AI:codex] fix(gate): declare typed approval reads and refresh train docs (Refs STORY-2401)`.
The host must create this round's new commit and publish it. No commit, push,
merge, hold clearance, review recording or queue completion occurred in this seat.

Round-6 targeted validation: `AIDA_CARGO_NO_SLOT=1 cargo test --profile agent
-p aida-cli-lib --lib gate_orchestrators_reach_outside_only_through_their_frozen_inventory
-- --nocapture` passed **1 test, zero failures**, 12,661 unrelated tests filtered
out. The fresh optimized test build took 25m 50s; the test itself took 0.09s.
Log: `/tmp/story2401-round6-inventory.log`. No unfiltered or workspace test run
was performed. All requested round-6 repairs are implemented and validation is
complete. Changed files: `OVERVIEW.md`, `aida-cli-lib/src/gate/approval.rs`,
`aida-cli-lib/src/gate/orchestrator_leaves.rs`, and this plan. The remaining
publication and active human merge-hold gates belong to the host/reviewer.


## Slice 4: resumable train intent (2026-11-06)

Binding advisor brief: implement AC3 only; publish one Refs STORY-2401 PR.
Workflow-definition digest evidence (AC4) remains slice 5. Existing confirmation,
merge leases, approval/corpus and dispatch gates remain authoritative. The journal
never grants merge permission or restores a saved mandate cover.

Implementation is in `pr_train.rs` and new `pr_train_journal.rs`. The main
worktree's `.aida/train/` owns a permanent nonblocking run lock and version-1
per-branch JSON entries. Entries retain the Built head/base/member pins, current
step (open, CI, gate, merge, retirement, member cleanup, done), recorded member
refusals and completed member cleanup. Atomic rename follows file fsync; directory
fsync completes the intent write before a call proceeds. Checkpoint failures stop
the operation. `forge.rs` adds no-op-outside-train checkpoints at GitHub command,
metadata, check-row and checked merge-attempt boundaries, so nested calls and
merge transport retries retain the active intent. The legacy train retry runner
also checkpoints each attempt.

Startup recovers unfinished entries before the abandoned-train sweep or candidate
admission. Known change numbers are read directly, including merged/closed trains.
An interrupted create without a saved number searches all states by exact branch;
two empty reads permit opening that branch, while ambiguous or failed reads stop.
Metadata must match the journaled head/base/branch. Existing forge_confirmation
and independent Git confirmation select landing, retirement or retention. A merge
already applied finishes only its remaining member cleanup. Recorded member refusals
are re-read and confirmed again before ejection; lifted refusals leave the member
eligible for a replacement train. Retirement independently confirms non-landing. Already-closed PRs are not
closed twice. Unknown/changed evidence stops admission; live gates and the merge
lease are re-acquired before any resumed merge. Journaled retirement preserves
remote branches rather than deleting a potentially moved or unconfirmed ref.
Legacy unjournaled trains keep the existing confirmed owner cleanup.

The fixture tests in `tests/story_1690_train_tests.rs` spawn a real test child,
wait for a receipt marker at the merge-call/eject-decision seam, SIGKILL it, then
resume using a fresh rails instance and independent forge fixture truth. They
assert no second open or completed merge, no ejection after a proven landing,
retirement after a confirmed ejection, and no replay of completed journal entries.
Additional fixtures cover unknown evidence, failed writes and partial member
cleanup, including removal of serialized mandate covers.

Validation: LIGHT gate only, `cargo test -p aida-cli-lib --lib pr_train` and
`make check-ci-fast`; results and publication declaration follow below. Initial
test compilation exposed Drop-related borrow lifetimes in existing port fixtures;
the journal's independent scope guard now owns cleanup without extending the
borrow of the train ports. No unrelated fixture changes are needed.

Native handoff: leave edits uncommitted; the host owns commit/PR publication.
No ship merge, review record or queue completion is authorized in this seat.
BUG-2463 / PR #3519 may require a host rebase before publication; this slice
leaves `forge_confirmation.rs` untouched.


Slice-4 targeted validation passed: `AIDA_CARGO_NO_SLOT=1 cargo test -p
 aida-cli-lib --lib pr_train` ran **163 tests, zero failures**, with 12,539
unrelated tests filtered out; runtime **58.55s**, incremental build **2m 00s**.
Log: `/tmp/story2401-slice4-tests-pass.log`. Both real child-kill recovery cases
passed, along with production checked-close, write-failure, unknown-evidence and
partial-member-cleanup fixtures. Earlier validation exposed an Option/Result
adapter mismatch and two fixture issues (crate-prefixed child test name and a
700-byte neighbouring-source match); these were corrected before this passing run.

Native publication declaration accepted:
`[AI:codex] feat(train): resume journaled intent from forge truth (Refs STORY-2401)`.
PR #3519 was still OPEN at the final overlap check; the host must rebase if it
lands first. Git metadata remains untouched by this seat. No commit, push, PR,
queue completion, review record or merge was performed. STORY-2401 remains
In Progress for slice 5.


Slice-4 fast validation passed: `AIDA_CARGO_NO_SLOT=1 make check-ci-fast`
completed **all 38 gates**, including feature-disabled CLI, standalone TUI,
formatting and guard fixtures. Log: `/tmp/story2401-slice4-fast-final.log`.
`git diff --check` passed. No implementation or validation blocker remains.
The host owns the uncommitted edits and publication under the recorded Refs
subject; existing punt/hold vetoes remain authoritative.

Changed files: `AGENTS.md`, `OVERVIEW.md`, `aida-cli-lib/src/forge.rs`,
`aida-cli-lib/src/pr_train.rs`, new `aida-cli-lib/src/pr_train_journal.rs`,
`aida-cli-lib/src/tests/story_1690_train_tests.rs`,
`docs/cli/04-git-lifecycle.md`, and this plan.

## Round 7: journal recovery review repairs (2026-11-06)

All four findings on PR #3533 at 649d36756499 are accepted.

1. Recovery ports explicitly clear named-member terminal consent. A regression
   starts with an interactive command naming #50 and a journal carrying guided
   #41; the member needs a fresh mandate and is refused without one. Both merge
   and retirement recovery use these same ports.
2. Gate/Merge recovery reuses normal CI waiting, repeated red observation and
   independently confirmed retirement. Pending CI waits rather than wedging
   admission. Changed pins and malformed/invalid journals remain fail-closed;
   the CLI guide documents inspection and operator discard under `run.lock`,
   preserving the entry in an unscanned archive after manual reconciliation.
3. Done is durably written before unlinking and directory fsync. Its active
   checkpoint is cleared so later nested calls cannot recreate the receipt.
   Startup prunes valid legacy Done receipts. Completed-train and crash-recovery
   fixtures require no remaining live journal; a fixture also tests legacy
   pruning and archiving a corrupt file outside the scan.
4. A PATH-isolated production-port fixture combines RecordingForge with a strict
   stubbed `gh pr list` and a real local Git remote. It tests the two-empty-read
   reopen, a unique existing change, ambiguous results, and each changed pin
   (head/base/branch), asserting no extra create or close. Handler candidate
   filtering is extracted unchanged and tested for landed/ejected exclusions,
   deduplication, all-recovered candidates and the empty-invocation guard.

Changed files: `aida-cli-lib/src/pr_train.rs`,
`aida-cli-lib/src/pr_train_journal.rs`, `aida-cli-lib/src/forge.rs` (test fixture
fields only), `aida-cli-lib/src/tests/story_1690_train_tests.rs`,
`docs/cli/04-git-lifecycle.md`, and this plan.

Native-seat handoff: edits remain uncommitted; the host must create a new round-7
commit and publish it. No prior-round commit counts. Publication uses a Refs
STORY-2401 trailer because slice 5 remains outstanding. Existing merge holds and
punt signals remain authoritative. Validation results follow below.

Native publication declaration accepted:
`[AI:codex] fix(train): constrain recovery authority and prune completed journals (Refs STORY-2401)`.
This records host publication intent only; no commit, push, PR creation, merge,
review record, hold clearance or queue completion was performed by this seat.

Round-7 focused validation passed: `AIDA_CARGO_NO_SLOT=1 cargo test -p
aida-cli-lib --lib pr_train` ran **168 tests, zero failures**, with 12,546
unrelated tests filtered out. Test runtime: **135.12s**; fresh unoptimized
build: **10m 11s**. Log: `/tmp/story2401-round7-train.log`. The five added
regressions and the updated real crash-recovery fixture all passed.

Round-7 fast validation passed: `AIDA_CARGO_NO_SLOT=1 make check-ci-fast`
completed **all 38 gates**. Log: `/tmp/story2401-round7-fast.log`. Formatting,
feature-disabled CLI, standalone TUI, documentation and guard fixtures passed.
Some guards printed refused `.git/FETCH_HEAD` writes under the native read-only
Git policy and completed successfully using their local-evidence path.
`git diff --check` passed. No implementation or validation blocker remains.
The host must create the new round-7 commit and publish the recorded Refs subject;
existing merge-hold clearance remains a separate authorized review action.

## Slice-4 integration follow-up: nested metadata retries (2026-11-06)

The current branch already contains the resumable journal and round-7 repairs,
plus main's BUG-2463 (#3519) and cross-node train-review changes (#3508).
The verified train gate calls `GitHubForge::change_metadata` while collecting
and refreshing its proof. That legacy reader used unchecked transport retries,
so a retry could spawn without another durable journal checkpoint. It now uses
`run_with_retry_checked` and refuses each attempt if the active journal cannot
be persisted. The checkpoint is a no-op for callers outside an active train.

A PATH-isolated production-adapter regression exercises both a failed initial
checkpoint and a retryable first forge response that replaces the journal
directory with an unavailable path. It asserts zero and one actual forge calls,
respectively, the retry count, and retained pinned Merge recovery evidence.
The existing SIGKILL-after-merge and SIGKILL-after-eject tests remain in the
binding `pr_train` test filter. No workflow-digest changes are included.

Changed files in this follow-up: `aida-cli-lib/src/forge.rs`,
`aida-cli-lib/src/tests/story_1690_train_tests.rs`, `AGENTS.md`, `OVERVIEW.md`,
and this plan. Native Git metadata remains untouched; host publication must
retain a Refs STORY-2401 trailer, with slice 5 outstanding. Validation results
follow below.

Follow-up validation passed:

- `AIDA_CARGO_NO_SLOT=1 cargo test -p aida-cli-lib --lib pr_train`:
  **169 passed, zero failures**, 12,554 filtered out; test runtime 59.84s,
  fresh build 8m 54s. Both SIGKILL recovery cases and the new production
  metadata-attempt checkpoint fixture passed.
  Log: `/tmp/story2401-current-train.log`.
- `AIDA_CARGO_NO_SLOT=1 make check-ci-fast`: **all 38 gates passed**.
  Log: `/tmp/story2401-current-fast.log`. Guards reporting read-only
  `.git/FETCH_HEAD` refusals passed using their existing local-evidence path.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Native publication declaration accepted:
`[AI:codex] fix(train): checkpoint final-gate metadata retries (Refs STORY-2401)`.
All five changed files remain uncommitted for the host. No commit, push, PR
creation, merge, review record, hold clearance or queue completion was performed.
No implementation or validation blocker remains. Slice 5 remains outstanding;
existing punt/hold signals remain publication vetoes. Turn ends with this handoff.

## Slice-4 integration follow-up: confirmation fetch checkpoints (2026-11-06)

The integrated checkout already includes the journal, round-7 recovery repairs,
metadata retry checkpoints and BUG-2463 (#3519). Independent confirmation still
issued nested base and absent-head Git fetches without another durable checkpoint.
The shared confirmation Git runner now persists the active train intent before
each remote fetch. Outside an active train the checkpoint remains a no-op.

A PATH-isolated production-runner fixture makes the journal unavailable either
before the base fetch or immediately after it succeeds. Both cases return
Transient evidence and preserve the pinned Merge entry. The first issues no
fetch; the second issues only the base fetch and refuses absent-head recovery.
The existing SIGKILL-after-merge and SIGKILL-after-eject fixtures remain in the
required `pr_train` filter. Slice 5 is unchanged.

Changed files: `aida-cli-lib/src/forge_confirmation.rs`,
`aida-cli-lib/src/tests/story_1690_train_tests.rs`, `AGENTS.md`, `OVERVIEW.md`,
and this plan. Validation and native host handoff results follow below.

Focused validation passed: `AIDA_CARGO_NO_SLOT=1 cargo test -p aida-cli-lib
--lib pr_train` ran **170 tests, zero failures**, with 12,564 unrelated tests
filtered out. Test runtime: **150.41s**; fresh unoptimized build: **11m 27s**.
Both SIGKILL recovery seams and the new confirmation-fetch fixture passed.
Log: `/tmp/story2401-integrated-train.log`.
`cargo fmt --all -- --check` and `git diff --check` passed.

Native publication declaration accepted:
`[AI:codex] fix(train): checkpoint nested confirmation fetches (Refs STORY-2401)`.
This records host publication intent only; the five changed files remain
uncommitted. No commit, push, PR creation, merge, review record, hold clearance
or queue completion was performed by this seat. Slice 5 remains outstanding;
existing punt/hold signals remain publication vetoes.

Fast validation passed: `AIDA_CARGO_NO_SLOT=1 make check-ci-fast` completed
**all 38 gates**, including the no-default-features CLI, standalone TUI,
formatting and documentation checks. Log: `/tmp/story2401-integrated-fast.log`.
Guards reporting native read-only `.git/FETCH_HEAD` refusals passed through
their existing local-evidence path. No implementation or validation blocker
remains. The host owns commit creation and publication of this follow-up with
the declared Refs subject. Turn ends with the five-file uncommitted handoff.

## Slice-4 integration follow-up: nested member-gate reads (2026-11-06)

The starting checkout already contains the slice-4 journal, both SIGKILL recovery
fixtures, and BUG-2463 (#3519). The member gate's labels and PR title/body reads
still bypassed per-call intent persistence after the outer gate checkpoint.
`LiveChecks` now checkpoints each of those nested reads. A PATH-isolated
regression makes the journal unavailable after a successful checkpoint and
asserts both reads refuse without spawning gh, retaining the pinned Gate entry.
This change stays within slice 4; workflow-definition digests remain slice 5.

Changed files: `aida-cli-lib/src/pr_train.rs`,
`aida-cli-lib/src/tests/story_1690_train_tests.rs`, and this plan.
Validation and native host publication results follow below.

Validation passed:

- `AIDA_CARGO_NO_SLOT=1 cargo test -p aida-cli-lib --lib pr_train`:
  **171 passed, zero failures**, 12,582 filtered out; runtime 76.84s.
  This includes both SIGKILL recovery seams and the new nested member-read test.
  Log: `/tmp/story2401-nested-member-train.log`.
- `AIDA_CARGO_NO_SLOT=1 make check-ci-fast`: **all 38 gates passed**.
  Log: `/tmp/story2401-nested-member-fast.log`. Some guards reported native
  read-only `.git/FETCH_HEAD` refusals and passed using their local evidence.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Native publication declaration accepted:
`[AI:codex] fix(train): checkpoint nested member gate reads (Refs STORY-2401)`.
No implementation or validation blocker remains. The three changed files remain
uncommitted for the host launcher; no commit, push, PR creation, merge, review
record or queue completion occurred in this seat. Slice 5 remains outstanding;
existing punt/hold signals remain publication vetoes. Turn ends with this handoff.


## Slice-4 integration follow-up: hold-label calls (2026-11-06)

The starting checkout already contains the resumable journal, both SIGKILL
recovery fixtures and BUG-2463. Shared hold-label runners still issued nested
reads and writes without persisting active train intent. The production read
and write runners now checkpoint immediately before each call, including label
mutation retries. Outside an active train these checkpoints are no-ops.

A PATH-isolated production-runner regression makes the journal unavailable
before a label read, after a successful probe but before label mutation, and
after a failed mutation but before retry. It asserts zero/one/one actual forge
calls and retains the pinned Gate entry in each case. Slice 5 remains excluded.

Changed files: `aida-cli-lib/src/merge_hold.rs`,
`aida-cli-lib/src/tests/story_1690_train_tests.rs`, and this plan.
Native-seat edits remain uncommitted for the host. Validation results and the
Refs publication declaration follow below.

Validation passed:

- `AIDA_CARGO_NO_SLOT=1 cargo test -p aida-cli-lib --lib pr_train`:
  **172 passed, zero failures**, 12,595 filtered out; runtime **139.85s**,
  fresh unoptimized build **10m 33s**. Both SIGKILL recovery cases and the
  new hold-label read/write/retry regression passed.
  Log: `/tmp/story2401-label-train.log`.
- `AIDA_CARGO_NO_SLOT=1 make check-ci-fast`: **all 38 gates passed**.
  Log: `/tmp/story2401-label-fast.log`.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Native publication declaration accepted:
`[AI:codex] fix(train): checkpoint hold label reads and mutation retries (Refs STORY-2401)`.
The three changed files remain uncommitted for the host launcher. BUG-2463
(#3519, `b815ab24313`) is already integrated. No commit, push, PR creation,
merge, review record, hold clearance or queue completion occurred in this seat.
No implementation or validation blocker remains. Slice 5 stays outstanding;
existing punt/hold signals remain publication vetoes. Turn ends with this handoff.

## Slice-4 integration follow-up: hold-gate polling (2026-11-06)

The integrated checkout already includes the resumable journal, both SIGKILL
recovery fixtures and BUG-2463 (#3519). The nested wait for a released hold
gate treated every check-row error as an unsupported-forge success, including
failure to persist train intent. Each poll now checkpoints before entering
that fallback, propagating persistence failures and stopping before a forge call.
Outside an active train the checkpoint remains a no-op.

A PATH-isolated production-adapter regression makes the journal unavailable
before the first poll or after the first pending response. Both cases return
an error instead of permission to continue, issue zero or one forge calls,
and retain the pinned Gate journal for recovery. Slice 5 remains excluded.

Changed files: `aida-cli-lib/src/ci_gate.rs`,
`aida-cli-lib/src/tests/story_1690_train_tests.rs`, and this plan.
Validation and native host publication results follow below.

Focused validation passed: `AIDA_CARGO_NO_SLOT=1 cargo test -p aida-cli-lib
--lib pr_train` ran **173 tests, zero failures**, with 12,614 filtered out;
test runtime **82.78s**, fresh unoptimized build **5m 40s**. The new hold-gate
poll regression and both SIGKILL recovery cases passed. Log:
`/tmp/story2401-hold-poll-train.log`.

Fast validation passed: `AIDA_CARGO_NO_SLOT=1 make check-ci-fast` completed
**all 38 gates**, including formatting, the feature-disabled CLI and standalone
TUI checks. Log: `/tmp/story2401-hold-poll-fast.log`. Guards reporting native
read-only `.git/FETCH_HEAD` refusals passed with local evidence.
`git diff --check` passed.

Native-seat handoff: the three files above remain uncommitted. Host publication
must use `[AI:codex] fix(train): stop hold-gate polling on journal failure (Refs STORY-2401)`.
No implementation or validation blocker remains. No commit, push, PR creation,
merge, review record, hold clearance or queue completion occurred in this seat.
Slice 5 stays outstanding; existing punt/hold signals remain publication vetoes.


## Slice-4 integration follow-up: approval proof fetches (2026-11-06)

The starting checkout already contains the resumable intent journal, both
SIGKILL recovery fixtures, and BUG-2463. Approval carry-over proofs and the
approval timestamp reader could still issue nested remote Git fetches without
persisting the active train intent. Those calls now checkpoint first. Carry-over
base reads distinguish an unavailable intent from an ordinary fetch failure, so
cached refs cannot swallow a failed checkpoint. This transient refusal is never
cached as a deterministic proof result. Missing timestamp evidence stays unknown.
Outside an active train the checkpoints remain no-ops.

The PATH-isolated production-helper regression refuses the initial base fetch
when its journal is unavailable, then separately allows one base fetch that makes
the journal unavailable and refuses the missing approved-object fetch. Both cases
also refuse the timestamp helper's fetch, retain the pinned Gate entry, and issue
zero/one remote calls respectively. The frozen gate dependency inventory explicitly
records these checkpoint calls; its human merge review requirement remains.

Changed files: `aida-cli-lib/src/gate/approval.rs`,
`aida-cli-lib/src/gate/review_carryover.rs`,
`aida-cli-lib/src/gate/orchestrator_leaves.rs`,
`aida-cli-lib/src/tests/story_1690_train_tests.rs`, and this plan.
Slice 5 remains excluded. Validation and native publication results follow below.

<!-- trace:STORY-2401 | ai:codex -->

Final-source targeted validation passed:

- `AIDA_CARGO_NO_SLOT=1 cargo test -p aida-cli-lib --lib pr_train`:
  **176 passed, zero failures**, 12,618 filtered out; runtime **73.45s**.
  Both SIGKILL recovery seams and the nested approval fetch regression passed.
  Log: `/tmp/story2401-proof-train-final.log`.
- The focused `gate_orchestrators_reach_outside_only_through_their_frozen_inventory`
  test passed (**1 test**). Log: `/tmp/story2401-proof-inventory.log`.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Native publication declaration accepted:
`[AI:codex] fix(train): checkpoint nested approval proof fetches (Refs STORY-2401)`.
This records host publication intent only. The five changed files remain
uncommitted. No commit, push, PR creation, merge, review record, hold clearance
or queue completion was performed. Existing punt/hold signals remain publication
vetoes. Fast-gate results follow below.

Fast validation passed: `AIDA_CARGO_NO_SLOT=1 make check-ci-fast` completed
**all 38 gates**. Log: `/tmp/story2401-seat-fast.log`. Guards reporting native
read-only `.git/FETCH_HEAD` refusals passed using their local-evidence path.
No implementation or validation blocker remains. The host owns commit creation
and publication under the accepted Refs subject. Slice 5 remains outstanding.


## Slice-4 integration follow-up: GitLab CI calls (2026-11-06)

The starting checkout already contains the resumable journal, both SIGKILL
recovery fixtures and BUG-2463. GitLab CI registration polling and nested
pipeline/job reads still bypassed durable per-call intent. Registration, watch
and post-watch rows now checkpoint; the GitLab API read runner checkpoints each
nested read. Post-watch row errors propagate as unavailable evidence instead of
becoming a red verdict.

A PATH-isolated production-adapter regression makes the journal unavailable
before registration, after registration, after watch, or between pipeline and
job reads (directly and within the post-watch fallback). It asserts exact remote
call counts and retained pinned CI recovery evidence. Slice 5 remains excluded.

Changed files: `aida-cli-lib/src/forge.rs`, `aida-cli-lib/src/pr_train.rs`,
`aida-cli-lib/src/tests/story_1690_train_tests.rs`, `AGENTS.md`, `OVERVIEW.md`,
and this plan. Validation and native host handoff follow below.

<!-- trace:STORY-2401 | ai:codex -->

Validation passed:

- `AIDA_CARGO_NO_SLOT=1 cargo test -p aida-cli-lib --lib pr_train`:
  **177 passed, zero failures**, 12,632 unrelated tests filtered out;
  runtime **66.85s**, fresh build **5m 12s**. Both SIGKILL recovery seams
  and the new GitLab CI regression passed.
  Log: `/tmp/story2401-gitlab-train.log`.
- `AIDA_CARGO_NO_SLOT=1 make check-ci-fast`: **all 39 gates passed**,
  including formatting, feature-disabled CLI and standalone TUI checks.
  Log: `/tmp/story2401-gitlab-fast.log`. Guards reporting native read-only
  `.git/FETCH_HEAD` refusals passed using their local-evidence path.
- `git diff --check`: passed.

Native-seat handoff: the six files above remain uncommitted for the host.
Publication subject:
`[AI:codex] fix(train): checkpoint GitLab CI reads before calls (Refs STORY-2401)`.
No implementation or validation blocker remains. No commit, push, PR creation,
merge, review record, hold clearance or queue completion occurred in this seat.
Slice 5 remains outstanding; existing punt/hold signals remain publication vetoes.

## Slice-4 integration follow-up: verified required-check reads (2026-11-07)

The starting checkout includes the resumable journal, both SIGKILL recovery
fixtures and BUG-2463 (#3519). The final verified train proof's nested
`github_required_checks` read still bypassed per-call persistence. It now
checkpoints active train intent immediately before invoking the forge. Outside
an active train this remains a no-op. Workflow-definition digests stay in slice 5.

A PATH-isolated production-helper regression refuses a read when the journal
is unavailable, and separately allows one read that makes it unavailable before
the next proof refresh. It inspects the checkpoint captured at call time,
asserts zero/one actual forge calls, and retains the pinned Merge entry with its
change number, train head and member head. The existing SIGKILL-after-merge and
SIGKILL-after-eject cases remain in the binding `pr_train` filter.

Changed files: `aida-cli-lib/src/forge.rs`,
`aida-cli-lib/src/tests/story_1690_train_tests.rs`, and this plan. Native-seat
edits remain uncommitted for the host. Validation results and the Refs
publication declaration follow below.

<!-- trace:STORY-2401 | ai:codex -->

The first fast-gate run exposed an integration mismatch with main's newly
merged TASK-3222 writer inventory: journal sinks were absent and the activity
writer still named the old `member_landed` function. Regenerated and reviewed
`scripts/store-writers.toml`, adding the journal's shared run-lock and live
confirmation boundary notes and replacing the stale activity-writer identity.
All rows retain their blocked S2 disposition; no runtime authority is added.
This inventory is the fourth changed file.

Targeted validation passed: `AIDA_CARGO_NO_SLOT=1 cargo test -p aida-cli-lib
--lib pr_train` ran **178 tests, zero failures**, 12,632 filtered out; test
runtime **79.04s**, cold build **5m 34s**. Both SIGKILL recovery seams and
the new required-check checkpoint regression passed. Log:
`/tmp/story2401-required-train.log`. Formatting and `git diff --check` passed.

Native publication declaration accepted:
`[AI:codex] fix(train): checkpoint verified required-check reads (Refs STORY-2401)`.
This records host publication intent only; no commit, push, PR creation,
merge, review recording or queue completion occurred in this seat.

Final fast validation passed: `AIDA_CARGO_NO_SLOT=1 make check-ci-fast`
completed **all 41 gates**, including the reconciled store-writer inventory,
feature-disabled CLI, standalone TUI and formatting checks. Log:
`/tmp/story2401-required-fast-final.log`. Guards reporting native read-only
`.git/FETCH_HEAD` refusals passed using their local-evidence path.
`git diff --check` passed. No implementation or validation blocker remains.
The host owns commit creation and publication under the accepted Refs subject;
slice 5 remains outstanding and existing punt/hold signals retain their veto.
Turn ends with the four changed files uncommitted.
