# Test isolation — process-global state under `cargo test`

How AIDA tests stay reliable when cargo runs them in parallel within a
single process. Read this before adding a test that touches env vars,
the current working directory, the process umask, or any other
process-global state.

## The failure mode

Cargo runs tests within a binary in parallel by default (one thread per
CPU). The Rust standard library executes every test in the same OS
process — there is no fork-per-test isolation. So a test that calls
`std::env::set_var("FOO", "x")` mutates the same `FOO` that every
sibling test reads. Two tests touching the same key without
coordination produce a classic data race: their results depend on
scheduling, the test-runner emits intermittent NotFound / unexpected-
value failures, and "cargo test passes" stops being a load-bearing
signal.

BUG-371 was an empirical instance of this. STORY-436's agent-launcher
tests passed a process-global env var to a fake-agent subprocess; when
STORY-439's calibration tests landed alongside and ran in the same
parallel batch, the two shared the var and the launcher tests started
failing with NotFound on a file-existence unwrap. Both tests were
"correct" in isolation; their parallelism was the bug.

## Two anti-patterns, two fix shapes

### Anti-pattern A — process-global env-var mutation

A test calls `std::env::set_var("AIDA_FOO", "x")` (or `remove_var`)
without serialising against sibling tests, and the code under test
reads `AIDA_FOO` directly.

The fix: route every such mutation through `crate::test_env::EnvVarGuard`
(in `aida-cli/src/test_env.rs`). The guard takes a single global mutex
for its lifetime, snapshots the prior value at construction, and
restores it on drop. Sibling tests using the same guard serialise on
the same lock; the prior value is restored whether the test passes,
fails, or panics.

```rust
// Bad — racy under --test-threads >1:
std::env::set_var("AIDA_TELEMETRY", "0");
assert!(!is_enabled(None));

// Good — serialised via the shared ENV_LOCK:
let _guard = crate::test_env::EnvVarGuard::set("AIDA_TELEMETRY", "0");
assert!(!is_enabled(None));
// `_guard` drops at scope end and restores the prior value.
```

When the test iterates over many values for the same key, hold the
guard across the loop and call `reset` / `reset_unset` to swap without
releasing the lock:

```rust
let mut guard = crate::test_env::EnvVarGuard::unset("AIDA_AUTO_BUMP");
assert!(auto_bump_enabled());                // unset → on
for off in &["false", "0", "no", "off"] {
    guard.reset(off);
    assert!(!auto_bump_enabled());
}
```

`EnvVarGuard::set` / `unset` / `reset` is the canonical pattern. New
tests should reach for it before coining another module-local mutex.

A handful of pre-existing helpers in the codebase (`with_env_vars` in
`advisor.rs`, `with_bg_fetch_env` and `scoped_prepend_path` in
`main.rs`, the `LOCK` in `workflow_hints.rs`) implement the same idea
with module-local mutexes. They're functionally equivalent — each
guards a distinct key, so they don't race with each other or with
`ENV_LOCK` — and pre-date `EnvVarGuard`. There's no value in churning
them; they stay as-is and the shared guard is the path forward for
anything new.

### Anti-pattern B — env var passed to a subprocess

A test generates a script, sets a process-global env var, and spawns a
subprocess that reads the var. Two parallel tests that set the same
var both spawn subprocesses; whichever runs second sees the other's
value.

The fix (Codex's BUG-371 pattern): don't pass the value through the
process env at all. Generate per-test temp paths via `TempDir` and bake
them into the generated script as literal values, or pass them via
`Command::env("KEY", value)` — which scopes the env var to the spawned
child only, not the parent process.

```rust
// Bad — pollutes the parent process's env:
std::env::set_var("AIDA_AGENT_CONTEXT_FILE", &ctx_path);
Command::new(&fake_agent).status().unwrap();

// Good — per-child env, no parent mutation:
Command::new(&fake_agent)
    .env("AIDA_AGENT_CONTEXT_FILE", &ctx_path)
    .status()
    .unwrap();
```

```rust
// Bad — fake script reads $OUTFILE from a global env var:
std::env::set_var("OUTFILE", out_path);
std::fs::write(&fake_agent, "#!/bin/sh\nenv > $OUTFILE\n").unwrap();

// Good — out_path is baked into the script body as a literal:
let script = format!("#!/bin/sh\nenv > '{}'\n", out_path.display());
std::fs::write(&fake_agent, script).unwrap();
```

Anti-pattern B is more common in integration-shaped tests that spin up
real subprocesses; anti-pattern A is more common in unit-shaped tests
that exercise a function reading an env var directly. The same root
cause underlies both: shared mutable state across the test set, with
no synchronisation.

## What stays out of scope

This doc is about *test* hygiene. Production code that mutates the
process env at startup — `aida queue work` setting `AIDA_AUTO_COMPLETE`,
the autonomy mode setting `AIDA_ZEN` — runs single-threaded between
parse and exec and is not affected by the guidance here.

## Verifying in CI

`cargo test --workspace` runs everything in parallel by default; a
flake under that command means there's an isolation gap somewhere.
When debugging a suspected isolation flake, narrowing to
`-- --test-threads=1` should make the failure disappear — if it does,
the flake is in this category. Use `cargo test -- --test-threads=N`
with progressively higher N to reproduce.

## Reference

- `aida-cli/src/test_env.rs` — the `EnvVarGuard` helper and its own
  internal tests
- BUG-371 — the empirical case that motivated the wider audit
- TASK-521 — the audit + migration that introduced this guide
