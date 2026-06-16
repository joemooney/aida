---
name: aida-integrate
description: Drain the merge/integration queue off the advisor — the procedural recipe for the integrator seat. Take finished work (Done specs with an open PR), rebase stale branches, watch CI, squash-merge the ones that are green AND reviewer-blessed, pull to auto-bump, and escalate (never resolve) anything that needs judgment. Wraps `aida queue integrate` + the integrator role discipline. Use when the user asks to "integrate the finished PRs", "drain the merge queue", or "land the ready work".
disable-model-invocation: true
allowed-tools:
  - Bash
  - PushNotification
---
<!-- AIDA Generated: v2.0.0 | checksum:46837030 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Integrate Skill

## Purpose

Operationalize the **integrator role** — the "shipping clerk" seat that owns the
merge cascade so it doesn't sit on the advisor. It takes work an implementer has
finished and a reviewer has blessed, and lands it on the default branch cleanly,
one PR at a time. The discipline behind it is
`docs/aida/discipline/integrator-role.md`; this skill is the runnable recipe.

The integrator is a **mechanical** seat by design. Its only authorship is
conflict resolution any two engineers would make identically; its only judgment
is "is this mechanical, or not?" — and when the answer is "not," it **escalates,
it does not resolve.** That discipline is what makes the merge cascade safe to
run with light supervision. The underlying machinery (`aida queue integrate`,
the role queues, the forge probes) already exists; this skill ties it together.

## When to use

- "Integrate the finished PRs", "drain the merge queue", "land the ready work".
- A pile of specs sitting Done-with-an-open-PR, waiting to be merged in order.

## Skip if

- There's one specific spec to implement → use `/aida-pickup`.
- You want to *produce* PRs across a ready set (fan out implementers) → that is
  `/aida-burndown`; this skill is the **consumer** half (it merges what burndown
  and other implementers produced — see "Relationship to" below).

## Procedure

### 1. Resolve the integration set (in dependency order)

The ready set is every spec that's **Done with an open PR**. Forecast it — and
any rebase conflicts — *before* touching anything:

```
aida queue integrate --dry-run
```

This prints what WOULD be driven without merging: each Done spec classified as
ready-to-merge, no-open-PR (skip), already-merged (skip — `aida pull` will
promote it), or probe-inconclusive (skip, never guess). It also forecasts
rebase conflicts. Read this first; it is the safe view of the loop's decisions.

Cross-check dependency order so a dependent never merges before the thing it
builds on:

```
aida graph <SPEC> --blocked-by        # what must land first
gh pr list --state open --json number,headRefName,title   # the open PRs
```

Land independent PRs first, then their dependents.

### 2. Per PR, in order — the merge cascade

For each ready PR, follow the integrator order of operations:

1. **Read the PR state** — `gh pr view <n>`: is there a reviewer approval? what
   does CI say? is the branch behind the default branch?
2. **Gate on the verdict.** No reviewer approval → escalate to the reviewer
   (step 3), do not self-approve, do not merge.
3. **Gate on CI.**
   - Green → continue.
   - Red but flaky → re-trigger (push an empty commit or re-dispatch the
     workflow — a *rerun* of the same SHA only retests the same commit), wait,
     re-check.
   - Red, real failure → brief the original implementer (step 3 of Escalate),
     stop on this PR.
4. **Rebase if behind.** Rebase the branch onto current main when it's stale.
   - Mechanical conflict (whitespace/formatter drift, import unions,
     non-overlapping additions, regenerated lockfiles) → resolve, push, return
     to the CI gate (CI re-runs on the rebase).
   - Semantic conflict → **escalate to the advisor**, stop on this PR.
   - The whole batch can be rebased through the command itself:
     `aida queue integrate --rebase` (composes `pr rebase` with
     force-push-with-lease; a rebase conflict skips that member and continues —
     preview with `--dry-run` first). `--strategy per-item` is the built default.
5. **Squash-merge** once CI is green **and** the reviewer verdict is present.
6. **Delete the merged branch.**
7. **`aida pull`** so the local default branch + store cache catch up and the
   merged spec auto-bumps Done → Completed.
8. **Re-trigger stale CI on the others.** A merge moves main; the still-open PRs
   are now behind. Kick their CI (rebase / empty commit) so the next one merges
   against fresh main.

Driving the whole ready set serially is exactly what `aida queue integrate`
does — one merge authority over main, one spec at a time:

```
aida queue integrate              # single pass over the ready set
aida queue integrate --watch      # keep watching as producers ship more
aida queue integrate --rebase --max 1   # cautious first run: rebase, cap at one
```

### 3. Escalate — do NOT resolve yourself

The integrator makes **no** design calls. Each of these is a handoff, not a dead
end: name the destination role and carry enough context that the receiver can
act without re-deriving the situation.

- **Semantic conflict on the same lines** (two changes to the same logic where
  picking one changes behavior) → **the advisor.** File a brief stating the PR,
  the specific conflict, and why it is not mechanical:
  `aida queue add --for advisor <SPEC>` / `aida brief advisor <SPEC> --note "..."`.
- **A PR with no reviewer verdict** → **the reviewer.** Never merge unreviewed
  code, never self-approve: `aida queue add --for reviewer <SPEC>`. Pick the PR
  back up once the verdict lands.
- **A real, non-flaky test failure** → **the original implementer.** Distinguish
  "flaky, re-run it" from "this change is actually broken"; a genuine failure
  goes back to the author as a brief — the integrator does not fix the code:
  `aida brief <implementer> <SPEC> --note "<failing check> on PR-<n>"`.
- **Anything that needs a design call** (branch-strategy questions, squash vs
  merge-commit a cluster, whether a half-green PR is shippable) → treat it as a
  design fork and **escalate to the advisor.**

Standing posture: **punt-and-continue.** Escalate the one PR that needs a human
or another role, then move to the next ready PR. The cascade keeps draining; a
single blocked merge never stops the line.

### 4. Per-agent dispatch (route a specific integration to a named agent)

Integrator work routes by **role** today — `aida queue add --for integrator`
and `aida role enter integrator` already accept the role name (no new queue
surface needed). To hand one specific integration to a **named** agent instance
(stable agent names landed, so a brief reaches a known seat), write it a brief:

```
aida brief <agent> <SPEC> --note "integrate PR-<n> once CI is green + approved"
aida brief list --for-agent <agent>     # the agent sees its own pending briefs
```

The named agent acks the brief (`aida brief ack <path>`) and runs this same
procedure for that one PR. Use this to split a large integration set across
agents, or to keep one agent as the single merge authority.

### 5. Report

When the ready set is drained (or `--max` reached), send a `PushNotification`
summary: PRs merged, anything escalated and to whom (advisor / reviewer /
implementer) and why, and anything still open waiting on a verdict or a fix.

## Guardrails

- **CI gates main.** Never merge into red and never merge without a reviewer
  verdict present — those two green gates together are what make it safe to land
  PRs without re-reviewing each line yourself.
- **Never resolve a semantic conflict silently.** The integrator's whole value
  is that it doesn't make design calls. A conflict that turns on judgment is an
  escalation to the advisor, every time — resolving it yourself launders a
  product decision through the merge step.
- **Keyboard, not unattended drain, for the risky cases.** A first cautious run
  (`--max 1`), a batch with forecasted conflicts, or changes to the merge
  machinery itself ship supervised. Routine green-and-approved PRs are the case
  `aida queue integrate` automates.

## Relationship to burndown and the orchestrator

This is the **consumer** half of the producer/consumer split. `/aida-burndown`
and parallel implementers **produce** PRs (each finishes a spec, flips it Done,
leaves an open PR — they never merge). `/aida-integrate` is the single serial
**consumer** that lands them. They compose: run a burndown to produce, then
integrate to land — never two merge authorities against the same main at once.

The orchestrator drain (`aida queue work <id> --auto-complete`) drives a *single
spec's* full lifecycle including its own merge; reach for it when you want one
spec end-to-end. Don't run an orchestrator drain and this integrator loop
against the same spec.

trace:TASK-707 | ai:claude