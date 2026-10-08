# SPIKE-77 findings — cmux's validated UX signals: attention badges, WezTerm fan-out, and the fleet-watch advisor

**Date:** 2026-07-07 · **Spec:** SPIKE-77 · **Status:** findings complete, child specs filed
**Question:** which of cmux's validated agent-hosting UX signals are worth adopting onto surfaces AIDA already ships — and what does the driving use case (a continuous fleet-watch advisor giving the operator one narrative view of every agent session) actually require?

**Method:** code reconnaissance of `aida-tui` / `aida-core::liveness` / `aida-cli::events` (subagent sweep, file:line-verified); live probes of `wezterm cli` and `tmux` on the operator's machine; cmux capability facts per the 2026-07-07 roster entry (`docs/competitive-analysis/marketplace-roster.md`).

---

## Verdicts at a glance

| Arm | Verdict | Mechanism | Effort (est.) | Child spec |
|---|---|---|---|---|
| 1 — per-session attention badges in `aida tui` | **GO** — but via substrate events, **not** escape sequences | `events.jsonl` per-spec actionable aggregation + existing per-row liveness seam | S-M (Rust, TUI) | STORY (draft) |
| 2 — burndown fan-out into WezTerm panes | **GO** — opt-in, faithful degrade | `wezterm cli spawn/split-pane/send-text`; adapter seam, wezterm backend first | S (launcher plumbing) | TASK (draft) |
| 3 — fleet-watch advisor skill (driving use case) | **GO** — highest value/effort; skill-only, no new Rust | substrate-first tick (`ps`/`awaiting`/`watch`) + `wezterm cli list`/`get-text` census fallback | S (skill + docs) | STORY (draft) |
| (sub-arm) OSC/bell/user-var detection in the TUI | **DROP** | see Arm 1 finding 2 | — | — |

---

## Arm 1 — attention badges / notification semantics in `aida tui`

### Finding 1: the substrate is already badge-shaped

`.aida/events.jsonl` (writer/taxonomy: `aida-cli/src/events.rs:52-120`, emit `:272-292`) is an append-only, per-spec-keyed event stream whose taxonomy *is* an attention taxonomy: `PuntFiled`, `SpecShelved`, `AdvisorEscalated`, `UnreadMail`, `CiTerminal`, `PrMerged`, `QueueDrained`. A pure classifier (`is_actionable`, `events.rs:135-150`) already splits wake-worthy from benign, and offset-tracking tailers exist (`event_wait.rs:90,146`; `watch.rs`; `advisor_watch.rs`). Each `Event` carries `spec` + `run_uuid` + `ts` (`events.rs:155-169`).

The default cockpit already renders **per-row** liveness (`redesign/liveness.rs:130-141, 227-264` — in-process, `/proc`-probed, 20s-TTL cached, the same machinery as `aida ps` per BUG-677). That per-row seam (`RowLiveness` at `liveness.rs:122-128`) is exactly where a parallel `RowBadge` hangs.

**Gap to close:** events.jsonl has no persisted per-consumer "seen" cursor — every reader keeps an in-memory offset. Cross-invocation "unread since you last looked" needs a small persisted marker (e.g. `.aida/tui-seen-offset`, per-clone runtime state — deny-by-default `.gitignore` convention already covers it).

### Finding 2: the cmux escape-sequence mechanism is unavailable where it matters — DROP

cmux's rings/badges ride terminal escape sequences because cmux *hosts* every pane's PTY. AIDA's **default** cockpit (redesign, default-on since TASK-1051) does not: it dispatches children by suspending the TUI and running them with **inherited stdio** (`dispatch.rs:81-95`, `term.rs:168`), so child bytes never pass through the TUI. Only the **legacy** PTY path observes raw output (reader thread, `pty.rs:236-249`) and even it discards bytes unscanned. An OSC/bell scanner would (a) cover only the opt-out legacy path, (b) introduce byte-inspection complexity for a minority surface. Fails the quiet-depth test on reach-vs-complexity. **Dropped.** (Output-quiescence wedge detection is likewise legacy-only in-TUI; the fleet-watch skill covers quiescence from *outside* via `wezterm cli get-text` instead — see Arm 3.)

### Recommended slice (filed as child STORY)

Badge cockpit rows from **per-spec aggregation of unseen actionable events**: tail `events.jsonl` with the existing offset pattern, group by `Event.spec`, render a badge glyph + count next to the liveness glyph, clear on row focus, persist the seen-cursor. No new event sources, no PTY work, no shell redesign.

---

## Arm 2 — burndown fan-out into WezTerm panes

### Findings

- **Primitives verified live** (wezterm 20240203, operator's machine): `spawn` and `split-pane` (both return the new pane-id on stdout), `send-text`, `get-text`, `list`, `kill-pane`, `set-tab-title`. Fan-out *and* scrape in one CLI.
- **The operator's sessions already live in WezTerm** — `wezterm cli list` from inside this session enumerated all four live panes with titles and CWDs. No mux-server setup was needed when a GUI instance is running; `--no-auto-start` gives a clean "not running" failure mode for the degrade check.
- **tmux** is installed but no server runs; treat it as a *second backend later*, not the first target.

### Design recommendation (filed as child TASK)

- Opt-in only: `aida queue work --batch X --auto-complete --panes wezterm` (and/or `[burndown] pane_host = "wezterm"` in config). Bare command behavior unchanged — the faithful-launcher rule (STORY-495): no hidden opinion.
- Degrade: `wezterm` binary absent or `cli list` fails → warn once, fall back to current background spawning. Never fail the drain over a display nicety.
- Mechanics: per fanned-out implementer, `wezterm cli spawn -- <the worktree-scoped agent command>` (or `split-pane` under a dedicated tab), `set-tab-title <SPEC-ID>`; record pane-id in the session lease so the fleet-watch skill and `aida ps` can cross-reference; `kill-pane` is **not** wired to lifecycle (operator closes panes; the drain must not reach into the terminal destructively).
- **Adapter seam: defer.** wezterm/tmux command surfaces are near-isomorphic, but one real backend beats a speculative trait. Add the seam when a second backend is actually demanded (revisit trigger recorded on the child spec).

---

## Arm 3 — the fleet-watch advisor (driving use case)

### Finding: more exists than assumed; the gap is judgment, not plumbing

| Need | Exists today | Gap |
|---|---|---|
| Continuous managed-drain watching | **`aida watch`** — tails `events.jsonl`, wake-lines only on actionable, exits with `WAKE drain-crashed` if the orchestrator dies | non-drain sessions not covered |
| Per-session liveness | `aida ps` (lease TOML + `/proc` claude probe + `creator_pid` kill(0) + 24h age; `aida-core/src/liveness.rs:355-370, 70-131`) | wedged-but-alive (pid up, output dead) invisible |
| "Where am I the gate" | `aida awaiting [--json]` | pull-only; no push cadence |
| Unmanaged-session census | — | **the real hole**: hand-started claude/codex in other terminals are substrate-invisible |
| Narrative synthesis + next-action recommendation | — | the advisor-judgment layer; nothing does this |

### Design (filed as child STORY) — substrate-first, scrape-fallback, routing-only

A skill (working name `/aida-fleet-watch`), runnable once ("what's going on?") or continuously (`/loop`), each tick:

1. **Substrate sweep** (authoritative): `aida ps` → live/STALE/orphaned; `aida awaiting --json` → operator gates; `aida watch`-style events tail since last tick → completions, shelves, punts, merges. Completion detection is *structural only* (lease released, spec Done, PR merged, drain exit code) — never inferred from output text. Scrollback phrasing varies run-to-run; structural signals don't (the BUG-707 lesson generalized).
2. **Terminal census** (fallback): `wezterm cli list` (+ tmux when a server exists) → enumerate panes; subtract panes matching known leases/pane-ids; for the remainder, `get-text` the tail and classify cheaply (agent-shaped? waiting at a prompt? quiescent N min?). This is the *only* legitimate scrape surface, and it also provides wedge detection for managed sessions (pid alive + output quiescent).
3. **Diff + classify** each session against the previous tick: `completed / progressing / stalled / needs-you / unmanaged / new`.
4. **Digest + recommendation**: plain-language state of the fleet, one recommended next action per non-nominal item.
5. **Decision envelope — route, never dispatch**: it may nudge (send-text a newline/status ask is *opt-in*), file findings, ack briefs, escalate into `aida awaiting`. It must **not** start new work, take leases, or merge — a second decider fighting the drain is the failure the single-drain-lock (BUG-538) exists to prevent. Anything beyond read+route is an explicit opt-in tier aligned with the autonomy ladder (`docs/architecture/autonomy-and-escalation.md`).

Skill-only slice: no new Rust needed for v1 — every input is an existing CLI surface. If the census loop proves valuable, a later `aida fleet` subcommand can absorb the mechanical parts (revisit trigger recorded).

---

## Quiet-depth ledger (every proposed slice, on the record)

| Slice | "Makes the TUI's quiet depth stronger when someone digs in?" |
|---|---|
| Cockpit row badges off events.jsonl | **Pass** — zero new surface; the depth (event taxonomy, per-spec keying) becomes visible exactly when someone looks |
| OSC/bell scanner | **Fail** — complexity in a minority path, invisible in the default UX. Dropped |
| `--panes wezterm` opt-in fan-out | **Pass** — bare command unchanged; the flag reveals the worktree-isolation machinery visually |
| Pane-host adapter trait (multi-backend) | **Deferred** — speculative surface; one backend first |
| `/aida-fleet-watch` skill | **Pass** — advisor-side; the substrate (ps/awaiting/events) is the depth it surfaces |
| Skill auto-nudge / auto-actions | **Gated** — opt-in tier only; default is read+route |

## Child specs filed (drafts, parent SPIKE-77)

1. **STORY-765 — cockpit per-row attention badges** off per-spec actionable-event aggregation + persisted seen-cursor. (Arm 1)
2. **TASK-1120 — opt-in `--panes wezterm` fan-out** for `queue work`/burndown with faithful degrade + pane-id in lease. (Arm 2)
3. **STORY-766 — `/aida-fleet-watch` skill**: substrate-first tick + wezterm census fallback + digest with per-item recommended action; routing-only envelope. (Arm 3)

Suggested order: **STORY-766 → STORY-765 → TASK-1120.** The skill needs no Rust, exercises every signal source, and its lived usage will tell us which badge/pane affordances actually earn their keep before we commit TUI/launcher code.

## Revisit triggers (recorded per spec)

- Adapter seam (arm 2): a second pane-host backend is actually requested, or tmux server usage appears on an operator machine.
- `aida fleet` subcommand (arm 3): the skill's census loop is used weekly+ or its shell mechanics exceed ~a screen of bash.
- Roster tripwire (unchanged from the roster entry): cmux or WezTerm ships structured work-unit state.
