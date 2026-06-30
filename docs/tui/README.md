# AIDA TUI (`aida tui`)

`aida tui` is a keyboard-driven **cockpit over your requirement graph**. It is
built on one idea (EPIC-54):

> **Pick a verb. Pick the targets. Run it.**

Everything — groom the backlog, approve a batch of drafts, route work to a
queue, preview a spec, check what's live — is the same gesture at two
altitudes: a whole scope, or a single item. There are no role tabs, no panes
to wire up, no hosted shell to drive. Two stacked lists and a status line; you
drill, multi-select, and act.

It is intentionally shallow on first sight. The depth — the graph, queue
routing, lifecycle, role gating — surfaces through use. See `OVERVIEW.md` →
"Public face: the TUI is the product."

## Launching

```bash
aida tui                        # the cockpit, opened on the Backlog scope
aida tui EPIC-54                # launch focused on an epic + its children
AIDA_TUI_REDESIGN=0 aida tui    # opt out to the legacy PTY-host TUI (below)
```

The TUI must run inside an AIDA project (a git repo with `.aida/`). It opens
the cache-backed store **once**, in-process — every scope list and spec
preview is an in-process read, so navigation is sub-millisecond and never
shells out per row.

## The model: scope → action → targets → execute

```
┌ Backlog › groom ─────────────────────────── role: advisor · 12 item(s) · 2 selected ┐
│ Scopes / Verbs (TOP panel — the current list)                                        │
│   ↵ groom            cross-spec grooming + disposition                               │
│     approve          advisor-only: draft → approved                                  │
│     reject           advisor-only: draft → rejected                                  │
│     archive          mark non-core specs archived                                    │
├──────────────────────────────────────────────────────────────────────────────────────┤
│ Targets (BOTTOM panel — the items the verb would hit)                                │
│ ▸[x] STORY-672  Story   ◐ in-progress   Fleet-wide queue view …                      │
│   [ ] BUG-619   Bug     ○ approved      aida tui lags navigating to PRs …            │
│   [x] STORY-689 Story   ○ approved      render preview as markdown …                 │
└ ↵ run · Tab items · Esc back · / find · ? help · q quit ─────────────────────────────┘
```

- **Top panel = the current list.** At launch it holds the **scopes**
  (Backlog, Open, Test, Queue, PRs, History, Findings, Sessions). Drilling into
  a scope replaces it with that scope's **verbs**.
- **Bottom panel = the target set** — the items of the current scope, always
  visible and multi-selectable. It is the live preview of *what an action would
  hit*.
- **Status line (top)** — the breadcrumb (`Backlog › groom`), your **role**, the
  item / selected counts, and the focus chip when an epic lens is set.
- **Key hint (bottom)** — the keys live for the panel you're in.

**Scopes are nouns; verbs are verbs.** A scope shows a `›` (it has children —
`↵` or `→` drills in). A verb shows a `↵` run glyph (Enter executes it on the
selection). Item-level actions ("preview this spec", "why is it open") are just
the N=1 case of the same protocol.

### Scopes

| Scope | Holds | Wired |
|-------|-------|-------|
| **Backlog** | approved + planned specs (the groomed, ready work) | yes |
| **Open** | the whole open backlog — every unfinished spec | yes |
| **Test** | shipped specs to verify (Done + Completed); a 🧪 marks rows with a `## Test Plan`, and `p` surfaces those do→expect steps | yes |
| **Queue** | routed work — each spec carries a `->role` badge so a routed draft is visible instead of vanishing; the row count is the queue depth | yes |
| **PRs** / **History** / **Findings** / **Sessions** | placeholders that prove the layout | not yet |

## Keybindings

Plain keystrokes are **hotkeys** (the list is not type-to-filter — that lives
in find mode, below). Keys are context-sensitive; the bottom hint always shows
what's live.

### Navigating

| Key | Action |
|-----|--------|
| `↑` / `↓` | move the highlight in the focused list |
| `→` | go deeper — drill a scope into its **verbs** (also from a focused item, so the verb list reflects that item's status) |
| `←` | go back one level (verbs → scopes, items → list) |
| `↵` Enter | **scope:** descend to the Targets panel · **verb:** run it on the selection · **item:** open its preview |
| `Tab` | focus the **Targets** (bottom) panel |
| `Shift-Tab` | focus back to the **list** (top) panel |
| `Esc` | clear an active find filter, else pop one level; at the top level it **quits** |
| `q` / `Ctrl-C` | quit |

### Selecting & acting on targets (bottom panel)

| Key | Action |
|-----|--------|
| `Space` | toggle-select the focused item |
| `a` / `A` | select all / select none (respects the active filter) |
| `p` (or `↵`) | open the item **preview modal** (full markdown body) |
| `↵` on a verb | run it. With nothing selected, an update verb that needs a target greys out; `groom` instead confirms "apply to all N?" |

### Find, focus, create, refresh

| Key | Action |
|-----|--------|
| `/` | **find mode** — live-filter the focused list (verbs on top, items by id/title on the bottom). `↵` keeps the filter, `Esc` clears it |
| `F` | open the **epic focus picker** — a fuzzy list of open epics; scope the whole TUI to one + its children |
| `C` | clear the epic focus |
| `n` | **new** — create a Draft spec from a typed title |
| `r` | **refresh** — re-read the store in-process so changes made outside the TUI (a CLI edit, another agent) appear without relaunch |
| `?` | context-sensitive help — what you're on, what it does, and the keys live here |

### In a preview modal

| Key | Action |
|-----|--------|
| `↑` `↓` / `PgUp` `PgDn` / `Space` | scroll the body |
| `←` / `→` | **carousel** to the prev / next spec (through the selected subset, else the whole list) without closing |
| `Esc` / `q` / `p` | close |

## Role-gated verbs — greyed, with the reason inline

This is the affordance worth knowing about. You always see a scope's **whole**
verb vocabulary (discoverability), but a verb that *can't* apply right now
renders dimmed, non-selectable, and **relabeled with why**. Three composing
axes decide it, and the most fundamental reason wins:

1. **Role** — an advisor-/reviewer-only verb you lack the seat for →
   `requires the advisor role` (or `requires the reviewer role`).
2. **Status** — a lifecycle-conditional verb the focused item doesn't match
   (`approve` on a non-Draft, `accept` on a non-Done) → `only for Draft specs`
   (or `Done`, etc.).
3. **Selection** — an update verb that would otherwise silently mutate the
   merely-focused row when you meant a set → `select item(s) first`.

So an **implementer** opening Backlog sees `groom` / `approve` / `reject` /
`archive` greyed with *"requires the advisor role"*; an **advisor** sees them
live. The TUI hides nothing and lies about nothing — and the gate is real:
greying is a courtesy, but the underlying `aida` transition is still
advisor-/reviewer-gated at the substrate, so the palette is never *more*
permissive than the CLI.

Role is **ambient context**, shown in the status line and set via
`AIDA_SESSION_ROLE` (default `advisor`). It is the lens that colors the
palette, not a navigation axis.

## Verbs

The functional verbs today (more land per slice):

| Verb | Where | Does | Gated to |
|------|-------|------|----------|
| `groom` | Backlog | cross-spec grooming + routine disposition over the selection (or "all N?") | advisor |
| `approve` / `reject` | Backlog, Open (Draft) | drive Draft → Approved / Rejected | advisor |
| `queue` | Open (Approved) | route Approved specs to the implementer queue | advisor |
| `request approval` | Open (Draft) | route drafts to the advisor queue | any role |
| `accept` | Open (Done) | reviewer's implementation-approval (Done → Completed) + acceptance comment | reviewer |
| `defer` | Open (any) | park specs off the active view with a typed revisit trigger | any role |
| `show` / `why` / `status` | Open, etc. | read-only: spec body / why-still-open / live work-state | any role |
| `archive` | Backlog | mark non-core specs archived (stubbed for now) | advisor |

Set-level verbs run over the **selected** items (or the focused item as the
N=1 default for reads); a long store-write batch runs on a background thread
with a spinner in the status line, so the TUI stays responsive and you can
keep navigating while it works.

## Status, liveness & autonomous-drain visibility

There's no separate status overlay — the cockpit *is* the status surface:

- The **status line** carries the breadcrumb, role, counts, and the focus chip.
- The **Queue scope** shows what's routed and to which role — the depth and the
  `->role` badge per spec.
- The per-item **`status`** verb (`aida status <spec>`) reports the live
  work-state of any spec: queued / In-Progress, backed by a **live ●** session
  (pid · started · elapsed) or **STALE ⚠**. This is how you watch an autonomous
  drain from the cockpit — open a queued/in-flight spec's `status` and see
  whether a real session is behind the flag.
- **`why`** explains why a spec is still open; **`r`** re-reads the store so
  state another agent changed shows up without relaunch.

## Epic focus lens

`F` opens a fuzzy picker of open epics; choosing one narrows **every** scope to
that epic plus its transitive children, and the status line shows
`focus: EPIC-54 — 6 done · 2 draft`. The pick is saved to `.aida/tui-focus`, so
relaunching the worktree re-focuses automatically. Launch focused with
`aida tui EPIC-54` or `AIDA_TUI_EPIC=EPIC-54`; the launch precedence is
`AIDA_TUI_EPIC` env → `.aida/tui-focus` marker → inference from the current
branch's commit trailers. `C` clears it.

## Terminal-state safety

The TUI takes the terminal into raw mode + the alternate screen via the same
RAII guard the rest of `aida-tui` uses, with a panic hook and a SIGTERM/SIGINT
handler chained in front, so a normal exit, a panic, or a signal all restore
cooked mode + the main screen + a visible cursor exactly once. `kill -9` is the
one uncatchable case; recover with `reset` or `stty sane && tput cnorm`.

## Choosing the TUI: redesign (default) vs legacy

`AIDA_TUI_REDESIGN` selects which TUI launches (TASK-1051). It is an
**opt-out** knob:

- **unset** (or any value other than the four below) → the **redesign**
  (this document) — the default.
- `0` / `false` / `no` / `off` (case-insensitive) → the **legacy PTY-host TUI**.

The two are different shapes:

| | Redesign (default) | Legacy (`AIDA_TUI_REDESIGN=0`) |
|---|---|---|
| Mental model | pick a verb → pick targets → run | a tmux-like shell hosting Claude Code sessions |
| What it shows | your requirement graph as scopes + targets | live Claude conversations in tabs |
| Acting | run AIDA verbs directly from the cockpit | drive Claude; pause to an action palette |

The legacy TUI is documented in [`legacy.md`](legacy.md) — its prefix-key
model (`Ctrl-a`), session tabs, status overlay (`prefix o`), pause/inject
action palette (`prefix p` / `Ctrl-Y`), autonomous-drain buttons, and crash
recovery. Use it if you want the session-hosting shell; otherwise the redesign
is the cockpit.

## Implementation

The redesign lives in `aida-tui/src/redesign/`:

- `state.rs` — the **pure** state machine: panel focus, the scope→verb
  navigation stack (and the breadcrumb it implies), the multi-select target
  set, the three grey-out axes (role / status / selection), the fuzzy filter,
  modals, and the epic picker. IO-free and unit-tested.
- `mod.rs` — the IO: the terminal guard, the in-process store reads, the
  render, background verb execution, and the keystroke→transition wiring.
- `store.rs` — the in-process cache-backed read backend (`SpecStore`): scope
  lists, spec loads, epic-descendant closures, focus markers.
- `list_row.rs` — the CLI-style columnar row renderer (id · type · status glyph
  · priority · title), so the cockpit and `aida list` share one color map.

Gate: `redesign::enabled()` (mirrored by the `aida tui` dispatch in
`aida-cli/src/main.rs`). Design + slice plan:
`docs/plans/2026-06-25-tui-action-target-redesign.md`. The legacy PTY-host
implementation (EPIC-26) is documented in [`legacy.md`](legacy.md).
