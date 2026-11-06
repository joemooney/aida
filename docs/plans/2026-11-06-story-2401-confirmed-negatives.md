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
