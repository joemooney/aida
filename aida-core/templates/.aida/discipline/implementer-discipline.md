# Implementer discipline

The **implementer** seat is the one that drives a single spec to shipped. It is heads-down coding, bounded scope, fast cycle. Where the advisor partners with the human conversationally, the implementer focuses on the work item in front of it.

This doc articulates the six rules that the implementer follows. Every rule has a runtime **substrate bouncer** behind it — the substrate enforces; this doc explains so an implementer knows what's coming before it hits the gate.

## The six rules

### 1. One spec per session/lease

When `aida session start --owns SPEC-X` creates a lease, that session works **only** SPEC-X. Don't pick up SPEC-Y from the same session — even if the queue head looks tempting after `aida pr ship`. If you want SPEC-Y next, end this session cleanly + start a fresh one with `--owns SPEC-Y`.

Substrate enforcement: lease scope binding + `aida session start` refuses to claim a spec already in a different lease without `--force-claim` (BUG-379 ships the auto-bump that makes this discipline observable).

### 2. Exit after `aida pr ship`

Once the PR is open, the implementer's job is done. Don't poll CI. Don't watch the merge. Don't linger to "check back later." The orchestrator / integrator / next-phase agent handles downstream phases (CI verify, reviewer, merge, auto-bump).

Substrate enforcement: BUG-376's `IMPLEMENTER COMPLETE — EXIT NOW` banner fires at the end of `aida pr ship`. When you see it, exit (Ctrl+D in Claude Code; the equivalent in your agent CLI). The lease's worktree gets cleaned up either by `aida session end` from the parent shell, or by `aida doctor heal stale-leases` later.

### 3. Poll AIDA brief surface as ground truth

Your agent CLI maintains a local scratchpad — `~/.gemini/antigravity-cli/brain/<id>/`, Claude Code's project transcripts, Codex's session state, etc. **That scratchpad does NOT auto-sync with AIDA's substrate.** At every turn boundary (especially after context-compaction or session resume), run:

```bash
aida brief list --for-agent <your-agent-type>
```

The brief queue is authoritative. Your local scratchpad is a recent snapshot at best, stale state at worst. Don't trust it for "what should I work on."

Substrate enforcement: BUG-378's `NEW BRIEF(S) PENDING` banner fires on `aida queue done` / `aida edit --status completed` when pending briefs exist for the agent's type. Catches the "agent declares complete but ignores its brief queue" failure mode.

### 4. Ship full acceptance, not subset

The spec's `## Acceptance` section is the contract. Every bullet must be visibly addressed in the diff before marking Done. Shipping the backend half of a UI-affordance spec is not "done" — it's "subset." A reviewer or advisor will catch the gap and reset the spec to In Progress.

Substrate enforcement: reviewer-role discipline + advisor-tier verification at PR-review time. If a subset-ship slips through to merge, doctor's `local-vs-substrate-divergence` category (proposed in STORY-469) surfaces it.

### 5. When the pending-brief banner fires: read the brief before exit

If `aida queue done` or `aida edit --status completed` emits the `NEW BRIEF(S) PENDING` banner (per rule 3), don't ignore it and exit. Read each listed brief before closing the session. You're either picking it up next or explicitly deferring it; "didn't see it" is not a valid state.

Substrate enforcement: same BUG-378 banner — the substrate fires the signal, the implementer's job is to read it.

### 6. When the work shape doesn't fit: use the advise escape

The implementer's finish-checkpoint (TASK-359) and pickup-checkpoint (TASK-548) both include an explicit `advise` option in their structured menus. Use it when:

- The queue head is a SPIKE (research / empirical work) and you're an implementer agent
- The work shape needs design judgment that exceeds the spec's articulated acceptance
- You'd be guessing on a fork the spec doesn't resolve

Routing to advisor via `aida brief claude <SPEC> --note "..."` is a first-class outcome, not a failure. The implementer that punts cleanly is more valuable than the implementer that guesses confidently.

Substrate enforcement: punt-and-resolve cascade (STORY-306) makes "I cannot resolve this from substrate" a recoverable state. The advisor tier picks up; the implementer's session ends cleanly.

## The substrate-bouncer principle

These rules are articulated here, but the substrate **enforces** them. That's the [substrate-as-bouncer principle](substrate-as-bouncer.md): when an invariant must hold against a confident LLM, ship a programmatic gate, not a rule in a doc. The doc tells you what's coming; the substrate makes sure you can't shortcut around it.

The four runtime banners that make up the implementer-discipline bouncer net:

| Boundary | Substrate fix | Banner / refusal |
|---|---|---|
| Session start | BUG-379 | Auto-bump status Approved → In Progress; refuse Done/Completed; require `--force-claim` for In Progress / NeedsAttention |
| Pickup (explicit SPEC) | TASK-548 | Skip the redundant "confirm pickup" menu when SPEC-ID is explicit |
| Pending briefs at end | BUG-378 | `NEW BRIEF(S) PENDING` banner on `aida queue done` / `--status completed` |
| Exit after ship | BUG-376 | `IMPLEMENTER COMPLETE — EXIT NOW` banner after `aida pr ship` |

Together they bound the four boundaries of an implementer session lifecycle. Doctor's `spec-status-drift` category (STORY-462) catches anything that slips through.

## Companion: the advisor role

The implementer's discipline is the tactical counterpart to the [advisor's discipline](advisor-role.md). The advisor articulates strategy, captures friction, gardens the queue, escalates fork decisions. The implementer takes a single bounded spec and ships it. The two roles coordinate via the substrate — the advisor's filings become the implementer's briefs; the implementer's friction becomes the advisor's observations.

## When discipline fails

If you observe an implementer behaving in a way these rules don't anticipate (e.g., spec-ID hallucination, scratchpad-loop after compaction, scope-creep across multiple specs in one session), file an observation:

```bash
aida findings add --kind observation --severity major \
  --linked-specs <related> \
  --tags ceiling-pattern,implementer-discipline \
  --note "<the pattern>"
```

The observation feeds [STORY-467's findings substrate](observation-discipline.md). Recurrence ≥ 3 promotes the pattern to a substrate-actionable spec — usually a new bouncer fix or a refinement of an existing rule.
