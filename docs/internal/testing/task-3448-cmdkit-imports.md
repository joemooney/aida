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

## PR #3867 round-2 publication correction

<!-- trace:TASK-3448 | ai:codex -->

Review of `411bbca35a3e` identified a missing commit-body declaration, not a
help-surface change. The framework import migration and registry re-export
removal change no command definitions, flags or help strings. The review reports
that all six test lanes passed, including `help_surface_matches_golden_snapshot`;
the `lane_fixture_fails` panic is an expected negative fixture. The failing
CI gate is the help snapshot co-change guard.

This round changes only this evidence note. The native publication declaration
supplies a UTF-8 commit body containing the standalone line
`Help-Golden: unchanged`. The host must preserve that body in the new commit
and eventual squash, retain `Refs TASK-3448` and the partial `Refs TASK-3428`
scope, and obtain fresh passing co-change and required CI checks at the
published head. The unchanged snapshot is not regenerated.

Local verification: `bash tests/test_check_help_golden_cochange.sh` passes.
The actual guard against `origin/main` still exits 1 at the existing committed
head, as expected until the host creates the new commit with the declaration.
The full fast gate is not established locally: its first TUI seam gate invokes
Cargo directly despite the fast-tier description; the bounded attempt cannot
replace launcher-run Cargo validation or fresh published-head CI.

The advisor's recorded measurement remains 610 modules / largest SCC 367
before and after the import migration. This round does not re-measure or claim
SCC reduction or spec completion. Moving the registry test helpers is a later
slice, sequenced after #3867 lands; its measurement must retain inline-test edges.
