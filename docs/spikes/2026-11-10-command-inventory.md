# SPIKE-714: command inventory baseline — 2026-11-10

<!-- trace:SPIKE-714 | ai:codex -->

This is a partial research deliverable, not a crate placement or design decision.
It does not establish SPIKE-714's acceptance criteria. The accompanying
`../../scripts/command-inventory.toml` enumerates registrations and preserves
source evidence for follow-up review; every classification remains explicitly
unresolved rather than presenting lexical matches as proven call paths.

## Snapshot and reproduction

Source: `eb1b39c30c9f9f6ce3f5ccf1c5ffd8966e5f8a23`. Run
`python3 scripts/command-inventory.py` from this checkout. It reads the bounded
`command_registry::REGISTRY` initializer, resolves each referenced static (including
nested-module re-exports), extracts the command name and handler, and joins source
file paths against version 2 of `scripts/store-writers.toml`. It emits one row per
registration, not one row per grep hit or subcommand. The output records its source
commit and is intentionally marked `complete = false`.

The arrays of `lexical_crate_roots` are explicit `crate::` tokens anywhere in the
registration's defining file, including types, comments and tests. They are neither
called modules nor a transitive dependency closure. `same_file_writer_candidates`
are exact writer-ledger IDs in that file, not evidence that a particular command
reaches those sinks. Empty arrays do not establish purity. `glob_sites` lists
lower-bound rows: every row with such a site needs expanded name resolution before
its edges can be trusted. The generator deliberately makes no automatic
classification or visibility-widening recommendation.

The host client specified by project instructions,
`/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph`, failed with “No such file
or directory” for `callees requirements_cli::handle_show`.
`aida doctor check codegraph` reported `codegraph,missing`. There was no successful
“no callers” result to confirm. No lane indexing or substitute seat-linked client
was used. Host action needed: deploy the reviewed query client at the supplied
absolute path and make its shared main index accessible. Once available, record
its indexed revision separately from this report's lane revision.

No #3829 commit was found by `git log --all --oneline --grep='#3829' -1` in this
checkout. This is local evidence only, not a claim about forge merge state. Globs
remain present, including `schema.rs:1807`, `help_catalog.rs:753` and
`digest_cmd.rs:175`. The TOML records the full per-row lexical glob inventory.

## Coverage result

The registry contains **167 entries**, all represented once in the table. Of these,
109 defining files have lexical crate glob sites and 58 do not (counts are per
command row, not unique file). Classification counts are 167 unresolved; verified
read-only/core-write/authority totals are unavailable. The spec’s approximate
173 count is not used as the denominator.

## Source-grounded observations

| Command | Observed path | Consequence for the complete audit |
| --- | --- | --- |
| `commands` | `help_catalog.rs:748` → `print_command_catalog` at 138 → catalog built from `command_registry::built_command` at 40 | A plausible small view extraction, but its registration at 765 has stakeholder `Write`. Do not equate that label with a writer sink. It depends on the complete CLI tree. |
| `schema` | `schema.rs:1806` → reflection/catalog rendering; registration at 1852 | Module explicitly documents no mutation. Dependencies include CLI-owned `FindingRow`, `PuntRecord`, `Directive`, `BriefListEntry`, `SessionLease` near lines 37–41, so “pure” does not mean dependency-free. |
| `changelog` | `changelog_cmd.rs:19` → `changelog::run`; `changelog.rs:65–82` selects stdout or `fs_atomic::write_atomic` | `preview` is a view, but `refresh` and `generate --out` write files. The requested three classes do not explicitly distinguish generated output files from substrate mutation. Advisor clarification is needed before assigning the whole registration a class. |
| `statusline` | `statusline_cmd.rs:882` and 946 call `std::fs::write` | The registered command includes configuration writers as well as display behavior. A view-only extraction needs verb-level review. These sites can be cross-checked by exact path/item in the writer ledger. |
| `usage` | `usage_cmd.rs:862` dispatches report helpers; module header says it never writes its input logs | A lexical scan finds fixture writes at 687 and 702. Counting them as production sinks would misclassify this report command. Shared invocation telemetry still needs separate review. |
| help queries | `help_catalog.rs:654` → `record_help_query` → `usage::append_value` at 671 | Apparently informational behavior can append telemetry. This does not prove `commands` reaches this helper; preserve the distinction between shared file membership and reachability. |

`cmdkit.rs` already owns shared registration vocabulary and dispatch types.
That is evidence of a current internal boundary, not evidence that a separate
`aida-cmdkit` leaf crate already exists or that consumers can move unchanged.
`command_registry.rs` owns the complete registry and parser and injects binary-owned
MCP handlers. An MCP transport extraction must preserve that inversion.

## Advisor questions and remaining acceptance work

No first batch of approximately twenty is certified by this baseline. `schema`
and `commands` are concrete candidates for item-level inspection, with blockers
above; they are not approved placements. A defensible ranked batch requires
production call closures, private-item visibility checks and separation of common
pre-dispatch effects from handler effects. Counting lexical crate roots would
reward wrappers that hide large transitive dependencies.

The advisor needs to resolve how to classify generated-file writes and shared
telemetry/cache effects, and whether one mixed top-level registration should take
its strongest class or expose verb-level detail beneath its required row. No
product behavior, helper visibility or crate boundary is changed here.

Follow-up research must:

1. Query the reviewed main index for each handler and validate edges against the
   pinned lane source, expanding globs, re-exports, callbacks, methods and trait calls.
2. Separate tests from production and join actual reachable sink items to the
   writer ledger; cite a writer or gate for every core-write/authority row.
3. Audit authority paths independently of store writers: grants, holds, leases,
   mandates, containment and injected transport callbacks.
4. Enumerate exact private helper/type blockers and count their occurrence across
   commands. Only then rank move stages and offer home-crate alternatives.
5. Supply class counts, the approximately twenty-command first batch, shared
   blocker rankings and the full writer-backed classification table.

The current table supplies coverage and evidence collection only. It must not
be used as a purity proof, authorization inventory, or migration allow-list.
