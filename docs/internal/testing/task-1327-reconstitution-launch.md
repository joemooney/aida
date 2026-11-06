# Reconstitution launch investigation — 2026-10-05

<!-- trace:TASK-1327 | ai:codex -->

The empty-log failure reproduces before a vendor process starts. The exit-zero
observation is the **report command's** status, not a confirmed vendor status.
The original four runs have no process trace, so their exact execution history
cannot be established retrospectively.

Subject pins remain those in the [north-star snapshot](../positioning/2026-10-05-north-star-check-aida-monitor.md):
code `aadafa46101198f4d794a6e7a7a5fa4b6d53b4d6`, store
`fbed31b886bea8d80e72a5648759e7f334d98309`. The investigation checkout is
`978c497429`, with the stderr diagnostic in this change. Build with
`cargo build -p aida-cli --bin aida`; use the resulting executable under
`$CARGO_TARGET_DIR/debug/aida` when that variable is set, rather than an older
`target/debug/aida` or the installed binary. The installed PATH binary here was
built on October 3; the ambient release binary also lacked TASK-1-216.

## Live retry

From `~/ai/aida-monitor`, using the rebuilt executable:

```bash
timeout 30 "$CARGO_TARGET_DIR/debug/aida" reconstitute STORY-3 --json --yes \
  > /tmp/task-1327-current.json 2> /tmp/task-1327-current.stderr
```

This retry finished in approximately 0.6 seconds and printed:

```text
store probe failed: no valid parent seat grant is active
```

The log `probe-story-3-20261006-065058.jsonl` was zero bytes. The scratch
directory was `~/.aida/reconstitute/story-3-01a10ffa-d2d0-7a13-babf-fab8cee43706`.
No regenerated artifact existed; the report still returned unknown scores
because there are zero criterion-traced tests. This stderr diagnostic exposes
the launch error independently of the report-layer fix in TASK-1-216.

A separate `strace -f -e trace=execve` retry with the ambient release binary
confirmed **no Claude, Codex, or AGY exec**. A direct Claude 2.1.291 print-mode
smoke test with the launcher's flags and a fresh session UUID returned exit 0,
13,959 bytes of stream JSON, and empty stderr. This establishes that the vendor
executable works here; it does not establish reconstitution success.

## Cause and remaining work

`reconstitute::run_headless` changes cwd to the empty scratch directory.
`session::spawn_vendor_headless_with_seat` creates the log, then resolves
`headless_worktree_root()` there and calls `seat_authority::issue_child`.
`current_grant` requires `roster_allows`; an empty scratch root has no distributed
store and cannot resolve the roster. Grant validation refuses before the
`Command::status` call, leaving the already-created log empty. Vendor tuning
and project configuration also use the scratch root in this path.

TASK-1-224 records the repair for advisor triage: preserve a trusted project
authority/configuration root separately from the isolated probe cwd, keep grant
delegation fail-closed, and preserve arm A's context boundary. That authority
change needs a sketch and master sign-off under the repository protocol.
This investigation does not alter seat validation or grant delegation.

TASK-1523 remains unevaluable: the launch failure needs repair, and STORY-1527
independently blocks a scored delta because criterion-traced tests are absent.
The published null snapshot is not evidence that any of its four agents ran.
