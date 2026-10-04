# Plan: STORY-1218 slice 1 — `aida shift tick` (the night-shift tick core)

Date: 2026-09-24
Specs: STORY-1218
Status: In Progress
Complexity: ~1500 prod LOC, ~1100 test LOC, 1 commit, risk high (keystone: a scheduler-run process that launches drains)

<!-- trace:STORY-1218 | ai:claude -->

## Approach

`aida shift tick` is a deterministic, LLM-free step that runs as a registered
substrate job inside the existing `aida schedule tick` (the cron driver that
is already installed; no new driver in this slice). Each tick takes a
per-clone single-flight lock, reaps finished sessions, settles the outcome of
the previous shift wave, evaluates a fixed list of fail-closed guards, and —
only when every guard passes — tags the next explicit-drain-mode queue slice
`batch:shift-YYYYMMDD-HHMM` and spawns one bounded, detached
`aida queue work --batch … --auto-complete --no-human=both …` wave through
the same drain lock a manual drain takes. It emits one `ShiftTick` event only
when it acted or its guard verdicts changed. It is OFF unless this clone's
uncommitted local layer (`~/.aida/shift-local.toml`, keyed by canonical repo
path) enables it; a committed `[shift] enabled = true` is ignored. This slice
has no re-drive, no systemd driver, no mailbox escalation and no cold-boot:
those are slices 2 and 3, filed as child tasks.

```
 aida schedule tick (cron, */15)          -- existing driver, 120s child kill
   └─ job "night-shift": aida shift tick  -- hook_allowed = false
        0. local layer enabled?  no → print "off", exit 0
        1. flock .aida/shift.lock (contended → exit 0)
        2. load .aida/shift-state.json (unparseable → exit 1, launches blocked)
        3. reap finished sessions         (skipped past the 90s deadline)
        4. settle last wave: pid dead → QueueDrained after launch? progress : zero
        5. guards (all must pass) ──fail──► record verdicts, exit 0
        6. record intent → tag batch:shift-* → setsid spawn → record pid
        7. ShiftTick event iff acted or verdict set changed
```

## Decisions

The advisor signoff on STORY-1218 (APPROVED, amendments A1–A13) is binding.
Where the sketch and the advisor disagree, the advisor wins. Slice-1 scope per
the advisor's Q6 ruling: tick core, local enable, guards (incl. A3, A9, A10,
A11), selection, detached launch (A2, A4), circuit breakers (A8), ShiftTick,
dry-run, `shift status`, the status line, reap — on the EXISTING cron driver.

- **A1 — local enable layer.** `enabled` (and `allow_uncovered_vendors`) are
  read ONLY from `<AIDA_HOME|home>/.aida/shift-local.toml` under
  `[repo."<canonical repo path>"]`. `aida shift enable|disable` write that
  file. The committed `.aida/config.toml [shift]` table may carry tunables
  (wave size, caps) but its `enabled` key is ignored and dry-run says so.
  **Rationale**: `.aida/config.toml` is tracked; a committed switch would arm
  unattended launches in every clone with a driver. The home layer is never
  inside a repository, so it cannot be committed by accident.
- **Operator gate (review round 1).** `aida shift enable` and
  `aida shift resume` arm or re-arm unattended launches, so both refuse
  unless stdin is an interactive terminal AND agent output mode is off, and
  both then require an explicit y/N (default no). This is the same
  human-at-a-terminal floor as `aida merge-hold clear` / the `pr ship` hold
  release. `aida shift disable` is never gated: turning launches off is
  always safe. TTY, agent-mode and confirm are injected (`Operator`) so tests
  never read a terminal or the real `~/.aida`.
- **A2 — hygienic spawn.** `setsid` (via `pre_exec`), stdin `/dev/null`,
  stdout+stderr to `.aida/shift-wave-<stamp>.log`; `AIDA_DRAIN_FORCE`,
  `AIDA_DRAIN_BORROW`, `AIDA_DRAIN_LOCK_STALE_SECS`, `AIDA_EVENTS_DISABLE`,
  `AIDA_SCHEDULE_CHILD` removed; `AIDA_SCHEDULE_INVOKER` kept. `systemd` is
  added to the documented invoker values in `usage.rs` for slice 2.
- **A3 — no-human acknowledgement.** Guard `no-human-ack` fails unless the
  persisted marker exists (`~/.aida/no-human-acknowledged` or
  `.aida/no-human-acknowledged`). The tick never sets
  `AIDA_NO_HUMAN_ACKNOWLEDGED`. `shift enable` checks and prints the command.
- **A4 — exit codes, deadline, kill-safe order.** Refusal, no-op, live lock,
  disabled: exit 0. Only internal errors exit non-zero (state unreadable,
  spawn failed, store write failed). Internal deadline 90s: past it, reap is
  skipped, and a launch is not started unless 30s remain. The clock is
  re-read after the reap and again between tagging and spawning; a spawn
  that would start late is skipped and its intent left for reuse. A tick
  killed after the spawn but before the pid write is held by the wave's
  drain lock on the next tick. Order: record
  intent (`last_launch` with no pid) → tag → spawn → record pid. A record
  with no pid is an un-launched batch that the next tick REUSES (re-filtered
  for eligibility) instead of tagging new specs.
- **A8 — circuit breakers in `.aida/shift-state.json`** (not events, which
  rotate): (a) a dead wave pid with no `QueueDrained` after its launch is
  zero progress; (b) 2 consecutive zero-progress waves stop launching, the
  ShiftTick that trips it carries `breaker` (actionable, once), and launching
  resumes only on `aida shift resume` or a change to the queue set (fingerprint
  recorded at the trip); (c) a spec in 2 shift waves within 24h without
  reaching a terminal state is excluded and escalated once (the `escalated`
  field of the ShiftTick event); (d) `max_waves_per_day` (default 8) counted
  from state. Missing state = fresh; unparseable = launches blocked (exit 1).
- **A9 — budget evidence covers the wave.** `budget-evidence` fails when the
  watchdog aggregates are missing or older than 1h, when its last verdict was
  `degraded`/`unknown`, or when the wave's resolved headless vendor is not
  covered by the aggregates (only `claude` is), unless the local layer lists
  it in `allow_uncovered_vendors` (dry-run prints the override). The watchdog
  state gains `last_run_at` / `last_verdict`. The argv always carries
  `--max-runtime` and `--max-iterations <wave_size>` beside `--max-tokens`;
  `--max-tokens` = min(wave budget, stop threshold − spent 24h) where stop
  threshold = `budget_stop_pct` (default 80) of the daily budget (the
  watchdog's, 6B built-in).
  The wave's vendor is resolved through the launch path's
  `resolve_enabled_headless_vendor` (honouring `[agents] enabled`), and a
  resolution error refuses.
- **A10 — cross-clone lock parity.** `lock-free` consults the shared drain
  claim on the local `.aida-store` checkout (the drain lock claim under
  its coordination directory, read-only, no pull) through `coordination::decide_claim`; a live foreign
  claim refuses.
- **A11 — selection floors.** Only an explicit `execution_mode = drain`
  counts; `presence::is_keystone_class` specs and specs with a live merge
  hold (hold record naming the spec) are excluded. Configured
  `[shift] batch` fails closed naming the offender. Dry-run prints the
  resolved queue user and role.
- **Q4 — `max_failures` default 2**, clamped to 1..=wave_size, configurable
  via `[shift] max_failures`. This is a CHOICE, not a SPIKE-82 finding: no
  recorded wave in that window ended by exhausting its failure cap. After five
  shift nights, re-derive it from ShiftTick/QueueDrained outcomes (follow-up).
- **Q5 — auto-tag `batch:shift-YYYYMMDD-HHMM`.** One shift tag per spec
  (re-tagging replaces a prior `batch:shift-*`); an un-launched shift batch is
  reused; tagging happens under `shift.lock` while the drain lock is free;
  dry-run writes no tags.
- **Wave size default 6.** SPIKE-82 H2 held: bounded serial waves of 4–8
  specs (named tags closed 8/8/4/2/3 members). 6 sits inside that band.
- **Wave runtime default 3h, wave token budget default = stop threshold ÷
  max_waves_per_day.** Choices, bounded by the per-day cap: SPIKE-82's window
  ran seven waves in fourteen hours (about two hours each).
- **Queue view = the drain's view.** The wave passes `--role implementer`;
  selection reads the same role-routed view (`queue_list_with_role_fallback`
  + `entry_matches_role_filter`) so the tick never tags a spec the wave cannot
  see.
- **Job registration.** `shift enable` appends the `night-shift` job
  (`command = "shift tick"`, `every = "10m"`) to the project's
  `[[schedule.jobs]]` when absent (committed entry is allowed by A1 and is
  inert without the local switch). Effective cadence under cron is
  max(`*/15`, `every`).
- **Deviation from the sketch**: slice 1 does not refactor `supervisor.rs`
  (re-drive is slice 3, off by default per A5) and does not touch doctor's
  driver status (slice 2).

## Files (in build-order)

### `aida-cli-lib/src/shift.rs` — the tick (created by this slice)

- `struct ShiftConfig` + `fn load_config`: committed tunables + local layer.
- `struct ShiftState` (+ `WaveRecord`, `WaveOutcome`) load/save; breakers.
- `struct Candidate`, `fn select_wave`, `fn eligibility`: A11 floors, A8(c).
- `struct GuardInputs`, `fn evaluate_guards`: the guard list below.
- `fn build_wave_argv`: A9 caps, never `--force-claim`/`--steal`/`--force`.
- `fn settle_last_wave`: A8(a)/(b)/(c) bookkeeping.
- `trait ShiftExec` + `RealExec`: reap, tag, spawn (A2), emit.
- `fn tick_core`: the pure-over-inputs tick; `fn run_tick`: gathers inputs.
- `fn handle_shift_command`, `fn print_status_line`.

Guards (all must pass to launch): `enabled`, `wave-in-flight`, `lock-free`,
`cross-clone-lock`, `no-human-ack`, `drain-mode-only`, `token-budget`,
`budget-evidence`, `watchdog-quiet`, `load-ceiling`, `mem-ceiling`,
`disk-headroom`, `wave-cap`, `no-progress`, `state-readable`, `deadline`.

### `aida-cli-lib/src/events.rs` — `EventKind::ShiftTick`

- New variant + `ShiftLaunch`; registered in `is_actionable` (actionable only
  when it carries a breaker trip or an escalation), `name()`, `known_names()`.

### `aida-cli-lib/src/runaway_seats.rs` — budget evidence

- `struct State`: add `last_run_at`, `last_verdict` (serde default).
- `fn budget_evidence(state_path, now)`: trailing-24h tokens + last run.

### `aida-cli-lib/src/coordination.rs` — `fn live_foreign_lock_claim`

### `aida-cli-lib/src/session_reap.rs` — `fn reap_quiet`

### `aida-cli-lib/src/maintenance_schedule.rs`

- `fn command_table`: `shift tick` entry, `hook_allowed: false`.
- `fn jobs_running_command`: registry lookup used by guards and enable.

### `aida-cli-lib/src/config_edit.rs` — `fn append_array_table`

### `aida-cli-lib/src/cli.rs`, `aida-cli-lib/src/git_backend_cmd.rs`, `aida-cli-lib/src/lib.rs`

- `Command::Shift(ShiftCommand { Tick, Status, Enable, Disable, Resume })`,
  distributed dispatch, legacy-mode refusal.

### `aida-cli-lib/src/status_cmd.rs` — one `night shift:` line when enabled

### `aida-cli-lib/src/usage.rs` — document `systemd` invoker value (A2)

### Docs

- `docs/autonomous-drain.md`: "Night shift" runbook section (preflight =
  `aida shift tick --dry-run`).
- `docs/cli/03-work-autonomy.md`: `aida shift` entry.
- `docs/environment-variables.md`: `AIDA_HOME` relocates `shift-local.toml`.
- `docs/cli-format-json-audit.md`: regenerated.

## Critical Files

- `aida-cli-lib/src/shift.rs`
- `aida-cli-lib/src/events.rs`
- `aida-cli-lib/src/runaway_seats.rs`
- `aida-cli-lib/src/coordination.rs`
- `aida-cli-lib/src/session_reap.rs`
- `aida-cli-lib/src/maintenance_schedule.rs`
- `aida-cli-lib/src/config_edit.rs`
- `aida-cli-lib/src/cli.rs`
- `aida-cli-lib/src/git_backend_cmd.rs`
- `aida-cli-lib/src/lib.rs`
- `aida-cli-lib/src/status_cmd.rs`
- `aida-cli-lib/src/usage.rs`
- `docs/autonomous-drain.md`
- `docs/cli/03-work-autonomy.md`
- `docs/environment-variables.md`

## Reusable helpers (do not reimplement)

- `drain_lock::probe_lock` — local drain lock classification.
- `coordination::decide_claim`, `coordination::lock_claim_path` — cross-clone claim.
- `process_probe::process_identity_is_alive` / `process_start_identity` — pid identity.
- `presence::is_keystone_class` — the one keystone classifier.
- `merge_hold::list_holds`, `merge_hold::read_hold_record` — live holds.
- `queue_role_fallback::queue_list_with_role_fallback`, `entry_matches_role_filter` — the drain's queue view.
- `runaway_seats::policy` — daily budget; `machine_readiness::probe` — disk.
- `session::resolve_headless_vendor` — the wave's vendor.
- `session_reap::scan_reapable` — reap predicate (unchanged).
- `config_edit` — section-preserving TOML writes.

## Risks + gotchas

1. **Risk**: a test launches a real drain. **Mitigation**: every launch goes
   through `ShiftExec`; tests use a recording mock; the one spawn test runs
   `/bin/sh` through the argv builder, never `aida`.
2. **Risk**: the 120s substrate kill takes the wave down. **Mitigation**:
   `setsid` puts the wave in its own session and process group, out of reach
   of the job's `killpg`.
3. **Risk**: a refused tick pages a seat every 15 minutes (BUG-1589).
   **Mitigation**: refusals exit 0; the event fires only on a verdict change.
4. **Risk**: re-queueing the same failing spec every tick. **Mitigation**:
   A8(b)/(c) breakers, `max_waves_per_day`, per-wave caps.
5. **Risk**: the committed config arms another clone. **Mitigation**: A1.
6. **Risk**: the tick's spend is invisible (Codex). **Mitigation**: A9.

## Tests (named, not "add tests")

- `shift_tick_launches_wave_when_lock_free_and_event_names_it`
- `shift_tick_second_tick_during_wave_is_noop`
- `shift_tick_launch_in_progress_before_lock_written_is_noop`
- `shift_tick_never_selects_drive_guided_operator_decide`
- `shift_selection_requires_explicit_drain_and_excludes_keystone_and_held` (A11)
- `shift_configured_batch_with_non_drain_member_refuses`
- `shift_tick_live_lock_noop_stale_lock_logs_recovered_pid_and_leaves_file`
- `shift_cross_clone_drain_claim_blocks_launch` (A10)
- `shift_wave_argv_never_forces` (A9 caps, max_failures clamp)
- `shift_wave_env_scrubbed_and_stdio_detached` (A2)
- `shift_refuses_without_no_human_ack_and_never_sets_env` (A3)
- `shift_refusals_exit_zero_and_unreadable_state_blocks_launch` (A4, A8)
- `shift_killed_between_tag_and_spawn_reuses_batch` (A4)
- `shift_deadline_skips_reap_and_launch` (A4)
- `shift_refuses_over_token_budget`
- `shift_refuses_when_budget_evidence_missing_or_stale` (A9)
- `shift_refuses_after_runaway_watchdog_trip`
- `shift_refuses_over_load_mem_disk_ceiling`
- `shift_dead_wave_without_queue_drained_is_zero_progress` (A8a)
- `shift_no_progress_wave_not_relaunched` (A8b, resume + queue change)
- `shift_spec_in_two_waves_excluded_and_escalated_once` (A8c)
- `shift_max_waves_per_day_counted_from_state` (A8d)
- `shift_disabled_by_default_launches_nothing`
- `shift_committed_config_enable_alone_launches_nothing` (A1)
- `shift_dry_run_on_fixture_queue_prints_argv_specs_guards_mail_and_writes_nothing`
- `shift_auto_tag_replaces_prior_shift_tag` (Q5, real fixture store)
- `shift_quiet_tick_emits_no_event`
- `shift_enable_requires_a_human_at_a_tty_and_a_yes` (operator gate; disable ungated)
- `shift_resume_requires_a_human_at_a_tty_and_a_yes` (operator gate)
- `shift_budget_guard_uses_the_launch_path_vendor` (A9, `[agents] enabled`)
- `shift_deadline_rechecked_after_reap_before_spawn` (A4)
- `shift_deadline_rechecked_between_tag_and_spawn` (A4)
- `shift_killed_between_spawn_and_pid_record_is_held_by_the_drain_lock` (A4)
- `schedule_command_table_shift_tick_not_hook_allowed`
- `status_line_silent_when_shift_disabled`
- `budget_evidence_reads_trailing_day_and_last_verdict` (runaway_seats)

## Verification

```bash
AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"
$AIDA_BIN shift tick --dry-run      # prints: enabled no (local layer), guards, argv; writes nothing
$AIDA_BIN shift tick                # disabled: "night shift: off", exit 0, no files
$AIDA_BIN shift status --json | python3 -m json.tool
cargo test -p aida-cli-lib --lib shift
```

## Followups

- Slice 2: systemd user timer driver, driver switch rules, doctor DriverStatus (A12, A13).
- Slice 3: re-drive step, mail-latency escalation, cold-boot (A5, A6, A7).
- Re-derive the max_failures default after five shift nights.
- Per-wave `systemd-run --user` transient unit (non-binding A12 note).

## Related

- Builds on: SPIKE-82, STORY-1226, BUG-538, STORY-638, STORY-1462, TASK-1298
- See also: docs/spikes/2026-09-18-spike-82-night-shift-post-mortem.md

## Slice 3 (TASK-1492): re-drive, mail latency (cold-boot deferred)

<!-- trace:TASK-1492 | ai:claude -->

- **A5**: `redrive` is read only from the local layer (`~/.aida/shift-local.toml`,
  beside `enabled`); a committed `redrive` is ignored. Default off; dry-run
  prints `re-drive: off (ADR-26 default)`.
- **A6**: `events::read_redrive_history_strict` folds `events.jsonl.1` and
  `events.jsonl`; it errors when `AIDA_EVENTS_DISABLE` is truthy, a present
  file is unreadable, or neither file exists. That error fails the
  `redrive-evidence` guard (no re-drive, no reclassification that tick).
  `supervisor::plan_redrives` is pure; `apply_cap` carries out the ADR-26 cap
  branch (needs-human tag, `ReclassifiedNeedsHuman`, cap finding). The manual
  `aida supervise` now also counts the archive.
- **A7**: `supervisor::apply_requeue` re-queues through the one requeue owner
  and, given a `QueueTarget`, upserts each applied spec at the head of the
  wave user's queue (`for_role = implementer`) in oldest-parked order. The
  tick prepends the re-queued specs to this tick's candidates. No force flag.
- Re-drive guards (separate from launch guards): `redrive-evidence`,
  `redrive-lock-free` (local lock, foreign claim, live shift wave),
  `redrive-breaker` (a re-queue would change the queue fingerprint and
  silently resume a tripped breaker), `redrive-state`, `redrive-deadline`.
  Floors: explicit drain mode, not keystone-class, no live merge hold.
- **Mail latency**: `mailbox_store::oldest_unread_by_recipient` (shared pure
  core with `collect_snapshot`); `shift::mail_escalations` is once per episode
  per recipient (`ShiftState.mail_episodes`); one combined `notify` call per
  tick so notify's per-rule `min_interval` cannot swallow a second recipient.
  A breaker trip also notifies (rule `shift-breaker`).
- **Cold-boot**: not built. Open questions returned to the advisor (see the
  TASK-1492 comments).
