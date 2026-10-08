# BUG-1808 common publication implementation

<!-- trace:BUG-1808 | ai:codex -->

Status: implementation plan, not production readiness. Assigned checkout is
`aida-bug-1808`, branch `bug-1808-delegation-publication`, lease
`01a119db6f34`. Initial clean HEAD and origin/main both equal
`5d4180e5ded8a55ad1b02beac5571c190d3da0ac`.

## Binding contract and precedence

Requirements and decisions remain canonical in AIDA. This document maps their
implementation; it is not a parallel requirements database. Read using
`/home/joe/ai/aida/target/agent/aida`.

The original BUG-1808 description and independent A–E require delegated-seat
preflight for setup-only/TUI too, exact assigned-child scope without recursive
delegation, unchanged invocation lifecycle/history on late refusal, selected
static seats before first admission and consistent explicit role selection.

The exact canonical common bodies verified against the handoff artifacts are:

| Body | Canonical comment | SHA256 |
| --- | --- | --- |
| v3 | 01a1197c-257b-72d3-a98b-0509656b8bf7 | d9d115040cc34d7dddec7826b1aacc24099c2fcd03f9cc4f010702253fe091de |
| v4 | 01a11990-553d-7f80-bcf6-21f9b3e06bfe | 7ed0561699c5f184572612e03f19b72aa3e06cf049eb4ea6958ef45da369e4f2 |
| v5 | 01a119a4-4908-7501-9c79-a18348497504 | 8c356c47fd5fd7aaf65b7e87ee679d0586819775e9063d59ef0940785244b5d5 |
| Launch | 01a119ac-133b-7580-9381-e1febdeb8849 | 4d67af90b7320da207c7f1181ff7ab95d52d27d0fe5e4890be256d1afbf571ec |
| Pane/containment | 01a119c7-6694-7192-9b58-fc3da9298094 | 2a0c2bf1d4a900edc0838b79e78e89a30b542221ba085b1eec85b0decaba5b12 |

Independent conditions govern conflicting author prose:

* Callback verdict `01a119af-a347-7961-b01d-66603478a59b`, full
  `INDEPENDENT_SKETCH_V5_SIGNOFF.md`: sealed closed execution domain, actual
  callback input, immutable decision/append-only observations, inclusive finite
  pump budget and nonchild-safe recovery. Preserve all v4 refinements. Admit
  only the exact internal notes-copy no-op with notes rewriting disabled; pin
  `maintenance.auto=false` and literal `GIT_EDITOR=:` for prepared continuation.
  Internal sequencer `commit -n` never exempts parent gates.
* Launch verdict `01a119b5-ea32-7ed0-b5e9-11858700f78b`, full
  `TASK1607_LAUNCH_V2_INDEPENDENT_SIGNOFF.md` plus receipt: READY and provisional
  Git precede final P4. P4 alone samples current A/J/I/W and enlisted O.
  Immediate historical dispatch consumes once after all locks drop; no second
  current-authority decision. New activation requires new authorization.
* Pane verdict `01a119d7-0711-7613-9c2a-c0643b331549`, full
  `PANE_CONTAINMENT_INDEPENDENT_SIGNOFF.md` plus receipt: private foreground
  display-only tmux; audited optional descriptor-exec containment bundle;
  actual W -> setup/S -> B=V graph and namespace-qualified observations;
  original creating thread/adopter/FD identity; still-anchored process-group
  cleanup and PTY pause/resume. No changed default PID policy or mandatory
  arbitrary-descendant/cgroup containment for vendors.

The last report concludes that the composition is coherently designed AS
AMENDED. Root's explicit implementation dispatch permits this work, not
activation, completion or merge. TASK-1607 consumer signoff is
`01a119d7-1315-7171-bdfd-f2901ac9b751`, checkpoint `56f7b55`.

## Critical files and phased symbols

New modules are private until their complete production composition is ready.
The phases below are implementation order, not independently shippable parent
acceptance. No interim environment switch, optional policy gate, public
release constructor or test transport enters production.

1. **Durable publication records and I/O.** New
   `aida-core/src/publication/{mod,durable,records}.rs` owns `PublicationError`,
   immutable `CommitDecision`, typed `Coordinator`, `JournalGeneration`,
   `PayloadRef`, `ObservationBudget` and `RetainedReceipt`. Durable operations
   use write-all, file sync, rename and directory sync; errors distinguish
   before-install from visible-but-unconfirmed outcomes. No existing global
   atomic-write helper is silently given different semantics. Immutable
   decision/observation targets cannot be overwritten. Records alone confer
   no authority or release.
2. **One home discovery owner and finite recovery.** New
   `publication/{guard,recovery,effects}.rs`: `PublicationGuard::enter`,
   `PickupPublication`, `recover_active`, exact `LockedPreimage` and finite
   `PreparedEffects`. Preexisting infrastructure only on pickup. One home
   coordinator points to validated home-resident generations/payloads and
   pinned repository/common-dir/store identities. Recover A before selecting
   B's repository lock, including outside-project callers. Missing/corrupt
   evidence blocks; PREPARED reverses exact owned effects, visible valid C
   always rolls forward. Git closure must survive removal of private scratch.
   Current locked bytes include legitimate peer updates; never status-back
   edits, compensating events or stale whole-file snapshots.
3. **Assignment and planning.** New
   `publication/{assignment,preflight}.rs` owns `PreparedAssignment`,
   `AssignmentBinding`, `ActivationAttempt`, `PreparedPickup` and the exact
   reciprocal validator. Extend existing `SessionManifest.launch_assignments`,
   `SeatGrant.binding`, retained roster/predecessor evidence, registry,
   `RunMarker.phase_assignments` and `OrchestratedLeaseReceipt` at integration.
   Manifest/lease key differs from actor/grant session identity. Standalone
   and complete token/member/phase/attempt scopes are distinct typed cases.
   New siblings have distinct actors; verified resume preserves its actor.
   Invalid assignment locators cannot fall back to launcher authority.
4. **Purpose-separated Git adapter.** New
   `aida-core/src/git_publication/{mod,profile,prepare,callbacks,pump}.rs` owns
   `PreparedGitStep`, `SealedGitDomain`, `GateReceipt`, `GitPublicationAdapter`
   and `CallbackPump`. No conversion to vendor `CommittedDispatch`. Closed
   Git 2.43/files/SHA-1 profile independently snapshots config/includes,
   attributes, runtime/loader and fixed production wrappers/client. External
   filter/editor/formatter/submodule/make+cargo preparation has no credentials
   or writable live roots and is quiescent before callback credentials exist.
   Import all declared bytes/modes/index/message/object and warning effects.
   Extract actual gates from production hook templates and early minimal
   callback dispatch; no ordinary CLI re-entry, recursive locks, arbitrary
   helper in the domain, disabled hooks or test-only replacement gates.
5. **Staged writers and history.** Integrate below CLI/MCP dispatch in
   `aida-core/src/{storage,team,git_ops,oplog}.rs`, `db/{git_backend,
   cached_git_backend}.rs`; CLI global queue, seat authority, manifests,
   registry, drain/orchestrator, lease/calibration/rework/worktree/stack and
   review/signal surfaces. Add guarded/staged forms, propagating failures.
   Home precedes repository, sorted seats, store, sorted queues, drain,
   manifests/episodes, verdict, merge, Git. Permanent sidecars persist.
   Existing event feed receives UUID-idempotent durable projection before
   retirement; receipts are not another history ledger. Team rejected push
   retains local committed history and reconciles privately. Network stays
   outside guards. Record every unmigrated reader/writer as an activation gate.
6. **Publisher and reviewed transport integration.** CLI
   `launch_publication::LaunchPublisher` prepares one grant privately, binds
   the actual waiting child and publishes through core. `bind_waiting_child`
   and `release_bound_child` are sole binding/release owners. Core owns the
   noncloneable private `CommittedDispatch`; no deserialization into release
   authority. Consume TASK-1612's reviewed `SealedExecImage`/`WaitingChild`
   only after root supplies the exact independently approved commit. Do not
   edit its active checkout or duplicate channel/cancellation/image code.
   TASK-1607 later supplies O and seat after-images through the same private
   enlistment seam; BUG-1802 supplies contributor/START policy later.
7. **All entrypoints and adapters.** `queue_cmd`, `session`, `lib` phase and
   foreground launch paths, `pane_host`, and TUI `app/pty` use that owner.
   Phase drivers directly launch assigned vendor instead of recursive pickup.
   Root run/first admission/first setup share one refusal boundary; dynamic
   admission checks precede checked registration, status and release.
   `PreparedLaunchAdapter::prepare_waiting` returns transport and finite
   observations. `TmuxPtyAdapter` owns foreground `tmux -D -f /dev/null -S`,
   separate `-N` client and authenticated display relay. `BwrapFdAdapter`
   privately bundles audited non-setuid descriptor entry, exact W/S/B joins,
   mount/credential/FD/adopter checks and anchored group settlement.
   `CodexNodeNativeAdapter` preserves audited installed wrapper/package,
   manager flags, argv/stdio/signals/exit without Node as leaf. Native daemon
   and unsupported profiles honestly refuse. Missing required positives block.
8. **Acceptance and documentation.** Full real entrypoint matrix, writer
   audit, failure injection and guard-removal/restoration. Update AGENTS,
   OVERVIEW and CLI docs only for actual behavior. Exact-head independent
   review follows each final head. No PR, merge, install or activation absent
   root dispatch. Parent remains in progress with closure pending.

## Decision and retention implementation

P0 resolves roles/static seats, scope, branch occupancy, support and authority
without lifecycle effects. P1 privately prepares finite effects and actual
waiting process. P2 recovers foreign publication under home, acquires own
repository and lower locks, captures current preimages and durably publishes
PREPARED plus locator before installation. P3 binds reciprocal process records
and runs provisional Git with the parent pump and no seat lock. Drop lower
locks before final seat acquisition; sample A/J/O/I/W and time at P4. Persist
immutable C, retained proof and possible-execution intent. Complete existing
history projection and retirement, drop every shared lock, then consume once.

No storage fault produces a release capability. A visible exact C after a sync
error is uncertain commitment, never a rollback permission. Corrupt evidence
blocks. No recovery mints vendor release. Alive/Unknown or possibly executed
attempts retain evidence; new activation must not auto-respawn a predecessor.
ACK, EOF and successful spawn are observations, not proof of vendor execution.

## Acceptance mapping and verification

| Contract | Named real witnesses and required controls |
| --- | --- |
| A–E, one issuance, exact assignment | `bug_1808_headless_delegation_refusal_is_atomic`, `bug_1808_explicit_selector_does_not_change_launcher_authority`, `bug_1808_all_entrypoints_preflight_before_mutation`, `bug_1808_assigned_phase_is_not_recursive_delegation`; valid delegated/assigned controls across head/explicit/synthetic/no-launch/TUI/rework/phase/batch/nextN/from-pr/through-ci |
| Strict early/late refusal and peers | `bug_1808_preflight_races_fail_closed`, `task1607_launch_publication_final_check_race`; pre-P4 revoke/expiry/transfer/member loss versus post-C immediate continuation and new invalid activation |
| Global discovery, retention, durability | `bug_1808_cross_repo_recovery_preserves_peer_queue`, `bug1808_adapter_interrupted_publication_recovery`; every file-sync/rename/dir-sync boundary, missing/corrupt evidence, A unavailable, independent B/outside recovery, current peer history and permanent inode preservation |
| Git origin and pump | `bug_1808_publication_git_hooks_do_not_reenter_home`, `git_callback_descendant_cannot_forge_observation`; genuine production callbacks and ordinary code/team/store positives, forged helper executing genuine client/wrapper, actual effect/gate imports, fresh recovery credentials and WORK/SERVICE distinction |
| Actual images and original parent | `bug_1808_bound_bootstrap_first_action_barriers`, `task1607_launch_exec_image_binding`, `bug1808_adapter_parent_thread_death`, `bug1808_adapter_replay_cancel_lost_ack`; sealed-source replacement, actual descriptor inventory, creator death, adopted nonchild recovery, installed Codex positive |
| Terminal/containment/job control | `bug1808_pane_real_entrypoint_single_commit`, `bug1808_pane_foreign_peer_and_fd_refusal`, `bug1808_containment_real_entrypoint_host_join`, `bug1808_namespace_ambiguity_and_credential_fence`, `bug1808_containment_descendant_settlement`, `task1607_adapter_real_pty_result_and_signal_ownership`; required no-PID positive, private server, exact anchored group cleanup/pause/resume |
| Existing invariants | Retain all TASK-1337 branch/refusal/manual-continuation, TASK-1603 checked member registration and C7 regression suites; no consumer policy implementation here |

Snapshot canonical full objects/history/store HEAD, refs/reflogs/index/FETCH_HEAD,
worktree/admin directories, queue order/metadata, calibration, grants, manifests,
leases, registry, receipts, run/member/phase/attempt, review/signal, locator and
staging, owned process/FD/namespace/credential identities and peer state.
Pair each new refusal with actual positive control and deterministic fault or
barrier; later remove and restore the actual guard to demonstrate nonvacuity.
Unit component witnesses cannot stand in for the real entrypoint matrix.

All tests use fake HOME/AIDA_HOME/roots/vendors/forges/servers and owned processes.
Strip inherited coordinating AIDA_BIN and native agent identity overrides from
test children. Use worktree-private `CARGO_TARGET_DIR=target/bug1808` and normal
Cargo slots; never AIDA_CARGO_NO_SLOT or the shared target/debug. Record hashes
for built binaries actually probed. Run targeted modified tests locally,
`cargo fmt --all -- --check`, normal/no-default CLI checks and affected CI
scripts. `make check-ci-fast` is required before every push. Workspace-wide
integration checks remain mandatory at integration, not an implied unit pass.

## Checkpoints and dependencies

Root has been notified of pickup and requested to supply the transport API
inventory and exact reviewed dependency SHA. Until then phases 1–5 can progress
disjointly; transport integration and production activation cannot. Every
checkpoint states delivered behavior, actual commands/results and open criteria.
Final handoff is `.aida/handoff/bug1808/IMPLEMENTATION_READY.md` only when ready,
otherwise an explicit blocked checkpoint with exact HEAD/push/clean evidence.
No undocumented schema fork is resolved by implementation guesswork.

### Recovered implementation checkpoint, 2026-10-07

The interrupted implementer's staged files were preserved. HEAD, origin/main,
branch and lease were reverified; the old component log recorded a Cargo-slot
wait, not a completed check. No owned prior check process survived the recovery
inspection. Replacement checks use isolated HOME and the same private target.

Private implemented modules now include `durable`, `records`, `preflight`,
`effects`, `observations` and `git_publication::callback_input`.
`VerifiedFileSet` validates complete before/after payload closure and all
current targets before writing. It rejects symlink traversal, hardlinks and
aliased entries, retains parent descriptors, applies exact bytes/modes in
forward/reverse order and resyncs already-matching recovered images. It does
not acquire locks, decide rollback/commit, create directories or implement
Git/worktree/object effects. Its caller must already hold the owner's locks
and retain the complete durable journal. No production caller exists.

`ObservationChain` stores immutable numbered records under a predeclared
origin/namespace/kind/count/encoded-byte budget. Identical replay is idempotent;
conflicting replay, gaps, corrupt bytes and foreign origin fail. A retained
`ObservationCheckpoint` detects a missing final suffix. Installation verification
cannot be relabeled as Git callbacks by a plan allowing only verification.
This codec does not authenticate producers or make COMMIT/release decisions.

An actual isolated Git 2.43 commit/rebase probe found `ORIG_HEAD`, `REBASE_HEAD`
and `CHERRY_PICK_HEAD` reference-transaction OID inputs, in addition to HEAD
and named refs. The parser now admits those bounded pseudoref names while
still requiring exact complete prepared tuples. A separate builtin merge/conflict
probe also ran. Raw callback streams and exact Git executable hash are retained
in `.aida/handoff/bug1808/checks/`. These used recording fixture hooks only;
they prove protocol behavior, not production-hook origin or sealed execution.

Full publisher/assignment/retained-receipt formats, global discovery and writer
enlistment, Git sealed domain/pump and real hook preparation, reviewed transport
integration, launch adapters and all production entrypoint witnesses remain
unimplemented. The private components cannot satisfy parent acceptance or
authorize an integration consumer. Final commands/results belong in the handoff
checkpoint; intermediate green runs are not final-head verification.
