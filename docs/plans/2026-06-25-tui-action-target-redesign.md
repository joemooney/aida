# `aida tui` redesign — the action→target command-palette model

- **Date:** 2026-06-25
- **Status:** Design locked enough to prototype; follow-on slices gated on the prototype.
- **Driver:** the current role-tabbed, persistent-right-pane TUI does not scale to AIDA's surface (~47 skills, dozens of verbs) and reads as "overwhelming" (cf. STORY-673). Replace it with a single, learnable interaction protocol.
- **Specs:** EPIC (filed) · prototype STORY (filed) · folds in STORY-689 (markdown/field-colored preview → becomes the item modal).

## 1. The protocol (the whole point)

One primitive, applied everywhere:

> **Identify an action (verb) and the set of items (targets) to apply it to.**

Navigation is **scope → action → targets → execute**:

```
Backlog            (scope)        ↵ drills to its verbs
  └ groom          (action/verb)  ↵ executes on the current selection
       └ [items]   (targets)      Space-toggle; none selected → "groom all?"
```

**Item-level actions are the N=1 case of the same protocol** — "preview this spec" / "open its PR" is just a verb on a one-item selection. There is *one* model, two altitudes (scope-wide and single-item). This unification is the design's reason to exist; everything below serves it.

Prior art this fuses: file managers (ranger / lf / nnn — navigate, multi-select, apply an operation) + fuzzy command palettes (Telescope / fzf / Raycast — type to find a verb). Both proven; the fusion is the idea.

## 2. Layout

Two stacked panels + a status line. **No role tabs.**

```
┌ Backlog › groom ───────────────────────────── role: advisor · 12 ready ┐
│ > gr|                                        (fuzzy filter, focused list) │
│   groom            cross-spec grooming + disposition                     │  ← TOP: current list
│   approve          advisor-only: draft → approved                        │     (scopes, then verbs
│   archive          mark non-core specs archived                          │      after a drill)
├──────────────────────────────────────────────────────────────────────────┤
│ [x] STORY-672  Fleet-wide queue view …                                   │  ← BOTTOM: the target set
│ [ ] BUG-619    aida tui lags navigating to PRs …                         │     (items of the scope;
│ [x] STORY-689  render preview as markdown …                              │      multi-select)
└─ Tab: items · Space: select · ↵: run · Esc: back ────────────────────────┘
```

- **Top panel** = the current list. At launch it holds the **scopes** (today's Nav items: Backlog, Queue, PRs, History, Findings, Sessions). Drilling into a scope replaces it with that scope's **verbs**.
- **Bottom panel** = the **target set** — the items of the current scope, always visible, multi-selectable. It is the live, space-efficient preview of *what an action would hit*.
- **Item content preview** (the full spec markdown body) is a **modal overlay**, not a persistent pane — conserves space, opened on demand (STORY-689's renderer lives here).
- **Status line**: breadcrumb (`Backlog › groom`), current **role**, and counts.

## 3. Gesture grammar (where palette UIs live or die — keep it tight)

| Key | Behavior |
|---|---|
| *type* | fuzzy-filter the **focused** list (verbs when on top; items — by id/title — when on bottom) |
| `↑`/`↓` | move within the focused list |
| `↵` Enter | **scope → drill** to its verbs · **verb → execute** on selection · **leaf action → run** |
| `Tab` | move focus to the **bottom** (target set) |
| `Shift-Tab` | move focus back to the **top** (actions) |
| `Space` | toggle-select the focused item |
| `Esc` | pop one level (verbs → scopes) · or close the modal |
| `p` (or `↵` on a focused item) | open the **item modal** (full markdown preview) |
| `a` / `A` | select all / none (filtered) |

**Navigation stack is explicit:** `Esc` pops; the breadcrumb always shows depth. The single biggest failure mode of drill-in UIs is getting lost — the breadcrumb + `Esc` are non-negotiable.

## 4. Decisions locked

1. **Scopes are nouns, verbs are verbs — visually distinct.** A scope shows a `›` (has children; `↵` drills). A leaf action shows a run glyph (`↵` executes). This removes the only ambiguity in the original sketch (Enter meaning two things) by making the entry *type* legible.
2. **Multi-select is first-class, everywhere.** Every item list is selectable; every scope exposes the verbs that apply to a selection. The same gesture grooms / archives / defers / queues / briefs / runs-compete on N items. `Space` toggles; **none-selected + run → confirm popup ("apply to all N?")**.
3. **Role is relocated, not removed.** Role becomes **ambient context** shown in the status line and set via an action (or a `:role` command). It *filters/colors the verb palette* — an advisor sees `groom`/`approve`, an implementer sees `pickup`. Role stops being a navigation axis and becomes the lens. (Do **not** drop role — the queue is role-scoped and `approve` is advisor-gated; losing it silently would break gating.)
4. **Two preview altitudes, two surfaces.** Scope contents → the persistent **bottom panel**. One item's full body → the **modal**. These never compete for space.
5. **Fuzzy is context-sensitive.** The `>` prompt filters whatever list is focused — verbs on top, items on bottom. One prompt, right behavior in every context (the Telescope model).

## 5. Open forks (decide before generalizing, not before prototyping)

- **Verb provenance:** are a scope's verbs hardcoded per scope, or derived from a registry (skills + CLI verbs tagged with the scopes/selection-shapes they accept)? A registry scales to the ~47-skill surface and lets new verbs appear without TUI edits — but is more work. *Lean registry, but the prototype hardcodes Backlog's verbs to validate the loop first.*
- **Where the role-set UI lives:** a `:role` command in the prompt, a dedicated scope, or a status-line toggle. *Defer until Slice 3.*
- **Execution feedback:** inline progress in the bottom panel vs a transient toast vs the modal. *Decide when the first long-running verb (groom/drain) is wired.*

## 6. Slice plan (prototype-first — de-risk the gesture grammar before the rewrite)

**Slice 1 — Prototype the loop on ONE scope (the keystone; filed as a STORY).**
`Backlog` scope → `groom` verb → multi-select items → execute (+ none→all confirm) → `p` opens the item modal. Hardcode Backlog's verbs. Goal: prove the gesture grammar *feels* fast and obvious in real use. If it does, generalize with confidence; if not, we spent a day, not a rewrite.

**Slice 2 — Generalize scopes + verbs.** Queue, PRs, History, Findings, Sessions over the same protocol; verbs per scope (registry or hardcoded-per-scope per the §5 fork).

**Slice 3 — Role as ambient context.** Remove role tabs; status-line role + palette filtering/gating by role.

**Slice 4 — Item modal = STORY-689.** The full markdown-rendered, field-color-coded preview becomes the modal (`p`). STORY-689's renderer + themeable field colors live here.

**Slice 5 — Fuzzy palette polish.** Context-sensitive fuzzy over verbs and items; select-all/none/filtered; breadcrumb + Esc stack hardening.

Slices 2–5 are **not yet filed as specs** — they are gated on Slice 1 validating the protocol (don't spec the whole rewrite on paper).

## 7. Risks + gotchas

- **Full rewrite scope.** Mitigated by the prototype-first slice plan: Slice 1 is a bounded, throwaway-able validation, not the rewrite.
- **Gesture-grammar feel is subjective** — it must be *used*, not reasoned about. Slice 1 exists to feel it.
- **Losing role gating** if §4.3 is botched — keep `approve`/advisor-only enforcement at the substrate, never only in the TUI palette filter (the TUI hides the verb; the CLI/MCP still gates it).
- **Preview fetch cost** — the bottom panel and the modal must not re-shell `aida show` on every cursor move (the current `preview_via_show` stdout-capture is the anti-pattern; render structured data, cache per row). See STORY-689.

## 8. Related

- STORY-689 — markdown + field-colored preview → the item modal (Slice 4).
- STORY-673 — `aida status` overwhelming output (same "quiet depth, not a wall" thesis).
- Current TUI: `aida-tui/src/` (`dashboard.rs` 3-pane + `preview_via_show`, `app.rs`, `state.rs` `RoleTab`).
