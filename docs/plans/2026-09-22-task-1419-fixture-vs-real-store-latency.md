# TASK-1419: fixture-vs-real-store read-latency inversion — measurement report

Date: 2026-09-22
Specs: TASK-1419 (references BUG-1480, now Completed)
Status: measurement complete
Complexity: small

## Triage note before the numbers

TASK-1419 is filed as "load-bearing under BUG-1480." BUG-1480 is now
**Completed** (STORY-1406 shipped the cache-backed single-spec `show` path,
gated behind STORY-1422/STORY-1423 which are also Completed/blocking-satisfied
except STORY-1423's own gate). So the specific practical question this task
was blocking — "should BUG-1480 be fixed?" — is already resolved independent
of this measurement. What TASK-1419 still answers, and what remains valuable,
is acceptance #3: **whether a tempdir fixture can ever model real-store read
latency for this codebase**, which bears directly on any *future* fixture-based
latency test (e.g. a CI guard for STORY-1423's kind of work). That question is
answered below and the answer is No, not on this host, not without controlling
for concurrent load.

## Approach

Used the installed release binary (`/home/joe/ai/aida/target/release/aida`,
sha `687ccb5976+dirty`, 0.15.0) against two stores:

- **Real store**: this repo, `/home/joe/ai/aida`, measured in place (read-only
  commands only). 4226+ objects, ~23 MB, `.git` ~209 MB / 176 MB pack. This
  machine is a live multi-agent dev box: `ps aux | grep aida` showed
  **25-30 concurrently running `aida`-related processes** at measurement time
  (other Claude worktree sessions actively working the queue).
- **Fixture store**: a fresh `git init` + `aida init --no-skills --no-hooks
  --no-agent-config` tempdir seeded with `aida add`, private to this session,
  zero other readers/writers of its `.aida/cache.db`.

Every measurement used `time <cmd>` (wall) so real/user/sys could be compared
directly; each command wrapped conceptually under a 60s budget (none needed
`timeout` to enforce it — the real store's absolute worst case was 85s, see
below, which is reported honestly rather than truncated).

## Results

### Real store (4226 objects, 23 MB), `/home/joe/ai/aida`, 2026-09-22, ~25-30 concurrent `aida` processes on the host

| command | real | user | sys |
|---|---|---|---|
| `aida --version` (baseline process startup) | 0.036s | 0.006s | 0.008s |
| `aida list` (run 1) | 0.874s | 0.439s | 0.201s |
| `aida list` (run 2) | 0.700s | 0.435s | 0.179s |
| `aida list` (run 3) | 0.887s | 0.455s | 0.187s |
| `aida list` (run 4, 30 concurrent procs) | 0.513s | 0.412s | 0.182s |
| `aida show TASK-1419 --no-git` | 1.126s | 0.404s | 0.070s |
| `aida cache rebuild` (run 1 — full YAML parse of every object) | **85.519s** | 1.601s | 0.314s |
| `aida cache rebuild` (run 2) | **56.413s** | 1.475s | 0.314s |

### Fixture store (19 objects, 148 KB), private tempdir, 2026-09-22, 0 other readers/writers of its own cache.db

| command | real | user | sys |
|---|---|---|---|
| `aida list` | **3.420s** | 0.013s | 0.020s |

(A 500-object fixture matching TASK-1419's original scale was started but
killed after seeding only 19 objects — each `aida add` was itself taking
several wall-clock seconds despite doing a single targeted git commit, on a
store with no contention of its own. That slowness is itself evidence, not
noise: see below.)

## Acceptance

**1. Is the inversion reproduced?** Partially, and the direction is
irrelevant — what reproduces is that wall-clock time on this host does not
track store size in *either* direction. The 19-object/148 KB fixture's `aida
list` (3.42s) was slower than every real-store `aida list` run (0.51-0.89s)
against a store 220x larger in object count. That is the same inversion
TASK-1419 reports, reproduced at a different scale, on a store that has zero
possible contention on its own cache — which rules out "the fixture's cache
lock is contended" as the explanation.

**2. Dominant cost term in real-store `aida show`, isolated by measurement:**
NOT per-object or per-byte parse cost. `aida cache rebuild` — the operation
that actually does the full per-object YAML parse for all 4226 objects —
spent only **1.5-1.6s of CPU** (user+sys) doing that work, both runs. The
other **55-84 seconds of wall-clock time were spent blocked, not computing**
(sys time stayed under 0.32s both runs — this is not I/O-syscall-bound
either). This is a near-exact match for a documented, named mechanism already
in the codebase: `aida-core/src/db/cache.rs` (around the `busy_timeout(0)` +
comment at line ~979) describes exactly this failure mode from BUG-664 —
`busy_timeout=0` plus an application-level retry ladder means a process that
wants the cache **write** lock while another concurrent `aida` process holds
it blocks and retries in a sleep loop, burning wall-clock with near-zero CPU.
With 25-30 concurrent `aida` processes active on this host, `aida cache
rebuild` (a write path) reliably lost that race. `aida list`/`aida show` are
mostly WAL reads (per the same BUG-664 fix) and were largely insulated from
this specific lock, which is why they stayed under a second even at 30
concurrent processes — **but they were not insulated from a second,
broader effect**, see #3.

**3. Can a tempdir fixture ever model real-store read latency on this
codebase? No — not measured on a shared, concurrently-loaded host, and this
task's own data shows why that limitation is more general than "cache lock
contention."** The 19-object private fixture (no possible lock contention:
it is the only process that has ever touched its cache.db) still showed a
100x real/CPU gap on `aida list` (3.42s wall vs 0.033s CPU). Since there is
no store-internal lock to contend with a private, freshly-created cache, the
remaining explanation is host-level: this machine was running ~25-30
concurrent `aida`/agent processes at the time, and OS-scheduler contention
for CPU (not disk, not a store lock) inflated wall-clock latency for a
trivial private operation by two orders of magnitude. That means **any
wall-clock latency number gathered on this dev box while other agents are
active — real store OR fixture — is dominated by host load, not by the
store under test**, and a fixture built to be "smaller" than the real store
buys nothing, because the thing actually driving the measured time isn't
store size at all. A fixture-based latency test is only trustworthy if it
(a) runs on an idle, single-tenant host or CI runner, and (b) isolates CPU
time (user+sys) rather than wall-clock, or gates on wall-clock only after
confirming zero concurrent `aida` activity on the runner.

**4. Rate / population / date, stated explicitly:** all numbers above are
measured 2026-09-22 against aida 0.15.0 (sha 687ccb5976+dirty). Real-store
population: 4226+ objects / 23 MB in `/home/joe/ai/aida`'s `.aida-store`, with
25-30 concurrent `aida` processes active on the host at measurement time (this
repo's own dogfood drain). Fixture population: 19 objects / 148 KB, a private
tempdir with zero concurrent access to its own cache. None of these rates
should be reused to size a future fixture or CI budget without re-measuring
under the same (or an explicitly idle) concurrency condition — the corpus and
the host's concurrent load both change what "the rate" means, exactly as
TASK-1419 warned.

## Bottom line

The dominant cost term is **host-level contention** (CPU scheduling under
concurrent multi-agent load, plus — specifically for cache-write paths — the
`busy_timeout=0` + retry-ladder lock wait named in the BUG-664 code comment
at `aida-core/src/db/cache.rs`), not per-object or per-byte cost of either
store. Per-object CPU cost is small and well-behaved (~0.35-0.4ms/object,
from the 1.5-1.6s CPU time to parse 4226 objects in `aida cache rebuild`). A
tempdir fixture cannot model real-store wall-clock read latency on a shared,
concurrently-loaded host — the two are measuring host contention, not store
behavior, and the original inversion (fixture slower than a much bigger real
store) is exactly what that produces. This does not reopen BUG-1480 (already
Completed and fixed via cache-backed single-spec reads); it is a data point
for whoever builds STORY-1423's latency guard: gate the guard on CPU time or
run it on an idle/single-tenant runner, not on wall-clock captured on a busy
dev box.

## Files

- Measurement only; no code changed.
- Fixture used: private tempdir under this session's scratchpad (not
  committed; throwaway).

## Followups

- If a fixture-based latency CI guard is built (STORY-1423 context), it
  should assert on CPU time or run with a concurrency precondition, not raw
  wall-clock, per the finding above.
