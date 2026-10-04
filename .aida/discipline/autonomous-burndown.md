# Autonomous backlog burn-down

The pattern for draining a backlog of ready work **without a human in the loop** — and, just as important, the discipline that keeps it from stalling. This is the *why and the rules*; the encoded command that runs it is `/aida-burndown` (see "The command" below).

## The problem it solves

The failure mode is not "the agent can't do the work" — it's that the agent *stops*: every few items it hits a fork and asks a question, or declares it can't proceed, and the drain dies waiting for a human who isn't there. Looping a `/goal` prompt, or begging the agent to keep going, does **not** fix this. The fix is operational discipline plus the right machinery — encode the rules into the runner so "don't stop" is structural, not a thing the agent must remember (see [`substrate-as-bouncer.md`](substrate-as-bouncer.md)).

## The pattern that works

1. **Front-load decisions once.** Disposition the backlog so the ready set is decision-free; tag decided-and-buildable specs. A spec with an unresolved design fork is *not* ready.
2. **Fan out implementer subagents in parallel**, each isolated in its own git worktree, each taking **one** bounded ready spec end-to-end to a PR (read spec → implement to acceptance → trace markers → build + test + fmt → commit → push → open PR). Worktree isolation means parallel agents never collide.
3. **The main session is the integrator** — it does *not* implement. It polls the PRs, merges the green + clean ones, reconciles them to Completed, pulls, and launches the next wave.
4. **Loop it event-driven.** Wait on the wave with the harness `Monitor` tool over the drain's wake feed — `Monitor(command: "aida watch --emit-wakes", persistent: true)` — which wakes the session **only** on an actionable verb (a PR shipped/merged, a CI verdict, a punt, a shelve, the queue drained) and stays silent through the benign phase churn, so the supervisor burns **zero tokens** between events: supervision cost is O(actionable-events), not O(time-elapsed). Keep a **long-interval** scheduled wake-up (30–60 min) as the degenerate fallback so a wedged or event-less watcher still resurfaces the loop.

## The non-negotiable rules

- **Pickability gate on every spec.** Only fan out work that is **ready + unblocked + bounded** (no unresolved design fork). A spec that fails the gate is skipped or parked — never dragged in. This is what makes "never stop to ask" *safe*: the runner only dispatches work that can go end-to-end without a human.
- **Punt-and-continue.** A blocker parks **one** spec (tag it + leave a note) and the pipeline rolls on. One spec's blocker must **never** halt the pipeline.
- **Never stop to ask / never down tools.** "I can't make further progress" is almost always false — there are other ready specs; go work one. For a fork: make the defensible call, or park that one spec and move on.
- **CI gates `main`.** A bad change parks (CI red → that PR doesn't merge); it never reaches `main`. This is what lets the integrator merge greens without re-reviewing each by hand.
- **Don't fan out coupled work — route it to a batch drain mode.** The parallel fan-out is for INDEPENDENT specs. A set that shares files or must land in order goes through the sequential batch drain instead of N colliding worktrees, and instead of hand-driving a `git reset --hard origin/main` between members: tag the members `batch:NAME`, then `aida queue work --batch NAME --auto-complete --sequential` (ordered, each member its OWN PR off freshly-pulled main; the flag governs ORDER and PR shape, not concurrency — one member at a time at the default `[drain] pipeline_depth = 1`, which only a single-batch drain can raise, and a `--batches A,B,C` chain is serial at any depth) or `--single-branch` (all members accumulate on ONE branch → ONE cluster PR). The failure rule differs by mode: **`--sequential` shelves the failed member and continues; `--single-branch` halts** (later increments build on earlier commits, so it stops rather than build on broken code).
- **Keep at the keyboard, not the drain:** releases/tags and changes to the autonomy machinery itself (the orchestrator, the runner) — those ship supervised, because a fix riding through a broken drain gets caught in the breakage.

## The command

`/aida-burndown` encodes this loop so it runs the same way every time, rather than depending on an agent to remember the rules. It takes a **flexible target**, all funnelled through the one pickability gate:

```
/aida-burndown --batch <name>      # a cluster
/aida-burndown --tag <tag>         # by tag
/aida-burndown --status approved   # the ready backlog (default)
/aida-burndown --queue             # the active role's queue
/aida-burndown "<description>"      # ad-hoc → resolved to a filter
```

`aida` resolves the ready+bounded set; the skill drives the worktree-isolated fan-out + integrator loop with punt-and-continue. Default target: the ready backlog for the active role.

## Harness scope: this fan-out is Claude-only

The parallel fan-out engine is **Claude-Code-harness-only.** Step 2's wave is the harness's native subagent fan-out — `Agent(subagent_type: …, isolation: "worktree")` — a primitive only the Claude Code harness provides. A non-Claude vendor (Codex, Cursor, Amp, a bare `claude -p` script) has no equivalent, so it **cannot** run this loop.

Those vendors drain the same ready set the **serial** way instead: `aida queue work --auto-complete` one spec at a time (add `--batch NAME` to walk a cluster member-by-member). Same lifecycle — implement → CI → review → merge → pull → build — but sequential rather than a parallel wave, and vendor-agnostic because the **orchestrator**, not the harness, owns the drive. The trade is the usual one: lower throughput, universal reach. (SPIKE-74 tracks the agent-agnostic drain backend — a drain engine behind a trait — that would let non-Claude vendors fan out too; until it lands, fan-out is Claude-only and serial is the fallback.)

## Relationship to the orchestrator drain

This is the **recommended** autonomous-drain path. It deliberately uses the harness's native subagent fan-out rather than `aida queue work --auto-complete` (the orchestrator-spawns-agent path). The two are **not** competitors: the orchestrator drain is hardened in parallel; `/aida-burndown` is the path to reach for now. Don't run both against the same set and wonder which to trust — pick `/aida-burndown` for hands-off backlog draining, and use the orchestrator drain where its single-spec lifecycle is what you want.

See also: [`workflow-patterns.md`](workflow-patterns.md) (when a fix must ship at the keyboard, not the drain), [`backlog-grooming.md`](backlog-grooming.md) (getting the ready set decision-free first), [`substrate-as-bouncer.md`](substrate-as-bouncer.md) (why the rules are encoded, not documented-and-hoped).


## Migrated Lessons

### feedback_batch_drain_ignores_approved_work_outside_the_batch

2026-09-19: throughput sat at ~0.75 merges/hour and the operator asked why. The cause was not the implementer (6.5 min median) and not CI (60 runs, 0s queue wait). My standing launcher drove `queue work --batch followups-0918h`, and that batch had drained to 3 open members while 16 approved, routed, mode-groomed specs — the whole next wave the advisor had already groomed — sat in the same `role:implementer` queue outside the filter. Every relaunch re-drove the same 2-3 specs; the last wave reported `shipped:0 shelved:2`. Switching the launcher to the current batch took the wave from 3 members to 18.

**Why:** `--batch` filters before the drain looks at the queue, and neither the start-up banner nor `aida drain status` mentions the queue the batch was drawn from. A batch that has drained to near-empty is indistinguishable from a healthy small batch. Filed as BUG-1284 (report the excluded count at start-up, in drain status, and on the QueueDrained event).

**How to apply:**
- Every time you start or restart a batch launcher, diff the two sets first:
  `comm -23 <(aida list --status approved --fields id | grep -oE '(BUG|TASK|STORY)-[0-9]+' | sort -u) <(aida list --tags batch:NAME --fields id | grep -oE '(BUG|TASK|STORY)-[0-9]+' | sort)`
- Re-run that diff after every advisor grooming pass — a new wave gets a new `batch:` tag and the launcher does not follow it.
- Join in-flight work to the new wave with `--add-tag`, never `--tags` (see [[feedback_aida_edit_tags_replaces_use_add_tag]]).
- `shipped:0` on a wave with a deep queue is the fingerprint. So is a launcher log line whose "N queued" count never changes.

Related: [[feedback_dont_declare_drained_from_filtered_view]], [[project_burst_usage_robustness_over_speed]], [[feedback_substrate_first_never_rely_on_agent_awake]].

### feedback_charge_forward_autonomously

The user explicitly granted standing autonomy and demanded pace: *"proceed with your best effort, no need to wait for my approval … we need to pick up the pace and stop relying on me … you are losing ground by constantly waiting for me … Charge forward without me."*

**Why:** the bottleneck was the advisor pausing for approval after nearly every item. That cadence is too slow for the project's stakes (needs real users / momentum; over-deliberation = irrelevance). Speed of shipping correct work matters more than per-step confirmation.

**How to apply:**
- **Don't pause for approval on routine work** (bug fixes, triage, ships, cleanup, captures). Pick the next on-phase item, do it, ship it, move on.
- **Keep the quality gates** — verify-before-fixing (many specs turn out stale; cheap to check), build + `cargo fmt --check` + tests green, proven-fail-without-fix on regressions, the `aida pr ship` arc. Speed ≠ recklessness; the gates are what make autonomous shipping safe.
- **Report in batches, not per-item** — surface progress periodically (what shipped, what's next), not a "shall I proceed?" before each step.
- **Reserve genuine check-ins** for: irreversible/destructive actions, architecture-class decisions with real forks the operator owns, or when verification reveals the work is materially different from the ask. Everything else: best judgment, charge forward.
- Stay on-phase ([[project_bugs_before_marketing_phase]]: bugs → stability → marketing). Defer off-phase work rather than stopping to ask.

Refines the advocacy stance in [[feedback_advocate_not_be_passive]] — and supersedes the cautious per-item-confirm habit. The license is standing until the user says otherwise.

### feedback_dont_declare_drained_from_filtered_view

Two compounding errors caught by the operator (2026-06-01) when I declared the
bug backlog "drained" and stopped the burn-down loop.

## Error 1 — concluding "drained" from a filtered view
I had run `aida list --type bug --status approved` and triaged only that. That
silently excluded **every Draft** (BUG-431 + BUG-422 were High-priority drafts)
**and every non-bug** (the whole approved high-pri story/spike/task list,
including TASK-615 — work I had planned + parked *that same session*). The
backlog was nowhere near empty; I was looking through a keyhole.

**Why:** an "empty" filtered result reads as "no work left," but the filter is
doing the emptying. A false "drained" stops productive autonomous work and
reads as "nothing to do" to the operator — the same silent-wrong failure mode
as a stale cache (cf [[feedback_verify_edits_landed_before_claiming_done]]).

**How to apply:** before saying a queue/backlog is exhausted, scan the FULL
open set — `aida list --status draft` AND approved, across **every type**
(bug/task/story/spike/epic), not just the slice you were working. Re-scan
(don't trust an earlier snapshot); new items (incl. external reports) land
mid-session. High-priority **drafts** are real work, not noise.

## Error 2 — over-excluding keystone work from "the loop"
I treated "orchestrator/autonomy-keystone → ship at keyboard"
([[feedback_reliability_fixes_use_keyboard_not_drain]]) as "can't touch in the
loop." Wrong: **the loop / autonomous burn-down is the advisor working
SUPERVISED** — reading, editing, unit-testing, and shipping each fix directly
with verification. That IS the keyboard-supervised mode the discipline
endorses. What that discipline actually excludes is running the *unsupervised
`--no-human` drain* to ship a fix that rides through the broken machinery
(recursive-failure risk). So keystone/orchestrator code DOES qualify for the
loop — done carefully, with tests, and validated by a *supervised*
`--zen --auto-complete` drain rather than `--no-human`.

**How to apply:** when triaging loop-eligibility, the real disqualifiers are:
(a) un-verifiable without infra I lack (e.g. CI-load flaky-test repro),
(b) a genuine architectural fork needing the operator's decision, or
(c) release-/recurrence-gated. "It's keystone code" is NOT a disqualifier —
it's a "do it carefully + supervised-validate" flag. Decompose big keystone
specs: bounded slices (a resolver heuristic, a timeout) are loop-suitable even
when the architectural root needs a sign-off.

## Composes with
- [[feedback_verify_before_filing]] / verify-before-claiming — same root: don't
  conclude from a partial probe.
- [[feedback_reliability_fixes_use_keyboard_not_drain]] — refined here: the
  exclusion is the *unsupervised drain*, not all keystone work.
- [[feedback_charge_forward_autonomously]] — keep finding + shipping; a
  filtered false-empty is not a license to stop.

### feedback_oversight_supervisor_layer_smooths_drains

Operator directive (2026-09-12): "a breakthrough this past week was your oversight of the process… I don't want to lose whatever magic we stumbled upon that helped this automated drain work more smoothly without my intervention or needing to communicate back and forth between agents." KEEP DOING IT until a better formalized strategy exists.

**What the "magic" actually was:** a third layer above the drain. Not the advisor (who DRIVES) and not the implementer (who EXECUTES) — an OVERSIGHT/meta-supervisor (I ran it from the product seat + monitors + mailbox) that:
- watched drain EVENTS (event-driven, not timer-poll) for stalls / false-parks / misalignment;
- caught the drain working the WRONG thing (e.g. 2026-09-12: a drain grinding bugs while the operator's PRIMARY objective EPIC-63 sat approved-but-unqueued — the oversight queued the children + realigned it);
- nudged/coordinated the advisor through the SUBSTRATE (mailbox + the single drain lock), NOT by relaying through the operator;
- triaged failures, salvaged parked work, rebuilt binaries when fixes merged;
- surfaced to the operator ONLY the things needing a human DECISION.

**Why it worked (the ingredients to preserve):**
1. Event-driven monitoring over timer-polling (cheap, wakes only on actionable events).
2. Substrate coordination (mailbox + lock), so the OPERATOR is out of the "what should I tell the advisor" relay — the oversight handles agent↔agent.
3. Clear layering: operator DECIDES · oversight COORDINATES/triages · advisor DRIVES · implementer EXECUTES.
4. Proactive alignment + triage (catch-and-fix, not wait-for-the-operator).

**How to apply / where it goes:** this is beyond the product seat's propose-only remit and is a candidate for a FORMALIZED oversight seat or a spawned supervisor process (operator's own framing: "a separate role or spawn a different process"). STORY-1051 (auto re-drive supervisor) + STORY-1052 (nudge loop) are the AUTOMATED slivers; the LIVE version is judgment-driven oversight. Ties to [[feedback-substrate-first-never-rely-on-agent-awake]], EPIC-62. Do NOT let this bleed into the EPIC-63 entry lane (which is deliberately NO-machinery).

### feedback_queue_autonomous_ready_filings_at_filing

When the advisor files a spec it has ALREADY judged straightforward + autonomous-safe (i.e. it deliberately gave it NO parking tag — no `needs-human` / `needs-supervised-build` / `deferred:*` / `review:draft-only`), QUEUE it in the same breath (`AIDA_SESSION_ROLE=advisor aida backlog groom --specs <id>`, or `aida queue add`). Queue membership + absence of a parking tag IS the durable "advisor-cleared-for-burndown" record — `aida burndown run` then picks it up with zero re-analysis.

**Why:** Filing `--status approved` but NOT queuing drops the spec into "backlog limbo" (`aida backlog list` = Approved-but-not-queued). It then needs a SECOND touch to reach burndown, and `aida backlog groom` re-derives a risk chip (low/med/high heuristic) that is NOT the advisor's recorded judgment and can diverge from it (e.g. a keyboard-supervised spec chips `high`; an autonomous-fine one chips `med`). The analysis the advisor already did at filing gets thrown away and redone. Operator caught this 2026-06-11: "you did the analysis once — don't make yourself do it again."

**How to apply:** Autonomous-ready filing → `aida add … --status approved` then immediately groom/queue it. NOT-autonomous → file WITH the right parking tag and leave it parked (the parking tag is the discriminator; queue membership is the positive clear). Don't auto-queue keystone-adjacent/meta work — leave it `awaiting sign-off` for a keyboard call. Atomic file+approve+queue is tracked as TASK-754 (`aida add --queue`). Generic AIDA-advisor discipline (any project with burndown). Links: [[feedback_dont_declare_drained_from_filtered_view]] [[feedback_capture_over_concentration]] [[feedback_reliability_fixes_use_keyboard_not_drain]].

### feedback_reliability_fixes_use_keyboard_not_drain

The dogfood-merge pattern ([[feedback_self_test_via_dogfood_merge]]) says *"ship the fix through the system being fixed; the merge exercises the new code path."* That principle holds for **most** fixes — feature work, bug fixes whose failure mode doesn't affect drain machinery itself, anything where the broken-vs-fixed state of the codebase doesn't cascade onto the shipping mechanism.

It does NOT hold for fixes whose failure mode **directly impedes their own shipping pipeline.** Those are *recursive-failure-risk* fixes. Shipping them headless risks them getting caught in the very pattern they're patching.

## When to recognize a recursive-failure-risk fix

- The fix targets the **orchestrator's lease management** (BUG-307: auto-clean dormant leases; BUG-311: --steal silent failure). A drain of these fixes runs in the broken lease environment they're fixing.
- The fix targets the **reviewer skill template under headless** (BUG-280, BUG-327). A drain of these requires the reviewer phase to work correctly to ship the reviewer fix.
- The fix targets the **implementer skill template under headless** (TASK-401, BUG-285). Same shape: implementer phase shipping implementer-phase fixes.
- The fix targets **phase-X recovery/retry/escalation** in the orchestrator (BUG-266, BUG-286). The drain shipping the retry-layer needs working error handling to survive its own drain's transient errors.

These are all "fix touches the layer the drain depends on to ship the fix."

## What to do instead — keyboard-driven `--zen --auto-complete`

For recursive-failure-risk fixes:

1. Run **`aida queue work <spec> --zen --auto-complete`** (NOT `--no-human=both`). Orchestrator drives all 6 phases, BUT the advisor is on standby and you're at the keyboard.
2. Watch phase 1 to confirm the implementer doesn't trip the very fault the fix is patching. If it does, you have eyeballs on it immediately and can intervene.
3. The advisor (this session) reads the implementer's intent + the spec's acceptance + sees the live implementation. Available for any design-fork that surfaces.
4. Phases 2-6 run normally; the fix exercises the system but with your eyes on the result.

## Why this isn't an argument against dogfood-merge

The dogfood-merge pattern surfaces the NEXT gap by running real work through real conditions. That value is preserved here:

- The fix STILL ships through the system being fixed (phases 2-6 run normally).
- The merge still exercises the new code path.
- The dogfood-merge surfaces remaining gaps in the same way.

The only thing changed: **phase 1 (implementer) and phase 3 (reviewer) are watched, not unsupervised.** If they hit the bug the fix patches, you observe it directly instead of waking up to a stalled drain.

## Empirical evidence

2026-05-22 overnight drain queued BUG-307, BUG-311, BUG-310 (three reliability fixes for the orchestrator) at the head, followed by three smaller specs. **The three reliability fixes ALL stalled at phase 1** in the dormant-lease-conflict state they were themselves patching. The implementer completed their work in worktrees but couldn't publish — recovery required manual push + PR + merge in the morning. The same drain shipped the three smaller specs (TASK-451, TASK-452, BUG-312) that follow them cleanly because those specs' failure modes didn't cascade onto their own shipping mechanism.

Net: drain shipped 50% of specs end-to-end; the half that stalled was exactly the recursive-failure-risk half. That's not noise; that's the pattern.

## Tradeoff explicitly named

Recursive-failure-risk fixes ship slower (~1 hour at-keyboard vs ~30 min headless), but they ship reliably. The cost of stalling overnight + recovering in the morning is higher than the cost of watching for ~1 hour at the keyboard.

## Composes with

- [[feedback_self_test_via_dogfood_merge]] — the broader principle this refines for the recursive case
- [[feedback_three_mode_autonomy_taxonomy]] — `--zen` mode is the correct surface for at-keyboard, advisor-on-standby work
- [[feedback_advocate_not_be_passive]] — naming this refinement is advocacy; not naming it lets reliability fixes keep stalling

### feedback_serial_not_fanout_on_this_host

Joe, 2026-09-27 09:42, visibly frustrated: *"can we just go back to solving one problem at a time,
I am so frustrated with the lack of progress. Fix one problem at a time, if it opens new defects fix
those next. keep doing that until we get one less item in the backlog?"* Then: *"in just 2 hours we
have burned through 15% for the week."*

**What I did wrong.** Dispatched 13 concurrent agents. The host is **6 cores** with a **2-slot cargo
gate**, so only 2 could build at a time while a ~10-12 min release build each. The queue reached
**25 deep**. Agents cannot block on a background job, so they **poll their log files**, and every
poll is a billable tool call. Five agents burned 130k-224k tokens each producing nothing. Net
backlog went **63 → 65** because I filed more defects than I closed.

**The second, larger cost was the orchestrator seat itself.** My context reached ~475k, so every
turn cost ~450k input. That dwarfed the subagents. Past roughly 150k the seat is the dominant spend.

**How to work here instead:**
- **One item at a time, to MERGED.** Pick the item closest to done, review it, PR it, merge it,
  confirm the spec Completed. Then the next. If it surfaces defects, fix those next.
- **Rotate the seat early** — well before the advisory ceiling, not at 3x it. Write the handoff and
  restart; a fresh seat's turns cost a fraction.
- **Never brief a full workspace test/clippy run** — see
  [[feedback_dont_brief_full_workspace_suite_per_agent]].
- **Tell agents: block once in the foreground, never poll a log file.**
- Concurrency was the wrong lever: the host went CPU-bound then **disk-bound** (~50% iowait),
  because 13 worktrees each build into their own target dir (~18G each, ~230GB of duplicate writes).

**What actually produced progress:** two batched integration PRs merged (5 specs), and one operator
command (`aida merge-hold clear 2224 && aida pr ship 2224`) closing three more. Serial, verified,
cheap.

Supersedes the fan-out enthusiasm in [[feedback_fan_out_implementers_in_parallel]] for this host, and
refines [[feedback_charge_forward_autonomously]]: charging forward means finishing one thing, not
starting twelve.

### feedback_substrate_learning_loop_calibration

The substrate makes many predictive decisions per day: this spec will need ~3h of work; this brief should go to Codex; this PR is high blast radius; this drain should auto-resolve vs escalate; this complexity is "low". Most of those decisions are made once and forgotten — which means the substrate cannot tell whether its judgement was right, and cannot improve. **The discipline correction: every load-bearing decision must record its prediction AS the decision is made, so the post-hoc comparison to outcome is queryable.**

**Why:** without recorded predictions, calibration is impossible. A heuristic that's wrong 40% of the time looks identical to one that's wrong 5% of the time — both produce decisions that mostly seem fine in the moment. Only the recorded estimate-vs-actual delta exposes the gap.

**How to apply:**

1. **Record at the decision point**, not after. When opening a spec, tag it with the estimated effort (`effort:open:2h`). When briefing an agent, log the predicted-assistance level (`assistance-predicted:advisor`). When sketching, write the complexity guess. The recording is the cheap part; the value is what compounds.

2. **Compare on landing.** When the spec ships, record the actual (`effort:impl:3h`, `actual-assistance:human-escalation`, `actual-complexity:medium`). The substrate now has a (predicted, actual) pair.

3. **Surface the delta as a query.** `aida load calibration --by-type` slices the deltas by spec attributes. `aida advisor calibration` for the advisor-decision case (STORY-347 already shipped). Make the deltas visible without requiring the operator to compute them.

4. **Fine-tune the heuristic over the rolling delta.** Once a pattern emerges ("specs filed by sibling agents underestimate effort by 2x", "advisor punts on TYPE=story over-escalate by 30%"), update the heuristic — or surface the pattern to the operator who decides. Don't over-engineer the auto-tuning; sometimes the right outcome is "operator now knows to mentally adjust."

## Why this is non-obvious

The instinct when designing a heuristic is to think "what's the right initial value?" — debate the choice, ship, move on. The instinct that beats it is "I don't need to be right initially; I need to be self-correcting." A 60%-accurate heuristic that calibrates beats an 80%-accurate heuristic that never updates, within weeks of real use. The compounding is the whole game.

## Empirical instances (2026-05-23)

The pattern names a cohesive set of specs filed during one session that all apply this principle to different load-bearing decisions:

- **STORY-347** (shipped) — cold-boot vs live-advisor calibration ledger. Records both advisor verdicts in parallel when in calibration mode; surfaces disagreement as substrate-gap signal.
- **TASK-340** — autonomy metric (autonomous-duration fraction + intervention count per drain). Maturity trend visible only when drain-by-drain data accumulates.
- **STORY-439** — assistance + complexity calibration at three touchpoints (pickup, ship, review). Three-way calibration on each spec.
- **STORY-451** — effort estimation at four touchpoints (open, plan, ship, review) + queue/backlog load aggregates. Quantitative effort the substrate fine-tunes over weeks. (Originally STORY-447; re-filed after orphan-store rebase ID-allocation race per BUG-372.)
- **TASK-513** — credit-burn workflow surface. The "is this PR worth `/ultrareview`-ing" decision becomes recorded + reviewable in retrospect.

Each spec is a touchpoint where a load-bearing decision is made. The pattern says: each of those decisions should record its prediction now so the substrate can grade itself later. Without the record, calibration is impossible; with it, the substrate's accuracy compounds across every drain, every ship, every advisor verdict.

## When NOT to apply

Decisions that don't matter — formatting choices, where to put a comment, what to name a temp file — don't need recorded predictions. Calibration overhead is real (the recording, the comparison, the storage). The rule: **load-bearing** decisions (the ones that drive other decisions, set priorities, route work, or determine outcomes the operator cares about) deserve the recording cost. Trivia doesn't.

A cheap heuristic for "is this load-bearing": **does the operator's day go differently if this decision is wrong vs right?** If yes, record + calibrate. If no, ship the heuristic and move on.

## Composes with

- [[feedback_substrate_as_bouncer_not_rules]] — calibration is itself a substrate-as-bouncer pattern: the substrate enforces the "record-and-compare" discipline, not the operator's memory
- [[feedback_capture_over_concentration]] — record the prediction immediately, even if you're not sure; it's data you can't recover later
- [[feedback_memory_pack_hygiene]] — calibration data ages; rolling-window queries (`--since 30d`) keep the trend signal fresh while history accumulates
- [[feedback_pushback_on_overengineering]] — calibration scope creeps easily; the MVP is "record + compare + show delta" — auto-tuning is a deferred follow-on
- [[feedback_advocate_not_be_passive]] — surfacing the deltas as the substrate notices them (not waiting for operator query) is the advisor advocacy version of this principle

### feedback_unbounded_goal_loops_the_stop_hook

A session-scoped Stop-hook `/goal` whose condition is **unsatisfiable or asymptotic** (2026-09-12: "close all open items", when closing spawns follow-ups so open never hits zero) makes the Stop hook fire on every turn-end indefinitely, re-invoking the assistant with the same "goal not satisfied" feedback.

**Why it's a trap:** the operator retiring the goal in PROSE ("stop the autonomous loop") does NOT mechanically clear it — the goal stays active until `/goal clear`. So the hook keeps looping even after the operator has explicitly moved on. The assistant then either churns redundant acknowledgements or manufactures busywork to feed the hook.

**How to apply:**
- When setting a standing goal for a long-horizon loop, prefer a BOUNDED, checkable condition (a batch merged, a PR count, a spec reaching Completed) over an open-ended "close everything."
- When the goal is retired mid-session, tell the operator plainly that the mechanical goal needs `/goal clear` (this IS the sanctioned early-clear path — distinct from the "don't mention /goal clear after success" rule, which is about a MET goal). Say it ONCE, then hold minimally — do not repeat the full explanation each time the hook re-fires, and do not invent work just to satisfy it.
- The operator's explicit halt supersedes the mechanical goal: correct behavior is to STOP autonomous work and wait, even while the hook keeps firing. Relates to [[feedback_charge_forward_autonomously]] (charge-forward has an off-switch) and [[project-burst-usage-robustness-over-speed]].

