# Live drain ownership regression coverage

BUG-1683 follows the spec's sketch and advisor signoff of 2026-09-26.
Local acquisition now ignores `started_at_utc`: it is an immutable display
value, not a lease expiry. A matching live PID/start identity refuses a second
drive; missing recorded/current identity retains the existing live-PID fallback.
The shared coordination claim still uses its existing heartbeat and TTL.

`queue integrate` keeps its current-worktree configuration/focus context, but
acquires the **main worktree's** lock, matching queue work and burndown. Top-level
`integrate --run/--watch` delegates to that same handler.

## Evidence matrix

The Linux CLI fixture `aida-cli/tests/bug_1683_live_drain_lock.rs` creates its
own git/AIDA repository, approved queued task, and linked sibling worktree.
The sibling attaches the main spec store via `.aida-store` symlink because
burndown's queue-union preflight scans that path directly. Runtime `.aida`
directories remain separate; no drain lock is symlinked.
A separately running child holds the lock with its actual kernel start identity
(from production `process_start_identity`). Only the launch timestamp is
backdated two hours. Each command has an isolated environment, zero shared TTL,
no FORCE/BORROW, and no origin remote, so shared coordination is unavailable.
No real agents are used. Stubs allow only exact `--version` capability probes;
other agent invocations and git merge/rebase/push attempts record a sentinel
and fail. Child cleanup is RAII; CLI invocations have a timeout.

| Surface | Evidence |
| --- | --- |
| queue work single, batch, batches, nextN, drain, from-pr, real resume-drain | Runtime invocation of each route from main and sibling; holder-specific refusal before dispatch |
| burndown run | Eligible approved/queued task, one lane; runtime holder-specific refusal from main and sibling before worker |
| drain start | Runtime existing pre-probe refusal from both checkouts |
| queue integrate, including watch | Runtime refusal against main lock from both checkouts |
| top-level integrate run/watch | Runtime refusal against main lock from both checkouts, plus delegation check |
| autoprogress | Runtime successful skip with unchanged lock and no launch; source routing check to queue work |
| shelved drain resume | Explicit source check of resume self-invocation through queue work/from-pr |
| zen / do drain and drive modes | Explicit source checks of do → zen / queue work and zen argument builder → queue work |
| shift | Explicit source checks of wave argument builder, guarded dispatch, and actual self-invocation using those arguments |

All refusing runtime cases require the holder PID **and command**, unchanged
lock bytes, no sibling lock, and zero worker/merge sentinel invocations. Local
acquisition cases additionally require the coordination-unavailable diagnostic.
Read-only/dry-run routes are exempt. Autoprogress's intentional skip succeeds.
Alias source checks are wiring evidence, not end-to-end runs of those wrappers.

Unit tests in `drain_lock.rs` cover matching old live identity, zero stale TTL,
legacy missing identity, invalid/future display timestamps, dead PID recovery,
confirmed live-PID identity mismatch recovery, FORCE override, BORROW+FORCE
precedence with a two-hour parent and byte preservation on drop, successor
cleanup, and guard lifetime. Existing `aida-core::liveness::tests` cover an
unreadable current start identity, matching/mismatched identities, and legacy
records without changing the probe contract.

## Validation commands

```sh
cargo test -p aida-cli-lib drain_lock::tests --lib
cargo test -p aida-core liveness::tests
cargo test -p aida-cli --test bug_1683_live_drain_lock
cargo fmt --all -- --check
```

Validated on Linux: 25 lock tests, 46 liveness tests, and both CLI fixture tests
passed (26 holder refusals, one successful skip, and alias wiring). Formatting
and whitespace checks passed.

The process fixture is Linux-only (matching the existing CLI fixture convention);
macOS/Windows process-level behavior is not claimed as tested. Burndown's real
machine-readiness check remains enabled and requires sufficient host capacity.

## Accepted limits

This fixes age-based replacement of observed-live local holders. Explicit FORCE
remains an intentional acquisition override; drain start retains its earlier
live-holder pre-probe even under FORCE. BORROW observes a live parent without
replacing or releasing it.

Boolean probe errors can still return false; unreadable/malformed locks still
look absent. Shared acquisition still precedes local refusal and can update a
same-clone claim. Acquisition is not compare-and-swap/exclusive, and cleanup is
still PID-only. Concurrent acquisition/replacement/cleanup races, distributed
filesystem locking, typed uncertainty and heartbeat/schema redesign remain
outside this bounded fix. A false probe is not advertised as proof of death.
