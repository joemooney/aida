# SPIKE-714 round 5: unresolved advisor prerequisite — 2026-11-11

<!-- trace:SPIKE-714 | ai:codex -->

Disposition: explicit punt, not completion. Source inspected:
`0a32b51bdbb0be085c9999c5064a839bf22c0b05`. Review addressed:
`a84ba6cee99c`, recorded 2026-11-11T07:44:43Z.

## Findings and disputed prerequisite

1. **Finding 1: accept the audit deficit; dispute proceeding without its
   expressly required advisor ruling.** The table still has 167 unresolved
   classifications and one shared evidence placeholder. Same-file writer
   candidates do not establish reachable production sinks. No per-class totals
   or complete writer/gate reachability audit has been supplied.
2. **Finding 2: accepted, incomplete.** The dependency/visibility/stage/home
   matrix and global blocker ranking remain absent. Neither a policy decision
   nor unavailable graph tooling proves that source-only dependency research
   is impossible. This punt does not claim to have addressed this finding.
3. **Finding 3: accept the missing batch; dispute certification before the
   advisor resolves the effect boundary, as the finding itself requires.**
   No approximately twenty-command dependency-ranked batch is certified here.

The research contract prohibits making the product/design decision in this
report. Required advisor decisions remain: whether a class covers the handler
and descendants or the complete CLI invocation; how direct CLI-owned writes
fit the named classes; and how mixed families aggregate effects. Returning the
task for rework does not specify answers to these three questions. This is a
dispute about proceeding past that prerequisite, not about the validity of AC1–3.

## Rechecked evidence and limits

`aida show SPIKE-714 --full --comments --no-git` returned eight comments,
including round five's findings. None of those visible comments records the
requested ruling. The output reports stale cache context, so this observation
does not establish absence of a ruling in all advisor channels.

Source searches reconfirmed `schema.rs:1862` registers `Handler::Early(run_schema)`,
`lib_part1.rs:2079` calls `presence::auto_flip_if_interactive`, and
`lib_part1.rs:389` calls `usage::append_event`. These corroborate the previously
documented boundary question; this round does not assert a new transitive
effect audit. See the round-three report for its bounded production witnesses.

The prescribed absolute client
`/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph` failed an executable
availability check. `aida doctor check codegraph` reported `codegraph,missing`.
No substitute client was derived from the seat link and no index was changed.
The host must deploy the reviewed client or supply a replacement reviewed
absolute path and indexed main revision to satisfy the requested method.

## Validation and handoff

Reproduce the deficit check by loading `scripts/command-inventory.toml` with
Python `tomllib`, counting `command` rows, classifications, and distinct
`classification_evidence` values: 167 rows, 167 unresolved, one evidence value.
`python3 -m unittest discover -s tests -p test_command_inventory.py` passed
all four tests. These validate extraction behavior, not acceptance completion.
No production code changed and no Cargo test was run.

Only this new report is left uncommitted for the host under the native-seat
contract. An explicit `aida punt` names findings 1 and 3's prerequisite dispute;
the publication veto remains in force. No commit, push, PR, completion, or ship
declaration is requested from this seat. Resume after an advisor records the
three boundary decisions and the host supplies the reviewed query client;
the full 167-row audit, global blocker ranking, and first batch still remain.
