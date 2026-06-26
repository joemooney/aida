# STORY-686 — Blocked/waiting board (TUI flow-cockpit home view)

- **Date:** 2026-06-25
- **Specs:** STORY-686 (parent EPIC-53; defers STORY-687 collab, STORY-688 role-split)
- **Status:** Approved — MVP, operator-blessed shape (2026-06-25)
- **Complexity:** Medium (new TUI home view; pure synthesis over existing fast surfaces)

## Approach

Re-orient the `aida tui` home from a status browser to a **flow cockpit**: one
board that answers "why isn't this moving, who must act, how do I unblock it."
Single-user, **visibility-first** (no new role), **read-cockpit-that-dispatches**
(launch the relevant `aida` subcommand; do not reimplement mail/questions/approve).

The board is the **7 reason-groups**, each a Nav section that reuses the
**STORY-685 two-pane model**: the Nav pane lists the reasons with `(count) · owner`;
selecting a reason fills the List pane with its items; Enter on an item dispatches
that reason's unblock action. The existing perspectives (Queue/Backlog/History/PRs/
Sessions) are retained, appended after the reason-groups in the Nav. The board is
the **default** view on launch.

```
NAV (reasons, count·owner)        LIST (items for selected reason)     ENTER →
needs approval (3) · you      →   FR-12  Add login validation          approve
needs an answer (2) · you         BUG-9  Null response on 500
blocked by dep (4) · wait
needs attention (1) · impl
awaiting review (2) · reviewer
in flight (5) · impl
deferred (6) · trigger
── perspectives ──
Queue / Backlog / History / PRs / Sessions
```

## Reason taxonomy → fast data source (the key design)

`aida list --json` already emits per-spec `status`, `queued`, `in_flight`,
`blocked` flags — so ONE cache-fast call (~0.26s) derives several reasons:

| Reason | Source (all cache-fast except noted) | Owner shown |
|---|---|---|
| needs approval | `aida list --status draft --json` | you / advisor |
| needs an answer | `aida questions` (open) | you (human) |
| blocked by dependency | `aida list --json` rows where `blocked == true` | — (wait) |
| needs attention | `aida findings list` / status NeedsAttention | implementer |
| awaiting review | `gh pr list` **(network ~1s — lazy-fill)** + `list --status done` | reviewer |
| in flight | `aida session leases` (or `list --json` `in_flight`) | implementer |
| deferred | `aida list --deferred --json` (+ revisit trigger) | — (trigger) |

**Perf constraint (hard):** compose ONLY these cache-fast reads. NEVER call
`aida status` (~3.75s, BUG-616). The one network source (`gh pr list`) must
lazy-fill — paint the cheap rows instantly, fill "awaiting review" async (reuse
the `collect_open_prs` memoization trick from `main.rs`).

## Dispatch (Enter on a List item) — lightest sensible per reason

Principle: Enter = the most common unblock action for that reason; fall back to
showing the item. Implementer picks the lightest; checkpoint if unsure.
- needs approval → preview + a key to `aida edit <id> --status approved` (reject/clarify stay as the existing commands)
- needs an answer → launch the `aida questions` flow for that spec
- blocked by dependency → jump selection to the blocking spec
- needs attention → launch `aida findings` triage for it
- awaiting review → `gh pr view` / open the PR
- in flight → info only (lease holder + age)
- deferred → show the revisit trigger; a key to `aida undefer <id>`

## Files (build order)

- `aida-tui/src/dashboard.rs` — add the reason-group `NavSection`s (or a parallel
  enum), the per-reason fetch fns (compose the fast sources above), counts+owner
  in the Nav labels, default-to-board on launch. Reuse `fetch_status`/`parse_list_json`.
- `aida-tui/src/launcher.rs` — Enter-dispatch per reason (extend `route_enter`);
  keep the STORY-685 two-pane routing (Right=into-list, Left=back, BUG-617).
- `aida-tui/src/*` — wherever Nav sections + the default view are defined.

## Critical files / reuse (don't reimplement)

- The two-pane focus model + `route_key`/`route_enter` (STORY-685, launcher.rs).
- `fetch_status` + `parse_list_json` + the `--json` flags (`queued/in_flight/blocked`).
- `aida queue list --json` (BUG-616) shape; `collect_open_prs` memoization (main.rs).

## Risks / gotchas

- Nav length: 7 reasons + 5 perspectives = 12 sections. If unwieldy, group under a
  header or collapse empty reasons — checkpoint.
- `gh pr list` network latency — MUST lazy-fill, else the home view re-inherits lag.
- Don't double-count: a spec can match >1 flag (e.g. blocked AND in_flight). Pick a
  precedence (in_flight > blocked > needs-attention > awaiting-review > needs-answer
  > needs-approval > deferred) so each item lands in exactly one group.

## Tests (named)

- `dashboard::tests` — per-reason classification from a fixture of `list --json`
  rows (blocked/in_flight/draft/deferred map to the right group; precedence holds).
- `route_enter` dispatch per reason emits the expected Intent (mirror the existing
  route_key tests).
- Empty-reason hidden/greyed; counts match.

## Verification (executable)

- `cargo build -p aida-cli` · `fmt --check` · `clippy` · `glyph-lint --block`
- `cargo test -p aida-tui` (dashboard + route)
- Manual: `aida tui` opens on the board; each reason's count matches the
  corresponding `aida` command; Enter dispatches; paint is instant (PR row lazy).

## Followups

- STORY-687 (cross-human collaboration) — deferred until this proves out.
- STORY-688 (product/advisor role-split) — deferred until visibility shows the
  advisor is still a measured throughput wall.
- Real `aida list --limit` + load-more pagination (TASK-897 follow-up).
- Surface `aida intake` proposals + advisor backlog explicitly in "needs approval".

## Related

EPIC-53, STORY-685 (two-pane nav), BUG-616 (perf — the fast-source constraint),
TASK-897 (panel cap), EPIC-26 (TUI is the product).
