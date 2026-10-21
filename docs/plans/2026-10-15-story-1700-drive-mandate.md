# STORY-1700: drive mandate (implementer sketch, revision 2)

Status: SKETCH REVISION 2, awaiting advisor signoff. No product code yet.

- Revision 1 (eaaebd1a24) got a STRICT review on 2026-10-16 08:18 that requested
  changes 1-7. Relay auto-restart moved to STORY-1706.
- This revision answers each required change in its own section (R1-R7 below)
  and names the files, functions and tests it will use.
- Grounded against origin/main 9f0ab97da0. Read the section "Open fork F1"
  before signing: the lane classifier the review names does not exist on main.

## What stays as signed in revision 1

These parts were signed in the review and are unchanged, except where an R
section below changes them:

1. **Record.** `Mandate` (`aida-cli-lib/src/mandate.rs`) gains
   `#[serde(default)] scopes: BTreeSet<Scope>` and `#[serde(default)] kind`.
   - `Scope` is a closed serde enum: `ship-held | sketch-signoff | dispatch`.
     It has no `#[serde(other)]` arm, so an unknown scope string fails
     deserialization. `load_valid` then ignores that record with a warning
     (fail closed; see test T7.13).
   - `relay-restart` is not a scope in this story. STORY-1706 adds it together
     with the relay launch record.
   - `scope` stays `"ship-held"` on disk, so the `record_problem` check
     `m.scope != SCOPE_SHIP_HELD` and the #2687 readers still accept a drive
     record and keep honouring its ship part.
   - `aida mandate start --drive [--ttl] [--reason] [--exclude-supervised]`.
     `--drive` implies ship-held and turns `include_supervised` on unless
     `--exclude-supervised` is passed.
   - `require_direct_human`, the 24h cap, no extension, one active mandate at
     a time and operator-only exclusion all stay as they are. `ship_decision`
     is reused, with the R1 gate added.
2. **Display.**
   - `aida mandate status --line` prints one line.
   - SessionStart prints the line through `aida-session-context.sh`, edited in
     the template master `aida-core/templates/hooks/`.
   - Each turn, `aida awaiting --notice` adds the line through
     `aida-mail-notice.sh`.
   - `compose_handoff_show` (`seat_rotation.rs`) prints an ACTIVE MANDATE block
     before STANDING ORDERS. The block is read live and never copied into the
     handoff text.
3. **Routing.** Ship, sketch signoff and dispatch go to the advisor seat as
   `MandateDriveDue` items, keyed by (lane, action, head-sha). The seat job
   `mandate-drive-route` (seats=[advisor]) surfaces them in `aida awaiting`.
4. **Gates.** `[mandate.human_gates] ops = []`, empty by default.
   - `ops` ⊂ {ship, sketch-signoff, dispatch, hold-clear}; an unknown op is a
     config-load error.
   - Layers can only add gates. The layer sources changed; see R1.
5. **No advisor.** If no advisor is live, the tick sends one operator
   notification per absence episode. It keeps the substrate actions running and
   leaves seat items queued for the next advisor.
6. **Ledger.** The event is renamed from revision 1's `MandateAction`, because
   `cli::MandateAction` already exists as the CLI subcommand enum. It is
   `MandateDriveAction{mandate_id, actor: tick|session, lane, subject, action,
   outcome: done|routed|gated|refused|skipped, why}` in `.aida/events.jsonl`.
   Each tick is also ledgered at `schedule/mandate-drive.yaml`
   (`schedule_ledger`). `aida mandate status` shows the action counts and the
   last 10 actions. The digest gets a "Mandate-driven" section.

**Removed from revision 1.** The tick no longer restarts a relay. Relay
restart is not a tick action in this story. See R6 and the classification
table in R7.

## Open fork F1 (needs a ruling): the lane classifier

The review says to build on "STORY-1699's lane classifier
(`fleet_lanes::classify_lanes`)". STORY-1699 is completed, but its only commit
(75ff185cbf) shipped `aida-cli-lib/src/relay_stall.rs`, a doctor scan that
returns `DoctorFinding`s. No `fleet_lanes` module and no `classify_lanes`
function exist on origin/main.

- **Option A (recommended): this story creates the classifier.**
  - New module `aida-cli-lib/src/fleet_lanes.rs` with
    `classify_lanes(project_root, now) -> Vec<Lane>` and
    `Lane{run_dir, run_id, seat_kind, class: LaneClass, evidence}`.
  - It reuses the parsing and liveness helpers in `relay_stall.rs`:
    `parse_status`, `owner_alive` and `is_acknowledged`. Each is made
    `pub(crate)` and moved, not copied.
  - `relay_stall::scan_at` becomes a thin mapping from `Lane` to
    `DoctorFinding`. Its existing tests in
    `story_1699_relay_stall_tests.rs` must stay green unchanged, which proves
    the doctor behaviour did not move.
  - Cost: the PR also touches the 1699 module.
- **Option B: a separate prerequisite slice.** File a STORY-1699 follow-up for
  `fleet_lanes` and block STORY-1700 on it. This keeps PRs smaller but adds a
  serial dependency.

I will not start either option until the signoff picks one.

## R1. Agents cannot loosen gates

**Change asked for:** read the committed gate layer from the origin/main blob,
not the working tree. Any PR that touches the gates or the mandate code is
always human-gated.

- **Gate sources.** New function `mandate_gates::load_human_gates(project_root)
  -> Result<HumanGates>` in a new module `aida-cli-lib/src/mandate_gates.rs`.
  - Committed layer: `crate::history::git_show_blob(project_root,
    "origin/main", ".aida/config.toml")`, parsed for the
    `[mandate.human_gates]` table only.
  - Operator layer: `~/.aida/config.toml`.
  - Result: the union of both layers.
  - The working-tree `.aida/config.toml` is never read for gates, so editing it
    on a branch, or in the main checkout before commit, changes nothing.
  - If the blob is unreadable (no `origin/main` ref, or the git call fails),
    the result is `Err`. The caller treats `Err` as "every op gated" (fail
    closed) and ledgers it as `gated` with the reason
    `gate config unreadable`.
- **Gate-touching PRs.** New constant `mandate_gates::GATE_PROTECTED_PATHS`:
  - `.aida/config.toml`. Any diff to it counts, because the gate table lives in
    it and a path filter cannot see which table a hunk touches.
  - `aida-cli-lib/src/mandate.rs`
  - `aida-cli-lib/src/mandate_gates.rs`
  - `aida-cli-lib/src/mandate_drive.rs`
  - `aida-cli-lib/src/fleet_lanes.rs`
  - `aida-cli-lib/src/seat_authority.rs`
  - `aida-cli-lib/src/pr_ship.rs`
  - the ship path in `aida-cli-lib/src/pr_cmd.rs`
  - `aida-cli-lib/src/merge_hold.rs`
- **How the check is wired.**
  - `ShipCheck` gains `changed_paths: Option<&[String]>`. In `pr_cmd.rs` the
    caller fills it from the forge's PR file list at the head SHA it ships.
  - `None` means the files could not be resolved. That refuses with the new
    `MandateRefusal::ChangedPathsUnresolved`.
  - A hit refuses with the new `MandateRefusal::TouchesGates{path}`.
  - The check runs inside `ship_decision` before hold cover, so it applies on
    every mandate ship path and cannot be routed around.
  - The tick classifies such a PR as `gated` (operator notification plus ledger
    row) and never routes it to the advisor as a ship.
- **Tests:** T7.9, T7.10 and T7.11.

## R2. The tick's authority does not depend on who runs it

**Change asked for:** scrub the session grant, role and purpose; take a
dedicated lock; share budget and backoff state across invocations.

- **One entry point.** New module `aida-cli-lib/src/mandate_drive.rs` with
  `run_tick(project_root, now, hooks: &TickHooks) -> TickReport`. The scheduler
  job, `aida schedule run mandate-drive` and a manual `aida fleet drive --tick`
  all call it.
- **Authority comes only from disk.** `DriveAuthority::resolve()` reads:
  - the mandate (`mandate::load_valid`),
  - the gates (R1),
  - the principal (R4).

  It never reads `AIDA_SESSION_GRANT`, `AIDA_SESSION_ROLE` or
  `AIDA_SESSION_PURPOSE`.
- **Scrubbing.** Every child process the tick spawns is built by
  `mandate_drive::scrubbed_command(program)`. That helper calls `env_remove` on
  those three variables, following the `agent_launch.rs:4721` precedent, and
  sets `AIDA_SCHEDULE_CHILD=1`.
- **Unattended means no grant.** If the tick finds any of the three variables
  in its own environment, it records the run as `actor: tick` with
  `why: "inherited session env ignored"`. Its decisions are the same either
  way, and test T7.6 asserts that.
- **Dedicated lock.** `.aida/mandate-drive.lock` is taken with
  `fs2::FileExt::try_lock_exclusive`, following the pattern of `try_tick_lock`
  in `maintenance_schedule.rs`. The lock is held for the whole tick and taken
  in addition to `schedule-tick.lock`, so a manual tick and a scheduled tick
  cannot overlap.
  - If the lock is contended, the tick exits 0 and writes one `skipped`
    ledger row: `another drive tick holds the lock`.
- **Shared state.** `.aida/mandate-drive-state.json` is read and written only
  while the lock is held, with an atomic temp-plus-rename write. It holds:
  - per `(lane_key, action)`: `attempts`, `last_at`, `next_eligible_at`. The
    backoff is 1, 2, then 4 ticks; a `(lane_key, action)` pair that is not
    eligible yet is `skipped` with the reason `backoff`.
  - `emitted_due`: the set of (lane, action, head-sha) keys already emitted, for
    deduplication across ticks.
  - per mandate id: `actions_this_mandate` and `absence_episode`.
  - The per-tick budget defaults to 10 actions. It is configurable as
    `[mandate.drive] max_actions_per_tick`, and only the operator layer can set
    it.
  - The state is keyed by mandate id, so a new mandate starts with a fresh
    budget.
- **Tests:** T7.5 and T7.6.

## R3. Re-read the mandate before each action

**Change asked for:** check for a stop tombstone or expiry immediately before
each action, not only at the start of the tick.

- **The check.** New function `mandate::recheck(id, principal, now) ->
  Result<Mandate, MandateRefusal>`. It calls `load_valid` on the mandate
  directory again and then `state_at`. It returns `Ok` only when the newest
  active mandate is still `id`; otherwise it returns `Stopped` or `Expired`.
  Nothing is cached.
- **Where it runs.** `mandate_drive::act(lane, action)` calls `recheck`
  immediately before every side effect: emitting a due item, a mail nudge, a
  lease expiry or an operator notification.
- **On failure.** That action is ledgered `refused` with
  `why: "mandate stopped mid-tick"` or `"... expired mid-tick"`. Every action
  left in the tick is ledgered `skipped` with the same cause, and the tick ends.
- **Test seam.** `TickHooks::before_action: Option<&dyn Fn(usize)>` lets a test
  write a stop tombstone, or move the injected clock past expiry, between
  action 1 and action 2.
- **Test:** T7.7.

## R4. The principal under the scheduler

**Change asked for:** warn when the installed tick would resolve a different
principal, or when no out-of-session schedule driver is installed.

**Why it matters.** `load_valid` keeps only records whose principal matches
`current_user_id(None)`. That function falls back through `AIDA_USER`, `USER`,
`USERNAME`, then the literal `"default"` (`kernel.rs:640`). If the driver runs
with a different `USER`, or with no `AIDA_USER`, the tick would see "no
mandate" and do nothing silently.

- **Resolving the driver's principal.** New function
  `mandate_drive::scheduler_principal(project_root) -> PrincipalProbe`, with
  variants `Resolved(String)`, `NoDriver` and `Unknown(why)`.
  - It reads the installed driver artifact through the existing
    `schedule_driver` / `maintenance_schedule` read-back: the crontab line
    carrying the `aida-schedule-tick:<repo>` marker, or the systemd user unit.
  - It applies the same order as `current_user_id`:
    1. an explicit `AIDA_USER=` in the unit's `Environment=` or the cron line;
    2. otherwise the passwd name of the current uid, which is what cron and the
       systemd user manager put in `USER`.
- **Warnings.** Both `mandate start --drive` and the new `aida doctor` category
  `mandate-drive` print a warning when:
  - the result is `NoDriver`: "no schedule driver installed; the drive tick
    will never run (aida schedule install …)";
  - the resolved principal differs from the mandate principal (or from the
    current user, at start). The text names both values and the fix:
    `AIDA_USER=<principal>` in the driver environment.
  - Start still succeeds, because the mandate is the human's decision and the
    warning only says the tick will not act on it.
- **The tick's own check.** A plain `load_valid` would hide this case, so the
  tick does an extra read with no principal filter. If an active drive record
  exists for a different principal, the tick ledgers one `refused` row
  (`principal mismatch: record=<a> tick=<b>`) and one operator notice per
  mandate id, then takes no action.
- **Tests:** T7.12, plus a doctor test.

## R5. AC3 reading (recorded on the spec)

This reading has been added as a spec comment headed "AC NOTE (AC3 reading)",
so merge-time completion is not disputed. With no live advisor:

- ship, sketch signoff and dispatch wait for the advisor seat. The STORY-1698
  ship gate needs an anchored advisor grant, and the tick holds no grant.
- the tick notifies the operator once per absence episode and keeps doing
  substrate actions: lease expiry on dead pids, mail nudges, classification,
  due-item emission and the ledger.

"Performs the next action itself" in AC3 is satisfied by those substrate
actions plus routed due items. Relay restart is out of scope here (STORY-1706).

## R6. Blocked relays: the due item carries the resolving action

**Change asked for:** the advisor's due item for a blocked relay carries the
resolving action itself. No "next tick restarts" trigger exists in this story.

- **What the due item holds.** For a `Blocked` lane (exit 3 / BLOCKED_ON_HUMAN)
  the `MandateDriveDue` payload, built by
  `mandate_drive::resolving_action(lane)`, contains:
  - the run id, the absolute handoff path and the `BLOCKED_ON_HUMAN` marker
    line (the question);
  - the resolving command. When the marker names a spec waiting on signoff:
    `aida show <SPEC> --comments`, then the signoff comment command. Otherwise:
    answer in the handoff file.
  - the resume instruction: relaunch the run with `bin/relay.sh` against the
    same handoff. Until STORY-1706 there is no recorded launch to replay, so
    the item points at `relay.status` (run, handoff) and the profile under
    `.aida/relay/profiles/` when one exists.
  - the acknowledgement step: `touch <handoff>.done`, which is the
    `is_acknowledged` marker that also stops `relay-stall-guard` from
    re-tripping.
- **What the tick does not do.** It never restarts or re-execs anything.
  Once the advisor resolves and relaunches, the next tick re-classifies the
  lane from fresh state (running, or acknowledged). The due-item key includes
  the run id and `updated`, so a new block on the same run is a new item.
- **Test:** T7.4.

## R7. Tests

Classification tests go in
`aida-cli-lib/src/tests/story_1700_lane_classify_tests.rs` (the F1 Option A
module). The rest go in `aida-cli-lib/src/tests/story_1700_drive_mandate_tests.rs`.
All use fixture run directories with a `relay.status` and an optional held
`relay.lock`.

**Classification table.** `fleet_lanes::LaneClass` and the tick's routing:

| relay.status / lock evidence | LaneClass | Tick outcome |
|---|---|---|
| lock held (any state) | `Running` | none |
| state=running or queued, lock free, trap never ran | `Crashed` | route advisor (crash notice) |
| state=exited exit=130 / 143 | `SignalStopped` | skipped (deliberate stop) |
| state=exited exit=0, no complete (session cap reached) | `SessionCap` | route advisor (dispatch continuation) |
| state=complete | `Complete` | none |
| exit=1 | `SetupError` | route advisor |
| exit=3 / state=blocked | `Blocked` | route advisor with resolving action (R6) |
| exit=4 / 5 | `UsageBin` | route advisor |
| exit=6 | `RunLocked` (duplicate launch; the live run continues) | skipped |
| exit=7 | `TerminalApi` (credits) | route operator |
| exit=8 | `NoSlot` | route advisor (relaunch when a slot frees) |
| any dead class on an advisor-seat run | same class, `seat_kind=Advisor` | route operator |
| `<handoff>.done` present | `Acknowledged` | none |

How the classifier decides two things:

- **Advisor seat.** relay.status records no role, so the seat comes from the
  handoff path. A handoff under `.aida/handoff/advisor*` counts as an advisor
  seat. Until STORY-1706 adds the launch record, an unreadable handoff path is
  also treated as an advisor seat (fail closed, so it routes to the operator).
- **Session cap.** The exit-0 exited case is told apart from complete by
  `state` (`complete` comes from `RELAY_FINAL_STATE`; the session cap leaves it
  unset, so it reads as `exited` with rc 0).

**Named tests:**

- T7.1 `classify_table_covers_every_exit` is table-driven over the rows above,
  including 130, 143, the session cap, 1, 4, 5, 6, 7 and 8.
- T7.2 `running_state_with_free_lock_is_crashed`
- T7.3 `held_lock_is_running_whatever_status_says`
- T7.4 `blocked_due_item_carries_resolving_action_and_no_restart` checks the
  payload contents and that no process is spawned.
- T7.5 `concurrent_ticks_one_acts_other_skips` runs two threads against one
  project root. Exactly one takes `.aida/mandate-drive.lock`; the other ledgers
  `skipped`. The state file holds no duplicate due keys and the backoff counters
  are shared.
- T7.6 `inherited_grant_is_scrubbed_and_ignored` sets `AIDA_SESSION_GRANT`,
  `AIDA_SESSION_ROLE` and `AIDA_SESSION_PURPOSE`, then checks two things: the
  decisions equal those of a run without them, and a child spawned through
  `scrubbed_command` (an env-dumping fake) sees none of the three.
- T7.7 `mandate_stopped_mid_tick_refuses_remaining` and
  `mandate_expired_mid_tick_refuses_remaining` use the `before_action` hook.
- T7.8 `due_items_dedup_across_two_ticks`
- T7.9 `gate_removed_in_working_tree_still_enforced_from_origin_main` uses a
  fixture repo. Its `origin/main` config has `ops = ["dispatch"]` and its
  working tree has `ops = []`, so dispatch must still be gated.
- T7.10 `pr_touching_gate_paths_is_gated`: a `ship_decision` with
  `changed_paths` containing `.aida/config.toml` returns
  `Err(TouchesGates)`, and the same for `mandate.rs`.
- T7.11 `unresolved_changed_paths_refuses` and
  `unreadable_gate_blob_gates_everything`
- T7.12 `principal_mismatch_warns_and_does_nothing` checks that a record for
  principal `a` with the tick resolving `b` produces one refused row and no
  actions, and that `scheduler_principal` reports `NoDriver` with no artifact
  installed.
- T7.13 `unknown_scope_string_fails_closed` (`"scopes":["relay-restart"]` or
  `"bogus"` is ignored with a warning) and
  `unknown_human_gate_op_is_config_error`
- T7.14 kept from revision 1:
  - a #2687-shaped JSON round-trips;
  - a drive record loads in the #2687 loader as ship-held;
  - `include_supervised` defaults on with `--drive`;
  - start and stop refuse non-TTY;
  - the `--line` and notice lines;
  - `handoff --show` order (mandate, then standing orders, then handoff);
  - one operator escalation per absence episode;
  - the `mandate.drive_active` predicate field type-checks
    (`schedule_predicate::Field::MandateDriveActive`, a flag);
  - `MandateDriveAction` is emitted for every outcome.

**Local runs:**

- `make test-touched`
- `make check-ci-compile`
- the area binaries: `cargo test --profile agent -p aida-cli-lib
  story_1700_`, plus `story_1699_relay_stall` (regression for F1) and
  `story_1698_mandate`
- `make check-ci-fast` before each push

## Non-blocking points from the review

- **hold-clear.** Drive does not cover hold-clear. Clearing holds stays on the
  human-TTY path (`merge_hold`). `hold-clear` is in the gate op list only so an
  operator can state it explicitly; it has no drive action in this story.
- **Expired/stopped line.** The per-turn "mandate expired/stopped" line is shown
  once per seat. The marker is `~/.aida/mandates/.seen/<mandate-id>.<seat>`, and
  `awaiting --notice` checks and writes it.
- **Codex/agy notice hook.** Whether the codex and agy relay lanes fire the
  per-turn notice hook will be checked against `aida-relay` `bin/relay.sh` and
  the codex/agy hook installs during implementation, and reported on the PR.
  If a lane does not fire it, the SessionStart line is that lane's only
  display, and a follow-up will be filed rather than widening this story.
- **Fail-closed deserialization.** Kept and tested (T7.13).

## Floors kept

- Independent attested review.
- No self-merge.
- No force-push or rewrite of shared branches.
- Only a human mints or stops a mandate.
- The tick never mints, holds or forwards a grant, and never restarts a relay.

## Sequencing

- #2687 (STORY-1698) is merged.
- F1 needs a ruling first. Implementation starts only after a later signoff
  comment on STORY-1700.
- This plan folds into the implementation PR. Do not merge this plan-only
  branch, because its commits carry a (STORY-1700) trailer.
