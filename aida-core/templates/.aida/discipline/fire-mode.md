# Fire mode

Fire mode is the advisor-driven parallel burndown. Under an operator mandate,
the advisor fans out fixers and independent reviewers, gates each PR by risk,
and merges green PRs itself, whoever wrote them. It trades open-ended review
loops for bounded, risk-scaled review plus filed follow-ups. It keeps every
safety floor.

The procedure is the `/aida-fire` skill. This guide is the authority model it
runs under.

<!-- trace:STORY-1692 | ai:claude -->

## Why it exists

PRs used to take hours to merge while CI took about twenty minutes. The time
went to human-only steps and to review rounds without an end. Fire mode moves
the merge decision to the advisor for a bounded window, scales review to risk,
and caps rounds. The floor below keeps that from turning into rubber-stamping.

## The mandate

Fire mode starts only on an **operator mandate**. It has three parts:

| Part | Meaning |
|------|---------|
| Scope | Which seats, queue, tags or PRs are in play |
| Authority | The advisor merges on green after a tiered review, whoever authored the PR |
| End time | When the mandate lapses. No end time means no mandate |

Until the mandate is substrate state, it lives where the next seat can quote
it: the operator's words in the current session, a seat handoff (with an end
time not yet passed), or a mailbox note or spec comment recording it. An
advisor that cannot find all three parts does not start, and asks the operator
instead. Inferring a mandate from "the operator seemed to want speed" is not a
mandate.

## What the mandate permits

- Dispatching fixers and reviewers in parallel across the scope.
- Recording an approval and merging a PR on green Build after its tiered
  review, including PRs the advisor did not write or launch.
- Ruling at the two-round cap: merge with follow-ups, or park.
- Filing follow-up specs for non-blocking findings without asking.
- Fixing main forward with a one-line FASTTRACK PR through the normal gate.

## What the mandate never waives

- **Real review for security, sandbox, authority and architecture changes.**
  These always take the STRICT tier. Architecture changes still need a sketch
  and signoff on the owning spec before implementation.
- **Functional verification for containment and sandbox PRs.** Reading the
  code is not enough. A real end-to-end run must show a write that should
  succeed succeeding and one that should be refused being refused.
- **A green Build at the head being merged**, computed against current main.
- **No weakened tests.** No deleting, ignoring or loosening a test to get green.
- **No `--no-verify`**, even for a one-line fix-forward.
- **Merge-hold clears go to the operator.** `aida merge-hold clear` requires a
  human at an interactive terminal; that is an integrity floor, and a fresh
  approval at head does not release a hold on its own. When the release
  condition is met, the advisor asks the operator to run
  `! aida merge-hold clear N`.
- **Spec-level gates.** An "operator merges" gate (or a supervised hold)
  always goes to the operator: the advisor asks, and ships only after the
  operator explicitly authorizes that merge in-session, recording the
  authorization on the spec. The advisor's own comment never satisfies it.
  Only a "needs advisor signoff" gate (e.g. on a sketch) may be recorded by
  the advisor, with its reasoning, on the spec before shipping.
- **Independent review.** A fixer never reviews its own PR. Each review runs
  as a separate reviewer process launched from Bash
  (`aida queue work PR-<n> --role reviewer --no-human`), never as an
  Agent-tool subagent, and the advisor never records a verdict itself: it is
  an author of every PR its fixer subagents wrote, so the ship gate refuses
  its approvals. <!-- trace:STORY-1654 | ai:claude -->

## Risk tiers

| Tier | Applies to | Review |
|------|-----------|--------|
| FASTTRACK | Docs-only, test-only, one obvious line | One light review |
| STANDARD | Ordinary code within one subsystem | Independent reviewer; acceptance, edge cases, tests that fail without the change |
| STRICT | Authority, gates, sandbox, containment, seats, merge machinery | STANDARD plus every grant/check/bypass path enumerated, fail-open hunted, refusal path tested; plus functional verification for containment |

When in doubt, take the higher tier. Reviewers rely on GitHub CI and do not
re-run the local suite.

## The two-round cap

A PR gets at most **two** review rounds. After round two, the advisor rules:
merge and file the remaining findings as follow-ups, or park the PR with its
reason on the spec. Non-blocking findings are **always** follow-up specs,
never another round. A blocking finding that survives round two usually means
the spec was under-specified: park it and clarify the spec.

## Shipping order

Greens are computed against the main that existed when CI ran. Two PRs that
are each green can break main together (one adds a required field, another
adds a literal without it). So after any merge, the next PR is updated from
main, gets a fresh Build, and is re-approved at its new head before it ships.
Ship one at a time. A stale-check override is not a substitute.

If main's Build goes red, **stop shipping**: every open green is void. Fix
forward through the normal gate, wait for main to go green, then resume.

## Hold-clear conditions

A merge hold is ready for the operator to clear when all of these are true:

1. An approval is recorded at the PR's current head.
2. Build is green at that head, against current main.
3. The PR is up to date with main.
4. For containment and sandbox PRs, functional verification passed at that head.

The advisor checks these, then asks the operator. It never clears a hold to
skip an unresolved verdict.

## See also

- [`advisor-role.md`](advisor-role.md): the advisor seat.
- [`autonomous-burndown.md`](autonomous-burndown.md): the hands-off queue drain, for when no mandate is in force.
- [`rework-brief-craft.md`](rework-brief-craft.md): writing the CHANGES brief for round two.
- [`integrator-role.md`](integrator-role.md): merge-cascade mechanics.
- FAQ-57: the record → ready → ship recipe.
