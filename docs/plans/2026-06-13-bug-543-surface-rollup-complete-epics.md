# BUG-543 — Surface rollup-complete epics as "ready to close"

- **Date:** 2026-06-13
- **Specs:** BUG-543 (relates: BUG-536, EPIC-42)
- **Status:** In progress
- **Complexity:** Small–medium (one core primitive + one new burndown bucket + view wiring)

## Approach

Operator chose **option (b): surface + one-click**, not auto-complete. A fully-delivered
epic (every child `Completed`) must stop reading as a generic "umbrella" item in
`aida human` / `aida burndown explain`; instead it surfaces with a distinct
**ready-to-close** signal naming the one-line close command. Detection only — no
status is mutated automatically (a rollup miscount must never silently close an epic).

```
collect_open_facts (main.rs, has store)
  └─ for each epic: child_status_rollup(store, epic) ──► OpenFacts.epic_rollup = Some((completed,total))
                                                              │
explain_open (burndown.rs, pure)  ◄───────────────────────────┘
  └─ epic && total>0 && completed==total ──► OpenBucket::ReadyToClose
                                              └─ "ready to close — all N children Completed; …"
  └─ else (epic) ──► OpenBucket::Umbrella  (unchanged)

aida human / burndown explain  ──► new bucket rendered first (most-actionable)
aida queue advance             ──► AdvanceAction::Close → `aida edit <id> --status completed`
```

## Decisions

- **Strict definition: `completed == total` (total > 0).** Matches the preview
  ("N/N children Completed"), lowest false-positive. An epic with a `Rejected`
  child is NOT auto-flagged (operator closes it by hand) — safe-by-omission.
- **Downward-only walk** (`Child` outgoing ∪ `Parent` incoming) so a *sub*-epic's
  rollup reflects only its own descendants, not its parent/siblings. Edges in this
  store are recorded on both endpoints (verified: EPIC-41 carries `Child`, STORY-607
  carries `Parent`), so either orientation finds the children.
- **Detection, never mutation.** No `aida pull` change. `ReadyToClose` is
  `needs_human()` = true (operator confirms). `AdvanceAction::Close` is NOT
  `is_autonomous()` — `--yes` will not auto-close.

## Files (build order)

1. `aida-core/src/graph_walk.rs` — new `child_status_rollup(store, root) -> StatusRollup` + tests.
2. `aida-cli/src/burndown.rs` — `OpenBucket::ReadyToClose` (+key/needs_human), `OpenFacts.epic_rollup`,
   `explain_open` epic branch, `AdvanceAction::Close` (+map/label/sentence), tests.
3. `aida-cli/src/main.rs` — `collect_open_facts` computes `epic_rollup`; add `ReadyToClose` to the
   two `order` arrays (`handle_list_human`, `handle_burndown_explain`); `advance_dispatch` Close arm.

## Critical files

- `aida-cli/src/burndown.rs::explain_open` — the single classifier both views share.
- `aida-cli/src/main.rs::collect_open_facts` — the only place with store access to compute the rollup.

## Tests

- `graph_walk`: `child_status_rollup` over a parent with mixed-endpoint child edges.
- `burndown`: epic with `epic_rollup = Some((n,n))` → `ReadyToClose`; `Some((k,n)) k<n` → `Umbrella`;
  `None` → `Umbrella`. `advance_action(ReadyToClose) == Close`; `Close` not autonomous.

## Verification

```
cargo test -p aida-core graph_walk
cargo test -p aida-cli burndown
cargo build
```

## Followups

- (a) opt-in auto-close on `aida pull` behind `[lifecycle] auto_complete_epics` — deferred per operator choice.
