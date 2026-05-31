---
name: aida-executable-direct
description: Use the global `aida` executable directly instead of `cargo run -p aida-cli` for routine commands to avoid build latency.
propagation: scaffolding-pack
metadata:
  type: feedback
---

When executing routine AIDA CLI commands (such as `aida show`, `aida list`, `aida add`, or `aida brief`), always use the global `aida` executable directly rather than `cargo run -p aida-cli`.

**Why:**
Running `cargo run -p aida-cli` compiles the workspace or checks build dependencies on every invocation. This introduces significant compilation latency and lock contention, especially in parallel agent pipelines. The operator is responsible for keeping the global binary up-to-date; rebuilding is only necessary when code changes in the CLI/core require testing.

**How to apply:**
1. Default to executing AIDA commands with `aida <subcommand> <args>`.
2. Do not use `cargo run` unless you have explicitly modified the Rust source code and need to run a development-specific test.
3. If you have modified the source code and need to update the local executable, use `cargo build -p aida-cli` (or `--workspace`) to rebuild, then execute with `cargo run` (or use the built target under `target/debug/aida`) strictly to verify the changes.

Composes with [[self-test-via-dogfood-merge]] and [[run-help-before-suggesting-flags]].
