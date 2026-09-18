# Night-shift post-mortem: 2026-09-17/18

<!-- trace:SPIKE-82 | ai:codex -->

## Scope and evidence

This report covers 18:00 September 17 through 08:00 September 18, 2026,
America/Phoenix (01:00–15:00 UTC). Counts below were recomputed from the
newline-delimited event ledger, rather than copied from session narration.

Primary evidence:

- `/home/joe/ai/aida/.aida/events.jsonl`, lines 3466–3791 for the bounded
  window. It records 17 `PrMerged`, 15 `SpecShelved`, seven `QueueDrained`,
  four `AdvisorEscalated`, 30 `RunStarted`, and 32 `CiTerminal` events across
  24 non-empty spec IDs. The 15 shelves split into ten
  `verdict:request-changes`, three `ci-red`, and two `tool-exit`.
- `git log --first-parent` on `/home/joe/ai/aida` records 28 commits reaching
  main in the same window. This is the authoritative main-throughput count;
  the 17 `PrMerged` events are the subset whose merge path emitted that event.
- The protocol proposal is preserved in
  `/home/joe/ai/aida/.aida/mailbox/ed24af1d-013e-4aef-a0b6-5763391c3514.json`;
  its acknowledgement is in
  `/home/joe/ai/aida/.aida/mailbox/a9a1a572-2433-4791-ae27-def498d4fd13.json`.
- Wave-boundary reports are in
  `/home/joe/ai/aida/.aida/mailbox/f62d3dfd-9802-4c32-86e4-41d402dc8308.json`,
  `/home/joe/ai/aida/.aida/mailbox/be831332-6f9f-4c74-a867-ac75ade2e2b2.json`,
  `/home/joe/ai/aida/.aida/mailbox/d7f38cf3-49f2-4f2a-a13f-31bbb80024ae.json`,
  and `/home/joe/ai/aida/.aida/mailbox/5133a8ed-da28-46d6-be89-2487e47303a8.json`.

The five named batch tags now contain 8, 8, 4, 2, and 3 closed specs
respectively (`followups-0917`, `followups-0917b`, `followups-0918`,
`followups-0918b`, `followups-0918c`). Tags overlap with work begun before the
window and do not map one-to-one to the 24 IDs observed in-window.

## Verdict

The night succeeded because the substrate made work resumable and observable,
while two live seats supplied the scheduler and exception handler that the
substrate did not yet contain. It was not a fully unattended success. The
event log, queue, drain lock, bounded waves, typed shelves, independent review,
and merge hold kept work safe and recoverable; continuous human/agent attention
kept choosing the next action quickly enough to sustain throughput.

### Hypotheses

1. **Held: two seats plus one explicit gate beat a single undifferentiated
   seat.** At 21:38 the advisor assigned drain launch/relaunch to the product
   seat and retained reviews, merges, harvest confirmation, and manual shelf
   pickup. The product seat explicitly acknowledged that it would not re-drive
   work whose hold belonged to the advisor. The commit history also shows
   reciprocal authorship at the gate: the advisor-authored BUG-1195 and
   BUG-1198 commits were reviewed/merged by the other seat, while Codex-authored
   changes flowed through the advisor gate. No evidence reviewed for this
   report shows an author approving and merging its own change.

2. **Held, with a wording correction: small serial waves, not every individual
   batch, stayed in the 4–8 range.** The named tags have 8/8/4/2/3 closed
   members. The last two were intentionally tiny cleanup waves after the
   backlog had contracted. Seven `QueueDrained` events show repeated bounded
   drains instead of one unbounded run. The single drain lock prevented
   concurrent mutation, and `--escalate-blocks` converted supervised work into
   four advisor handoffs. `--max-failures` was a safety bound, but this window
   does not isolate its causal effect because no recorded wave ended by
   exhausting it.

3. **Mostly held: event-driven supervision kept the loop moving, but the
   “persistent Monitor” was implemented as an awake seat's watchers.** The
   protocol mail says the product seat already watched the lock; its reply says
   an exit watcher would relaunch from the implementer queue. Boundary mail at
   02:10, 03:45, 05:08, and 06:00 shows the seat reacting to drain completion,
   shelves, and new work by rebuilding and launching the next wave. This is
   event-driven behavior rather than fixed-clock polling. It is not proof of
   autonomous supervision: if that session disappeared, the watcher and its
   judgment disappeared too.

4. **Partially held: presence/check-ins supplied a heartbeat, but causality is
   not independently measurable from the ledger.** Mailbox check-ins coincide
   with each wave transition and prevented ambiguity about gate ownership.
   They clearly turned idle boundaries into launches and triage. The event log
   does not record `/goal` state or counterfactual idle time, so the stronger
   claim that the goal loop itself caused the throughput remains unproven.

5. **Held: live dogfood fixes improved later waves, at a real cycle cost.**
   Failures were converted into concrete bug specs, fixed against the running
   system, rebuilt, and exercised by later waves. The final two waves shipped
   2/2 and 3/3 with no shelves. That trend is consistent with the fixes, though
   the smaller/easier tail means it is not a controlled comparison.

## Reliability-fix cycle accounting

“Round” below means one orchestrator run/attempt visible in the event ledger;
for the pre-window causal incident, the source spec and mailbox give the
attempt count.

| Fix | What exposed it | Cost before landing | Result |
| --- | --- | --- | --- |
| BUG-1195 | BUG-1193's reviewer envelope failed twice even though its review story was queued | One affected reviewer phase with two tool attempts; BUG-1195 itself was fixed at the keyboard and merged at 18:02, before this window's first recorded run | Reviewer lookup now distinguishes lookup failure from a genuine missing story |
| BUG-1218 | BUG-1207 stale-base preflight left a scratch worktree after a refused force-push; retry was then poisoned by the checked-out branch | One BUG-1207 drain round with two reviewer attempts exposed it; BUG-1218 then reached the advisor gate in one implementation/CI/review run | Scratch cleanup and typed refusal removed the deterministic retry poison |
| BUG-1213 | BUG-1197 rework advanced with no new commit and could repeat unchanged findings | Five BUG-1213 runs: two `ci-red`, two `verdict:request-changes`, then one `AdvisorEscalated`; merged at 04:55 | Rework no-ops are detected and repeated identical findings escalate instead of looping |

BUG-1213 is the strongest evidence for “fix at the keyboard, then dogfood”: the
03:45 boundary report names two genuine failing tests, the following attempts
exposed two review defects, and the fifth run reached the advisor rather than
silently cycling. It also demonstrates why the night was not unattended.

## What was substrate, attention, and luck

**Substrate:** durable queue entries and tags; a single global drain lock;
isolated worktrees; typed lifecycle events; CI/reviewer phases; shelf and
rework state; advisor escalation; merge holds; and mailbox records. These made
failures inspectable and made relaunch safe enough to repeat seven times.

**Live attention:** selecting defensible rework versus genuine human need;
rebuilding after fixes; relaunching as soon as the lock freed; independently
reviewing supervised changes; hand-reconciling BUG-1207; removing an invalid
spike from an autonomous wave; rejecting auto-drafted phase-record bugs; and
feeding newly discovered work into the next wave.

**Luck/environment:** both interactive sessions stayed alive on one shared
machine; mailbox lag did not hide a decisive request long enough to stall the
night; the lock owner remained observable; and later waves were smaller. A
shell watcher and process-name/liveness heuristics are not durable scheduling.
The result therefore depended on presence even though correctness did not
depend on wall-clock timing.

## 18:00 operator runbook

- [ ] Pull main, rebuild the exact `aida` binary that the drain will execute,
  and run `aida dev status` to rule out stale-binary behavior.
- [ ] Set distinct seat identities and roles. Name one launch/triage seat and
  one independent advisor/merge gate; neither merges its own code.
- [ ] Review the implementer queue. Keep guided, keystone, credential-bound,
  and operator-choice work out of the autonomous set.
- [ ] Tag a bounded wave (normally 4–8 specs), record its member IDs, and make
  execution mode explicit (`drain` versus supervised `drive`).
- [ ] Mail the protocol before launch: who owns relaunch, who owns reviews and
  merge holds, who may rework shelves, how stepping away is handed off, and
  which items must remain parked.
- [ ] Launch one drain with bounded failure policy and block escalation, for
  example `aida queue work --batch <tag> --auto-complete --no-human=both
  --max-failures <n> --escalate-blocks`. Never bypass a live drain lock.
- [ ] Watch `events.jsonl` from EOF for `PrMerged`, `SpecShelved`, `CiTerminal`,
  `AdvisorEscalated`, `QueueDrained`, and `UnshippedWorkDetected`. Also watch
  mailbox unread age and the actual drain lease/process identity.
- [ ] On a shelf, classify it once: concrete mechanical correction → rework;
  unchanged/repeated finding → advisor; genuine decision/credentials → park;
  substrate failure → file/fix/rebuild before re-driving.
- [ ] At each `QueueDrained`, reconcile shipped/shelved counts against the
  tagged membership, confirm ledgers and holds, then launch the next bounded
  wave only after the lock is free.
- [ ] Stop when no drain-safe work remains. Report operator-only items; do not
  manufacture filler.

## Gaps required for a no-awake-seat night shift

These are requirements for `STORY-1218`, not optional polish:

1. **A durable shift tick.** A systemd user timer (or equivalent supervisor)
   must inspect queue, lease, process, event cursor, and mailbox state and
   launch/relaunch without an interactive shell remaining alive.
2. **Event cursor plus idempotent actions.** Persist the last consumed event
   and an action key so restart cannot double-launch, double-rework, or repeat
   mail. Wake on event append; use periodic ticks only as crash recovery.
3. **Lease-based liveness.** Replace `pgrep`/name matching with PID plus process
   start identity, heartbeat, command/run UUID, and an expiry/reap policy. A
   live lease, not a matching process string, owns the lock.
4. **Policy-coded shelf triage.** Automatically re-drive only typed, bounded,
   demonstrably transient failures. Escalate repeated finding hashes,
   `rework-no-op`, permission boundaries, architecture work, and missing
   credentials. Cap attempts per spec and per shift.
5. **Mailbox latency enforcement.** Emit `MailReceived`, track oldest unread
   age, and escalate/reassign when the gate misses its service window; do not
   assume a recipient session is reading.
6. **A durable advisor gate service.** Preserve author/reviewer independence
   and merge holds without requiring an advisor terminal. The service may
   release only after an independent verdict, required CI, and harvest/ledger
   checks; judgment-required changes remain held for morning.
7. **Version pinning and rebuild handoff.** A running wave must identify its
   binary build SHA. A newly landed substrate fix should trigger a controlled
   rebuild and affect only the next run, with the transition recorded.
8. **End-of-shift reconciliation.** Produce a machine-generated report from
   event counts, queue membership, open PRs, holds, findings, unread mail, and
   remaining operator-only work. Flag the current merge-event coverage gap:
   28 main commits versus 17 `PrMerged` events in this window.

Until those gaps are implemented, the honest operating claim is: AIDA can run
an overnight supervised drain with high throughput and strong recovery, but it
cannot yet run the same loop with no interactive seat awake.
