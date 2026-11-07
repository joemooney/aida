# Advisor operations without permission bypass

<!-- trace:STORY-2571 | ai:codex -->

Status: **sketch v2 signed off; runtime implementation remains separate work**.
The owning spec's 2026-11-06 19:56 UTC advisor comment signs off v2 at `14bc7fc`
and directs one Refs PR per slice, starting with supervised review-watch.
This document originated in the sketch-only first lane requested at 17:28 UTC;
this revision remains documentation-only. Nothing here changes the deputy
contract or enables a service, permission exception, merge, grant issuance, or
process termination.
The [script inventory](../internal/agents/advisor-script-inventory.md) records
the inspected host snapshot and every proposed disposition.

Revision basis: the binding 2026-11-06 19:09 UTC advisor ruling accepts all four
findings on PR #3526. This revision extends shipped dispatch, reuses branch-update
mechanics, retires the superseded stall wrapper and includes review-watch's
respawn wrapper. The later signoff accepts that design; it does not establish
runtime acceptance or a prompt-free advisor sweep.

## Contract and dependencies

A Claude advisor launched with `aida agent new claude --role advisor
--permission-mode auto` must perform a sweep without permission bypass: bind
services, dispatch queued work, update a conflicting PR, and invoke ordinary
ship gates. AIDA owns the mechanics; the advisor retains existing authority.
Services must stop admitting work when that authority is unavailable.

BUG-2467 is a prerequisite: advisor/integrator child grants must retain only
the parent's permitted delegation set, including across rotation. Its observed
status was Needs Attention during this sketch; a related review is not evidence
that the fix has landed. Verify the landed implementation and grandchild tests
before enabling these services. BUG-2468 covers legacy deputy runner compatibility and honest unknown results
when host processes are invisible. Reuse that work; the authenticated host
observation proposed here is additional scope, not a claimed BUG-2468 feature.
STORY-2476's pipeline stall alarm is Completed; retire `stall-check.sh` as
superseded without duplicating its checks.

Existing surfaces to reuse:

- `advisor_cmd_root_args.rs` / `advisor_cmd_root.rs`: advisor command routing.
- `deputy_shell/{socket,attribution,runner}.rs`: peer attribution, live submitter
  validation, grant refresh, audit and host execution.
- `dispatch.rs`, `dispatch_args.rs`, `dispatch_flow.rs` and
  `gate/dispatch_auth.rs` (STORY-1917, PR #3206): shipped typed, mandate-authorized,
  deduplicated `aida dispatch request impl|take|flow|review|train`,
  `list/show/cancel/tick` and host executor `aida dispatch serve`. Reuse its
  request/result records, claim lock, recovery ledger and execution revalidation.
- `pr_rebase.rs` / `rebase_cmd.rs`: `aida pr rebase` metadata, temporary-worktree
  and conflict-reporting mechanics; its rebase and force-push-with-lease remain
  a separate operation from the proposed local merge.
- `review_watch_update.rs`: server-side `gh pr update-branch` and typed
  `UpdateFailure::refused`; `forge.rs::github_update_branch_argv`: expected-head
  pinning; dispatch flow already runs the server-side branch update.
- `gate/seat_authority.rs`: grant validity and delegation checks.
- `review_watch*.rs`: persisted admissions, exact-head receipts and reservations.
- `aida-core/src/liveness.rs`, `leases.rs`, `status_cmd.rs`: common liveness
  semantics and their readers.
- Ordinary queue, native publication, train, ship and merge-hold commands remain
  the only authorities for their respective transitions.

## Proposed command surface

Existing dispatch commands below remain the command family. Slot and condition
options, resume intent, loop binding and host-merge are proposed extensions;
this table does not claim those extensions are shipped.

| Command | Effect |
| --- | --- |
| `aida advisor loops status [--json]` | Report desired/running state, binding generation, authority health, pending jobs and last failures. |
| `aida advisor loops rebind` | Bind enabled services to the authenticated current advisor generation; idempotent for the same binding. |
| `aida dispatch request impl\|take\|flow\|review\|train ...` | Extend existing typed requests with queue/dependency conditions and explicit resume intent; retain per-request `--vendor` where supported. |
| `aida dispatch list/show/cancel/tick` | Reuse request inspection, cancellation and scheduling; expose blocked conditions, capacity and receipts through existing records. |
| `aida dispatch serve [--slots N]` | Supervise the existing grant-bound host executor; proposed slot cap is bounded by trusted policy. |
| `aida pr host-merge N [--json]` | Proposed local-merge fallback after server-side update refusal; return a job id and any conflict report. |
| `aida advisor ops log JOB [--tail N]` | Read a bounded, authorized result/transcript through the host service. |

Service enablement, maximum slots, vendors and project roots are installed by
the operator in trusted policy. `rebind` cannot enable another service, raise a
cap, widen delegations, or change a deputy's TTY policy. A fresh binding resumes
only previously enabled services. An operator pause persists across rotations.

## Host supervisor and grant rotation

Supervise the existing service controllers and `aida dispatch serve` through
fixed host operations in the operator-started deputy. Retain its TTY-only startup
and current grant/mandate restrictions. Service requests select fixed operations,
never arbitrary argv, script bodies, executable paths or environment overrides.
Existing generic script submission retains its separate contract.

Do not introduce a new supervisor ledger or a parallel advisor dispatch queue.
Extend existing dispatch request/result/claim/recovery records under
`.aida/dispatch/` and their permanent claim lock with the bounded slot accounting
and binding-generation evidence they need. Keep review reservations in the
existing review-watch runtime state, and train evidence in its existing owner.
Loop status projects these service-owned records; it is not another source of
admission authority. Persist desired state, validated binding generation and
worker identity with each owning service's existing state. Grant secrets never
appear in status or logs. Unknown schema, unsafe files, ambiguous identity and
lock timeout refuse writes.

The advisor launcher/rotate relay sends an authenticated rebind after the new
generation's grant is established. The host validates peer attribution, project,
seat, roster, parent chain, expiry and allowed delegations at acceptance and
again at each work admission. It compares generations under each service owner's
existing lock; a delayed old session cannot replace a newer binding. A bare advisor
registration UUID is informational and cannot authorize a rebind.

Do not edit another process's environment or discover grants by scanning
`/proc/*/environ`. Stop admission for the old generation first; reconcile its
receipts, then restart service controllers under new child grants. An old worker
is signalled only through its verified boot/PID-namespace/start identity. An
unconfirmed exit or ambiguous receipt retains its reservation and blocks a
replacement. Long-running children remain bound to their original grants and
cannot be retroactively reparented; if invalidated, use their existing failure
and recovery paths. Preserve active reviewer reservations across controller
restart and never issue a second review for an unresolved prior attempt.

If the old binding expires before a new generation arrives, services become
`blocked-authority`. Rotation can recover automatically when a valid narrowed
successor arrives; neither host availability nor a remembered advisor handle
permits minting grants from an expired/revoked parent. Deputy grant refresh is
separate from advisor grant refresh. Loss of deputy authority/mandate stops
admission under the existing policy; recovery may require the operator.

AC2's supervised services are review-watch, auto-rework, auto-train and
`aida dispatch serve`. Serve inherits its launching advisor terminal's grant;
it must participate in the same authenticated rotation/rebind, rather than
continuing with an expired launch binding. Rebinding serve does not rewrite
pending requests' requester grants or bypass `DispatchExecutionContext`
revalidation; requests whose original authority is invalid remain refused until
an authorized fresh request is admitted through ordinary dispatch.

Replace both `launch-review-watch.sh` and `review-watch-loop.sh`. At the original
inventory snapshot, one PR refusal could exit `aida review watch`. BUG-2471 is
now Completed (landed via PR #3536); `review_watch.rs` records per-PR refusals
and continues the pass. Build the supervised service on that behavior, retaining
bounded retry/backoff rather than adding another respawn loop. If
the controller exits, reconcile reservations before supervised restart. Authority
loss still blocks admission; neither a restart nor a per-PR refusal clears
ambiguous review capacity. Persist exact PR-head deduplication and train ejection
evidence across restarts.
Each tick uses typed forge observations: failed/partial reads remain unknown,
never an empty ready set or proof of success. Rework checks current verdict/CI
and canonical spec ownership; train uses ordinary readiness and hold gates.
Do not port shell title regexes or string-parsed CI decisions as authority.

## Dispatch and publication

Extend STORY-1917's dispatch scheduler and host executor, rather than adding
`aida advisor dispatch`. Reuse mandate/human-gate checks, typed verbs, request
idempotency, claim recovery and `DispatchExecutionContext` revalidation. Existing
per-request vendor selection for impl/take/review/flow remains the typed input;
add explicit resume intent and queue/dependency conditions to those records,
without importing arbitrary launch-script options. Inspect these additions via
existing list/show results, preserving existing status vocabulary.

Under the existing dispatch claim lock, reserve capacity before launching. Count
durable dispatch reservations and verified live implementer leases, including lanes launched
elsewhere; deduplicate the same child across those sources. Enforce the
configured cap plus existing global queue/seat admission limits. Unknown
liveness and an accepted job with no resolved launch receipt retain capacity.
Two fillers and retries of the same request cannot consume two slots.

The host invokes ordinary `queue work` launch logic using the validated advisor
binding, requested allowed vendor and existing containment posture. Admission
revalidates deferral, dependency, ownership, lease and grant gates. Do not port
the scripts' automatic `--force-claim`, branch-name guessing, or lease-ending
fallbacks. A blocked request stays visible with canonical recovery guidance.
Store run-specific logs and launch/publication receipts; no log truncation or
matching a generic `launching` line establishes success.

Dependency waiters become persisted queue conditions evaluated at admission;
unknown dependencies stay blocked. Explicit resume intent must name the existing
lane/session and pass ordinary resume ownership/lease gates; it cannot implicitly
force-claim, end a lease or create a second worker. Native lane exit flows through
the existing host publication path, preserving punt/hold vetoes, failed-exit Refs,
and declared partial subjects. The dispatch controller cannot infer completion
from a PR title or publish work held back by its author. `push-held.sh` is retired
without reproducing its override. Queue completion follows successful host
publication under the existing contract.

## Host merge and conflicts

Keep `update-branch` as the existing GitHub server-side merge in review-watch
and dispatch flow. Reuse `review_watch_update.rs` and its typed refusal, plus
`forge.rs::github_update_branch_argv` for expected-head pinning; retain ordinary
`aida dispatch request flow` orchestration. `aida pr rebase` already provides a
temporary-worktree rebase and force-push-with-lease. Reuse its metadata,
worktree and conflict-reporting helpers where applicable, without changing its
history-rewriting semantics.

The only new branch mutation is `aida pr host-merge`: a LOCAL merge and conflict
report when GitHub's server-side update refuses, covering BUG-2456's Codex seats
with read-only `.git`. It runs on the authorized host; it grants no Git write
access to the seat. A transient/unknown forge result is not a refusal and must
not silently trigger this fallback. No new `aida pr update-branch` command is
introduced, and local merge never adopts rebase's force-push behavior.

A fixed host operation resolves the PR through the configured forge, verifies
same-repository branch ownership and pins both the PR head and default-branch
tip. Serialize against other branch writers and refuse active author/merge
ownership conflicts. Carry forward the `host-merge.sh` guard added for
STORY-2537 on 2026-11-06: inspect seats/worktrees on the target branch and refuse
if any seat holds commits not present on the remote. Bring those commits onto
the PR through ordinary publication before retrying; merging onto the stale
remote head would strand the lane's work. Unreadable seat/ref evidence refuses.
Recheck this guard at the push boundary under branch-writer serialization.
Fetch explicit refs into job-private refs; create a unique
scratch worktree with recorded identity. Never reuse `/tmp/hmN` or forcibly
remove another job's worktree.

Merge the pinned default tip into the pinned PR head. An already-contained tip
is a successful no-op. A clean merge uses a non-completing `(Refs ID)` subject
from canonical change credit. Before ordinary non-force push, re-read PR
identity/head, binding authority and punt/hold signals; refuse a moved head and
retry from fresh evidence. A failed push is a failed job, never `merged clean`.
The new head requires ordinary fresh review and CI; this operation grants no
merge clearance or completion credit.

On conflict, publish a bounded report with pinned SHAs, conflicted paths, job id
and retained scratch identity. Never auto-resolve semantic conflicts. An advisor
can dispatch an explicitly scoped resolution lane; any later host publication
must revalidate the expected head, scratch identity and existing gates. Retain
conflicted/dirty work until explicit recovery; cleanup compare-checks ownership.

## Liveness, logs and sandbox transport

Build an authenticated host observation, alongside BUG-2468's runner fixes,
for both `ps` and session leases,
including registry and review-controller records. Requests identify records,
not arbitrary PIDs or paths. The host correlates host/boot/PID namespace/start
identity and returns observed live/dead with observation time. A bounded timeout,
foreign host, missing identity or unreachable host remains explicit unavailable
or unknown evidence and never authorizes cleanup. The healthy host-backed path
eliminates foreign-pidns unknowns; outages cannot honestly promise certainty.
Do not set `AIDA_TRUST_PID_NAMESPACE=1` or reinterpret heartbeat age as death.
Destructive callers obtain fresh evidence again at their own mutation boundary.

The current script-deputy socket is blocked in a sandbox. The existing documented
escape is a narrow `excludedCommands` entry for submit/cancel. Propose equivalent
exact AIDA front ends for these fixed operations and bounded result reads, with
host peer attribution preserved. Do not exempt all `aida`, `bash`, `gh`, or unix
sockets, and never add these exemptions to contained implementer/reviewer lanes.
No raw shell-script bridge is added for this feature. The implementation review
must verify prefix/argument matching cannot reach unrelated commands. If Claude
auto mode still prompts for a proposed entry, record it as an evidence failure;
do not silently disable the sandbox or widen the exclusion.

Host log reads resolve only authorized job ids with bounded output and safe file
handles. They do not expose arbitrary HOME paths, grant files or environment
contents. Forge check/log reads and remote reconcile reuse ordinary commands via
existing deputy authorization; no new credential or remote-mutation authority.

## Delivery and evidence gates

1. Advisor signoff is recorded at 2026-11-06 19:56 UTC for sketch v2 at
   `14bc7fc`, including service binding succession, fixed operation transport,
   slot accounting and conflict recovery. Its delivery direction takes
   precedence over the numbered order below: start with the supervised
   review-watch slice, build on BUG-2471, and publish one Refs PR per slice.
   Confirm BUG-2467 and BUG-2468 integration before dependent runtime work.
   Remaining steps describe the required work and evidence, not permission to
   skip prerequisites when starting the first slice.
2. Implement authenticated host liveness and bounded read transport; test healthy
   and unavailable hosts from a real nested PID namespace.
3. Extend service-owned state and rotation handshake, including dispatch serve;
   test concurrent rebinds, stale successors, revocation, narrowed delegations, corrupt state, crashes
   between reservation/spawn/receipt and recovery with active reviews.
4. Extend existing dispatch and publication integration; test competing fillers,
   pre-existing lanes, blocked/unknown dependencies, per-request vendor and resume
   admission, ambiguous spawn and punt/hold vetoes. Assert existing mandate,
   requester and serve-grant checks survive rotation.
5. Implement local host-merge fallback; test server-update success versus refusal
   versus unknown, clean/no-op/conflicting local merges, moved heads, failed
   pushes, dirty recovery, stale grant, author ownership conflicts and seats with
   unpushed commits (including a seat advancing before push).
6. Move rework/train decisions into typed ticks with exact-head deduplication;
   keep ordinary hold/ship gates, including mandatory human-clear cases. Test
   that a single PR refusal leaves supervised review-watch serving other PRs,
   with bounded retries and preserved reservations across controller restarts.
7. Run the operator-authorized evidence session using `--permission-mode auto`.
   Record build/config identity, narrow exemptions, launch command, grant rotation,
   service receipts, dispatch id, conflict report/resolution, reviewed new head,
   CI/ship outcome and every prompt (tool, requested permission, decision).
   Zero prompts and no bypass are the acceptance target, not a claimed result
   of this sketch. Poll any long waits in steps no longer than 60 seconds.
8. Retire inventoried wrappers only after callers migrate and active jobs finish.
   Archive receipts, not live scripts as an alternative control plane. Rollback
   pauses new service admission and preserves jobs/reservations; it does not
   revive expired grants or restart old shell loops automatically.

This lane validates documentation and inventory completeness only. It does not
satisfy runtime acceptance criteria 2–6 or establish a prompt-free live sweep.
