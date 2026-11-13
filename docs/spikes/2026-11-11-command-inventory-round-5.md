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

## Round-five retry at ddaf14c06e52 — CI repair and renewed disposition

This retry inspected `ddaf14c06e52e0bfcf4adb2c8bebd10727f815b1` on
2026-11-11. The visible spec now has ten comments. Comment 9 records the
previous round-five punt and return to Approved without a triage reason; it
does not supply the requested effect-boundary ruling. The CLI still reports
stale cache context. An attempted canonical `read_punt` MCP read was refused
because that tool requires approval and this seat's approval policy is never.
Consequently this report cannot establish whether a ruling exists elsewhere.
The prescribed absolute codegraph client remains unavailable; doctor again
reports `codegraph,missing`.

There is a newly verified, independently repairable CI defect. Reading failed
logs for run 42982877272 with `gh run view --log-failed` showed the store-writer
ratchet refusing exactly `scripts/command-inventory.py|generate|python:write_text`.
The aggregate Build failure follows the failed gates lane. The source at
`scripts/command-inventory.py:108` writes only the generated checkout artifact
`scripts/command-inventory.toml`; it is not a reachable registered-command sink.
`generate` is called by this script's `main`, not by REGISTRY dispatch.

Running `python3 scripts/check-store-writers.py --regenerate` added exactly that
one site to `scripts/store-writers.toml`, retaining the conservative blocked
metadata used for adjacent offline Python generators. This ledger maintenance
makes no command-class, crate-placement or runtime-authority decision. It also
does not convert same-file candidates into reachability evidence.

Findings 1 and 3 are explicitly disputed only as instructions to certify classes
or the batch before their expressly required advisor ruling. The audit deficits
in both findings are accepted. Finding 2 is accepted and remains incomplete:
the full matrix and global helper ranking have not been delivered. The current
TOML check still counts 167 unresolved rows and one evidence value. AC1–3 remain
unmet; this retry supplies a CI repair, not audit completion.

The four extraction tests pass again. The regenerated ledger contains 3500 sink
sites and changes one row. No Rust code changed or Cargo test ran. Both this
report and the ledger change are left uncommitted for the host. A renewed punt
records the decision fork; no publication declaration is made while that veto
is active. Required recovery remains an advisor-recorded ruling on handler
versus invocation scope, direct CLI-owned writes and mixed-family aggregation,
plus the host-deployed reviewed query client and indexed main revision.
