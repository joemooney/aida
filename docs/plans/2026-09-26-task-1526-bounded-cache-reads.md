# TASK-1526: single-flight bounded stale reads

Owner: leased implementer session `01a0e0eb7cff`, branch `task-1526`.
Binding design: STORY-1484 advisor signoff, amendments A2–A6 and A9–A13.
Slice C (TASK-1527) is excluded. TASK-1514's reader expansion is excluded.

## Binding pre-C decision

Independent advisor decision `01a0e0fb-8d6b-7790-b04b-f808236dbe2d`,
accepted by product, amends the parent signoff with B1–B6. A compatible
non-TTY full-rebuild winner returns `Deferred`; an incremental SQLite busy
winner makes one attempt and returns `WriterBusy`. Neither promises background
progress. Only observed refresh flock holders authorize `WorkerRunning`; a
holder disappearing without freshness gives `Deferred`. Interactive human TTY
full rebuilds, missing/unusable caches, migration, unsupported-flock fallback,
and strict mutation boundaries retain their signed-off exceptions. Labels use
one pinned committed snapshot for rows and metadata. No C worker/request code,
TASK-1514 caller expansion, or BUG-1674 show implementation belongs here.

## Implementation

1. Diagnostic identity: optional boot ID and PID namespace; mismatch or an
   unavailable comparison is Unknown. Hostname is display only. Legacy records
   retain their local PID classification.
2. Refresh flock uses cache_sidecar_path. A process-global registry owns the
   descriptor; same-thread nested callers share it, other threads contend.
3. Preserve strict mutation paths. Delete the sidecar-based stale shortcut.
   Winner double-checks; incremental refresh gets one SQLite attempt; a loser
   only polls committed metadata for its bounded budget.
4. SQLite transactions recheck HEAD before applying rows. Explicit rebuild is
   still unconditional. Pre-resolve candidate epic statuses before the write
   transaction, retaining authoritative reads for new-ancestor misses.
5. Collect stale observations per CLI invocation / MCP call. Preserve top-level
   arrays; object outputs always carry cache metadata; stderr emits one note.
6. Enforce no blocking refresh wait with a store/SQLite write lock held.
   Migration readers never consume old-schema rows. Unsupported flock is strict.

## Validation

Use fixture stores and fake HOME. Run the signoff's named B tests (single-flight,
loser timing/labels, winner recheck, incremental lock error, unsupported flock,
migration, nested calls, transactional recheck, refill skip, epic pre-resolve,
identity, every stale-allowed JSON/TOON surface and MCP per-call labels).
Then workspace build/tests, relevant MCP smoke, clippy and static CI guards,
and `cargo fmt --all -- --check`. Record exact commands and results on TASK-1526.

## Delivery

Commit `[AI:codex] feat(cache): single-flight bounded stale reads (TASK-1526)`,
push `task-1526`, open a PR to main and post SHA/tests handoff for the independent reviewer.
No self-review or merge.
