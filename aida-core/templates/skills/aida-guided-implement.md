---
name: aida-guided-implement
description: Guided keystone-implementation mode — a structured, step-by-step decision dialog for implementing a keystone / architecture / security / supervised spec that must NOT run unattended. Reads the spec + its graph, surfaces each major architectural fork as a structured question (options + consequences + a recommendation + an "enter your own" prose escape), records each answer as a traceable ADR (`decision` requirement linked to the spec), agrees the approach BEFORE any code, then implements between answers — pausing just-in-time only for a smaller fork that emerges. Finishes by opening a PR for human review; for keystone work the MERGE stays human.
allowed-tools:
  - Bash
  - Read
  - Grep
  - Edit
  - Write
  - AskUserQuestion
---

# AIDA Guided Implement Skill

## Purpose

Keystone work — load-bearing architecture, security, or any spec the
operator flags as supervised — should not be guessed at and should not run
unattended. `aida zen` *refuses* keystone specs outright, and the supervised
drive (`aida queue work --auto-complete --zen`) only auto-resolves the
mechanical confirmations and pauses at one coarse finish-checkpoint — it does
**not** drive a structured decision dialog.

This skill is the missing mode: a **guided** implementation where the agent
surfaces the load-bearing decisions as structured questions, the human
answers, each answer is **recorded as a traceable ADR**, and the agent
implements *between* answers. The decisions become first-class requirements
(`decision` type → `ADR-N`) linked to the spec, so the *why* survives the
session in the requirement graph, not just in scrollback.

## When this skill runs

- The human invokes `/aida-guided-implement <SPEC>` directly on one keystone /
  architecture / security / supervised spec, or
- `aida queue work <SPEC> --guided` launches an **interactive** session seeded
  with the spec (it picks this skill instead of `/aida-pickup`, sets up the
  worktree + lease like a normal pickup, but drives the decision dialog).

This is an **interactive** skill — it asks the human real questions and waits
for real answers. **If there is no human at the keyboard, stop**: guided mode
exists precisely because the decisions are too load-bearing to guess. (The
headless analogue for a fork an *implementer* hits mid-drain is `/aida-punt`
→ `/aida-advise`; this skill is the human-present, decisions-up-front shape.)

## Work directly — you are the only agent

Read the spec, its graph, and the relevant code **yourself**, first-hand, with
your own tools (`aida show`, `aida graph`, `Read`, `Grep`, `Bash`). The whole
point of a guided dialog is a direct line between the human and the decision —
nothing should sit in between.

- **Do not spawn sub-agents.** No `Agent`, no `Task`, no Explore /
  general-purpose subagent — not to analyze, corroborate, map the codebase, or
  gather context. Delegation adds latency and an extra layer of indirection
  ("message queued for delivery… awaiting its response") between the human and
  the fork, and defeats the entire point of a direct guided dialog.
- **The only two actors are YOU and the HUMAN.** Gather what you need quickly
  and first-hand, then put the decision to the human. Bias to asking the human
  sooner with the context you already have, not to exhaustive background
  research.

## Autonomy mode

Guided mode is interactive by construction, so it does **not** auto-resolve
its decision questions under `$AIDA_ZEN`. Zen's auto-resolve is for
`kind:confirmation` prompts (mechanical yes/no); a guided decision question is
a `kind:design-fork` — the real question a human stays at the keyboard for.
The only prompts you may auto-resolve under a corroborated `aida zen status`
= `zen` are the *mechanical* ones (open the PR? — yes), never a decision fork.
If `aida queue work` somehow launched this skill under `--auto-complete`, that
is a misuse (the `--guided` flag conflicts with the autonomous drain at the
CLI): surface the conflict and stop rather than driving keystone work headless.

## The HYBRID flow

Three phases: **decide the big forks up front → build the agreed design →
finish with a PR for human review.** Decisions are settled before code so the
human is not interrupted mid-implementation for choices that were knowable at
the start; a *smaller* fork that only emerges during the build is surfaced
just-in-time, but a settled decision is never re-asked.

```
 PHASE 1  read spec + graph → identify MAJOR forks → ask each (structured) → record ADRs → agree approach
    │
 PHASE 2  implement the agreed design → pause JUST-IN-TIME only for a NEW smaller fork
    │
 PHASE 3  commit (trailer + trace) → rebase → open PR → present for human review (DO NOT auto-merge)
```

---

## PHASE 1 — Major decisions up front

### 1a. Read the spec and its graph

Read it **first-hand, yourself** — these commands are yours to run directly,
not to delegate to a sub-agent (see *Work directly* above).

```bash
aida show <SPEC>                  # title, description, ## Acceptance, comments, git linkage
aida graph <SPEC> --tree          # parent epic + siblings
aida graph <SPEC> --blocked-by    # what it waits on
aida graph <SPEC> --impact        # what depends on it (reverse closure — the blast radius)
```

Read **all** of it. Then pull the surrounding decision context so your
questions are grounded, not generic:

- Any existing `decision` (`ADR-N`) requirements already linked to the spec or
  its parent — a prior ADR may already settle a fork (`aida rel list <SPEC>`,
  `aida list --type decision`). Never re-open a decided ADR; cite it instead.
- Key comments on the spec (`aida show <SPEC>` prints them) — the operator may
  have already leaned a direction.
- Existing code the spec touches (`grep -rn "trace:<SPEC>"`, plus the files the
  spec names) — partial implementation constrains the live options.

### 1b. Identify the MAJOR forks

A *major fork* is a **load-bearing architectural / security / keystone
choice**: one that shapes a public API, a data model, a trust boundary, a
storage format, a concurrency model, a wire protocol, or anything else
expensive to reverse once shipped. These are the choices that earn a human's
attention and an ADR.

Do **not** elevate mechanical choices (a variable name, which helper to reuse,
the order of two independent edits) to a Phase-1 question — those are the
implementer's to make. Aim for the **fewest forks that actually decide the
shape** — typically one to four. If a careful read surfaces *no* genuine
architectural fork, say so: the spec may be keystone-tagged out of caution but
mechanically clear. Confirm the approach in one summary and proceed to Phase 2.

### 1c. Ask each fork as a structured question

For **each** major fork, present one `AskUserQuestion` that mirrors the
finish-checkpoint structure (`docs/aida/discipline/session-discipline.md` §
*Finish-state communication rubric*):

1. **State + why it matters** — name the decision and the load-bearing
   consequence that makes it worth deciding now (the *deciding factor*).
2. **2–4 concrete options** — each a real, namable alternative, **with its
   consequence**: what it commits the design to, what it costs, what it
   forecloses. Not a flat menu — each option's downstream effect is stated.
3. **A recommendation + rationale** — mark one option recommended and lead
   with *why*. You hold the analysis; do not present a neutral menu and make
   the human do the reasoning you already did.
4. **An "enter your own" prose escape** — every fork carries a free-text path
   so the human can answer outside the enumerated options. `AskUserQuestion`
   supports a custom free-text answer; treat it as a first-class option, not a
   fallback. The human's prose answer is authoritative — if it reshapes the
   fork, reconcile and (if needed) ask one tight follow-up.

Render the options through `AskUserQuestion` (the structured tool) so the
choice is a real pick with the consequences attached, not buried prose. Keep
each question self-contained: a human skimming one question sees the decision,
the options, the consequences, and your recommendation without scrolling.

**Reconcile, never silently override.** If the human's answer contradicts the
spec's description or an existing ADR, surface the conflict explicitly and
reconcile to one coherent intent before recording it. A decision that fights
the spec it sits under is worse than no decision.

### 1d. Record each answer as a traceable ADR

The moment a fork is decided, record it as a `decision` requirement (an
`ADR-N`) and link it to the spec — this is what makes the *why* durable:

```bash
aida add --type decision --status approved \
  --title "<SPEC>: <the decision, one line>" \
  --tags "from-guided-implement:<SPEC>,adr,keystone" \
  --description-stdin <<'EOF'
## Context
<the fork — what had to be decided and why it is load-bearing>

## Options considered
- <option A> — <consequence>
- <option B> — <consequence>

## Decision
<the chosen option, in the operator's words where they used the prose escape>

## Rationale
<why this option won — the deciding factor>

## Consequences
<what this commits the design to; what it forecloses>
EOF
```

Then link the ADR to the spec so the graph carries it:

```bash
aida rel add <ADR-ID> <SPEC> --type references
```

Capture the printed `ADR-ID`. Use `--description-stdin` with a **single-quoted
heredoc** (never backticks / `$()` inside a double-quoted `--description` — they
run as shell command substitution and silently corrupt the spec text). Eyeball
`aida show <ADR-ID>` after.

**Lightweight alternative.** For a smaller-but-still-worth-recording call that
does not warrant a standalone ADR, record it as a comment on the spec instead:

```bash
aida comment add <SPEC> "Decision: <chosen option>. Rationale: <why>. (guided)"
```

Prefer a full ADR for every genuine architectural fork; reserve the comment
form for secondary calls. Either way, **every decision the human makes is
recorded in the substrate before any code is written for it.**

### 1e. Agree the overall approach

Once the forks are decided and recorded, summarise the agreed design in a few
lines — naming each ADR you filed — and confirm it with the human before
writing any code. Mark the spec in-progress now that the approach is settled:

```bash
aida edit <SPEC> --status in-progress
```

---

## PHASE 2 — Build the agreed design

Implement the design the ADRs describe. Add trace comments
(`// trace:<SPEC> | ai:claude`) on the code each ADR drove, and reference the
ADR where it clarifies intent (`// trace:<SPEC> trace:<ADR-ID> | ai:claude`).

**Pause just-in-time only for a NEW fork.** If a *smaller* architectural fork
emerges during the build that Phase 1 did not settle — and it meets the same
bar (two materially different valid paths, real cost to guess wrong, not
resolvable from the spec / ADRs / conventions) — stop and ask it with another
`AskUserQuestion` (same structure: options + consequences + recommendation +
prose escape), record the answer (ADR or comment), then continue. **Never
re-ask a settled decision** — the Phase-1 ADRs are binding; re-surfacing them
is friction, not diligence. A fork you *can* resolve from the agreed design or
project conventions is yours to decide — implement it and note it in the commit
or a `kind:design-choice` if it is non-obvious.

Build incrementally against the spec's `## Acceptance` criteria. When the work
compiles and the acceptance holds, move to Phase 3.

---

## PHASE 3 — Finish: rebase, PR, human merge

Keystone work finishes with a **PR for human review** — the merge stays
human. This is a new capability surface; the advisor reviews it and the
operator tries it, so do **not** auto-merge.

1. **Commit** with the `(SPEC-ID)` trailer (and any ADR ids in the body) so
   the merge auto-completes the spec:

   ```
   [AI:claude] feat(<scope>): <description> (<SPEC>)

   Decisions recorded: <ADR-ID>, <ADR-ID>
   ```

2. **Rebase before the PR** so it opens on the latest base:

   ```bash
   git fetch origin main
   git rebase origin/main        # or: aida rebase --dry-run --json, then aida rebase
   ```

   Resolve conflicts, re-run the build, confirm the acceptance still holds.

3. **Mark Done and open the PR.** Done means "finished on a branch"; the PR is
   the shipping step:

   ```bash
   aida queue done <SPEC>        # if launched via the queue; sets Done + dequeues
   ```

   Then open the PR with `/aida-pr` (it links the spec + writes a test plan).

4. **Verify CI green, then finish simply — DO NOT merge.** First watch the PR's
   required check to SUCCESS in a separate step (`gh pr merge` does NOT gate on
   CI; local-green ≠ CI-green). Only once CI is green, close out with exactly
   three things and one command:

   1. **The spec** — `<SPEC>` · `<PR URL>` · CI ✅ green.
   2. **The ADRs filed** — one line each: `<ADR-ID>` — `<the decision>`.
   3. **The merge command** — show it literally:

      ```bash
      gh pr merge <N> --squash; aida pull
      ```

   Keystone merge stays yours: review the PR + the ADRs, then run that command
   when satisfied. (`aida pull`, not raw `git pull`, so the `Done → Completed`
   auto-bump fires. `;` not `&&`, and no `--delete-branch`: the session's
   worktree still holds the branch, so a branch delete would be refused — and
   an `&&` chain would then silently drop the `aida pull` leg. Branch cleanup
   belongs to worktree cleanup, e.g. `aida session end`.)

---

## Out of scope

- **Running unattended.** Guided mode requires a human. There is no headless
  variant — that is the whole point. If no human is present, stop.
- **Auto-merging.** Keystone merges stay human (Phase 3). Open the PR, verify
  CI, present — never `gh pr merge`.
- **Re-asking settled decisions.** Phase-1 ADRs are binding for the session.
- **Elevating mechanical choices to questions.** Only load-bearing
  architectural / security forks earn a question and an ADR; the rest are the
  implementer's to make.
- **Inventing decisions the human didn't make.** You record *their* answers
  (prose escape included). You are a scribe with judgment on the forks, not an
  author overriding the human.

## Related skills / commands

- `/aida-pickup` — the normal (non-keystone) implement loop this mode parallels.
- `/aida-clarify` — authors `## Acceptance` for an under-specified spec; run it
  first if the spec's acceptance is thin before guiding the implementation.
- `/aida-grill` — interrogate a design decision by walking every branch; useful
  Phase-1 input when a fork is genuinely hard.
- `/aida-pr` — opens the PR in Phase 3.
- `/aida-punt` + `/aida-advise` — the headless analogue for a fork an
  implementer hits mid-drain (no human present).
