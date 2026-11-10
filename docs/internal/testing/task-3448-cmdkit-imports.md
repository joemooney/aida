# TASK-3448: consumer import migration

<!-- trace:TASK-3448 | ai:codex -->

The advisor ruling of 2026-11-10 00:15 UTC authorizes a partial import-only
migration with a `Refs TASK-3448` publication. The file lists were fetched at
pickup using `gh pr view N --json files` for PRs #3731, #3744 and #3651.
Every file in those lists is excluded from edits. Compatibility re-exports
in `command_registry.rs` remain unchanged until all consumers migrate.

137 Rust consumer files now name `cmdkit` directly for its framework types
and helpers, including grouped imports, fully qualified references and the
external MCP transport consumer. Rustfmt adjusts layout after paths shorten;
command definitions, handler behavior and help text are unchanged. The module
is public in `aida-cli-lib` so the separate `aida-mcp` crate can import its
public framework types directly. The Python grant-expiry source assertion
now expects the same direct `Handler` path.

The six remaining framework importers are deliberately skipped:

| File under `aida-cli-lib/src/` | Open PR |
| --- | --- |
| `burndown_run.rs` | #3731 |
| `findings_gate.rs` | #3731 |
| `queue_cmd/rework.rs` | #3731 |
| `pr_cmd.rs` | #3744 |
| `merge_hold_cmd.rs` | #3651 |
| `upgrade_release.rs` | #3651 |

All other files touched by those PRs also remain untouched; they have no
framework references eligible for this migration. Parser/tree consumers
continue to use `command_registry` for functions actually defined there.

## Graph evidence

The unchanged TASK-3425 measurement script, with its final measurement calls
replaced by `a=measure('HEAD')`, `b=measure('WORKTREE')` and
`print('leaving base SCC:', ', '.join(sorted(a-b)))`, reports:

| Tree | Production modules | Largest SCC | `cmdkit` in largest SCC |
| --- | ---: | ---: | --- |
| Base `8f434152668380b710e1d3ab9dc76d91ad559e1c` | 595 | 334 | no |
| Working tree | 595 | 334 | no |

No module leaves the largest SCC in this slice. `cmdkit` still has only
`spec_arg` as a named module edge. The six excluded framework importers retain
edges to `command_registry`; its registration table reaches them in return.
Other consumers retain real parser/registry dependencies, for example
`cli::Command` parsing, `help_catalog`'s built tree, `mcp_serve`'s injected
handler, and `lib_part1`'s dispatch. This textual module metric also counts
references in comments and inline tests, as in the original measurement.
The deferred SCC-reduction acceptance is not claimed complete.

The host query executable specified by AGENTS.md was unavailable at
`/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph`;
`aida doctor check codegraph` reported missing tooling. Searches and the
read-only measurement used `rg` and Git, without indexing from the lane.

## Validation

- Launcher job `j20261110T004146133-2-0`: 21 command-registry tests passed,
  including `help_surface_matches_golden_snapshot`. This preceded the module
  visibility change; final rerun `j20261110T004743816-2-0` also passed all
  21 tests, including help golden parity.
- All 137 migrated Rust consumers pass rustfmt. A reconstructed base-to-head
  comparison confirms exactly mechanical path substitutions plus rustfmt and
  the task trace. The public-module declaration is formatted separately.
- Scope audit: all open-PR files are untouched, `command_registry.rs` is
  byte-identical to base, and every remaining framework importer is excluded.
- Python grant-expiry hook suite: 13 passed after updating its source assertion.
- Help co-change guard fixtures and `git diff --check` passed.
- Launcher job `j20261110T004732344-2-0`: MCP transport test passed
  (1 executed test), confirming the public framework path across crates.
- Forty-one distinct fast CI gates passed across batches.
- The fast CI gates were run in batches using the existing runner's workflow
  bodies. This is not a full `make check-ci-fast` pass: the containment gate
  reached an operator-role wait and was interrupted; the two compilation
  gates were omitted from the batches under the lane Cargo policy. Formatting
  was checked directly; the host must run the committed-history help guard
  after committing the required `Help-Golden: unchanged` trailer. Mail-wake
  fixtures initially refused shared-write directories and passed with umask
  077. No production change was made for that environment issue.
