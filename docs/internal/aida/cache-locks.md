# Cache Lock Retry Tuning

AIDA keeps a local SQLite cache at `.aida/cache.db`. During heavy multi-agent
activity, one process may briefly hold the cache while applying schema changes
or rebuilding rows. Other processes retry before failing with a lock-holder
message.

Default retry budget:

```text
100,200,400,800,1600,3200,6400,12800 ms
```

That is about 25 seconds total, enough for normal schema-apply contention seen
in multi-agent drains.

## Overrides

Use overrides only when the default is wrong for the environment:

- `AIDA_CACHE_RETRY_COUNT=0` makes cache operations fail fast. Use this in
  tests or CI paths that must not wait.
- `AIDA_CACHE_RETRY_MS=2000 AIDA_CACHE_RETRY_COUNT=10` gives a fixed 20 second
  retry budget. Use this when diagnosing long live writers.
- If the error names a dead PID or stale sidecar, run
  `aida doctor heal stale-locks --yes` instead of increasing the retry budget.

Lock errors include the holder PID, command, and start time when
`.aida/cache.db.lock-info` is available.
