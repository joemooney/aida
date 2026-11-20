# S5 process-identity migration inventory

<!-- trace:TASK-3367 | ai:codex -->

This is an audit of the remaining TASK-1750 compatibility allowances, not
zero-legacy acceptance. The reviewed S5 scope is in the
[identity sketch](../../plans/2026-11-05-story-2537-proc-identity.md).

The baseline below retains **56 files / 113 syntax allowances** after moving
cache-migration temp cleanup and review-watch legacy fetch-ref cleanup to
`proc_identity::observe_legacy_registry`. Those PID-only artifact readers retain
ForeignUnknown and clean up only on Dead. Neither reader upgrades a persisted
PID to a newly captured identity. Cache age floors and timestamped fetch-ref
expiry remain separate artifact policies.

## Inventory limits

The existing guard is a syntax ratchet, not a complete count of authority
checks. Nine matches are repository/build `.probe` calls: three in
`dev_autopromote`, two in `dev_build`, three in `review_watch`, and one in
`review_watch_ready`. They do not represent legacy process decisions. Conversely,
function-pointer uses such as `decide_lock(...,
process_probe::process_identity_is_alive)` in drain and solo acquisition do not
match the call-only expression. Imports, aliases, and other process probes also
need review before claiming zero. Do not delete allowances merely to make the
fixture empty or rename helpers to evade the expression.

The independent `tests/test_ownership_liveness_ratchet.py` inventories legacy
lease readers by enclosing item, including function references. It scans only
CLI-library sources and has different exclusions; it is complementary evidence.
The fetch-ref artifact exception was removed after migrating that reader.

## Reader/writer compatibility decisions still needed

- Pool entries and retirement tokens (`aida-core/src/worktree_pool.rs`) persist
  PID plus optional raw ticks, with no namespace or boot binding. `Pool` has no
  version discriminator; old serde readers accept added fields. Merely adding
  ProcIdentity leaves older heal/acquire paths free to reclaim using partial
  evidence. Choose a reader-first rollout or a version/path boundary that old
  readers refuse before enabling new writers. Retain reservation generation,
  durable lease and mail-retirement checks.
- Drain and solo JSON locks also lack namespace-qualified identity. Drain's
  `read_lock` treats parse failure as no lock, so an incompatible JSON version
  alone would let an old launcher acquire rather than safely refuse. A rollout
  must fence old launchers as well as teach current readers about unknown
  identity; retain force policy, grant revocation and serialized writes.
- Strict signal, grant, seat and publication readers need positive evidence;
  lock retention readers can retain unknown. Replacing every boolean call with
  one shared boolean would conflate these policies. Inventory both writers and
  rollback readers per family, then exercise namespace, reuse and subreaper
  cases before removing its allowance.

These are not process-death findings. Missing namespace/boot evidence stays
unknown, never recaptured from today's PID. The already landed review-watch
unknown-verdict backstop and relocated TASK-2997 notes remain valid; their
presence does not complete S5.

## Remaining baseline

Paths are relative to the repository root; counts are maximum syntax matches,
not unique decisions. The committed fixture remains the executable ratchet.

| Production file | Allowances |
| --- | ---: |
| `aida-cli-lib/src/advisor.rs` | 2 |
| `aida-cli-lib/src/agent_codex_background.rs` | 2 |
| `aida-cli-lib/src/agent_registry_root.rs` | 4 |
| `aida-cli-lib/src/claim.rs` | 1 |
| `aida-cli-lib/src/codex_bg_receipt.rs` | 3 |
| `aida-cli-lib/src/coordination.rs` | 2 |
| `aida-cli-lib/src/deputy_shell/attribution.rs` | 2 |
| `aida-cli-lib/src/deputy_shell/runner.rs` | 1 |
| `aida-cli-lib/src/deputy_shell/socket.rs` | 3 |
| `aida-cli-lib/src/dev_autopromote.rs` | 4 |
| `aida-cli-lib/src/dev_build.rs` | 2 |
| `aida-cli-lib/src/dispatch.rs` | 7 |
| `aida-cli-lib/src/doctor_root.rs` | 1 |
| `aida-cli-lib/src/drain_lock.rs` | 3 |
| `aida-cli-lib/src/drain_signal.rs` | 2 |
| `aida-cli-lib/src/drain_state.rs` | 4 |
| `aida-cli-lib/src/driver_cmd.rs` | 1 |
| `aida-cli-lib/src/gate/dispatch_auth.rs` | 1 |
| `aida-cli-lib/src/gate/fleet_lanes.rs` | 1 |
| `aida-cli-lib/src/gate/ledger/migrate.rs` | 1 |
| `aida-cli-lib/src/gate/review_authority.rs` | 3 |
| `aida-cli-lib/src/gate/review_marker.rs` | 2 |
| `aida-cli-lib/src/gate/seat_authority.rs` | 1 |
| `aida-cli-lib/src/headless_linger.rs` | 2 |
| `aida-cli-lib/src/idle_eligible.rs` | 1 |
| `aida-cli-lib/src/lane_job.rs` | 2 |
| `aida-cli-lib/src/lane_liveness.rs` | 2 |
| `aida-cli-lib/src/last_drain.rs` | 2 |
| `aida-cli-lib/src/leases.rs` | 11 |
| `aida-cli-lib/src/lib_part5.rs` | 1 |
| `aida-cli-lib/src/orchestrator.rs` | 1 |
| `aida-cli-lib/src/orchestrator_leases.rs` | 2 |
| `aida-cli-lib/src/ownership/dual.rs` | 1 |
| `aida-cli-lib/src/ownership/judge.rs` | 1 |
| `aida-cli-lib/src/pipeline_stall.rs` | 1 |
| `aida-cli-lib/src/pipeline_stall_signals.rs` | 1 |
| `aida-cli-lib/src/pr_train.rs` | 1 |
| `aida-cli-lib/src/ps_integrate.rs` | 3 |
| `aida-cli-lib/src/regen/score.rs` | 3 |
| `aida-cli-lib/src/relay_mail.rs` | 2 |
| `aida-cli-lib/src/review_watch.rs` | 3 |
| `aida-cli-lib/src/review_watch_ready.rs` | 1 |
| `aida-cli-lib/src/seat_occupancy/runtime.rs` | 1 |
| `aida-cli-lib/src/session_misc.rs` | 1 |
| `aida-cli-lib/src/shift.rs` | 3 |
| `aida-cli-lib/src/ship_from_seat.rs` | 1 |
| `aida-cli-lib/src/solo_cmd.rs` | 1 |
| `aida-cli-lib/src/solo_lock.rs` | 1 |
| `aida-cli-lib/src/standalone_implementer.rs` | 1 |
| `aida-cli-lib/src/test_home.rs` | 1 |
| `aida-cli-lib/src/triage_cmd.rs` | 1 |
| `aida-cli-lib/src/worktree_gc.rs` | 2 |
| `aida-core/src/driver_registry.rs` | 1 |
| `aida-core/src/mailbox_lifecycle/retire.rs` | 1 |
| `aida-core/src/worktree_pool.rs` | 3 |
| `aida-tui/src/redesign/drain_panel.rs` | 1 |
