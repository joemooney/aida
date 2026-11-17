# Stale service binaries and the format-2 ledger fence

<!-- trace:BUG-2654 | ai:codex -->

The reported ENOENT did not come from a format-2 append. Pre-promotion
processes opened the old ledger lock through its intentionally dangling fence.
The old error wrapper named the ledger even though opening the lock failed.
Do not create the fence's target directory: that would admit unbound writes.

Deputy-supervised review/train controllers now resolve the configured dev live
`aida` path for each launch. The deputy loop also executes that path on runner
restart. Both explicitly set `AIDA_BIN` to that program so startup does not
inherit the old supervisor's coordinating binary. If no live installation
exists, services use the running executable's filesystem path; a missing or
non-executable image refuses. Ordinary same-seat control children retain
`/proc/self/exe` and their selector handling.

Promotion and bindings heal report stale AIDA executable images. Doctor's
`gate-ledger-bindings` category reports them when the format-2 fence is present,
under a separate report-only `stale-binaries` finding group, without automatic
process termination. This keeps manual restarts from blocking the safe bindings
bootstrap heal. The inventory identifies deleted images,
replaced filesystem images and previous live-generation images. It is a
read-only, best-effort `/proc` observation: inaccessible processes and processes
that exit during inspection may be absent; it grants no writer authority and
is not proof that every writer runs the latest build.

Existing supervisors must be restarted once to acquire this fix. Restart deputy
runners after active jobs finish, and rotate stale relays. Already running
controllers and review jobs keep their executable; this change does not replay
lost approvals or restart jobs automatically.

Targeted regressions: `bug_2654` in `aida-cli-lib` executes a selected service
path after replacement and checks missing-image refusal; a private proc fixture
checks previous/deleted images against current and unrelated controls.
