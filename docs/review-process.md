# Review process — who reviews, by execution mode

<!-- trace:STORY-553 trace:STORY-587 trace:STORY-522 trace:STORY-569 | ai:claude -->

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
| **`--no-human=both` / `burndown run`** | headless `claude -p` | headless (cold-boot) `reviewer` phase | *absent* — only on escalation | headless advisor tier (`/aida-advise`) resolves-or-escalates |
| **fasttrack item (any drain)** | agent / headless | **none — CI is the only gate** | *absent* unless it punts | implementer punts if non-trivial |

## Reviewer ≠ advisor tier

Two different jobs, easy to conflate:

- The **reviewer** does the *code review* of a PR — every mode has one (the
  advisor by hand, or the orchestrator's reviewer phase).
- The **advisor tier** (`/aida-advise`) fires only on a *punt* — a design fork
  the implementer couldn't decide. It resolves the decision or escalates; it is
  not the code-review gate.

**Fork-from-live is not watch-only.** The in-drain advisor tier **forks-from-live
when a live advisor is registered** (`advisor::plan_fork` → `AdvisorPass::Fork` —
it copy-resumes the registered session's transcript so it inherits your context,
~$0.03 warm) and **cold-boots only as the fallback** when no live advisor exists
(`AdvisorPass::ColdBoot`, persistent substrate only). So fork-from-live (SPIKE-11)
is used by **both** the in-drain escalation tier *and* `aida advisor watch`;
cold-boot is the no-live-advisor fallback, not the in-drain default. (The
"both runs fire" double-verdict is **calibration-mode only** — a shadow fork
beside the cold-boot driver — not normal operation.)

When a reviewer's verdict conflicts with the advisor's intuition on whether to
merge, **trust the reviewer** — it read the code; the advisor read the commit
messages.

## Draining: fasttrack vs review — one drain, tag-differentiated

There aren't two separate drains. There is **one** machinery —
`aida queue work --auto-complete`, of which `aida burndown run` is the parallel
form over the whole ready set — and each spec's `lifecycle:` tags decide which
phases short-circuit:

| Category | Phases | Reviewer |
|---|---|---|
| **fasttrack** (`lifecycle:no-review`) | implementer → CI → ~~review~~ → merge | **none — CI is the only gate** |
| **default** | implementer → CI → review → merge (or punt) | reviewer phase (cold-boot under `--no-human`) |

**Integrity gates never skip in either** — CI must be green before merge, and
merge + `aida pull` auto-bump always run. So "fasttrack vs needs-review" is a
per-spec **tag**, not a separate queue or pipeline. (`lifecycle:trivial` =
`no-review` + `no-build` + `no-ci-wait`; prefer plain `no-review` for fasttrack
so CI still *gates* the merge rather than merging optimistically.)

**Fasttrack ⊂ the burndown ready set.** `lifecycle:no-review` is a phase-skip,
**not** a parking tag, so it does not exclude a spec from the ready set.
`aida burndown run` drains fasttrack and non-fasttrack items together — it just
skips the review phase on the fasttrack ones. `aida queue work --batch fasttrack`
is simply a *filtered* burndown over that subset. So yes: every fasttrack item
is a burndown candidate.

**Optimistic-by-default, demoted-on-discovery.** A low-risk item that turns out
non-trivial does not silently merge. The headless implementer **punts** (parks
`NeedsAttention`) → the advisor tier (fork-from-live if a live advisor is
registered, else cold-boot) tries to resolve → if it can't, it escalates to the
human. The `/aida-fasttrack` skill encodes the same rule for
the interactive path ("punt out of the lane if it turns out non-trivial").

**The honest gap.** The *only* thing distinguishing "safe to fasttrack" from
"needs review" today is the **tag a human/advisor put on the spec at filing**.
There is no automatic triviality classifier — a spec mis-tagged `no-review` that
isn't actually trivial has only CI plus the implementer's punt as a safety net.
Keep `lifecycle:no-review` a *deliberate* tag on genuinely-cosmetic work, never a
default.

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

## How completion reaches review — the handoff loop

A finished implementation has to *reach* a reviewer. How it does depends on the
finish mode:

- **default `aida queue work`** — the session **waits for you** (Ctrl+D); you
  drive the exit and relay the PR. No handoff is filed.
- **`--zen`** — the session **auto-exits on a clean finish** and **auto-files a
  review brief to the advisor's mailbox** (STORY-569) — no clipboard. It still
  pauses on design forks (you decide), and it does *not* review or merge.
- **`--no-human` / `--auto-complete`** — the orchestrator runs the reviewer
  phase itself (cold-boot) and merges; you see only escalations.

**The autodetect loop** (in assembly) chains those into hands-off completion:

```
--zen clean finish ──(STORY-569)──▶ review brief in advisor mailbox
        │
        ├──(STORY-585, read-side)──▶ surfaced into a live advisor session's context
        │
        └──(STORY-586 `aida advisor watch`)──▶ if you're away, a forked advisor
                                                reads the mailbox and acts:
                                                merges the bounded, ESCALATES the careful
```

When all three are live *and* `aida advisor watch` is running, completion →
review flows without a human relaying it. Two invariants the loop preserves:

- **Keystone/careful PRs escalate, never auto-merge** — even inside the loop.
  The watch-fork is scoped to mechanical-work-plus-escalate, so a careful PR
  surfaces to the human. The loop removes *detection* toil, not *judgment*.
- **It needs `aida advisor watch` running** for the away case. Nothing
  autodetects on its own — `aida away` alone leaves handoffs sitting in the
  mailbox until a session reads them.

## Resolving decisions, not code — the questions inbox

Code review (above) gates *implementations*. A parallel surface gates
*decisions* — the design forks an implementer or advisor can't settle alone.
Same "separate the thinker from the doer" spirit, two paths:

| Path | Who thinks | How the human answers |
|---|---|---|
| **`aida questions ask` → `list` → `answer`** | the advisor **pre-distilled** it (question + enumerated choices recorded ahead of time) | **pick a choice — no agent, no LLM session** (a pure operator data op) |
| **`aida questions clarify <spec>`** | an agent **thinks live**, interrogating the operator and generating options now | answer the interactive agent |

The intended workflow is **ask-ahead, answer-async**: the advisor converts vague
`needs-human` specs into pre-recorded structured DecisionRequests (`questions
sweep` detects candidates; `questions ask` records the distilled question +
choices), so the human drains them later with pure picks at their own pace —
reserving the expensive live-`clarify` agent only for specs too under-specified
to even enumerate choices for. Answering a DecisionRequest **applies** its
resolution (binds acceptance / clears a gate / rejects) and auto-queues the
now-decision-free spec onto the burndown ready set.

So the two drains are symmetric:

- **`aida burndown run`** drains the decision-*free* ready set with autonomous
  agents — needs *no* human.
- **`aida questions answer`** drains the decision inbox — needs *only* the human
  (no agent, no LLM).

Keep the inbox populated by running **`aida questions sweep`** as part of the
advisor's periodic garden pass — otherwise each decision falls back to a live
`clarify` session.

## Related

- [`docs/lifecycle.md`](lifecycle.md) — the spec states and the Done→Completed merge transition this review gates.
- [`docs/autonomous-drain.md`](autonomous-drain.md) — the `--auto-complete` / `--no-human` orchestrator in depth.
- [`docs/architecture/autonomy-and-escalation.md`](architecture/autonomy-and-escalation.md) — the implementer → advisor → human escalation cascade.
- `aida review <SPEC>` — the human/advisor review verb (STORY-553).
