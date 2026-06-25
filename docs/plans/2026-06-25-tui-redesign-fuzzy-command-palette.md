# aida tui redesign — a clean, Claude-Code-like fuzzy-command front door

**Date:** 2026-06-25 · **Specs:** EPIC-52 (filed) · **Status:** Design · **Complexity:** High

Operator directive (2026-06-25, while away): *"the tui must be extremely clean and
claude-code-like, the look and feel must match claude. Think deeply. Maybe just an
input line that has a fuzzy find and you select the aida command you want to run and
from there you press enter to run it, and it may in turn be another TUI — the point
being claude code does not overwhelm you but goes in the opposite direction of
simplicity. If you have a menu system already, then maybe typing 'menu' could drop
you into that."*

## 1. The thesis

The current `aida tui` opens as a **session supervisor / dashboard** (PTY-host tabs,
status overlay, quick-action grid, the STORY-244 Launcher). That's a *wall* — it
shows you everything up front. It also won't launch standalone (`aida tui` errors on
the fd-3 intent channel; you must go through the `aida-tui` shell wrapper). Both are
the *opposite* of the directive.

Claude Code's genius is **anti-overwhelm**: it opens to a single prompt. You type;
depth appears only on demand (slash menu, sub-modes). The redesign applies that to
AIDA's machinery: **the front door becomes one input line with fuzzy command
search.** Everything AIDA can do is reachable by typing a few characters and pressing
Enter — and the result may itself be a richer TUI (config menu, status board, a chat
session). The depth is *behind* the line, never *on* it.

This is the Trojan-horse philosophy made literal (CLAUDE.md): the surface is a humble
prompt; the graph/queue/lifecycle/sessions surface through use, not on sight.

## 2. What it looks like (look & feel = Claude)

Cold open of `aida tui`:

```
  aida ▸ ▏                                              3 queued · TASK-892 next
  ──────────────────────────────────────────────────────────────────────────
```

That's it — a prompt glyph (`▸`), a cursor, and one muted status line (queue head /
session count). No panels, no grid. Matches Claude's restraint.

As you type, a fuzzy-filtered list rises beneath the line (like Claude's `/` menu):

```
  aida ▸ qu work▏
  ──────────────────────────────────────────────────────────────────────────
   ▸ queue work <scope>        drain a spec end-to-end (opens a session)
     queue list                show the work queue
     queue next                claim the next item
```

- **Type → fuzzy match** over the command surface (subcommands, common actions, and —
  later — spec IDs and queue items).
- **↑/↓ or Tab** moves the selection; **Enter** runs it.
- **Esc / Ctrl-D** exits cleanly.
- Theme/colors come from the existing `theme.rs` (already Claude-tuned); the prompt
  glyph + muted secondary text carry the aesthetic. Minimal borders, generous
  whitespace.

## 3. Progressive depth — what "run" means

The selected command is dispatched **in-process**, by *kind*:

| Command kind | Behaviour | Examples |
|---|---|---|
| **CLI / read** | Run, render output inline in a scrollable region, `Esc` returns to the line | `list`, `status`, `findings list`, `show <ID>` |
| **Sub-TUI** | Open that TUI; on exit, return to the line ("may in turn be another TUI") | `config menu`, the status board, the picker |
| **Session** | Launch the chat session (today's PTY-host) entered *from* the clean line; `Ctrl-a p` etc. still apply; exit → back to the line | `queue work <scope>` |
| **Keyword** | `menu` drops into the menu system (the richer browse-don't-type mode) | typing `menu` |

So the command line is the **hub**; the existing PTY-host session, the config menu,
the status board, and CLI output are **spokes** reachable through it. Nothing is lost —
it's *reframed* behind one entry point.

## 4. The launch fix (kills the fd-3 / wrapper friction)

Today the Launcher writes an `Intent` to fd 3 and a **bash wrapper** reads it,
dispatches, and re-launches the TUI (`intent.rs` rejects when fd 3 == stdout/stderr,
hence the error the operator hit). That two-process design is the un-clean surface.

Redesign: **move dispatch in-process.** When the user picks a command, the binary
executes it directly — subprocess for CLI/read commands (output captured to the inline
region), a direct call for sub-TUIs, `spawn`/exec for sessions — then loops back to the
line. No fd 3, no shell wrapper, no `aida dev shell-init` prerequisite. `aida tui`
Just Works from any shell. (The Intent/fd-3 path can remain as an *optional* power-user
hook, but is no longer required for the default experience.)

This single change fixes the launch bug **and** removes the biggest "un-clean" wart.

## 5. Slices (build order)

1. **Self-sufficient launch + in-process dispatch** (STORY, foundational) — `aida tui`
   runs standalone; selecting a command dispatches in-process; fixes the fd-3 bug.
   *PTY-testable headlessly (does it launch + render + dispatch?).*
2. **The fuzzy-command line UI** — the input line, fuzzy matcher over the command
   surface, the filtered list, Enter-to-run. The core.
3. **Dispatch by kind** — CLI-inline / sub-TUI / session / `menu` keyword, per §3.
4. **Spec-ID + queue-item search** — fuzzy-find also surfaces `STORY-86`, queue items
   ("work next"), recent commands.
5. **Polish** — Claude-exact aesthetic pass (prompt glyph, spacing, the one-line
   status, empty-state hint), help affordance, fuzzy-rank tuning.

## 6. Decisions / open questions for the operator (morning)

- **D1 — Does the redesigned `aida tui` *replace* the current dashboard front door, or
  sit alongside it (e.g. `aida tui` = new line; `aida tui --classic` = old dashboard)?**
  Recommendation: replace; keep the old behaviours reachable as commands (`menu`,
  `status`, sessions) so nothing is lost.
- **D2 — Fuzzy library:** use a small dependency (`nucleo`/`fuzzy-matcher`) or a tiny
  in-repo subsequence matcher? Recommendation: a tiny in-repo matcher first (zero new
  deps, full control of the Claude-like ranking), upgrade only if it's not good enough.
- **D3 — Command source of truth:** derive the fuzzy command list from clap's command
  tree (so it never drifts from the real CLI) vs a curated list. Recommendation: derive
  from clap + a curated "common actions" boost list for ranking.

## 7. Why this is right

- It is the **literal** expression of the operator's "opposite of overwhelm."
- It **fixes the launch bug** as a side effect of the cleaner architecture.
- It makes AIDA's depth **discoverable by typing**, not by reading a dashboard — the
  single highest-leverage UX move for a tool whose value is its machinery.
- Everything already built (PTY-host sessions, config menu, status board, the pause
  bridge EPIC-51) becomes a **spoke** off the new hub — additive, not throwaway.

trace:EPIC-52 | ai:claude
