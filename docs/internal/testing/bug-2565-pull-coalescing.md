# Pull coalescing controls and regression coverage

Concurrent pull waiters reuse success only after checking that the requested
store and code remote heads are contained locally. Failed or unknown probes
veto reuse. An upstream commit advertised during an older pull therefore
requires another pull.

The db sync no-op path uses one fallible remote-head query. Only a clean store
whose HEAD exactly equals origin skips conflict and collision scans. Pending
writes and unpushed local commits retain canonical validation.

Operator controls (read on each invocation):

- `AIDA_PULL_COALESCE=0`: disable coalescing and run the ordinary pull.
- `AIDA_PULL_COALESCE_WAIT_SECS`: wait budget in seconds (default 1200).
- `AIDA_RECONCILE_WATERMARK_TTL_SECS`: watermark lifetime (default 900);
  zero disables watermark reuse.

Invalid unsigned integer settings use the defaults. Zero wait permits the first
200 ms lock poll before falling back to an ordinary pull.

The `bug_2565_store_pull_coalesce_tests` suite exercises a bare remote moving
while a pull is in flight, independent code freshness, local canonical errors
and duplicate spec IDs, and a real db sync window with 1200 ledger directories.
The no-op window must finish within 10 seconds without fetching; an invalid
unchanged canonical object proves that neither strict full-store scan ran.
Separate tests cover reconcile watermark behavior and operator controls.

The coalescing lock is held only for the decision, never across the pull. The
suite checks that the pull body runs with the lock free and the running marker
present, that a marker left by a dead process does not block, and that the
marker is removed after the state is written. In
`aida-cli/tests/task_1717_store_writer_lock.rs`,
`a_pull_started_by_a_hook_inside_the_window_does_not_wait_for_its_parent`
drives a real `aida pull` from a `post-rewrite` hook of a running pull and
requires it to return without the coalescing wait.

<!-- trace:BUG-2565 | ai:codex -->
