# STORY-543 — Lock-tolerant reads: WAL cache + bounded read-wait + fail-soft staleness

Date: 2026-06-08
Specs: STORY-543
Status: Implemented (DRAFT PR, held for operator review — data-layer)
Complexity: Medium

## Approach

Three layers, matching the spec's L1/L2/L3 design:

- **L1 (root cause)** — enable `PRAGMA journal_mode=WAL` on `.aida/cache.db` at
  connection open (matching the legacy `sqlite_backend.rs`). In WAL mode a reader
  sees the last-committed snapshot without blocking on a writer's EXCLUSIVE lock,
  which is the ~25s read hang. This removes the bulk of the pain with no new flag.
- **L2 (bound)** — reads no longer share the ~25.6s write retry ladder. A bounded
  per-connection `busy_timeout` (`DEFAULT_READ_WAIT_MS` = 1000ms, overridable via
  `AIDA_CACHE_READ_WAIT_MS`) caps the read-side lock wait. Writes keep the ladder.
- **L3 (fail-soft)** — a read that still can't acquire after the bounded wait
  degrades instead of erroring: `CacheRead { value, degraded }` carries best-effort
  rows + a `degraded` flag. CLI surfaces staleness OUT OF BAND — stderr warning +
  exit 0 by default (pipeline-safe); `--strict-fresh` → reserved exit code 75
  (EX_TEMPFAIL); `--json` carries a top-level `stale` boolean.

```
aida list / search
   └─ CachedGitBackend::{list_summaries_soft, search_soft}
        ├─ ensure_cache_fresh_soft()   (rebuild is fail-soft too)
        └─ Cache::{list_summaries_soft, search_soft}
             └─ soften_lock(read, fallback)  → CacheRead{value, degraded}
                  read query runs under set_read_busy_timeout (bounded, WAL)
```

## Decisions

- Kept the write-through model untouched (conservative; data-layer). WAL +
  busy_timeout + graceful fallback are the safe primitives only.
- `soften_lock` only swallows DatabaseBusy/DatabaseLocked; every other error
  (corruption, schema faults) still propagates — fail-soft is for contention.
- Staleness is signalled out of band (stderr / JSON field), NOT by overloading
  exit code — a non-zero exit would break `aida list && …` pipelines. Exit 75 is
  opt-in behind `--strict-fresh` for callers that demand guaranteed-fresh data.
- `aida list --json` shape changed from a bare array to `{ stale, rows }` to carry
  the required `stale` field; the TUI launcher parser (`dashboard.rs`) was made
  tolerant of BOTH shapes so nothing breaks.
- Scope held to the two cache-contended user-facing reads (`aida list`,
  `aida search`). `aida queue list` reads the file-based queue, not the cache, so
  it is out of scope for cache-lock tolerance.

## Files (build order)

1. `aida-core/src/db/cache.rs` — WAL pragma at open; `read_wait_ms` /
   `set_read_busy_timeout`; `CacheRead<T>` + `soften_lock`; bounded read-wait in
   `list_summaries`/`search`; `list_summaries_soft`/`search_soft`; 6 new tests.
2. `aida-core/src/db/cached_git_backend.rs` — `list_summaries_soft`/`search_soft`
   + `ensure_cache_fresh_soft`.
3. `aida-core/src/db/mod.rs`, `aida-core/src/lib.rs` — re-export `CacheRead`.
4. `aida-cli/src/cli.rs` — `--strict-fresh` on `list` + `search`.
5. `aida-cli/src/main.rs` — soft-read wiring, `STALE_READ_EXIT_CODE`,
   `emit_cache_stale_signal`, `{ stale, rows }` JSON envelope.
6. `aida-tui/src/dashboard.rs` — tolerant `parse_list_json` (both JSON shapes).

## Tests

- `cache_opens_in_wal_mode` — journal_mode persists as WAL; `-wal` sidecar exists.
- `wal_read_does_not_block_on_concurrent_writer` — read completes < 1s with a
  concurrent BEGIN IMMEDIATE writer holding the cache.
- `read_wait_budget_is_bounded_and_configurable` — default + env override + 0 +
  garbage-falls-back-to-default.
- `soften_lock_degrades_on_lock_but_propagates_other_errors` — lock → degraded;
  other error → propagates; ok → fresh.
- `soft_reads_return_fresh_on_happy_path` — backend soft APIs fresh on happy path.

## Verification

```
cargo build --release -p aida-cli
cargo test -p aida-core      # cache tests
cargo test -p aida-cli
cargo fmt --all -- --check
cargo clippy -p aida-cli
git check-ignore .aida/cache.db-wal .aida/cache.db-shm   # both ignored
```

## Followups

- Read-through-to-git best-effort projection on degrade (currently degrade =
  empty rows + staleness signal; a true read-through would reuse the YAML store).
- Mirror `--strict-fresh` / `stale` onto MCP `list_requirements` /
  `search_requirements` if/when those surface cache-busy degradation.

## Related

- Refines the operator's "ignore the lock / dirty read / signal via return code"
  ask; WAL delivers the lock-free read, staleness signal moved to stderr + JSON.
- Legacy `sqlite_backend.rs` already ran WAL — this brings the git-canonical cache
  to parity.
