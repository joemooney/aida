# CI test lanes

<!-- trace:TASK-1949 | ai:codex -->
<!-- trace:TASK-1979 | ai:claude -->
<!-- trace:TASK-1988 | ai:claude -->

The `build` matrix runs seven Ubuntu lanes on the `ci-dev-profile` Rust
dependency cache. Gates and contracts share the base cache, which gates saves.
Each test-lane group (`lib`, `cli`, `rest`) saves and restores its own cache
under a suffixed shared key (`ci-dev-profile-lib`, and so on), because the test lanes compile test binaries and dev-dependencies that
gates never builds.
`build-result` retains the required `Build (ubuntu-latest)` check and runs even
when a lane fails or is cancelled. It succeeds only when the whole matrix does.
The separate merge-hold workflow is unchanged.

- `lib-0` and `lib-1`: `aida-cli-lib` library unit tests, which form one test
  binary. Each lane lists the binary's tests and runs half of the top-level
  modules as libtest name filters. Filters match substrings, so modules whose
  `<module>::` filter also matches another module's tests share a half.
  Groups are balanced by measured time, largest first: `[lib_module_seconds]`
  in `scripts/test-time-budget.toml` records each module's summed test
  seconds, derived from timestamped lane logs with
  `scripts/ci-test-shards.py --lib-weights`. A module without an entry weighs
  its test count times the mean per test. A lane fails if the halves overlap
  or leave a listed test out.
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

Before running anything, every test lane checks the shard union: the planned
commands must select every target `cargo test --workspace` runs (each
test-enabled lib, bin, integration, example, or bench target, and each
doctest), and each exactly once (the two lib halves count as one). A target
that no lane selects, or that two lanes select, fails the lane and is named.

Gates and contracts set `AIDA_CI_PREBUILT_CLI=1`: their shell contract tests
use the workspace binary those lanes already built instead of running
`cargo build -p aida-cli`, whose different feature set recompiled the CLI.

The test lanes run their selections under cargo-nextest
(`scripts/ci-test-shards.py --runner nextest`, the `ci` profile in
`.config/nextest.toml`): every test in its own process, no fail-fast, no
retries, a `serial` test-group for tests that guard a resource outside the
process, and a 180-second hang guard. Doctests stay on `cargo test`, which
nextest cannot run. Before the swap, a non-gating shadow lane ran nextest
beside cargo test; on main runs 39455014512, 39475194172 and 39476669725 each
lane ran the same tests, by name, under both runners.

Docs-only changes skip the test execution and expensive gates; the existing
unconditional documentation guards still execute in their respective gate lanes. Every test
lane uploads its log, its nextest JUnit reports and its timing JSON, enforces
the per-test budget in `scripts/test-time-budget.toml` (read from the JUnit
reports; doctests per crate, from the log), and checks untracked root entries
even after a test failure. Ignored build and runtime files remain allowed.

Both store-attach guards compare Git's discovered top-level directory with
`$PWD/.aida-store` before resetting the store. A plain directory inside the
checkout fails rather than inheriting the enclosing checkout's Git discovery.

`make check-ci-fast` still reads workflow step bodies. `make check-ci-compile`
uses the shard planner's `--compile --profile agent` mode to compile the whole
workspace without running tests. An explicit full local check selects all lanes
on the agent profile, under the same runner (`make test`, or one lane with
`make test-lane LANE=cli-0`); without cargo-nextest installed it falls back
to `cargo test` with a note. It is not a pre-push requirement.

For coverage comparisons, run `scripts/ci-test-shards.py --totals` on each test
lane's raw log (the job summary prints the same line) and add
`passed + ignored` across the lanes. It reads both runners' output, so a
nextest lane compares with an older cargo-test lane by count. For an older
unsharded main run, use `gh run view RUN --log`.
Do not concatenate interleaved multi-job logs before parsing them.

Baseline: main `6657ffff6d`, run `39476669725`, **11,799 passed + ignored**
tests (lib-0 3,887, lib-1 5,404, cli-0 188, cli-1 143, rest 2,177). PR timing and coverage evidence belongs in the PR body; the
12-minute target requires an actual completed PR run.
