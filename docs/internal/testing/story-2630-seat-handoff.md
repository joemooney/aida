# STORY-2630 seat handoff, 2026-11-08

<!-- trace:STORY-2630 | ai:codex -->

PR body material for the host publisher (PR #3662). Publication remains vetoed
by the blocked-dependency punt until the host/advisor resolves acceptance.
This is a Refs slice; it does not complete STORY-2630.

Verified against local `origin/main` at
`5cbe14e5fc6be0d87709049b48f3b7eaeab77808` (no fetch/rebase from this seat,
whose Git metadata is read-only):

- AC1 routing is on main: `primary_index_link` and its runtime setup caller.
  Delivering/selecting the host proxy client remains host work.
- AC2 is not complete: no contained client/descriptor/socket admission or live
  read-only grant evidence is available. No index write grant was added.
- AC3 navigation guidance is on main in AGENTS.md and the shared dispatch
  template. Bare `codegraph` still requires host delivery of the reviewed
  client; a portable absolute deployment path cannot be inferred from this seat.
- AC4 direct-index diagnostics are on main. This candidate adds proxy snapshot
  age and reachability reporting via `doctor --lane-host`.
- AC5 is blocked; exact attempted seat command and output:

```text
$ codegraph callers primary_index_link
/bin/bash: line 1: codegraph: command not found

$ aida doctor --lane-host --json
error: unexpected argument '--lane-host' found

Usage: aida doctor [OPTIONS] [COMMAND]

For more information, try '--help'.
```

Neither `.codegraph` in this seat nor `/home/dirtlab/ai/aida/.codegraph` exists.
`rg -n primary_index_link aida-cli-lib/src/worktree_gc.rs` confirms the known
caller at line 1124, but text search does not satisfy graph-query acceptance.

This rework fixes review findings 1, 6, 7, 8 and 9: classify the bounded client
identity read; keep successful upstream JSON separate from stderr while bounding
both streams; publish heartbeats every three seconds; document shared worker
capacity and mention saturation in the doctor refusal; allow 50 seconds for
lock acquisition, snapshot backup and upstream query. The capacity change is a
diagnostic/documentation correction, not reserved status capacity.

Host owner Joe must deploy the reviewed host-owned client and proxy outside
seat writable roots, deliver its fixed command/PATH to lane prompts, provide
reviewed contained read admission, and run real upstream plus non-empty contained
`callers` acceptance and the candidate `doctor --lane-host`. Paste their exact
output and grant evidence into the PR body. Advisor must resolve the completing
ruling versus this blocked Refs slice and the publication veto. The host must
also rebase against current main and obtain final-head CI; this seat cannot
publish or repair the PR body itself.

Punt persisted successfully as Needs Attention / blocked-dependency. The CLI
reported `could not write punt signal file: Read-only file system (os error 30)`;
the host publisher must honor the persisted punt and this explicit veto even
without the signal file.

Validation so far: Python proxy suite ran 18 tests: 17 passed, real installed
CodeGraph compatibility skipped (not installed). Store-writer inventory passes
(3371 reconciled sites). `make sync-templates` fails at `sync-agent-skills` with
`Read-only file system (os error 30)` for the protected `.agents` pack;
`make check-templates` passes. `git diff --check` passes. Rust codegraph tests
submitted through lane job `j20261108T093927702-2-0`; result recorded below.

Rust lane job completed successfully: `cargo test --profile agent -p
 aida-cli-lib --lib -- codegraph::tests`, 13 passed, 0 failed, exit 0.
No commit, push, PR publication, ship declaration, review record or queue
completion was performed. All six changed files remain uncommitted for the
host, subject to the persisted punt/publication veto.
