# SPIKE-714 round 6: explicit decision-gate punt — 2026-11-11

<!-- trace:SPIKE-714 | ai:codex -->

Source inspected: `80d9ee24449bfc5962ca1b4bf92e28bef442c686`.
Review: `80d9ee24449b`, recorded 2026-11-11T08:44:33.149898938Z.
Disposition: explicit punt; AC1–3 remain incomplete. This report makes no
classification, placement, visibility-change or extraction decision.

## Finding-by-finding disposition

1. **Finding 1: dispute proceeding past the expressly required advisor ruling.**
   Accept the substantive deficit: all 167 rows remain unresolved, with one
   shared classification-evidence value. Neither the earlier ledger repair nor
   same-file writer candidates supplies production reachability. The advisor
   must define handler versus invocation effects, direct CLI-owned writes under
   the named taxonomy, and mixed-family aggregation before certification.
2. **Finding 2: accepted, still incomplete.** The complete called-module,
   visibility, stage and proposed-home matrix and global blocker ranking have
   not been supplied. The missing client and effect decision do not establish
   that independent source-only dependency research is impossible. This report
   supplies no substitute matrix and requests no acceptance credit for it.
3. **Finding 3: dispute certification before the expressly required effect-boundary
   ruling.** Accept that the approximately twenty-command ranked batch and its
   per-command blocking helpers are missing. No batch is certified here.

## Fresh evidence and reproducible checks

`aida show SPIKE-714 --full --comments --no-git` exposes twelve comments. The
latest pair returns the task to Approved without a triage reason and repeats
the round-six findings. None of these visible comments records the three
requested boundary decisions. The CLI labels its cache stale, so this does not
prove absence of a ruling in other channels or canonical advisor responses.

At the pinned source, `aida-cli-lib/src/schema.rs:1862` registers
`Handler::Early(run_schema)`. The invocation wrapper still calls
`usage::append_event` at `lib_part1.rs:389` and
`presence::auto_flip_if_interactive` at `lib_part1.rs:2079`. Reproduce these
observations with `rg -n 'Handler::Early\(run_schema\)'` in schema.rs and
`rg -n 'auto_flip_if_interactive|usage::append_event'` in lib_part1.rs.
These are bounded wrapper observations, not a new transitive audit of schema
or any other command; the round-three report contains the earlier sink witnesses.

Loading `scripts/command-inventory.toml` with Python `tomllib` and counting
`command`, `classification`, and distinct `classification_evidence` values
returns 167 rows, 167 unresolved classifications, and one evidence value.
`python3 -m unittest discover -s tests -p test_command_inventory.py` passes
four tests. These validate extraction only; no Rust changes or Cargo tests.

`test -x /home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph` fails;
`aida doctor check codegraph` reports `codegraph,missing`. No replacement
executable was derived from the seat link and no index was changed.

## Advisor and host handoff

Record explicit answers to the three effect questions above, and deploy the
reviewed query client or supply a reviewed replacement absolute path and its
indexed main revision. Returning this task for rework alone does not record
those answers. The full audit, matrix, blocker ranking and first batch still
need research after recovery; this is not a completion declaration.

The native-seat instruction requires uncommitted edits and forbids commit,
push, PR creation and queue completion here. Only this new report is handed
off. An explicit `aida punt` names findings 1 and 3's prerequisite dispute;
the resulting publication veto must be respected. No ship declaration is made.
