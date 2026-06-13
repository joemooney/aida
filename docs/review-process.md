# Review process — who reviews, by execution mode

<!-- trace:STORY-553 | ai:claude -->

Every code change in AIDA is reviewed by **a different entity than the one that
wrote it**. That invariant never changes. What *does* change — by execution
mode — is *who* that reviewer is, how deliberate the review is, and how much of
it a human sees. This doc maps the review topology across the three modes you'll
actually use, and when to pick each.

The one rule that holds everywhere:

> **The implementer never reviews its own work.** Self-review is worthless — an
> agent that just wrote the code is the worst-placed entity to find its flaws.
> This is why agents open PRs but never self-merge.

## The three modes

### 1. Manual brief fan-out → **the advisor reviews, by hand**

You dispatch an implementer (a paste-ready brief to a `claude`/`codex` session).
It works in its own worktree, opens a PR, and stops. The **advisor session**
(the strategic partner you're talking to) is the reviewer: it reads the *diff*,
verifies it against the spec's acceptance criteria, checks CI, builds the
combined `main`, and merges — serially.

The implementer's PR summary is an **assertion, not a review**. The advisor
trusts the diff, not the prose. This is the highest-assurance mode: a deliberate
line-by-line read by a separate agent with you present. It catches subtle
defects an automated pass waves through — provenance rewrites, specs marked Done
with no implementing code, semantic conflicts that break `main` even when each
PR's own CI was green.

The cost is throughput: review is bounded by the advisor's pace, one PR at a
time.

### 2. `--auto-complete` drain (human present) → **an automated reviewer *phase***

`aida queue work --auto-complete` runs the orchestrator lifecycle:

```
implementer → CI → reviewer → merge → pull → build
```

The **reviewer** is a distinct `reviewer`-role agent the orchestrator spawns —
still a separate entity from the implementer, but now an *automated LLM reviewer
phase*, not the advisor's considered read. Its verdict (approve / RequestChanges)
gates the merge.

- **Default** (human present): the orchestrator pauses at design-forks and
  checkpoints; you supervise and can intervene, but the code review itself is
  the reviewer phase.
- **`--zen`** (human present, auto-proceeds on clean): same reviewer phase; the
  session auto-exits on a clean finish instead of waiting for you. On a clean
  finish that left an open PR, it files a review brief to the advisor's mailbox
  (STORY-569) so the review handoff happens through the substrate, not your
  clipboard.

### 3. `--no-human=both --auto-complete` → **headless reviewer + headless advisor tier**

Fully unattended. The implementer is a headless `claude -p`, and so is the
reviewer phase. On a **design-fork punt** (the implementer hit a decision it
couldn't safely make), the orchestrator routes to a headless **advisor tier**
(`/aida-advise`, STORY-306) that resolves-and-resumes or escalates. No human
reads code; you surface only for escalations the advisor tier couldn't resolve.

## At a glance

| Mode | Implementer | Code reviewer | Your role | Forks / punts |
|------|-------------|---------------|-----------|---------------|
| **manual brief fan-out** | agent (you dispatch) | **advisor, by hand (reads the diff)** | dispatch + watch | advisor escalates to you |
| **`--zen --auto-complete`** | agent (interactive) | automated `reviewer` phase | present; handle forks, can intervene | pause for you |
| **`--no-human=both`** | headless `claude -p` | headless `reviewer` phase | *absent* | headless advisor tier (`/aida-advise`) resolves-or-escalates |

## Reviewer ≠ advisor tier

Two different jobs, easy to conflate:

- The **reviewer** does the *code review* of a PR — every mode has one (the
  advisor by hand, or the orchestrator's reviewer phase).
- The **advisor tier** (`/aida-advise`) fires only on a *punt* — a design fork
  the implementer couldn't decide. It resolves the decision or escalates; it is
  not the code-review gate.

When a reviewer's verdict conflicts with the advisor's intuition on whether to
merge, **trust the reviewer** — it read the code; the advisor read the commit
messages.

## Which mode to use

- **Manual brief + advisor review** — best *decisions*. Use for keystone,
  reliability, governance, or anything with real blast radius. Reliability fixes
  for the autonomy machinery itself ship here, at the keyboard with the advisor
  watching — never an unsupervised drain (a buggy drain reviewing its own buggy
  work is a recursive-failure risk).
- **`--auto-complete` (esp. `--no-human`)** — best *throughput*. Use for
  bounded, low-blast-radius, mechanical work where a reviewer-agent's judgment is
  good enough and you only need to see escalations.

The trade is explicit: **interactive = better decisions, headless = better
throughput.** Pick by the cost of a missed defect, not by what's fastest.

## Related

- [`docs/lifecycle.md`](lifecycle.md) — the spec states and the Done→Completed merge transition this review gates.
- [`docs/autonomous-drain.md`](autonomous-drain.md) — the `--auto-complete` / `--no-human` orchestrator in depth.
- [`docs/architecture/autonomy-and-escalation.md`](architecture/autonomy-and-escalation.md) — the implementer → advisor → human escalation cascade.
- `aida review <SPEC>` — the human/advisor review verb (STORY-553).
