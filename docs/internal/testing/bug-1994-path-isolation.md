# PATH isolation in Rust tests

The shared environment mutex serializes cooperating writers. It cannot protect
production `Command::new("git")` readers in other test threads. A forwarding
shim is still a foreign executable selected by those readers, and its fixture
can disappear while an unrelated child is resolving it.

A test that changes PATH starts with:

```rust
if crate::test_env::run_path_isolated_test() {
    return;
}
```

Call this before any fixture or environment guard. The parent re-executes the
current test binary with the exact test name and one test thread, waits for it,
and verifies that one test passed. The child gets its own HOME and Git identity.
Every PATH setter, unsetter, reset, prepend and restoration snapshot in the
shared guards requires this process boundary. Other environment keys retain the
existing mutex and restoration behavior. Production startup is unchanged.

The original PATH-changing assertions still run in the child. The live Git
probe keeps a fake Git active until the parent finishes an unlocked real Git
lookup, and retains the shim through child exit. The source guard parses Rust
calls, aliases and macro tokens rather than exempting whole files; a checked
setter does not authorize another function.

`history_and_path_mutators_pass_15_rounds_at_16_threads` runs the history-cache
suite alongside every migrated PATH-changing test in one 16-thread harness per
round. It lists members first, checks that every writer is included, and checks
completed pass counts for all 15 rounds. The stress takes about 18 minutes,
so it is `#[ignore]`d in the default suite and does not run in per-PR CI (the
Build job keeps its 40-minute bound). It runs as the **PATH and history
concurrency stress** job in `.github/workflows/weekly-linux.yml`, on that
workflow's weekly schedule or on manual dispatch:

```bash
gh workflow run weekly-linux.yml
```

That dispatch runs every job in the workflow. To run only the stress locally:

```bash
cargo test -p aida-cli-lib --lib \
  test_env::bug_1994_path_isolation_tests::history_and_path_mutators_pass_15_rounds_at_16_threads \
  -- --ignored --exact --nocapture < /dev/null
```

It retains all 15 rounds and 16 threads. A 40-minute step timeout and a
60-minute job timeout bound it. Rounds have no load-sensitive deadline, and any
assertion failure fails the scheduled run. The guardrail test
`stress_runs_only_in_its_bounded_scheduled_job` pins the `#[ignore]`, the
scheduled job's command and bounds, and its absence from `ci.yml`.

The earlier BUG-1955 investigation retracted the alleged foreign-shim strace
proof: those exec entries were ENOENT search probes. The original Git
`unable to create temporary file` / `failed to write commit object` failure
remains unconfirmed. PATH isolation is a concrete fix for shared executable
resolution; a passing stress run must not be represented as proof of that
separate original mechanism.

<!-- trace:BUG-1994 | ai:codex -->
