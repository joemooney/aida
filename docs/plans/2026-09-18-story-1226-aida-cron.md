# STORY-1226 — `aida cron` (per-seat job registry + ledger) as the grown `aida schedule`

- **Date:** 2026-09-18
- **Specs:** STORY-1226 (guided), ADR-46 (decisions), references STORY-1218 (tick / installer / unattended runs — NOT this slice), TASK-1279 (seat parity), SPIKE-82 (consumer of the ledger)
- **Status:** approved plan, implementation in worktree `story-1226`
- **Complexity:** high (new schedule kinds + predicate grammar + store ledger + two delivery surfaces), bounded by reusing `maintenance_schedule.rs`

## 1. Approach

Grow the existing STORY-1047 scheduler (`aida-cli-lib/src/maintenance_schedule.rs`, command `aida schedule tick|run|status|emit-cron`, config section `[schedule]`, ledger `.aida/schedule-state.json`) into a **per-seat job registry** and register `aida cron` as an alias of `aida schedule`. Do not add a third scheduler.

```
 [schedule] in .aida/config.toml (project)  +  ~/.aida/schedule.toml (global)
            └── merged by job name (project wins) ──► Registry { jobs }
 job = { name, seats=[advisor,…], kind=substrate|seat|fires_task,
         every="30m" | on=[EventKind…] | when="<predicate>", command|prompt }
            │
   ┌────────┴──────────────────────────────────────────────────────────┐
   │ substrate job → run command (existing allow-list runner)          │
   │ seat job      → DUE-LINE delivered to whoever holds the seat      │
   │ fires_task    → STORY-262 cadence-task (compat), deprecated       │
   └────────┬──────────────────────────────────────────────────────────┘
            ▼
   Ledger: one store object per job  (aida-store: schedule/<job>.yaml)
           { last_run, last_by{seat,session,vendor}, result, episode{first_true,fired_at,cleared_at} }
            ▼
   Surfaces: aida schedule status/due/run/done · awaiting --notice (CronChannel) ·
             pickup prompt DUE JOBS block · agent-new "## Due Jobs" · events CronJobFired
```

Delivery is vendor-neutral by construction: the per-turn notice rides the existing `aida-mail-notice.sh` relay that is already wired for Claude (`settings.json`) and Codex (`.codex/hooks.json`), so **no hook/template changes**. Claude additionally gets a hint to mirror seat jobs as in-session cron entries. The cold-boot of a headless seat for an overdue seat job is the tick's business (STORY-1218) and is out of scope here except for the ledger fields it needs (`cold_boot_at`, rate-limit window).

## 2. Decisions (from ADR-46)

1. Dual delivery on Codex/Antigravity: due-lines always; cold-boot ≤1/hour/seat by the tick (STORY-1218).
2. Ledger = store object per job (survives rotation, syncs across hubs); debounce writes for substrate jobs (≥60 s between ledger commits per job unless the result changed).
3. Registry = project `[schedule]` section + global `~/.aida/schedule.toml`; merge by name, project wins.
4. `when` = small typed expression grammar over a fixed field set; once-until-cleared firing.
5. Grow `aida schedule`; `aida cron` alias; `schedules.toml` (STORY-262) folded in as kind `fires_task`, deprecated.

## 3. Files (build order)

1. `aida-cli-lib/src/maintenance_schedule.rs` — extend `TaskConfig` → `JobConfig` (serde, back-compat with existing fields): `seats: Vec<String>` (default: `["*"]` = substrate/any), `kind: JobKind {Substrate, Seat, FiresTask}` (default Substrate), `every: Option<String>`, `on: Vec<String>`, `when: Option<String>`, `command: Option<String>`, `prompt: Option<String>`, `enabled`. Keep `load_config`; add `load_global_config(home)` and `merge_registries(project, global)`.
2. `aida-cli-lib/src/schedule_predicate.rs` (new) — grammar: `expr := term (('&&'|'||') term)*`, `term := field op value`, `op ∈ {>,>=,<,<=,==,!=}`, values: integer, duration (`15m`,`2h`,`90s`), bool. Field set (documented in the doc comment and in `docs/cli`): `mail.unread`, `mail.oldest_unread_age`, `drain.lock_free`, `queue.drain_mode_ready`, `queue.depth`, `sessions.finished_unreaped`, `ci.red_prs`, `findings.open`, `escalations.open`. `Snapshot` struct filled from `collect_awaiting_report` + `drain_lock::read_pid_live_lock` + `session_reap::scan_reapable` (cheap, file-only). Pure `eval(expr, &Snapshot) -> Result<bool>`.
3. `aida-cli-lib/src/schedule_ledger.rs` (new) — store object `schedule/<job>.yaml` under the aida-store worktree root, copied from `aida-core/src/alias.rs` (`load`/`save`/`write_cas` with pull-rebase → save → add → commit → push, retry on rejected push; solo writes locally). Fields per decision 2 + `episode`. Register a last-writer-wins resolver for `schedule/*.yaml` in `aida-core/src/git_ops.rs` next to `resolve_nodes_conflict`. Debounce: skip the commit when only `last_run` moved by <60 s and `result` unchanged.
4. `aida-cli-lib/src/events.rs` — `EventKind::CronJobFired{job,seat}`, `CronJobFailed{job,seat,error}`, `MailReceived{to}` (emitted by `mailbox_cmd` send path); classify in `is_actionable()`; add names to `headless_tail::known_kinds()`.
5. `maintenance_schedule.rs` handlers — `tick` evaluates `every` (existing), `on` (scan `events.jsonl` since the job's `last_seen_event_ts`), `when` (predicate; once-until-cleared via `episode`); substrate jobs run through the existing allow-list runner; seat jobs only update `due` state (never execute). New subcommands: `list [--seat]`, `due [--seat <role>] [--json]`, `done <job> [--note]` (seat reports back; ledgers `last_by` from `AIDA_SESSION_ROLE`/session id/vendor), `run <job>` (substrate: execute; seat: print the prompt + mark due-acknowledged). Keep `status`/`emit-cron`.
6. `aida-cli-lib/src/cli.rs` — `Command::Schedule { action }` gains the new actions; add `#[clap(alias = "cron")]` on the Schedule command (and `visible_alias`), plus `command_groups()` row text mentions the alias. `Command::Cron` must NOT be a separate variant.
7. `aida-cli-lib/src/awaiting_you.rs` — `CronChannel { due: usize, next: Option<String> }` mirroring `DirectivesChannel`; wired through `total()`, `render()`, `to_json()`, `compact_line()`; populated in `collect_awaiting_report()` (lib.rs) from registry+ledger with file-only reads; `statusbar_cmd::you_channels` gets a `cron` row.
8. `aida-cli-lib/src/queue_cmd.rs` `derive_queue_work_prompt_with_round` — prepend a `DUE JOBS (seat: <role>)` block (construction copied from `rework_pickup_prompt`) listing due seat jobs for the pickup's role with the `aida schedule done <job>` report-back line; `lib.rs::render_agent_launch_context` — `## Due Jobs` section between `## Mailbox` and `## Queue Snapshot`; Claude launches (`aida agent new claude`) append one line: "you may mirror these as in-session cron entries; report each run with `aida schedule done <job>`".
9. `aida-cli-lib/src/init_cmd.rs` — extend `init_schedule_config_section()` with commented default jobs: substrate `session-reap` (every 30m, `aida session reap -y`), `doctor` (every 6h, `aida doctor --quiet`), `queue-gc` (every 2h), `mailbox-latency` (when `mail.oldest_unread_age > 15m`), seat `mailbox-triage` (advisor, every 30m + on MailReceived), `groom-drafts` (advisor, every 1h), `capture-sweep` (product, on QueueDrained). All three config-write paths.
10. Legacy fold: `schedule.rs` (STORY-262) — `aida schedule list` shows `schedules.toml` entries as kind `fires_task` with a one-line deprecation note; no behaviour change to `aida pull`.
11. Docs: `docs/cli/03-work-autonomy.md` entry for `aida schedule` (alias `cron`) with the job shape, schedule kinds, field set and the seat/substrate split; `docs/environment-variables.md` if any `AIDA_SCHEDULE_*` knob is added; `docs/autonomous-drain.md` "seat jobs" paragraph pointing at STORY-1218 for unattended runs. Run `docs/cli/verify-manual.py` and `verify-interface-changes.py`.

## 4. Critical files

`maintenance_schedule.rs` (the seam everything else hangs off), `awaiting_you.rs` + `collect_awaiting_report` (the per-turn delivery for both vendors), `events.rs` (exhaustive match — new variants must be classified), `git_ops.rs` resolver table (or concurrent clones conflict forever on the ledger).

## 5. Reusable helpers (do not reimplement)

- `maintenance_schedule::{load_config, due_at, quiet_now, record_outcome, emit_cron}` — extend, don't fork.
- `alias.rs::link_cas` — the CAS store-write loop to copy for the ledger.
- `agents_config::resolve_*_from` — the three-arg global/project precedence pattern.
- `schedule.rs::parse_cadence` — duration parsing (`30m`, `2h`, `1d`); reuse for `every` and for predicate durations.
- `awaiting_you::DirectivesChannel` — template for `CronChannel`.
- `rework_pickup_prompt` — template for the DUE JOBS preamble.
- `drain_lock::read_pid_live_lock`, `session_reap::scan_reapable`, `mailbox::unread_counts` + `mailbox_store::read_all_watermarks` — snapshot inputs (file-only, no network).
- `events::read_all` / `classify_since` — `on` evaluation.
- `toon::{scalar, table_raw}` — list/status output.

## 6. Risks + gotchas

- `--notice` path must stay file-only and sub-100 ms: never touch git or the network when populating `CronChannel`; read the ledger from the store worktree files directly.
- `EventKind::is_actionable()` is exhaustive — compile error until new variants are classified.
- Ledger commits on every substrate tick would spam the store: debounce (decision 2).
- `AIDA_SESSION_ROLE` is unset in tests — role-gated tests need `env -u AIDA_SESSION_ROLE`; use `crate::test_env::env_lock()` when mutating env.
- `mail.oldest_unread_age` needs message timestamps vs the seat's watermark — compute from `mailbox_store::read_local_messages` + `read_watermark`; TASK-1271 (in flight, rework queued) will land a richer latency row — keep the field name compatible (`oldest_unread_age`).
- Don't rebuild the main checkout's `target/release` — a drain is live there. Build in this worktree's own target dir.
- Doc-comment trap: `///` on clap fields is `--help` text; keep `trace:` comments as `//`.

## 7. Tests (named)

- `schedule_predicate::tests::{parses_field_op_duration, and_or_precedence, unknown_field_is_error, evaluates_against_snapshot}`
- `maintenance_schedule::tests::{merge_registries_project_wins, seat_job_never_executes, on_job_fires_once_per_new_event, when_job_fires_once_until_cleared, every_and_on_combined, legacy_task_config_still_parses}`
- `schedule_ledger::tests::{roundtrip_yaml, debounce_skips_noise, episode_lifecycle}`
- `awaiting_you::tests::{cron_channel_counts_in_total_and_compact_line}`
- `queue_cmd::tests::{pickup_prompt_leads_with_due_jobs_for_role}`
- `events::tests::{cron_kinds_are_actionable}`
- init: `init_schedule_section_lists_default_jobs`

## 8. Verification (executable)

```bash
cd /home/joe/ai/aida-story1226
cargo build -p aida-cli --release
env -u AIDA_SESSION_ROLE cargo test -p aida-cli-lib --release -- schedule_predicate maintenance_schedule schedule_ledger awaiting_you::tests::cron cron_kinds pickup_prompt_leads_with_due
./target/release/aida cron list            # alias works
./target/release/aida schedule due --seat advisor --json
./target/release/aida schedule done mailbox-triage --note "manual"
./target/release/aida awaiting --notice     # shows the cron channel when something is due
python3 docs/cli/verify-manual.py && python3 docs/cli/verify-interface-changes.py
cargo fmt --all -- --check && cargo clippy -p aida-cli -- -D clippy::correctness
```

## 9. Followups

- STORY-1218: `aida schedule install --systemd-user|--cron` + unattended execution of substrate jobs + cold-boot of overdue seat jobs (≤1/hour/seat).
- TASK-1271 rework: fold its latency row into the `mailbox-latency` job's snapshot field.
- Retire `schedule.rs` (STORY-262) once no project carries a `schedules.toml`.
- Deprecation shim if any user has `aida schedule run <task>` scripted against the pre-seat `TaskConfig` names (keep names stable).

## 10. Related

ADR-46, STORY-1218, TASK-1279, SPIKE-82, STORY-1047 (prior art grown here), STORY-262 (folded), TASK-1181 (Codex hooks parity), STORY-741 (`aida awaiting`).

<!-- trace:STORY-1226 | ai:claude -->
