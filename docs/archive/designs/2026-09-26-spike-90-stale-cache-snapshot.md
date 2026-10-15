# SPIKE-90: Immutable stale cache snapshot for lock-tolerant reads

- Date: 2026-09-26
- Spike: SPIKE-90 (drive mode, advisor review before merge)
- Author: claude (spike worktree `claude/spike-90`)
- Binary measured: `aida 0.15.0 (f9c900b)`, bundled SQLite 3.45 (rusqlite 0.31 `bundled`); probe scripts used Python `sqlite3` 3.45.1
- Constraints honoured: no production code; `busy_timeout=0` plus the AIDA retry ladder (STORY-543 rejection) left in place; any background work must be a durable single-flight worker, never an untracked in-process task (Joe, 2026-09-25)

## Verdict: NO-GO

**NO-GO on a separate immutable snapshot file.** The measurements show that the live cache already serves a consistent last-committed snapshot to readers in WAL mode (STORY-580). A plain read against a fresh cache takes 0.04 s even while another connection holds the write lock. Slow reads do not come from readers being unable to *read*. They come from readers trying to *refresh* the cache, which turns them into writers that queue on the ~25.6 s write ladder, sometimes twice. A second copy of the database would not remove that behaviour. It would add seconds of snapshot creation per refresh, a third freshness state, and new failure modes (in-place modification under `immutable=1`, rename semantics on Windows, schema skew between the snapshot and the binary).

**The cheaper alternative, filed as drafts:** apply stale-while-revalidate to the live WAL cache. Readers never enter the write ladder. A single-flight refresh, held under a kernel `flock` and backed by a durable refresh request, brings the cache current. Readers that lose the race serve the last committed state with an explicit stale label. Four follow-ups (section 9) cover a symlink bug that disables the existing reader guard in every worktree, the single-flight refresher, the strict `load()` read path, and atomic freshness stamping.

## 1. Problem, with evidence

### 1.1 How the cache is read and locked today (code references)

| Mechanism | Where | Behaviour |
|---|---|---|
| WAL journal | `aida-core/src/db/cache.rs` `open_connection_inner` (STORY-580) | `PRAGMA journal_mode=WAL` on every open. Readers see the last committed transaction and never wait for a writer's lock. |
| `busy_timeout=0` | same function | rusqlite's busy handler is disabled, so AIDA owns retry timing (STORY-543 rejection context). |
| Retry ladder | `cache.rs` `with_cache_retry_observed`, `DEFAULT_CACHE_RETRY_DELAYS_MS = [100 … 12800]` | About 25.6 s total per operation on `SQLITE_BUSY`/`SQLITE_LOCKED`. It is overridable with `AIDA_CACHE_RETRY_COUNT` and `AIDA_CACHE_RETRY_MS`. A 150 ms fast-fail ladder is used only by `aida awaiting --notice` (BUG-681). |
| Write lock-info sidecar | `cache_lock.rs` `write_cache_lock_info` (`create_new` JSON at `<cache path>.lock-info`), `foreign_writer_holds_lock`, `classify_lock_owner` (TASK-1484) | Written just before each write transaction. It records the PID, the start identity and the phase. |
| Reader guard | `cached_git_backend.rs` `ensure_cache_fresh_for_read` (BUG-664) | If the cache is stale *and* the sidecar names a live foreign writer, the reader skips the refresh and reads the last committed state. Otherwise it refreshes inline. |
| Staleness | `cache.rs` `is_stale` | `cache_meta.source_head_sha != git HEAD` of the store. |
| Refresh | `ensure_cache_fresh` then `try_incremental_update` (BUG-636), falling back to `full_rebuild` | Incremental refresh runs one write transaction per changed row, then a separate `set_source_head_sha`. A full rebuild is one row transaction followed by three separate meta transactions (`source_head_sha`, `built_at`, `schema_version`). |
| Snapshot-only open | `with_inner_cache_snapshot` (BUG-1569) | Used by `awaiting --notice`: opens without refreshing and validates hits against YAML. |
| Shared cache across worktrees | `aida-cli-lib/src/lib.rs` (~l.36540, BUG-52) | Every sibling worktree symlinks `.aida/cache.db`, `-shm` and `-wal` to the main checkout. **`cache.db.lock-info` is not symlinked**, and the sidecar path is derived from the *unresolved* cache path. |
| History cache | `aida-cli-lib/src/history_cache.rs` (TASK-1505/1507, shipped) | A separate database with its own WAL and an `flock` indexer lock at `<db>.lock`. It never opens the requirements cache. |

### 1.2 Measurements (read-only, against a copy)

Setup: `git clone --branch aida-store` of the live store into a scratch directory (remote removed), plus a `cp` of `.aida/cache.db` into a scratch project. The live store and `.aida/cache.db` were never opened for writing. Scale: 4,498 specs, 26 MB of YAML, and a 43.8 MB cache (10,686 × 4 KiB pages). Each scenario starts from a byte-identical fresh cache. A "writer" is a Python connection holding `BEGIN IMMEDIATE`. "Stale by one commit" sets `source_head_sha` to the store's parent commit, a two-file diff. The probe command is `aida list --status approved` unless noted otherwise.

| # | Scenario | Result |
|---|---|---|
| A | Fresh cache, foreign writer holds the write lock, no sidecar | **0.04 s**, exit 0. WAL already makes reads of a fresh cache lock-free. |
| E | Fresh cache, 4 concurrent readers | 0.06–0.09 s each |
| Iseq | Stale by 1 commit, single reader, no lock | 0.82 s (2.02 s on a cold page cache) for the incremental refresh |
| Dseq | Cache needs a full rebuild, single reader | 13.4 s (`aida cache rebuild` alone takes 8.1 s) |
| **B** | Stale by 1 commit, writer holds the lock, **no sidecar** (a race window, or a lock holder that is not AIDA) | **52.1 s, then exit 1.** Incremental refresh exhausts the ladder once (~25.6 s), prints `warning: incremental cache update failed (database is locked …); full rebuild`, then the full rebuild exhausts it again. |
| C | Same as B, but the sidecar names the live holder in the same `.aida/` | **0.04 s**, exit 0. The BUG-664 guard works, but it serves stale data with **no stale label**. |
| **W** | Same as C, but the reader runs from a **worktree** whose `.aida/cache.db` is a symlink (the fleet layout) | **52.9 s, then exit 1.** The reader looks for `wt/.aida/cache.db.lock-info`, never sees the holder's sidecar in the main `.aida/`, and falls into case B. |
| C2 | Same as C, other commands | `search` 0.02 s, `status` 0.48 s, `queue list` 0.05 s, `show` 0.07 s, `history` 0.11 s; **`graph tree EPIC-62` 51.5 s, exit 1**. Read commands that call `backend.load()` or `list_requirements()` use the strict `ensure_cache_fresh` even after the constructor's tolerant check. `aida-cli-lib` has about 300 `.load()` call sites in 35 files. |
| **I** | Stale by 1 commit, **4 concurrent readers**, no lock | Three runs: wall 41.4 s (13.2 / 41.4 / 31.4 / 7.2), then 7.3 s (0.55 / 2.8 / 5.1 / 7.3). One reader's incremental refresh lost the lock and fell back to a full rebuild. A single reader needs 0.82 s. |
| **D** | Full rebuild needed, **4 concurrent readers**, no lock | Wall 28.9 s, then 51.6 s. Worst reader: 51.6 s against 13.4 s alone. |

The root cause in cases I and D is a **thundering herd with no single-flight and no double-check**. Every reader that sees staleness loads the store and races for the write lock. The sidecar is written only when the *write transaction* starts, so the multi-second store load before it is invisible to the other readers. A loser that gets the lock later does its own redundant rebuild, because `rebuild_from_store` does not re-check `is_stale` under the lock.

Snapshot-creation costs, measured on a copy of the same 43.8 MB cache:

| Operation | Cost |
|---|---|
| `VACUUM INTO` (consistent read transaction) | 5.4 s idle; 3.4 s while another connection held an uncommitted write transaction (it succeeded and saw the committed 4,498 rows) |
| Online backup API (single step) | 8.1 s; the output keeps the WAL header |
| Snapshot size | 29.7 MB (`VACUUM INTO`) / 43.8 MB (backup) |
| `PRAGMA quick_check` / `integrity_check` on the snapshot | 0.21 s / 0.24 s |
| Read through `file:…?immutable=1` | < 1 ms for count and FTS queries; no `-wal`/`-shm` created |
| `rename(2)` publish | 40 µs |

A naive-copy hazard appeared during setup. At the moment the live `cache.db` was copied, its `-wal` held 0.9–1.1 MB of uncommitted-to-main frames, and the copy's recorded head (`435eaf5`) was already behind the store (`d91dbb4`). A `cp` of only the main file silently yields an older state. A `cp` of the main file and the WAL taken while a checkpoint runs can tear. Snapshots must never use a filesystem copy.

### 1.3 Field history

The theme recurs: BUG-455 (a drain failed with `database is locked`), STORY-543 (the 25 s read hang, rejected), STORY-580 (WAL, shipped), BUG-664 (reader guard, shipped), BUG-681 (fast-fail notice path), BUG-1569 (unrefreshed snapshot open for the notice), TASK-1484 (lock-owner metadata). Each fix narrowed the window, but none removed the reader-as-writer behaviour, and the BUG-664 guard does not reach worktrees (case W).

## 2. Options considered

| Option | Summary | Verdict |
|---|---|---|
| O0 | Status quo | Rejected. Cases B, W, C2, I and D show 29–53 s stalls and hard failures for plain read commands. |
| O1 | Immutable snapshot file (`cache.snapshot.db`) produced by a durable single-flight worker with `VACUUM INTO` → temp → `quick_check` → `rename`. Readers fall back to it with `immutable=1` when the live cache is locked or rebuilding. | **NO-GO** (section 3). |
| O2 | SQLite `sqlite3_snapshot_get/open` | Rejected. It is WAL-only, it lives inside one connection within one process, it cannot outlive a checkpoint, and rusqlite 0.31 does not expose it. It does not help across processes. |
| O3 | Raise `busy_timeout` for readers | Rejected. It conflicts with the STORY-543 decision that AIDA owns retry timing, it hides the lock holder, and readers would still block. |
| O4 | STORY-543 as filed: a separate read ladder of about 1 s plus fail-soft | Partially superseded. WAL shipped. The bounded read wait and fail-soft label survive in O5, but as "readers never write" rather than a second ladder. |
| **O5** | **Stale-while-revalidate on the live WAL cache:** readers never enter the write ladder; a single-flight refresh runs under a kernel `flock`, backed by a durable refresh request plus a detached worker; readers get a bounded wait for the refresh, then the last committed state with a stale label. Also fixes the sidecar symlink bug and makes freshness stamping atomic. | **Recommended** (section 4) |
| O6 | Fall back to the canonical git store on lock | Keep it as the last resort, which is today's behaviour for a missing or corrupt cache. It is too slow for the hot path (a store load takes several seconds). |

## 3. Why the snapshot file (O1) is a NO-GO

Short answers to each question in the spike description:

1. **SQLite backup, snapshot and atomic-replace guarantees.** `VACUUM INTO` and the backup API both read inside one read transaction, so the output is a consistent committed state. This was verified with a concurrent uncommitted writer. `rename(2)` within one filesystem is atomic on POSIX, and readers holding the old inode keep a valid file. On Windows, `MoveFileEx(REPLACE_EXISTING)` fails while a reader has the file open, so it needs a retry or a generation-numbered file name. The guarantees exist, but they solve a problem WAL already solves for the live file (case A).
2. **Copying a live database or WAL unsafely.** Only `VACUUM INTO` or the backup API should ever be used, never `cp` (see the setup hazard in section 1.2). Either one holds a read transaction for 3–8 s at this scale. It does not block writers, but it pins the WAL, so checkpoints cannot complete past it and the `-wal` grows during every snapshot.
3. **Freshness metadata and labelling.** `cache_meta` already has `source_head_sha`, `built_at` and `schema_version`, and the snapshot inherits them. A snapshot creates a *third* state (live-fresh, live-stale, snapshot-staler) that every reader must reason about and label.
4. **Lock-owner detection and bounded wait.** The detection O1 would need is the same detection O5 needs, and it is broken today in worktrees (case W). Fixing it is required either way.
5. **Which commands may read stale data.** The answer is the same list as for O5 (section 4.3); O1 adds nothing here.
6. **Schema migration and corruption.** A snapshot written by binary *N* and read by binary *N+1* must be rejected on `schema_version` or column drift. Because many worktrees run dev builds at different SHAs (TASK-1478, BUG-627), this happens often, and the fallback would frequently be missing exactly when needed. Corruption can be caught before publish (`quick_check` takes 0.2 s).
7. **Permissions and multi-process access.** `immutable=1` disables locking and change detection. If anything ever modified the snapshot in place (a stray `aida cache` verb, a user `sqlite3` session, the BUG-683 self-heal), readers would get undefined results or `SQLITE_CORRUPT`. Guarding against that is a lasting burden.
8. **Interaction with the history-event cache.** TASK-1505/1507 shipped a separate `history-v*.db` with its own WAL and `flock` indexer lock. It never touches the requirements cache, so there is no coupling. A snapshot would not cover history queries. O5's worker can reuse the history cache's lock pattern.
9. **Worth it?** No. The windows where the live WAL cache is truly *unreadable* are narrow: a migration drop that commits empty tables before the rebuild, the BUG-683 corruption delete, or a fresh clone. They are rare, and section 9 F4 closes the first one directly by making drop, apply and rebuild a single transaction. Every stall measured in section 1.2 comes from the refresh protocol, which a snapshot does not change. A snapshot costs 30–44 MB more disk, 3–8 s of worker time per publish, and WAL growth.

**Revisit trigger:** revisit O1 only if, after F1–F4 ship, telemetry still shows read failures caused by an *unreadable* live cache rather than a locked one, or if AIDA must support a filesystem where WAL is unusable (network filesystems, per the SQLite WAL documentation).

## 4. Recommended design (O5): stale-while-revalidate on the live WAL cache

### 4.1 Rules

1. **Readers never write the cache and never enter the write ladder.** `busy_timeout` stays 0. The ~25.6 s ladder stays exactly as it is, but it applies only to writers: the spec writer paths, `aida cache rebuild`, and the refresh worker.
2. **Single-flight refresh** under a kernel `flock` on `<canonical cache path>.refresh.lock`, the same pattern as the history cache's `IndexLock`. The kernel releases the lock when its holder dies, so no liveness healing is needed. A process that acquires the lock **re-checks `is_stale` before doing any work**, which removes the redundant rebuilds seen in cases I and D.
3. **Durable refresh request.** A reader that finds the cache stale and cannot win the lock (or is on a latency-bounded path) makes sure `.aida/cache.refresh-request` exists, recording the target HEAD and the requester. It then spawns, at most once, a **detached** `aida cache refresh --worker` process (`setsid`, stdio to `.aida/cache-refresh.log`), which takes the `flock`, refreshes, stamps, and clears the request only if the cache head is at or past the requested head. This is a separate OS process and a recorded request, not an in-process task, so it survives the CLI exiting. If the worker crashes, the request stays on disk. The next reader, or the next `aida schedule tick` (via a substrate job `cache refresh --if-requested`), sees "request pending and lock free" and re-spawns the worker.
4. **Reader freshness protocol**, replacing `ensure_cache_fresh_for_read` and applied to *every* read path, including `load()` and `list_requirements()` when called by read commands:
   - Fresh: serve.
   - Stale and `try_lock` on the refresh lock succeeds: refresh inline (0.8 s incremental in the common case; this keeps read-after-`aida pull` behaviour), then serve.
   - Stale and the lock is held: poll `source_head_sha` for a bounded wait (default 1.5 s, `AIDA_CACHE_READ_WAIT_MS`; 0 on the `--notice` path), then serve the last committed state **with the stale label**.
   - The cache is missing or unreadable: today's O6 fallback applies (strict rebuild or store read).
5. **Atomic freshness.** Row changes and the `source_head_sha` and `built_at` stamps commit in **one** transaction, for both full rebuilds and incremental refreshes. Incremental currently runs one transaction per row, so it becomes one transaction per HEAD. A migration's drop, apply and rebuild are also one transaction. A reader then never sees rows from HEAD *B* stamped with HEAD *A*, or empty tables.
6. **Lock-owner metadata on the canonical path.** The lock-info sidecar and the refresh lock both derive from the *canonicalized* cache path, so every worktree sharing the symlinked cache sees the same owner (fixes case W).

### 4.2 Stale label (the wording)

- stderr, printed once per command, never on stdout, exit code unchanged:
  `note: showing cached results from <built_at, local time> (store has moved on; refresh in progress). Re-run in a few seconds for current data.`
- `--json` / TOON: a top-level `cache` object: `{"stale": true, "cache_head": "<sha>", "store_head": "<sha>", "built_at": "<rfc3339>", "refreshing": true}`.
- MCP read tools: the same `cache` object in the tool result.
- No new exit code by default. This keeps STORY-543's pipeline-safety reasoning. A `--strict-fresh` flag (read wait becomes a strict refresh) is optional and deferred until someone asks for it.

The wording avoids SPEC IDs and internal jargon, per the CLI-prose rule.

### 4.3 Which commands may read stale data

**Allowed (labelled):** `list`, `search`, `status` (including `--full` cache counts), `queue list`, `graph` / `graph tree`, `digest`, `awaiting --notice` (wait 0; already snapshot-only through BUG-1569), statusline, `burndown`/`fleet` read views, and MCP read tools.

**Always fresh (strict; may use the writer ladder):**
- every mutating command, together with the reads it makes inside its store lock;
- the `aida add` spec-ID collision check (cache-backed since BUG-701), because a stale check could allow a duplicate ID;
- gates that decide state: validate, complete, merge and integrate checks, lease and claim;
- `aida cache rebuild` and `aida cache status`;
- `aida show`, which already reads the YAML object directly and is unaffected.

### 4.4 Consistency boundaries

- The git store stays canonical. The cache is a projection that is, at worst, one refresh behind, and it is always labelled when it is.
- Within one read, rows are consistent to a single committed HEAD once rule 5 lands. WAL gives each read transaction one snapshot.
- Read-your-own-writes: writer paths still upsert and restamp in-process (unchanged). If that upsert fails, the SHA is cleared and the next read refreshes. Under O5 that read either wins the lock or is labelled stale; it is never silently wrong.
- Whole-graph derived columns (`in_degree`, `blocked`, epic rollups) keep their existing "authoritative after a full rebuild" contract.

### 4.5 Operational cleanup and recovery

- A dead worker releases its `flock` automatically. A leftover `cache.refresh-request` is picked up by the next reader or tick.
- A leftover lock-info sidecar is cleared by the existing TASK-1484 compare-and-delete. `aida doctor heal stale-locks` should also report a request older than N minutes with no lock holder.
- Worker runtime is capped (for example, twice the last full-rebuild time, or 120 s). On timeout the worker exits nonzero and the request stays for a retry.
- The worker log is `.aida/cache-refresh.log`, bounded or rotated like other `.aida` logs.
- `aida cache status` shows the cache head, the store head, `built_at`, and whether a request is pending or the lock is held.

## 5. Risks

| Risk | Mitigation |
|---|---|
| Agents act on stale data they did not notice | A stderr label plus a structured `cache.stale` field; state-deciding commands stay strict (section 4.3); the bounded wait makes the common incremental case (< 1 s) return fresh data. |
| Detached worker processes pile up or become orphaned | `flock` single-flight (at most one worker holds it; extras exit immediately); a spawn guard ("request exists and lock held" means no spawn); a runtime cap. |
| Windows has no `setsid`/`flock` equivalents | `fs2` `try_lock_exclusive` already abstracts `LockFileEx`; detach with `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`. Test in the Windows CI leg. |
| Mixed binaries in the fleet (dev worktrees at different SHAs) run the worker | The worker runs `current_exe()` of the requester; TASK-1478's ordered schema-version rules still apply; the worker is subject to the same drift checks. |
| Moving incremental refresh to one transaction lengthens a single write-lock hold | A two-file diff took 0.8 s in total; readers are unaffected under WAL; only other *writers* wait, and they already use the ladder. The 500-file threshold remains the fallback to a full rebuild. |
| Changing `load()` to the tolerant path for read commands may alter behaviour in the ~300 call sites | F3 classifies call sites; writers keep a strict entry point; parity tests cover the change. |
| WAL growth from long readers | Unchanged from today; O5 adds no long read transactions (unlike O1's snapshot creation). |
| Sidecar path canonicalization changes where `doctor heal stale-locks` looks | F1 includes doctor. |

## 6. Failure modes (O5)

| Failure | Behaviour |
|---|---|
| Worker crashes mid-refresh | The transaction rolls back (WAL), the `flock` is released, the request stays, and the next reader or tick re-spawns the worker. Readers keep serving the last committed state with the label. |
| Worker hangs | The runtime cap kills it. Until then, readers wait at most 1.5 s and are labelled. |
| A lock holder that is not AIDA (for example a user `sqlite3` shell) | Readers are unaffected under WAL. The worker waits on the ladder and fails. The request stays, the error names "no recorded owner" (TASK-1484 enrichment), and doctor surfaces it. |
| Cache missing or corrupt | Unchanged BUG-683 self-heal plus a strict rebuild. |
| Schema drift | Unchanged BUG-1097 retry. With F4, drop, apply and rebuild are atomic, so readers never see empty tables. |
| Store HEAD moves during a refresh | The worker stamps the HEAD it refreshed to; the next check sees stale again and a new request is created. Refreshes stay single-flight. |

## 7. What stays unchanged

`busy_timeout=0`; `DEFAULT_CACHE_RETRY_DELAYS_MS`; the fast-fail ladder for `--notice`; WAL; git as canonical; the history cache; the BUG-683, BUG-1097 and TASK-1478 self-heal paths.

## 8. Prototype

No prototype code was written. The measurements used the released `aida` binary and small Python `sqlite3` probe scripts against scratch copies outside the repository. No cargo build was run.

## 9. Follow-ups (filed as drafts, each references SPIKE-90)

| ID | Title |
|---|---|
| BUG-1644 | The cache lock-info sidecar is resolved per worktree, so readers in a worktree never see a live writer in the shared cache and stall ~52 s, then fail |
| STORY-1484 | Single-flight cache refresh: `flock`-guarded, double-checked, with a durable refresh request and a detached worker; readers get a bounded wait and then a labelled stale read |
| TASK-1514 (blocked by STORY-1484) | Read commands that call `load()` / `list_requirements()` use the strict refresh and can stall ~51 s behind a writer; give them the read-tolerant path |
| TASK-1515 | Stamp cache freshness in the same transaction as the rows (full and incremental), and make migration drop, apply and rebuild atomic |

Suggested order: F1 (BUG-1644, small and independent) → F4 (TASK-1515) → F2 (STORY-1484; an architecture change, so it needs a sketch and advisor signoff) → F3 (TASK-1514, depends on F2's reader protocol).
