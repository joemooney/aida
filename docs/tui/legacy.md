# AIDA TUI — legacy PTY-host shell (`AIDA_TUI_REDESIGN=0`)

> **This is the legacy TUI.** As of TASK-1051 the default `aida tui` is the
> action→target command-palette redesign — see [`README.md`](README.md). The
> PTY-host shell documented below is now the opt-out: launch it with
> `AIDA_TUI_REDESIGN=0 aida tui`.

The legacy AIDA TUI is a thin **process-supervisor shell that hosts Claude Code
sessions** (EPIC-26). It owns the terminal, reserves a one-row status
strip at the bottom, and runs each Claude session as a PTY child. You
drop out of a live conversation to a status overlay, take a
one-keystroke action, and drop back into the *same* conversation — "tmux
for AIDA workflows," not a dashboard.

It is intentionally shallow on first sight. The depth — the requirement
graph, queue routing, lifecycle, `/goal` composition — surfaces through
use. See `OVERVIEW.md` → "Public face: the TUI is the product."

## Launching

```bash
aida tui                 # empty shell — a welcome panel shows the keys
aida tui EPIC-26         # host a session working EPIC-26 in the first tab
aida tui --no-recover    # skip crash-recovery re-attach (see below)
```

The TUI must run inside an AIDA project (a git repo with `.aida/`). The
hosted child is always `aida queue work`, never `claude` directly — so
all lease / worktree / manifest / permission-mode logic is inherited.

## Hosting model

- The supervisor is synchronous and thread-per-source: an input thread
  and one reader thread per PTY feed a single event channel.
- A **focused** tab blits its PTY output straight to your terminal —
  Claude renders natively, zero parser cost. A `vt100` parser is fed in
  the background so the screen can be repainted on tab-switch or
  overlay-close.
- The bottom row is a status strip: the tab list, a notification badge,
  and a key hint that rotates (~3s) through the command vocabulary so
  the prefix keys are discoverable without a busy strip.

## Keybindings

All commands go through a **prefix key** — `Ctrl-a` by default
(configurable, see below). Press the prefix, then a command key. Plain
keystrokes pass straight through to the focused Claude session.

| Keys | Action |
|------|--------|
| `prefix` `n` | open the **new-session picker** |
| `prefix` `o` | open the **status overlay** |
| `prefix` `p` | **pause** the focused chat and open the deterministic action palette |
| `prefix` `?` | open the **keybinding cheatsheet** |
| `prefix` `[` / `]` | focus the previous / next tab |
| `prefix` `1`…`9` | focus tab N |
| `prefix` `d` | **detach** — quit the TUI; conversations persist and are re-attached next launch |
| `prefix` `q` | **quit** — confirms first when sessions are live |
| `prefix` `prefix` | send one literal prefix byte to the focused child |

`prefix` `?` opens a grouped cheatsheet (Sessions / Tabs / Overlays /
Lifecycle) of every binding; `Esc` / `q` / `?` close it. In the empty
shell a bare `?` opens it too — there is no hosted child to receive the
keystroke.

## Empty shell

With no scope (and nothing recovered) the TUI opens an **empty shell**:
a centered welcome panel naming the prefix key and the top five
bindings, plus a hint to relaunch as `aida tui <SCOPE>`. The rotating
status-strip hint reinforces the same vocabulary. This is the persistent
home of the TUI — it is *not* a dead-end black screen.

The TUI is a persistent shell: when the last hosted session ends, the
supervisor drops back to this welcome panel rather than exiting. `prefix
q` quits; `prefix d` detaches.

## Tabs (multi-session)

N Claude sessions are hosted at once, one focused. `prefix n` opens the
new-session picker:

- **start** a queued spec fresh — `aida queue work <spec> --session-id
  <uuid>`;
- **resume** a recorded conversation for the launch scope — `aida queue
  work <scope> --resume <id>` (sessions already on a tab are filtered
  out).

`↑/↓` (or `j/k`) select, `Enter` opens the session in a new focused tab,
`Esc` cancels. A soft cap (`MAX_TABS`, default 4) bounds concurrent
Claude children — N sessions is N× CPU / tokens / API.

## Status overlay (`prefix o`)

A read-only `ratatui` view built from `aida status --json`. The first
paint is cache-only (`--no-ci`, sub-millisecond); a background
`gh`-backed refresh repaints when it lands, so a slow `gh` never stalls
the overlay opening.

Panels: **Session lease**, **Branch · PR · CI**, **Queue**, **Activity
log**, **Actions**. `←/→` (or `h/l`) select an action, `Enter` runs it,
`Esc` / `q` close.

### Quick actions

| Button | What it does |
|--------|--------------|
| Next in queue | `aida queue next` — preview the queue head |
| End session | `aida session end --yes` — confirmed |
| View PR | `gh pr view` — the branch's PR |
| Drain → review | start an autonomous drain, reviewer in the loop |
| Drain → merge | start an autonomous drain, autonomous-merge each PR |

The first three run as captured subprocesses — their output lands in the
Activity log panel. State-changing actions arm a `y`/cancel confirm.

## Action palette (`prefix p`) — pause, act, inject, resume

`prefix p` **suspends** the focused chat (`SIGSTOP`s its whole process
group) and opens a deterministic AIDA action palette. The palette is
zero-LLM: typing fuzzy-filters a curated verb set (`queue`, `punts`,
`findings`, `status`, `list`, `history`), and `spec <ID>` / `run <cmd>`
run a parametric `aida show <ID> --json` / arbitrary read-only command.
`Enter` runs the highlighted action as a captured subprocess; the result
renders inline in the Result pane. The chat stays frozen the whole time —
no keystroke reaches it.

| Keys | Action |
|------|--------|
| type | fuzzy-filter the action list |
| `↑` / `↓` / `Tab` | move the selection |
| `Enter` | run the highlighted (or `spec`/`run`) action |
| `Ctrl-Y` | **resume + inject** the last result into the chat |
| `Esc` | resume the conversation (no injection) |

`Ctrl-Y` is the EPIC-51 payoff: it `SIGCONT`s the chat **and** types the
last action's result into the chat's PTY stdin as a quoted, fenced
context block (`[AIDA palette result — \`aida queue list --json\`]` …),
so the conversation continues with that result in view. It is a result
block, not a directive, and long output is head-clipped. The hint only
appears once an action has produced a result; with no result, `Ctrl-Y`
degrades to a plain resume.

### `Ctrl-D` as an alternate open trigger (opt-in)

EPIC-51 frames the palette's literal entry point as **`Ctrl-D` from the
chat**. Because `Ctrl-D` is also the terminal EOF byte (`0x04`), this is
**off by default** — `Ctrl-D` passes straight through to the hosted child
unless you opt in with `[tui] ctrl_d_palette = true`. When enabled, a raw
`Ctrl-D` typed in a focused chat opens the same palette `prefix p` opens
(suspend the chat → palette). It fires only with a child actually hosted;
in the empty shell `Ctrl-D` is left alone. `prefix p` remains the
always-available, EOF-free trigger regardless of this setting.

## Autonomous drains & `/goal` composition

The two **Drain** buttons start an autonomous queue drain. They never
hand-write `/goal` text — selecting one types `/aida-drain-queue --mode
review` (or `--mode merge`) into the focused Claude session and closes
the overlay so it runs there.

`/aida-drain-queue` (the skill) assembles the `/goal` prompt with real
command flags and the mechanism clause that matches the mode — the
structural fix for the `/goal` phrasing trap (a hand-rolled `aida queue
work --next` is a non-existent flag; a hand-picked mechanism clause
silently chooses the workflow). `review` keeps the reviewer in the loop
via `aida session end`; `merge` autonomously merges each PR.

Watch progress by reopening the overlay — the Queue panel shows the
queue draining.

## Crash recovery

A TUI crash kills its PTY children, but each Claude conversation is a
durable `.jsonl` file. The TUI records the live tab set to
`.aida/tui-state.json` on every spawn / close. On the next launch it
re-attaches each recorded session via `aida queue work <scope> --resume
<id>`.

- `prefix q` (clean quit) clears the state file — nothing to recover.
- `prefix d` (detach) and a hard crash leave it — the next launch
  re-attaches.
- `aida tui --no-recover` discards stale state and starts clean.

## Configuration

A `[tui]` block in `.aida/config.toml`:

```toml
[tui]
prefix_key     = "Ctrl-a"   # command-mode prefix (also: "ctrl+a", "C-a", "alt-b")
max_tabs       = 4          # soft cap on concurrently hosted sessions
ctrl_d_palette = false      # opt-in: Ctrl-D from the chat opens the action palette
```

Missing file / section / keys fall back to the defaults — a config error
never blocks launching the TUI.

## Terminal-state safety

The TUI puts the terminal into raw mode + the alternate screen, then
hides the cursor. Three exit paths restore the terminal back to a sane
state — cooked mode, main screen, cursor visible:

- **Normal exit** (`prefix q`, `prefix d`, last session ending in a
  `--no-recover` launch) — `TermGuard::drop` runs the restore sequence
  on the way out of `aida_tui::run`.
- **Panic** (a bug in the supervisor or a hosted child's drop chain) —
  a panic hook chains in front of the default one so restore runs
  *before* the backtrace prints (otherwise the trace scrolls past in
  raw mode with no newlines).
- **SIGTERM / SIGINT** — `kill <pid>` from another terminal, or `Ctrl-C`
  at a parent shell that propagated through. A signal handler installed
  at launch restores the terminal before the process dies. Windows
  equivalents (`CTRL_C_EVENT`, `CTRL_BREAK_EVENT`) are covered by the
  same handler. The three paths share a single atomic restore-gate, so
  a Drop / panic / signal race ends in exactly one restore.

**SIGKILL is the one case left.** Signal 9 is uncatchable by design —
no handler runs, no Drop runs. If you `kill -9 <pid>` an `aida tui`
process (or the kernel OOM-killer does it for you), the parent shell is
left with cursor hidden and raw mode on. Recovery: `reset`, or
`stty sane && tput cnorm`. trace:BUG-110

## Implementation

The `aida-tui` workspace crate (`aida tui` dispatches into it before
storage init). Modules: `term` (raw-mode + panic-safe teardown), `pty`
(PTY host), `tab` (tab manager), `statusbar`, `app` (event loop +
routing), `overlay`, `actions`, `picker`, `state` (crash recovery),
`welcome` (empty-shell panel), `help` (keybinding cheatsheet). The crate
ships in release binaries as of STORY-137.

Implementation plan: `docs/plans/2026-05-15-epic-26-tui.md`.
