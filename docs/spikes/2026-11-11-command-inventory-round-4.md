# SPIKE-714 round 4: explicit decision-gate punt — 2026-11-11

<!-- trace:SPIKE-714 | ai:codex -->

SPIKE-714 remains incomplete. This report records a punt, not acceptance or
extraction authorization. No classification policy or product decision is made.

Source inspected: `4b53019754189c9d64c281baa6bc776debcabd61`.
Review under response: `4ec84acc04df`, recorded 2026-11-11T07:13:39Z.
Method: read the current full spec and its six comments with `aida show
SPIKE-714 --full --comments --no-git`, inspect the inventory generator and prior
report, parse the TOML inventory, re-run the extraction tests, and check the
operator-specified host query client. The spec output labels its cache stale;
therefore absence of a ruling in that output is not proof that no ruling exists
elsewhere. No ruling was supplied in this round's authoritative instructions.

## Disposition of the three findings

1. **Accepted audit deficit; disputed only as an instruction to finalize classes
   without the required advisor ruling.** There are 167 unresolved classes and
   one shared placeholder evidence value. Same-file candidates are not reachable
   sinks. The finding itself requires resolving handler-versus-invocation and
   direct-writer treatment with the advisor first. Neither this report nor the
   previous partial source paths supplies that ruling. AC1/AC2 remain unmet.
2. **Accepted without dispute.** The full called-module, visibility, stage and
   home-alternative matrix and global blocker ranking are absent. Missing graph
   tooling does not establish that this work is impossible using source reads;
   it prevents claiming compliance with the required navigation method. No
   lexical result has been promoted into verified dependency evidence.
3. **Accepted missing deliverable; disputed certification before the advisor's
   effect-boundary ruling.** The finding explicitly requires that ruling before
   certifying candidates. The three conditional inspections are not the required
   approximately twenty-command batch. AC3 remains unmet. This is a sequencing
   dispute, not a claim that the acceptance criterion is invalid or satisfied.

The research contract says to surface decisions for the advisor. Repeating the
requested outcome in a rework comment does not select between the documented
alternatives. The round-three report's production witnesses remain relevant:
`lib_part1.rs:2079` invokes presence handling before early dispatch, while
`lib_part1.rs:389` appends usage after successful return. `schema.rs:1862`
registers the early schema handler. Those current sites were rechecked with
`rg`; this round does not claim a fresh transitive audit of their descendants.
See [round-three evidence](2026-11-11-command-inventory-round-3.md) for the exact
writer-ledger witnesses and their conditions.

## Concrete recovery inputs

The advisor needs to record whether the class describes the registered handler
and descendants, or the whole CLI invocation including common hooks; how direct
CLI-owned writes fit the three named classes; and the aggregation rule for
mixed read/write/authority command families. These are requests for decisions,
not recommended or adopted answers. Home-crate alternatives can be researched
without adopting their placement.

The host needs to deploy the reviewed query client at the supplied absolute path
`/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph`, or supply a replacement
reviewed absolute path and indexed main revision. This path is still absent;
`aida doctor check codegraph` returns a `codegraph,missing` finding. No client was
inferred from the seat link and no index was initialized or refreshed.

After those inputs, the remaining deliverable is still the entire audit:
production call paths and exact writer/gate witnesses per applicable row,
actual external modules and helper/type blockers for all 167 registrations,
per-class totals, global blocker ranking, and a named dependency-ranked batch
of approximately twenty views. No acceptance credit is claimed for this report.

## Validation and native-seat handoff

All four existing `test_command_inventory.py` tests passed. Parsing the current
TOML confirmed 167 rows, all `unresolved`, with one distinct classification
 evidence value. These checks verify the deficit and parser regressions, not
completion. No Cargo test is needed for this documentation-only change.

Only this dated report is changed. It is left uncommitted under the native-seat
contract. The explicit punt names findings 1 and 3's decision sequencing dispute;
finding 2 remains accepted outstanding work. The punt is a publication veto:
no commit, push, PR creation, ship declaration, review record or queue completion
is performed by this seat.
