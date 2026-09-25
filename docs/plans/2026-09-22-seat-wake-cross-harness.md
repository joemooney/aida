# Plan: Cross-harness seat wake (CR-7)

Date: 2026-09-22
Specs: CR-7 (to be decomposed by the advisor); related BUG-1589, TASK-1245, STORY-586, STORY-790, STORY-1051, ADR-26
Status: Draft
Complexity: ~900 prod LOC, ~600 test LOC, 5-7 commits, risk medium

## Approach

An agent seat that ends its turn goes idle. Nothing inside the harness wakes it
when work addressed to it arrives. The 2026-09-22 incident: a Codex advisor was
idle for about 90 minutes while its delegated lanes opened PRs, then said it was
"still working". The fix is to make the harness's own **Stop hook** the waiter.
At turn end the hook runs `aida seat wait`. That command records the turn end,
blocks (zero tokens) on `.aida/events.jsonl` until an actionable event addressed
to the seat arrives, then answers in the vendor's continue format with a brief
built from AIDA state. The model gets a new turn with a concrete reason, so
nothing has to "self-correct". When the hold times out, the process crashes, or
the harness has no hooks, a **resume fallback** takes over. It resumes the
session only when addressed work arrives, never because the seat was idle.
Model-side polling (CronCreate, `/loop`, ScheduleWakeup) stays forbidden
(BUG-1589), and terminal scraping stays out.

### Diagram

```
 turn ends ─► Stop hook ─► aida seat wait ──┬─ actionable event for seat ─► continue(brief) ─► new turn
                               │            ├─ timeout / wake cap hit ────► stop ─► seat suspended
                               │            └─ non-normal stop (error) ───► stop + record failure
                               ▼
                     SeatTurnEnded → events.jsonl
 seat suspended ── addressed work arrives ──► mark suspended ▸ re-check ▸ kill ▸ vendor resume + wake brief
```

## Decisions

- **The Stop hook is the primary wake**, not an outside daemon. **Rationale**: Claude Code, Codex and Antigravity all support a Stop hook that can block the stop and inject a continuation (confirmed by agent-inbox #71/#72 and by the hook docs embedded in `agy`). It keeps the live context and costs zero tokens while waiting.
- **All logic lives in Rust (`aida seat wait`); handlers are one line.** **Rationale**: testable, no python/jq dependency, one implementation shared by four harnesses.
- **Resume only on addressed work, never on idle alone** (threa/harnessd rule). **Rationale**: the wake prompt *is* the reason to act; an idle-timer resume just produces another idle turn.
- **Rolling per-hour wake cap (default 12), and only events newer than the last wake count.** **Rationale**: BUG-1589 (9.8B tokens from idle re-fires); cc-resume-watchdog uses the same guard.
- **OpenCode uses a `session.idle` plugin**, not a Stop hook. **Rationale**: `session.stopping` does not exist (opencode #16626); `session.idle` + `client.session.prompt` is proven (opencode-auto-continue).
- **The interactive hold policy waits on the spike.** **Rationale**: Antigravity documents that hooks "run synchronously and block the agent loop"; if the TUI locks input, a long hold would lock out the human.

## Vendor contract

| | Claude Code | Codex | Antigravity | OpenCode |
|---|---|---|---|---|
| Config | `.claude/settings.json` Stop | `.codex/hooks.json` Stop (one-time `/hooks` trust) | `.agents/hooks.json` `{"<name>":{"Stop":[{type,command,timeout}]}}` — flat list, timeout in seconds, cwd = hooks.json dir | `.opencode/plugins/aida-seat.ts` on `session.idle` |
| stdin | `session_id`, `stop_hook_active` | `session_id`, `turn_id`, `stop_hook_active` | `conversationId`, `terminationReason`, `fullyIdle` | plugin passes session id |
| Wake | brief on stderr, exit 2 | brief on stderr, exit 2 | stdout `{"decision":"continue","reason":brief}` | brief on stdout → `client.session.prompt` |
| Let it stop | exit 0 | exit 0 | `{"decision":"stop"}` or `{}` (enum `stop\|continue\|block`; not `allow`) | no prompt |
| Resume | `claude --resume <id>` | `codex exec resume <id>` | `agy --conversation <id>` | `opencode --session <id>` (verify) |

## Files (in build-order)

### `aida-core/src/telemetry.rs` — seat boundary events

- `enum EventKind`: add `SeatTurnStarted`, `SeatTurnEnded { seat, vendor, session_id, reason, owned_lanes }`, `SeatWoken`, `WakeRateLimited`, `SeatSuspended`.

### `aida-core/src/seat_wait.rs` (new) — core

- per-vendor payload parsing (`enum StopPayload`), hold predicate (`terminationReason == model_stop`, `stop_hook_active` handling), wake cap, event-follow with an addressed-to-seat filter reusing the `aida watch` classifier, brief builder, `enum VendorResponse` formatter.

### `aida-cli-lib/src/` — `aida seat wait` subcommand (new)

### `aida-core/templates/hooks/aida-seat-wait.sh` (new) — `exec aida seat wait --vendor "$1" --timeout "${2:-600}"`

### scaffolding — Stop wiring for Claude, Codex and Antigravity; the OpenCode plugin; a `.gitignore` decision for `.agents/hooks.json` (BUG-1511 rule)

### `aida-cli-lib/src/` — `aida agent resume`: suspend, re-check and kill; vendor resume adapters; wake brief

### `aida-core/templates/.aida/discipline/advisor-role.md` — status-claim honesty

## Critical Files

- `aida-core/src/telemetry.rs`
- `aida-core/src/seat_wait.rs` (new)
- `aida-core/templates/hooks/aida-seat-wait.sh` (new)
- `aida-core/templates/settings.json`
- `aida-core/src/scaffolding/mod.rs`
- `aida-core/templates/.aida/discipline/advisor-role.md`

## Reusable helpers (do not reimplement)

- the `aida watch` actionable-event classifier (wake on CI terminal, PR shipped/merged, punt, shelve, escalation)
- `aida awaiting --notice` — cache-backed per-seat inbox summary
- the turn clock under `~/.aida/turn-clock/` — human-presence oracle
- `aida agent resume` (STORY-790) and the agent registry from `aida agent new`
- `aida supervise redrive` attempt state and backoff (ADR-26)
- `aida session reap` — tearing down finished seats

## Risks + gotchas

1. **Held hook locks TUI input** → spike first; hold=0 or a short grace period when a human is present.
2. **Wake storms (BUG-1589)** → per-hour cap, newer-than-last-wake filter, actionable-only classifier.
3. **Antigravity hooks do not fire headless** (antigravity-cli #893) → spike; the resume fallback covers it.
4. **Codex hook trust** → a scaffolded hook is inert until a human approves it via `/hooks`; `aida doctor` should detect untrusted hooks.
5. **Two processes on one session id** → the resume path kills before resuming; the lock comes from the agent registry.
6. **Huge context on resume** → past a threshold, start a fresh seat from `/aida-handoff` instead.
7. **Hook timeout kills the handler mid-wait** → vendor hook timeout = hold + 10s.

## Tests

- `seat_wait::parses_{claude,codex,antigravity}_stop_payload`
- `seat_wait::no_hold_on_error_termination`
- `seat_wait::wake_cap_stops_after_n_per_hour`
- `seat_wait::ignores_events_older_than_last_wake`
- `seat_wait::ignores_events_addressed_to_other_seat`
- `seat_wait::antigravity_response_is_valid_stop_schema`
- `seat_wait::claude_codex_wake_is_exit2_stderr`

## Verification

```bash
# Step 0 spike: put a `sleep 30` Stop hook on each harness, type during the sleep, try Ctrl+C
echo '{"conversationId":"x","terminationReason":"model_stop","fullyIdle":true}' \
  | aida seat wait --vendor antigravity --timeout 5    # expect {"decision":"stop"}
cargo test -p aida-core seat_wait
```

## Followups

- Claude Code Channels as an optional Claude-only push route once it leaves research preview.
- `aida doctor` check: "seat owns live lanes but has no wake mechanism".

## Related

- BUG-1589, TASK-1245, STORY-586, STORY-790, STORY-1051, ADR-26, STORY-712
- Prior art: agent-inbox #71/#72, threa PR #2153, cc-resume-watchdog, opencode-auto-continue, claude-code #92264 / #61735
