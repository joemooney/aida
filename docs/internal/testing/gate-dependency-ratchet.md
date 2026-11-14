# Gate downward dependency ratchet

<!-- trace:TASK-3785 | ai:codex -->

`python3 scripts/check-gate-dependencies.py` checks the exact upward item-edge
inventory in `scripts/gate-dependencies-legacy.json`. The Build job's gates shard
runs it with its PR/merge-group/push base revision, and classifying that step as
`fast` also includes it in `make check-ci-fast`.

The initial ceiling is **150**, measured from the current tree rather than the
older deep-dive estimate of 144. An edge is a unique `(source Rust file, target
module, first item)` under `aida-cli-lib/src/gate/**/*.rs`, including test files
and inline tests. Enum variants and associated methods belong to their first
item. Module-only imports do not add an item edge. The watched modules are
`pr_cmd`, `session`, `seat_clone`, `queue_cmd`, `auto_complete`, `awaiting_you`,
`orchestrator`, `pr_ship`, `pr_train`, `drain_lock`, `drain_state`, `store_sync`
and `merge_hold`.

The scanner reuses the ExecContext Rust tokenizer and grouped-use parser. It
ignores comments and literals, expands import aliases, counts item imports and
qualified references, and refuses crate-root and watched-module glob imports.
Current-crate aliases declared with `extern crate self as root` resolve to
`crate`, including references and imports through the alias.
<!-- trace:BUG-2567 | ai:codex -->
Ancestor-only module imports such as `use super::super as root` and grouped
`use super::super::{self as root}` also refuse: the scanner has no source-module
context to retain those bindings safely. Use an explicit `crate::` path instead.
Alias bindings are conservatively unioned across a file, so shadowing can
increase the count. This is a source ratchet, not Rust name resolution: macro
expansions, root re-exports under unrelated names, generated/out-of-tree sources
and arbitrary value-flow indirection are not resolved. New Rust files under the
gate directory are scanned before staging as well as after it.

For a burn-down, inspect the changed edges, then run:

```sh
python3 scripts/check-gate-dependencies.py --write-ledger --base origin/main
python3 -m unittest tests/test_gate_dependency_ratchet.py
```

The proposed ledger must match the source exactly. Relative to the base ledger,
the ceiling can only decrease or stay equal, and new target item identities
refuse even when other items retire. Relocation within gate can retain an item.
The writer checks the base before replacing the ledger. Only the initial
introduction may bootstrap when the verified base commit has no ledger. Local
checks without `--base` verify the working tree; CI supplies the growth check.

Burn-down follow-ups should move authority decisions into gate or introduce
ports owned by gate, with command modules implementing the effects. They must
retain the existing authority and containment boundaries and run their targeted
Rust tests. The ratchet grants no new authority.

## Follow-up filing handoff

The implementation seat attempted to file these three draft children of
TASK-3785, blocked by TASK-3785 so they start only after the ratchet lands.
All three writes refused before creating an object: the host's
`/home/dirtlab/ai/aida/.git/aida-work-dispenser.toml` has mode `0664`, while
allocation requires no group/other write permission. The host must repair that
metadata outside the native seat and file the children. No child IDs exist yet.

1. **Gate burn-down: invert ship and train orchestration dependencies.** Move
   ship, approval and freshness authority helpers into gate or introduce
   gate-owned traits implemented by `pr_cmd`, `pr_ship` and `pr_train`. Preserve
   forge observations, CI freshness, approvals and publication behavior; run
   targeted ship, approval and train regressions.
2. **Gate burn-down: isolate merge-hold and store-sync effects behind gate
   ports.** Move authority functions into gate or introduce gate-owned traits
   for `merge_hold` and `store_sync`. Preserve pinned discovery, bounded reads,
   clearance and recusal; run hold, ledger and clearance regressions.
3. **Gate burn-down: invert queue, drain and session command dependencies.**
   Move authority functions or introduce gate-owned ports for `queue_cmd`,
   `auto_complete`, `awaiting_you`, `orchestrator`, `drain_lock`, `drain_state`,
   `session` and `seat_clone`. Preserve grants, occupancy, child containment and
   phase ownership; run queue, drain, session and seat-authority regressions.

Each follow-up must prune the ledger and coordinate authority-boundary changes
with STORY-2325 before implementation. These are proposed PR scopes for triage,
not authorization to change those boundaries.
