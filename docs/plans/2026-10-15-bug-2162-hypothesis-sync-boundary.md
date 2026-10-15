# Plan: Preserve hypothesis evidence across store synchronization

Date: 2026-10-15
Specs: BUG-2162
Status: Draft — implementer sketch awaiting advisor signoff; no product code
Complexity: estimated 350–650 production LOC, 400–650 test LOC, 2–3 commits, risk high

<!-- trace:BUG-2162 | ai:codex -->

## Approach

Put an evidence preflight and a checked finalization around every AIDA-owned
store rebase, union merge, and reconcile fast-forward. Pin both input commits,
read their canonical graphs and merge bases strictly, and refuse contradictory
frozen descriptions before changing the live branch. After Git succeeds, check
the complete resulting graph against the pinned evidence and persist any sticky
locks first established by the combined graph before returning success. Hold the
existing store write lock throughout. Retain Git's existing abort discipline for
in-progress failures; never reset, force-push, or silently repair conflicting
scientific claims. This is separate from STORY-1675's accepted mutation slice.

```text
store lock + safe-state check
  -> pin local HEAD and fetched/merge target (+ all merge bases)
  -> strict canonical graph reads + evidence preflight
       incompatible -> Err, live branch/index unchanged
       compatible -> existing rebase / union / fast-forward
          Git failure -> existing abort + error reporting
          Git success -> strict resulting-graph validation
             -> persist only necessary lock fields + checked commit
             -> verify clean tree, stamps, identities, bodies and attached HEAD
             -> success / existing StorePullOutcome
```

The checked finalization is a success gate, not a transactional rollback claim.
If a read, write, or commit fails after Git has finished, return an explicit
error with pinned input and resulting SHAs and actual recovery state; leave
history intact. No clean-success summary, publish retry, or cache-fresh claim may
follow an evidence refusal. Advisor signoff on this failure behavior is required.

## Decisions

These are proposed implementation choices, not an assertion of approval.

1. **Stack on the hypothesis model, keep the follow-up separate.** The planning
   worktree starts at STORY-1675 head
   `df7fe5e5f206a7abd9119309833e9bae765a4d87`. No STORY-1675 edits, extra checker
   round, or change to its accepted scope. Before product work, refresh its
   independent verdict and actual merged/reviewed base. If it remains unmerged,
   keep BUG-2162 explicitly stacked; do not publish the parent's commits as a
   second independent feature. Sketch approval does not grant merge authority.
2. **Use one boundary at the low-level entry points.** Guard recognized store
   trees in `pull_rebase`, `pull_rebase_auto_merge`, and `merge_union_auto`.
   Generic non-store repositories retain their current behavior. Ensure the
   store lock inside those entry points, re-entering it when callers already
   hold it. Add a guarded fast-forward helper for `remote_create::merge_and_validate`
   rather than leaving its direct `git merge --ff-only` path outside the gate.
   The deprecated generic `git_ops::pull` must refuse a recognized store and
   direct the caller to the guarded helper; it must not remain an escape hatch.
3. **Pin authority inputs.** Keep `fetch_rebase_target`'s invocation-private ref
   and resolved commit; do not use FETCH_HEAD or re-read a tracking ref. Resolve
   the merge argument to a full commit once before use. Read all merge bases
   rather than choosing an arbitrary base in a criss-cross graph. Missing blobs,
   unreadable trees, malformed YAML, duplicate UUIDs or canonical spec IDs,
   ambiguous identity/path mappings, and non-regular object blobs refuse. An
   unrelated-history merge without a usable base refuses before live mutation.
   A proven unborn local branch is an explicit empty-graph bootstrap case;
   a failed HEAD read is not an empty graph.
4. **Preserve existing evidence conservatively.** Collect stamped rows from the
   input tips and merge bases; validate every stamp against its carrier body.
   A stamped base remains authoritative. With no stamped base, use the existing
   deterministic earliest-stamp rule from `hypothesis::preserve_merge` only
   when all stamped carriers agree on the body/hash. Different frozen bodies,
   invalid stamps, protected deletion, UUID/spec replacement, or identity rename
   refuse. Same-body independently created timestamps converge by that rule;
   never substitute a fresh sync timestamp for existing evidence. A stale tip
   without a lock may inherit a compatible stamp, but a conflicting offline tip
   body refuses before Git, even if scalar LWW could otherwise choose a winner.
   Older *unstamped* merge-base bodies alone do not invalidate later compatible
   stamped tips. Do not resurrect a deleted object or overwrite an offline body
   as an automatic recovery action.
5. **Do not treat a snapshot union as a new canonical graph.** Build a policy
   projection from input graphs: union qualifying hypothesis identity/type/tag
   facts, qualifying references/verified-by edges in both directions, and
   Completed result facts. Include each base and tip so tag/type/edge removal or
   result reopening at this boundary cannot cancel first-result evidence.
   Reuse/factor the predicate from `hypothesis::enforce`; do not concatenate
   duplicate Requirement rows and let HashMap insertion order choose a version.
   Evaluate each actual input graph as well as cross-tip combinations. Facts
   absent from one clone can meet facts present in another. Existing stamps
   remain sticky regardless of projected eligibility.
6. **Refuse ambiguity in a newly established prediction.** If the projected
   boundary newly locks a row, require all extant input-tip versions of that
   UUID to agree on the description and identity. An object absent because the
   other clone predates its creation is allowed; an object removed relative to
   a base is a deletion and refuses once protected. If eligible tips disagree
   on the body, refuse; do not infer causality from Git or row wall clocks. If
   they agree, freeze that body with a new lock at this sync boundary. The
   post-Git body must still match. Subsequent unlink/tag/type/status edits do
   not remove the planned stamp.
7. **Validate the entire output before writing lock companions.** Strictly read
   resulting objects, recompute the same policy with the candidate graph, and
   verify every protected UUID, spec identity, hash and description. A candidate
   that reveals previously unprojected eligibility must meet the same body
   agreement rule. Compute all lock-only changes first. Reuse canonical object
   paths, atomic writes, explicit path staging, and a checked commit. Do not use
   a full backend save, lossy loader, best-effort auto-commit, broad `git add .`,
   amend, rebase rewrite, synthetic history events, or cache reload as the
   validation/persistence mechanism. No writes when there is no new evidence.
   Re-read and verify persisted locks plus clean status before success. A new
   reconciliation commit is a normal descendant of the completed Git operation.
8. **Make refusal survive best-effort callers.** Use a typed evidence-refusal
   error retained through anyhow contexts, with a small classification helper
   for callers that currently tolerate transport errors. On evidence refusal,
   those callers must stop their current mutation/publication attempt (void
   cleanup helpers may report and return), instead of treating it as offline
   success. Preserve existing handling of ordinary transport failures. Audit
   coordination, node-registration/CAS, block allocation, init/upgrade, scheduled
   ledger sync, explicit db sync and pull. This is error propagation for this
   invariant, not a new retry or coordination policy. Post-Git finalization
   failures carry the same fatal classification. No new persisted marker or
   unlock flag is proposed.
9. **Keep success and storage contracts small.** No YAML schema change beyond
   the already-reviewed HypothesisLock; no new public StorePullOutcome variant.
   Document that Clean means no text conflicts and may include a required
   lock-materialization descendant commit. CLI success follows finalization,
   and cache maintenance/publication sees the finalized head. All execution-
   capable Git calls still use `host_git_command` and the verified store profile.
   No new worktree sandbox, external dependency, or alternate store is required.

## Files (in build-order)

### `aida-core/src/hypothesis.rs` — policy reuse and strict evidence planning

Factor the current eligibility predicate and deterministic stamp selection into
small internal helpers; add a pure sync planner over separately validated graph
snapshots and candidate graph. Keep mutation semantics and its existing tests.
Return the complete list of lock-only output updates or a typed refusal.

### `aida-core/src/git_ops.rs` — checked synchronization boundary

Add strict pinned-tree reads (NUL-delimited listing plus bounded Git blob reads
through the hardened command factory), guard both rebase entry points and every
success exit of the union merge, and supply a guarded fast-forward entry point.
Use the existing abort paths while Git is in progress. Finalize and verify under
the same lock before any Ok. Expose refusal classification only as needed by
cross-crate callers. Keep non-store Git behavior unchanged.

### `aida-cli-lib/src/remote_create.rs` — remove the fast-forward bypass

Route `merge_and_validate`'s direct fast-forward through the same guarded core
boundary. Keep multi-hub topology verification and push policy unchanged.

### `aida-cli-lib/src/coordination.rs`, `aida-cli-lib/src/db_block.rs`

Stop best-effort operations on evidence refusal. Preserve existing transport
failure behavior and function cleanup contracts.

### `aida-core/src/git_ops.rs`, `aida-cli-lib/src/store_sync.rs`, `aida-cli-lib/src/git_backend_cmd.rs`, `aida-cli-lib/src/upgrade_release.rs`, `aida-cli-lib/src/init_cmd.rs`, `aida-cli-lib/src/schedule_ledger.rs`, `aida-cli-lib/src/store_push_pull.rs`

Audit every pull/merge caller, especially ignored Results and retry/publish
paths. Change only the sites that swallow an evidence refusal or derive
success/publication from an unfinalized head. Existing propagated Result sites
need regression coverage rather than mechanical edits.

### `AGENTS.md`, `OVERVIEW.md`, `docs/cli/10-storage.md`

Document compatible convergence, conservative refusal, new combined-graph
locking, and actual post-Git failure recovery. Record that raw external Git,
old binaries, and lock-free readers are outside transaction isolation; do not
claim an all-or-nothing Git transaction or protection from arbitrary history
rewriting. Refresh the final file inventory after implementation.

## Critical Files

- `aida-core/src/hypothesis.rs`
- `aida-core/src/git_ops.rs`
- `aida-cli-lib/src/remote_create.rs`
- `aida-cli-lib/src/coordination.rs`
- `aida-cli-lib/src/db_block.rs`
- `aida-cli-lib/src/store_sync.rs`
- `aida-cli-lib/src/git_backend_cmd.rs`
- `aida-cli-lib/src/upgrade_release.rs`
- `aida-cli-lib/src/init_cmd.rs`
- `aida-cli-lib/src/schedule_ledger.rs`
- `aida-cli-lib/src/store_push_pull.rs`
- `AGENTS.md`
- `OVERVIEW.md`
- `docs/cli/10-storage.md`

## Reusable helpers (do not reimplement)

- `db::with_store_write_lock`, `store_write_lock_held`: existing same-root,
  thread-reentrant writer exclusion and safe-state checks. Store then cache;
  never push or make forge calls inside the lock.
- `git_ops::fetch_rebase_target`: private-ref fetch without shared FETCH_HEAD.
- `git_ops::host_git_command`, `git`, `git_with_env`: verified hook profile and
  transaction Git configuration. Use byte-preserving subprocess output when
  parsing NUL/batch formats; the current text wrapper trims/lossily decodes.
- `hypothesis::enforce`, `hypothesis::preserve_merge`: eligibility, canonical
  hash/body invariant and established stamp precedence. Factor shared pure
  logic without replacing mutation enforcement with the sync algorithm.
- `git_ops::ensure_store_write_safe`, `rebase_in_progress`, `store_worktree_issue`:
  attached-head safety and accurate recovery state; no fabricated abort advice
  after a completed rebase.
- `object_store::relative_object_path`, `write_object_if_changed`,
  `git_ops::commit_paths`: canonical paths and checked explicit-path commit.
- `conflict::merge_spec_three_way`, `git_ops::resolve_spec_conflict`: retain
  structured merge semantics; final verification also covers clean text merges.
- `git_ops::has_changes_checked`, `head_sha`: fallible status and exact head.
- Existing two-clone tests beside `store_pull_pins_single_branch_and_preserves_shared_fetch_head`,
  `failed_pull_rebase_aborts_and_restores_branch`, and
  `merge_union_auto_unions_diverged_registry_and_spec_tips`: extend those fixture
  patterns; no standalone reusable harness. `aida plan helpers BUG-2162`
  found no trace-derived helpers; the list above was verified from source.

## Risks + gotchas

1. **Lossy reads manufacture a missing hypothesis.** The ordinary
   `load_all_objects_with_fingerprints` deliberately skips some malformed rows.
   It is unsuitable here. Strictly enumerate and parse every canonical object
   in each pinned snapshot and the final tree; report every read/parse failure.
2. **Correct final fields do not imply a safe first-result transition.** A
   lock may disappear textually, or cancellation can split across clones.
   Input/base evidence and projected eligibility remain constraints on output.
3. **History is not a trusted timestamp oracle.** Do not guess which of two
   divergent pre-result bodies was "really first". Conservative refusal is
   intentional. This gate does not claim to detect arbitrary prior raw-Git
   deletion of evidence from all available tips/bases or forged historical
   metadata; stronger history auditing would require a separate signed scope.
4. **Rebase has several success returns.** Both clean and conflict-continuation
   exits must run exactly one finalizer; merge success and fast-forward must do
   likewise. Unknown conflicts and failed aborts retain existing errors.
5. **Post-Git I/O failure cannot safely be undone by reset.** Return fatal
   evidence-finalization failure, include exact SHAs and actual dirty/rebase
   state, and stop dependent mutation/publication. Dirty companion writes stay
   for explicit recovery; no blanket abort advice or automatic stash/reset.
   This is a proposed acceptance interpretation requiring advisor signoff.
6. **Lock-free readers can observe Git's working-tree update window.** The
   existing writer lock serializes cooperating writers, not reads or raw Git.
   The guarantee is validated evidence before AIDA sync success, not snapshot
   isolation for every reader. No new reader lock or staging repository here.
7. **Cost scales with store size.** One strict graph read per distinct pinned
   tree plus the candidate, with batched blob reads and per-invocation reuse.
   No process per object, persistent index, statusline path, or cache rebuild.
8. **Best-effort failures can become publication.** Several coordination and
   core registry paths discard pull errors and later commit/push. Prove fatal
   evidence classification survives contexts and stops those paths; do not
   merely improve the explicit `aida pull` success message.

## Tests (named, not "add tests")

Proposed names below; parameterize across plain rebase, structural rebase,
union merge and guarded fast-forward where meaningful, and reverse clone roles.
Use native Git in private temp repositories, never the live store.

- `bug_2162_clean_text_rebase_refuses_offline_body_change_before_git`: one clone
  completes a result, the other changes only the description so raw Git merges
  cleanly. First prove the fixture has no textual conflict on a disposable
  copy; then assert AIDA refuses with original live HEAD/index/body unchanged.
- `bug_2162_clean_sync_preserves_exact_existing_lock`: compatible clone edits
  preserve timestamp/hash/body, while unrelated title/comments still converge.
- `bug_2162_conflicted_sync_cannot_drop_frozen_evidence`: real YAML conflict,
  structured path plus final guard, including same-body independent timestamps
  and incompatible stamped-body refusal.
- `bug_2162_cross_clone_first_result_materializes_lock`: tag/qualifying link and
  completion arrive from separate clones; assert persisted stamp/hash, a clean
  descendant head, repeat-pull no-op and later canonical description refusal.
- `bug_2162_boundary_cancellation_stays_sticky`: both directions and edge types,
  tag/type removal, unlink and reopen split across input graphs; preserve a
  stamped base even if both tips omit the stamp.
- `bug_2162_newly_eligible_divergent_body_refuses`: no pre-existing stamp, but
  the cross-tip graph establishes eligibility with incompatible prediction
  bodies. No LWW or wall-clock guess.
- `bug_2162_protected_identity_or_deletion_refuses`: UUID replacement, spec-ID
  rename/clear, delete/modify, duplicate identities, creation versus deletion.
- `bug_2162_strict_snapshot_read_failures_refuse`: bad YAML, missing blob/tree,
  symlink/nonblob object, duplicate path identity, invalid stamp/hash, unborn
  bootstrap and unrelated-history cases.
- `bug_2162_all_success_exits_finalize`: initial clean return, each successful
  rebase continuation, clean/conflicted union and reconcile fast-forward.
- `bug_2162_finalization_failure_never_claims_or_publishes_success`: injected
  candidate read, atomic write and checked-commit failures; verify exact error
  state, no push/cache-fresh action, no reset and no false abort instruction.
- `bug_2162_best_effort_callers_stop_on_evidence_refusal`: context-wrapped fatal
  error stops coordination/registry and allocation publish; ordinary offline
  errors retain their prior behavior.
- `bug_2162_sync_lock_serializes_and_reenters`: second writer blocks until
  completion/refusal; already-held store lock does not deadlock.
- `bug_2162_non_store_and_untagged_sync_unchanged`: code repositories and normal
  store edits keep existing Git behavior, with no lock-only commits.

## Verification

Planning-only checks (run now):

```bash
aida plan verify docs/plans/2026-10-15-bug-2162-hypothesis-sync-boundary.md
git diff --check
```

After signed approval and implementation, in the correctly claimed worktree
and ordinary Cargo slot (never set AIDA_CARGO_NO_SLOT):

```bash
AIDA_CARGO_PRIORITY=high cargo test -p aida-core --profile agent bug_2162_
AIDA_CARGO_PRIORITY=high cargo test -p aida-core --profile agent hypothesis
AIDA_CARGO_PRIORITY=high cargo test -p aida-cli-lib --profile agent bug_2162_
AIDA_CARGO_PRIORITY=high cargo test -p aida-core --profile agent store_pull_
AIDA_CARGO_PRIORITY=high cargo test -p aida-core --profile agent merge_union_auto_
AIDA_CARGO_PRIORITY=high cargo test -p aida-core --profile agent failed_pull_rebase_
cargo fmt --all -- --check
make check-ci-fast
```

Then required compile gates at the exact published head, independent STRICT
review and advisor/operator publication gates. Add precise CLI caller filters
when the implementation identifies changed tests; do not imply these proposed
filters or unimplemented tests have run. The standing signal-test warning
remains acknowledged; no signal accommodation belongs in this feature.

## Followups

No additional feature proposed. Raw external Git tampering, full historical
forensic auditing and reader snapshot isolation are outside this sync-success
boundary; do not add an unlock flag, new lock protocol or reset recovery.

## Related

- Builds on STORY-1675 / PR #2684, canonical mutation scope accepted by advisor
  comment `01a141c6-a3ee-7970-b15f-aecab238d952`.
- BUG-2162 approval and sketch-first requirement:
  `01a141c6-b927-7da2-bc7c-2793bae214af`.
- TASK-1604: private fetch targets; TASK-1717: shared writer transactions;
  BUG-1229: disposable rebase HEAD; BUG-1929: host-store Git hardening.
- Approval requested for all proposed decisions above, especially conservative
  incompatible-body refusal, same-body stamp convergence, cross-tip eligibility,
  inclusion of union/fast-forward and fatal propagation, and non-destructive
  post-Git finalization failure. No implementation authorized by this document.
