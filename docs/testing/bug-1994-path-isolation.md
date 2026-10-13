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
completed pass counts for all 15 rounds. The normal GitHub workspace test step
runs this stress test; no separate compile is needed.

The earlier BUG-1955 investigation retracted the alleged foreign-shim strace
proof: those exec entries were ENOENT search probes. The original Git
`unable to create temporary file` / `failed to write commit object` failure
remains unconfirmed. PATH isolation is a concrete fix for shared executable
resolution; a passing stress run must not be represented as proof of that
separate original mechanism.

<!-- trace:BUG-1994 | ai:codex -->
