# Plan: STORY-712 — Event-driven zero-token supervision

Date: 2026-06-29
Specs: STORY-712 (parent EPIC-56)
Status: Draft
Complexity: ~250 prod LOC + ~150 test LOC for the first slice; ~600/400 for the full arc, K≈4 commits, risk medium

## Approach

Today supervision burns tokens because the **supervising loop is the LLM**: `advisor_watch.rs::run_advisor_watch` wakes on a timer (`poll_interval_secs`) and forks a full `claude -p` pass every `fork_interval_secs` *whether or not anything happened* (`plan_watch_tick` returns `Fork` on bare cadence). The drain already knows every state change — it calls `drain_state::set_phase` / `set_member_outcome` / `set_run` / `append_retry` and `punt::append_to_ledger` at exactly the right moments — but those writes are **in-place JSON mutations** (`drain-state.json`) or scattered across `punts.jsonl`, so a watcher can only *poll* them. The fix is to add one **append-only event stream** (`.aida/events.jsonl`) that the drain appends a single structured line to at each state change, plus a tiny **bash classifier** (`aida watch`) that tails it, absorbs the benign majority in cheap code, and emits one wake line **only on an actionable verb**. The supervising LLM session consumes that wake line through the harness **Monitor** tool — which blocks on `tail -f` and fires the session *only when a line appears*, no timer. The escalation cascade is unchanged; we only change *what wakes the supervisor*. When the event substrate is unavailable (no drain writing events, watcher not running) the system **falls back to the existing long-interval poll** — the floor never goes away.

### Diagram

```
  DRAIN (Rust, already running)                    SUPERVISOR (LLM session)
  ┌───────────────────────────┐                    ┌──────────────────────────┐
  │ set_phase / set_member_*   │  append 1 JSON ln  │  Monitor(command:         │
  │ append_retry / punt ledger ├──► .aida/events.jsonl   "aida watch --emit-wakes")
  │ + new: events::emit(ev)    │        │           │        ▲ wake line ONLY   │
  └───────────────────────────┘        │           └────────┼──────────────────┘
                                        ▼                    │ on actionable verb
                              ┌──────────────────────┐       │
                              │ aida watch (bash/Rust)│───────┘
                              │ tail -f → classify:   │  benign 90% → silent
                              │  done/failed/punt/PR/ │  actionable → 1 line
                              │  merged/blocked → WAKE │
                              └──────────────────────┘
  degenerate: no events file / no drain → fall back to ScheduleWakeup long poll
```

## Decisions

- **Decision: add a dedicated append-only `.aida/events.jsonl`, do NOT overload `punts.jsonl` or `drain-state.json`.** **Rationale**: `drain-state.json` is a *mutable snapshot* written via `write_atomic` (`drain_state.rs::DrainState::write`) — a Monitor cannot `tail -f` an atomically-replaced file reliably (rename swaps the inode; the tail follows the old one). `punts.jsonl` is append-only but semantically narrow (punt/shelve decisions only) and consumed by morning audit + calibration — widening it to all phase transitions would pollute `aida findings` analytics. A new stream keeps each substrate single-purpose, matches the existing "one-way writes, append, names that survive `ls`" discipline (`autonomy-and-escalation.md` §5), and reuses the proven `punt::append_to_ledger` write pattern (single `write(2)`, JSONL).
- **Decision: the classifier is CHEAP CODE (`aida watch`), not an LLM.** **Rationale**: this is the story's core lever — "a watcher classifies every wake in CHEAP CODE, absorbs the benign majority, wakes the LLM only on an actionable verb." Phase-1-started, phase-2-CI-pending, retry-blip are benign; phase-done-with-PR, CI-terminal, punt-filed, spec-parked, PR-merged, queue-drained are actionable. The classification table lives in Rust (testable, pure), the same shape as `auto_complete.rs::PhaseReconcile` and `ci_idle_timeout.rs::ci_wait_verdict` decision cores.
- **Decision: the consumer is the harness `Monitor` tool over `aida watch --emit-wakes`, not a new daemon.** **Rationale**: Monitor blocks on a streaming command and turns each stdout line into a session event with **zero token cost while silent** — it is the event substrate the story asks for. `aida watch` is a thin `tail -f`+classify process; no long-lived server, no "is the daemon up?" failure mode. Filesystem stays canonical (`autonomy-and-escalation.md` §5).
- **Decision: keep the escalation cascade untouched; events are a *trigger*, not a *tier*.** **Rationale**: the punt→advisor→human cascade (`auto_complete.rs::run_advisor`, §2 of the architecture doc) already decides *what to do*. STORY-712 only changes *when the supervisor is woken to look*. An emitted `punt-filed` event wakes the supervisor; the supervisor still runs the existing advisor/triage logic. This keeps blast radius small and composable.
- **Decision: degenerate fallback = the existing timer, with the interval lengthened.** **Rationale**: §5's "the simpler thing earns its keep by always working." If `aida watch` is not running or `events.jsonl` is stale (drain crashed — detectable via `drain_state::probe` returning `Stale`), the supervisor reverts to a long-interval `ScheduleWakeup`/poll. Correctness never depends on the event path; it only makes the common case free.
- **Decision: emit is best-effort and non-blocking, mirroring `drain_state`'s existing contract.** **Rationale**: every `drain_state` mutator is documented "best-effort — a missing file is a no-op; the drain still runs, just unobservable." `events::emit` adopts the identical contract so a full disk / permission error can never stall a drain.

## Event taxonomy

The drain already calls a mutator at each of these points; emit piggybacks there.

| Event verb | Emit site (existing call we co-locate with) | Actionable? | Why |
|---|---|---|---|
| `run-started` | `drain_state::set_run` | silent | bookkeeping; supervisor already knows it launched |
| `phase-entered` | `drain_state::set_phase` | **silent (default)** | benign majority — phase 1→2→3 churn |
| `ci-pending` | `wait_for_ci_terminal` loop entry (`main.rs`) | silent | in-flight; the drain itself blocks here |
| `ci-terminal` | `wait_for_ci_terminal` return (`CiProbe`) | **WAKE** | CI green/red is a real decision point |
| `retry-blip` | `drain_state::append_retry` (`DrainStateSink::on_retry`) | silent | transient gh/git retry; absorbed |
| `phase-done-pr` | `set_member_outcome(completed=true, pr=Some)` | **WAKE** | a spec shipped a PR — supervisor may merge/advance |
| `spec-shelved` | `punt::append_failure_to_ledger` | **WAKE** | NeedsAttention park (EPIC-28) — triage candidate |
| `punt-filed` | `punt::append_to_ledger` (punt request) | **WAKE** | design-fork hit the cascade — the load-bearing case |
| `advisor-escalated` | `run_advisor` escalate path | **WAKE** | human tier reached |
| `pr-merged` | phase 4 merge success | **WAKE** | integration milestone |
| `queue-drained` | drain exit summary (`drain_summary.rs`) | **WAKE** | the terminal "agent is done" the overnight loop waits for |
| `unread-mail` | (already wired — `advisor_watch::advisor_unread_count`, TASK-776) | **WAKE** | preserve the one event-driven trigger that exists today |

The classifier's default is **silent**; only the WAKE rows above emit a line. This is the "absorb the benign majority" property. `phase-entered` carries the data a verbose mode (`aida watch --all`) can surface for debugging, but does not wake by default.

## Current-state evidence (file:symbol)

- **The token-burning idle loop**: `aida-cli/src/advisor_watch.rs::run_advisor_watch` — `loop { … std::thread::sleep(poll_interval_secs) }`; `plan_watch_tick(presence, has_unread_mail, secs_since_last_fork, fork_interval_secs)` returns `WatchTick::Fork` on bare cadence (`Some(s) if s >= fork_interval_secs => Fork`). `fork_and_run` → `session::spawn_claude_headless_resume` forks a **full `claude -p`** every pass. This is "advisor_watch forks a claude -p per pass = tokens on non-events" verbatim. Note: TASK-776 *already* added one event trigger (`has_unread_mail` beats the timer) — proof the pattern is wanted; STORY-712 generalizes it.
- **The in-phase CI poll**: `aida-cli/src/main.rs::wait_for_ci_terminal` — `const POLL_INTERVAL_SECS: u64 = 30; loop { … sleep(30s) }`, driven by `ci_idle_timeout::ci_wait_verdict` + `ci_progress_fingerprint`. This is a *blocking* poll *inside* a phase (acceptable — the Rust process is cheap), but its terminal result is exactly a WAKE event.
- **Emit points already exist as state mutators**: `aida-cli/src/drain_state.rs::{set_phase, set_member_outcome, set_run, clear_run, append_retry}` — each reads-modifies-writes `drain-state.json` via `write_atomic`. These are the natural emit sites; they fire at every state change but write a *snapshot*, not a *stream*.
- **The append-only writer pattern to reuse**: `aida-cli/src/punt.rs::append_to_ledger` (and `append_failure_to_ledger`) — JSONL, single `write(2)` per record, creates parent dir, best-effort. `events::emit` is a near-clone targeting `.aida/events.jsonl`.
- **Phase sequencing + reconcile**: `aida-cli/src/auto_complete.rs` — `enum Phase`, `PhaseDriver` trait, `PhaseReconcile` (BUG-241 reconcile-against-reality). The driver is the orchestrator that owns the per-phase outcomes; emit hooks attach to its phase boundaries.
- **The consumer**: harness `Monitor` tool (`command: "aida watch --emit-wakes"`, `persistent: true`) — each stdout line becomes a session event; silence costs nothing.
- **Substrate doctrine**: `docs/architecture/autonomy-and-escalation.md` §5 (file-based async handshake, filesystem-canonical, always-available fallback) and `docs/autonomous-drain.md` ("Limits of this cut": *"There is no liveness watchdog yet… the stream-json log is written so the watchdog can be added (TASK-298)"*). STORY-712's events.jsonl is the structured sibling of that stream-json log.

## Emit + consume mechanism

**Emit (Rust, in the drain).** New module `aida-cli/src/events.rs`:
- `enum EventKind { RunStarted, PhaseEntered{idx,slug}, CiTerminal{green:bool}, PhaseDonePr{pr:u32}, SpecShelved{phase,kind}, PuntFiled{spec}, AdvisorEscalated{reason}, PrMerged{pr}, QueueDrained{shipped,shelved}, UnreadMail }` with `fn is_actionable(&self) -> bool` (the pure classifier core — unit-testable like `ci_wait_verdict`).
- `struct Event { ts, spec: Option<String>, run_uuid: String, kind: EventKind }`.
- `fn emit(project_root: &Path, ev: &Event)` — append one JSON line to `.aida/events.jsonl`, best-effort, single `write(2)` (clone `punt::append_to_ledger`). Co-locate calls beside the existing `drain_state::*` mutators so emit and snapshot stay in lockstep.

**Classify + stream (the `aida watch` command).** Extend the CLI with `aida watch [--emit-wakes] [--all] [--once]`:
- Default: `tail -f .aida/events.jsonl` (Rust follow-loop modeled on `headless_tail.rs::FOLLOW_POLL`, or shell `tail -f` for v0), parse each line, call `EventKind::is_actionable`. On actionable, print one terminal-friendly wake line: `WAKE punt-filed STORY-712 — design-fork at .aida/punts.jsonl`. On benign, stay silent (or print under `--all`).
- `--once`: drain the backlog, classify, exit (cron / test mode), mirroring `advisor_watch::WatchOpts::once`.
- Liveness: if `drain_state::probe` returns `Stale` (orchestrator PID dead) it emits one `WAKE drain-crashed` line and exits — so the supervisor learns the drain died instead of waiting forever (closes the §"no liveness watchdog yet" gap, TASK-298).

**Consume (the supervising LLM session).** The supervisor (or the `/goal` overnight loop, or `advisor_watch` reworked) replaces its timer with:
```
Monitor(command: "aida watch --emit-wakes", description: "drain wake events", persistent: true)
```
The session does nothing — burns zero tokens — until `aida watch` prints a wake line; then the harness delivers that single line as an event and the supervisor acts (run the advisor pass, triage the shelve, merge the PR, or note the drain is done).

## How it composes with the autonomy ladder

- **Default / `--zen`**: unaffected — a present human is the supervisor; events are merely available for `aida watch` if they want a live feed.
- **`--no-human`**: the cascade (`auto_complete.rs::run_advisor`, Type A/B/C calibration, `--escalate-blocks`/`--escalate-defaults`) is **unchanged**. STORY-712 sits *above* it: a `punt-filed` or `advisor-escalated` event is the *signal* that wakes the supervising/advisor-watch session, which then runs the **existing** tiered logic. The event taxonomy maps 1:1 onto the cascade's existing artifacts (punt ledger, findings, verdict files), so no new decision surface is introduced.
- **`advisor_watch` (STORY-586) becomes the first consumer**: its `plan_watch_tick` already special-cases `has_unread_mail` as event-driven. Generalize: replace the `fork_interval_secs` cadence-fork with "fork only when `aida watch` emitted an actionable event," keeping presence (`presence::current_presence`) as the hard gate and the long-interval timer as the **degenerate fallback** when no event stream is live. This is the smallest behavioral change that kills the idle-loop token burn.

## Token math (what gets cheaper, and how much)

Concrete baseline from the brief: an overnight 8-spec drive where an advisor woke on `ScheduleWakeup` ~hourly just to check "is the agent done." Say an 8-hour drive.

- **Today**: `advisor_watch` forks a `claude -p` garden pass per cadence. Cold-boot pass ≈ $0.50–$1.00 (per `docs/autonomous-drain.md`); fork-from-live ≈ $4 first then ~$0.03 cached. At a 1h cadence over 8h that is ~8 forks. Even at the cheap cold-boot end: **~8 × $0.75 ≈ $6 of pure idle-check spend**, and most of those 8 wakes found *nothing actionable* (the drain was mid-phase). With fork-from-live's first-fork tax the idle cost is dominated by repeated cache-creation: easily **$10–$30** of non-event spend.
- **With STORY-712**: the supervisor wakes only on the ~8 spec-completions + however many punts/shelves actually occurred — say ~10–12 real events across the night. The benign phase-churn (dozens of `phase-entered`/`ci-pending`/`retry-blip` transitions) is absorbed in `aida watch` (bash/Rust, **$0**). Idle-check spend → **$0**; the only LLM spend is one pass *per actionable event*, which you were going to pay anyway. Net idle savings on an 8h drive: **roughly $6 (floor) to $20+ (fork-mode)**, i.e. the supervision overhead drops from O(hours) to O(real events). The classifier's marginal cost is a `tail -f` and a JSON parse per line.

The drain's own per-spec implement→review cost (~$3/spec, SPIKE-7) is unchanged — STORY-712 only removes the *supervisory* tax layered on top.

## Files (in build-order)

### `aida-cli/src/events.rs` (new) — the event substrate + classifier core
- `enum EventKind` + `struct Event`; `fn is_actionable(&self) -> bool` (pure).
- `fn emit(project_root, &Event)` — append-only JSONL, best-effort (clone of `punt::append_to_ledger`).
- `fn events_path(project_root) -> PathBuf` → `.aida/events.jsonl`.

### `aida-cli/src/drain_state.rs` — co-locate emit beside snapshot writes
- `set_phase`: also `events::emit(PhaseEntered)`.
- `set_member_outcome`: emit `PhaseDonePr` / `SpecShelved`.
- `set_run`: emit `RunStarted`. (`append_retry` → `RetryBlip`, benign.)

### `aida-cli/src/main.rs` — emit at the phase boundaries the driver owns
- `wait_for_ci_terminal` return: emit `CiTerminal{green}`.
- phase-4 merge success: emit `PrMerged`.
- `auto_complete.rs::run_advisor` escalate path: emit `AdvisorEscalated`.

### `aida-cli/src/punt.rs` — emit on ledger append
- `append_to_ledger`: emit `PuntFiled`. `append_failure_to_ledger`: emit `SpecShelved` (or dedupe with drain_state path — pick one site).

### `aida-cli/src/watch.rs` (new) — the `aida watch` streaming classifier
- Follow-loop over `events.jsonl` (model on `headless_tail.rs`), `--emit-wakes` / `--all` / `--once`, `drain_state::probe` liveness check → `drain-crashed` wake.

### `aida-cli/src/cli.rs` — wire the `Watch` subcommand args
- Extend existing `Watch {…}` (currently advisor-watch) or add a top-level `aida watch`.

### `aida-cli/src/advisor_watch.rs` — make it the first event-driven consumer
- `plan_watch_tick`: add an `actionable_event` input that forks immediately; demote `fork_interval_secs` cadence to the degenerate fallback when no event stream is live.

### `aida-cli/src/drain_summary.rs` — emit `QueueDrained`
- On drain exit, emit the terminal event so an overnight `/goal` loop wakes exactly once on "done."

## Critical Files

- `aida-cli/src/events.rs` (new)
- `aida-cli/src/watch.rs` (new)
- `aida-cli/src/drain_state.rs`
- `aida-cli/src/advisor_watch.rs`
- `aida-cli/src/punt.rs`

## Reusable helpers (do not reimplement)

- `punt::append_to_ledger` / `append_failure_to_ledger` (`aida-cli/src/punt.rs`) — the proven append-only JSONL writer (single `write(2)`, best-effort, creates dir). `events::emit` is a near-clone.
- `drain_state::{set_phase, set_member_outcome, set_run, append_retry, probe}` (`aida-cli/src/drain_state.rs`) — the existing emit sites and the `DrainStatus::{Active,Stale}` liveness probe for the fallback.
- `ci_idle_timeout::{ci_wait_verdict, ci_progress_fingerprint}` (`aida-cli/src/ci_idle_timeout.rs`) — pattern for a pure, unit-tested decision core; `is_actionable` follows it.
- `headless_tail.rs` follow-loop (`FOLLOW_POLL`) — the file-follow pattern for `aida watch`'s Rust path.
- `aida_core::write_atomic` (`drain_state.rs` uses it) — for any snapshot write; note events use *append*, not atomic-replace, precisely so `tail -f` works.
- `advisor_watch::{plan_watch_tick, advisor_unread_count}` — the existing event-vs-timer tick logic to generalize (TASK-776 already proves the shape).
- Harness `Monitor` tool — the zero-token consumer; no new IPC to build.

## Risks + gotchas

1. **Risk: `tail -f` misses lines if the file is rotated/replaced.** **Mitigation**: events.jsonl is **append-only, never atomically replaced** (unlike drain-state.json). Add a size-cap rotation only as a later followup, and when added, rotate by *truncate-after-copy* or use `tail -F` (capital, follow-by-name).
2. **Risk: a crashed drain leaves the supervisor blocked forever on a Monitor that never fires.** **Mitigation**: `aida watch` polls `drain_state::probe`; a `Stale` verdict emits a `drain-crashed` wake line and exits. Belt-and-suspenders: keep a long-interval (e.g. 30–60 min) `ScheduleWakeup` as the degenerate floor so a wedged watcher still surfaces.
3. **Risk: emit on the hot path stalls the drain (disk full, permission).** **Mitigation**: `emit` is best-effort and non-blocking, identical to every `drain_state` mutator's documented contract; failures are swallowed.
4. **Risk: classifier mis-labels an actionable event as benign → silent failure (worse than over-waking).** **Mitigation**: `is_actionable` is a pure function with exhaustive `match` over `EventKind` (no wildcard), unit-tested per-variant; default-on-unknown is **actionable** (wake-safe, mirrors the §1 "un-annotated prompt defaults to design-fork" pause-safe doctrine).
5. **Risk: double-emit (e.g. `punt.rs` and `drain_state.rs` both emit a shelve).** **Mitigation**: pick exactly one emit site per verb; document it in `events.rs`. Idempotency is not required if sites are disjoint.
6. **Risk: scope creep into a daemon/bus (the §6 v3 trap).** **Mitigation**: `aida watch` is a stateless `tail -f`+classify process owned by the Monitor that launched it — no server lifecycle. Filesystem stays canonical.
7. **Risk: events.jsonl grows unbounded over a long batch.** **Mitigation**: out of scope for slice 1 (a night's events are KBs); followup adds a per-drain truncate at `run-started`.

## Tests (named)

- `is_actionable_wakes_on_punt_and_shelve_and_done` — taxonomy WAKE rows.
- `is_actionable_silent_on_phase_churn_and_retry_blip` — benign absorption.
- `is_actionable_unknown_variant_defaults_to_wake` — wake-safe default.
- `emit_appends_single_jsonl_line_creates_dir` — writer parity with `append_to_ledger`.
- `emit_is_noop_on_unwritable_path` — best-effort contract.
- `watch_once_emits_only_actionable_lines` — classifier end-to-end on a fixture file.
- `watch_emits_drain_crashed_on_stale_probe` — liveness fallback.
- `plan_watch_tick_forks_on_actionable_event_before_cadence` — generalize the TASK-776 shape.
- `plan_watch_tick_falls_back_to_timer_without_event_stream` — degenerate path.

## Verification

```bash
TMP=$(mktemp -d); cd "$TMP" && git init -q && aida init -q
AIDA_BIN="$(git -C /home/joe/ai/aida rev-parse --show-toplevel)/target/debug/aida"

# Simulate a drain emitting events; assert the classifier wakes only on actionable ones.
"$AIDA_BIN" watch --once --emit-wakes &   # or feed a fixture events.jsonl
printf '%s\n' '{"kind":{"PhaseEntered":{"idx":2,"slug":"ci"}}}' >> .aida/events.jsonl   # benign
printf '%s\n' '{"kind":{"PuntFiled":{"spec":"STORY-1"}}}'        >> .aida/events.jsonl   # actionable
"$AIDA_BIN" watch --once --emit-wakes | grep -c WAKE   # expect: 1 (only the punt)
```

(Positive: actionable line emitted. Negative: phase-entered produced no WAKE.)

## Followups (out of scope now)

- events.jsonl rotation / per-drain truncation at `run-started`.
- A verbose `aida watch --all` TUI feed for live debugging.
- Wire `Monitor`-based consumption into the `/goal` overnight loop skill template (skills-side, not Rust).
- Migrate `wait_for_ci_terminal`'s in-phase 30s poll to consume CI webhooks where the forge supports them (further-out; the in-phase poll is cheap Rust, not token spend).
- Calibration: count benign-absorbed vs actionable events per drain in the exit summary to prove the lever empirically.

## Proposed TASK breakdown (list, do not file)

- **TASK-A — event substrate + classifier core** (`events.rs`): `EventKind`, `Event`, `is_actionable`, `emit`. Pure + writer tests. *No behavior change yet.* **← smallest valuable first slice.**
- **TASK-B — emit at drain state-changes**: hook `events::emit` into `drain_state::*`, `wait_for_ci_terminal`, `punt::append_to_ledger`, `run_advisor`, `drain_summary`. Produces a complete `events.jsonl` on every drain.
- **TASK-C — `aida watch` streaming classifier**: `watch.rs` + `cli.rs` wiring, `--emit-wakes/--all/--once`, stale-drain liveness wake.
- **TASK-D — make `advisor_watch` event-driven**: generalize `plan_watch_tick` to fork on actionable event, demote the cadence timer to degenerate fallback. This is where the token savings land.
- **TASK-E (skills) — supervisor consumes via Monitor**: update the overnight `/goal` / advisor-watch skill templates to launch `Monitor(command: "aida watch --emit-wakes", persistent: true)` instead of a timer.

## Recommendation

Build **TASK-A → TASK-B → TASK-C → TASK-D** in order; TASK-E is a skills-template change that can follow. The **smallest valuable first slice is TASK-A + TASK-B**: once the drain emits a real `events.jsonl`, you get an immediately useful artifact (`tail -f .aida/events.jsonl` is a live drain feed for *humans*) with **zero risk** — emit is best-effort and changes no control flow. That slice de-risks the taxonomy against a real overnight drain before any consumer depends on it. The token-saving payoff lands at **TASK-D**, but TASK-A+B is the foundation that proves the event stream is complete and correctly classified, and it is independently shippable and observable. Recommend slicing TASK-A+B as the first PR, validating the taxonomy against one real `--no-human=both` drain, then proceeding to the consumer (C/D).

## Related

- Parent: EPIC-56 (AXI incorporation). The firstmate "biggest lever."
- Substrate: `docs/architecture/autonomy-and-escalation.md` §5, `docs/autonomous-drain.md`.
- Builds on: TASK-776 (the one existing event trigger — unread-mail), TASK-298 (the stream-json log / liveness-watchdog gap this closes).
