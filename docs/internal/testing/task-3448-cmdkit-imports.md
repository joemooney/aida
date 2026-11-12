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

## Remaining injected-helper callers, bounded slice (2026-11-12)

<!-- trace:TASK-3448 | ai:codex -->

The binding 14:35 UTC advisor brief follows PR #3878 at
`a447d559aaa5a5634409cb5138dc58a95ab36d70`. This slice uses the existing
injected `crate::TEST_CLI` methods; it adds no helper or production behavior.
It replaces 159 `try_parse_from` and 16 `built_command` references across
39 Rust files: all seven remaining nested `gate`/`regen` callers and the
32 non-story callers under `src/tests/`. Including this evidence file, the
publication changes exactly 40 files.

The remaining test callers exceed the file budget even within `src/tests/`,
so that directory is split by filename group: its 19 `story*` callers remain
for the next slice. The registry-owned pristine-tree and command-registry
suites also retain unqualified calls inherited through `use super::*`; a
follow-up should distinguish their parser/tree calls from genuine registry
inspection. Nine top-level files still use production registry entry points;
these must not be changed to test-only helpers.

### SCC evidence

The exact Python measurement function in `task-3425-cmdkit-rework.md` was run
with only its invocation footer changed to compare `measure('HEAD')` and
`measure('WORKTREE')`, then print the base SCC members absent from the latter.
Inline-test edges and the original filters remain intact.

| Tree | Modules | Largest SCC | `cmdkit` in largest SCC |
| --- | ---: | ---: | --- |
| Base `a447d559aaa5a5634409cb5138dc58a95ab36d70` | 610 | 367 | no |
| This working tree | 610 | 367 | no |

No base SCC member leaves. The original metric excludes files under a `tests`
directory, so most changes in this slice cannot affect that metric. The two
`regen` inline-test files do remove that owner's direct registry helper edge,
but it still reaches `cli`, which reaches `agent_launch`, which still builds
the production registry tree. Other production registry users remain as
recorded in the preceding slice. `cmdkit` retains only its `spec_arg` edge.
The SCC acceptance remains unmet; use `Refs TASK-3448`.

### Validation and publication

- Mechanical reconstruction from base, applying only the two helper-path
  replacements and rustfmt, reproduces every changed Rust file exactly.
- `git diff --check` passes. Changed sources were formatted with
  `rustfmt --edition 2021 --config skip_children=true`.
- Launcher check job `j20261112T144242514-2-0` passed (exit 0):
  `cargo check --profile agent -p aida-cli-lib --tests` finished in 5m 28s.
- Changed-file rustfmt checks and help co-change guard fixtures passed.
- No local Rust test run is requested by the newer brief: CI shards remain
  the test gate. **untested=true** until executed-test evidence exists.
- The prescribed host codegraph client is absent; `aida doctor check codegraph`
  reports an unreachable index. Navigation used source searches without
  initializing or changing any index.

This native seat leaves the edits uncommitted. The host publication must keep
`Refs TASK-3448` in the subject and the standalone `Help-Golden: unchanged`
line in the commit body and eventual squash message. The command definitions
and help snapshot are unchanged.

Changed Rust files (relative to `aida-cli-lib/src/`):

```text
gate/review_publication/tests/admission_tests.rs
gate/tests/bug_2011_grant_expiry_verdict_tests.rs
gate/tests/story_1687_rework_release_tests.rs
gate/tests/story_1700_drive_mandate_tests.rs
gate/tests/story_1887_pr_ship_batch_tests.rs
regen/consensus.rs
regen/score/controls.rs
tests/agent_launcher_tests.rs
tests/bug_015_spec_arg_tests.rs
tests/bug_1609_gitlab_reviewer_preflight_tests.rs
tests/bug_1631_history_spec_id_tests.rs
tests/bug_1701_drain_routing_tests.rs
tests/bug_1983_grant_verdict_tests.rs
tests/bug_2002_launch_lease_tests.rs
tests/bug_2017_audit_ids_tests.rs
tests/bug_2166_contained_parent_escape_tests.rs
tests/bug_2385_vendor_routing_tests.rs
tests/bug_2467_delegator_launch_tests.rs
tests/bug_home_042_stacked_ship_tests.rs
tests/dev_build_tests.rs
tests/doctor_cmd_story_462_doctor_tests.rs
tests/help_catalog_tests.rs
tests/mcp_tests.rs
tests/pr_ship_tests.rs
tests/push_pull_scope_tests.rs
tests/queue_work_tests.rs
tests/search_output_flags_tests.rs
tests/statusline_tests.rs
tests/task_1063_prune_verb_crossref_tests.rs
tests/task_1472_bare_aida_picker_tests.rs
tests/task_1491_schedule_driver_tests.rs
tests/task_1512_history_transition_filters_tests.rs
tests/task_1562_worktree_reclaim_tests.rs
tests/task_1715_pr_review_alias_tests.rs
tests/task_1737_authority_trailers_tests.rs
tests/task_2205_list_fields_tests.rs
tests/task_492_brief_tests.rs
tests/time_bound_flags_tests.rs
tests/zen_front_door_tests.rs
```
