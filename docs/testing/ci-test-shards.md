# CI test lanes

<!-- trace:TASK-1949 | ai:codex -->

The `build` matrix runs six Ubuntu lanes with the same `ci-dev-profile`
Rust dependency cache. The gates lane saves the cache; other lanes restore it.
`build-result` retains the required `Build (ubuntu-latest)` check and runs even
when a lane fails or is cancelled. It succeeds only when the whole matrix does.
The separate merge-hold workflow is unchanged.

- `lib`: `aida-cli-lib` library unit tests.
- `cli-0` and `cli-1`: Cargo metadata supplies every CLI integration target.
  Targets sort by decreasing measured main-run duration, then name, and alternate
  between halves. `scripts/test-time-budget.toml` records those measurements in
  `shard_seconds`, separately from enforcement budgets. New targets join with
  weight zero; no target allowlist needs updating.
- `rest`: remaining workspace packages, CLI binary units, CLI-library doctests,
  and any CLI-library integration targets. The remaining workspace selection
  explicitly retains the core feature union formerly enabled by the CLI members.
- `gates`: static/shell guards, feature-disabled and TUI checks, protobuf
  regeneration, the live bwrap witness, clippy, and provenance/doc-intent gates.
- `contracts`: checks that use the built CLI, including MCP/history contracts,
  the CLI manual, monitor/templates/glossary, and distributed-store fixtures.
  Both gate lanes build the workspace. The baseline's non-suite steps total
  1,001 seconds, so splitting tests while leaving a serial gate lane would
  exceed the 12-minute target.

Docs-only changes skip the test execution and expensive gates; the existing
unconditional documentation guards still execute in their respective gate lanes. Every test
lane uploads its own log and timing JSON, enforces the same per-binary budget,
and checks untracked root entries even after a test failure. Ignored build and
runtime files remain allowed.

Both store-attach guards compare Git's discovered top-level directory with
`$PWD/.aida-store` before resetting the store. A plain directory inside the
checkout fails rather than inheriting the enclosing checkout's Git discovery.

`make check-ci-fast` still reads workflow step bodies. `make check-ci-compile`
uses the shard planner's `--compile --profile agent` mode to compile the whole
workspace without running tests. An explicit full local check selects all lanes
on the agent profile; it is not a pre-push requirement.

For coverage comparisons, run `scripts/test-timings.py` separately on each test
lane's raw log and add `passed + ignored` across their JSON rows. For an older
unsharded main run, use `gh run view RUN --log` and pass `--step 'Run tests'`.
Do not concatenate interleaved multi-job logs before parsing them.

Baseline: main `128a5bb6eb`, run `39348896342`, 85 binaries and **11,721 passed +
ignored** tests. PR timing and coverage evidence belongs in the PR body; the
12-minute target requires an actual completed PR run.
