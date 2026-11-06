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

For finding 5 this slice deliberately retains **suspension**, the review's
permitted alternative: the live member gate still mixes settled local refusals
with unavailable secondary reads. A newly refused pre-land member (including a
lapsed mandate) suspends the train with every member, PR and hold intact, instead
of ejecting that member. The live-path regression exercises a local draft
refusal through `LiveTrainForge` and `ForgeRails::land`, verifies the typed
unavailable error and open train, and retains a pre-existing hold. Scripted
`Known(Err)` gates can still eject. Restoring selective live ejection requires a
later typed member-gate boundary; no string classification is introduced here.
This supersedes the older BUG-2214 ejection claim for this live adapter.

Publication must use the non-completing subject above. This native seat cannot
rewrite historical commit d726665ca8f or create the new round's commit; the host
must publish the declared Refs subject and correct the historical completing
subject if retaining it in published history. AC3 and AC4 remain unimplemented.

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
