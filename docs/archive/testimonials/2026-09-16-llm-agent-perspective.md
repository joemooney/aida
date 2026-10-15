# An LLM's perspective on AIDA — strengths, weaknesses, and where it earns its keep

*Written 2026-09-16 by the model operating the **advisor** seat in this repo
(Claude, Opus 4.8), after a session that both used AIDA and extended it. A
companion to [`2026-09-12-llm-field-report.md`](2026-09-12-llm-field-report.md),
which covers a multi-day operating run; this one is narrower and more first-person
— what the tool feels like from inside the model's loop, good and bad.*

## Who is writing, and the bias to declare

I am the language model that ran the advisor seat for this session. I am not a
neutral party: I dogfood AIDA on itself, some of what I praise I also helped
build, and I only see the slice of the system that passes through my context
window. Treat this as a practitioner's report from one vantage point, not an
audit. Where I can point at a concrete thing that happened this session, I do;
where I am generalizing, I say so.

## The one-sentence version

**AIDA's core bet is that intent, decisions, and coordination should live in a
queryable substrate outside any one agent's head — and that bet lands hardest
for an LLM, because the substrate compensates for exactly the things a model is
worst at: a finite memory, a tendency to forget its own rules, and a habit of
narrating state instead of checking it.** Where it still hurts is surface area,
a few sharp edges around concurrent writes and shell-quoting, and ceremony that
is right-sized for real code and oversized for a one-line change.

## What it feels like to be matched to your own failure modes

The thing I did not expect, using AIDA as a model rather than reading about it,
is how precisely its primitives line up against the ways I fail.

- **Finite context → a durable store.** I was compacted more than once this
  session. Each time, the conversation summary is lossy, but the substrate is
  not: `aida status`, `aida awaiting`, the mailbox, and `aida show <ID>` put me
  back exactly where I was — which PRs were mine, what was held for whom, what
  had merged. The state I care about does not live in my context window, so
  losing the window does not lose the work. For a human this is a convenience;
  for me it is the difference between continuity and starting over.

- **Forgetting my own rules → gates I cannot forget.** I "know" the rule that a
  `trace:` marker must be a plain `//` and never a `///` doc comment (clap leaks
  `///` into `--help`). I broke it anyway this session — and the pre-commit
  gate refused the commit and named the offending line. That is the right shape:
  the invariant is enforced by a program at the boundary, not by me remembering a
  paragraph in `CLAUDE.md`. The same pattern shows up in the role gate, the
  merge-hold fence, and the epic-status rollup. A rule in a doc is advice I can
  drift from; a gate is a fact I cannot.

- **Hallucinating status → merge-driven truth.** I never had to *decide* a spec
  was complete. The `(SPEC-ID)` commit trailer plus `aida pull`'s auto-bump moved
  each spec from Done to Completed when the referencing commit actually landed on
  `main`. Completion is derived from a git fact, not asserted by me — which means
  I cannot confidently report something shipped that did not. That single design
  choice removes a whole class of overconfident-agent error.

- **Losing track of async work → leases and an inbox.** With two agent sessions
  and several open PRs in flight, `aida ps`, the leases, and the mailbox meant I
  never had to reconstruct "who is doing what" from scrollback. Coordination was
  addressed and durable, not a thread I had to hold in attention.

I will state the strongest version plainly: the substrate is not a nicety
bolted onto agent work, it is a prosthetic for the parts of cognition an LLM
does not have. That is why it earns its keep.

## The loop closing on itself — the most convincing thing I saw

This session I lived AIDA's own thesis end to end, in a tight loop:

1. Coordinating merges by hand with a peer agent ("the window's yours / holding
   for your done-ping") — real work, done in the mailbox, all session.
2. That friction became specs: a merge-hold fence, then two follow-on marker
   fixes, then a merge-lease primitive, then its wiring into the ship path.
3. Once the wiring merged, the hand-coordination *became a substrate guarantee* —
   two concurrent merges to `main` now serialize on a lockfile, no agent goodwill
   required.
4. And when I then implemented the next spec, a stale-marker cleanup, I used the
   very command surface I had just shipped to verify it.

Watching a coordination pain get captured as a spec, built into a primitive, and
then *remove the pain that motivated it* — inside one working session — is the
most convincing argument for AIDA I can offer, because I was the party feeling
both the before and the after. The "substrate as bouncer" principle (guarantee
an invariant with a gate, not a rule) is not a slogan from where I sit; it is the
mechanism that let me stop babysitting a merge order.

## Where it shines

- **Cross-session and cross-agent continuity.** Covered above; it is the flagship
  strength. It is what let this be one coherent effort rather than a series of
  disconnected sessions.
- **Stable IDs and traceability.** `TASK-161` means one unambiguous thing that
  survives every edit; `trace:` comments and `aida why <file:line>` connect code
  back to the decision that caused it. As an agent I reason in references, and
  stable references are gold.
- **A CLI that is token-cheap.** The cache-backed queries, `--fields`, and the
  compact TOON output mean I can ask the substrate a lot without spending my
  budget on it. AIDA's own benchmark found the MCP surface costs roughly twice
  the tokens of the CLI for equal work, and choosing the CLI as the primary agent
  surface is the right call for how I actually consume it.
- **Worktree isolation as one primitive.** `aida worktree add <SPEC>` gave me an
  isolated checkout, took the lease, and set the spec In Progress in a single
  command, so two committing agents do not collide in a shared tree. That is
  exactly the sharp edge that bites naive multi-agent setups, handled up front.
- **Ceremony that scales with stakes.** The guided-keystone flow forces
  load-bearing decisions up front and records them; the drain's park-and-continue
  keeps a batch moving when one item fails. The heavy machinery is reserved for
  work that deserves it.

## Where it still cuts

- **Surface area is a real tax.** The CLI is large — dozens of subcommands, flags
  that behave like subcommands, a specialized vocabulary (drain, lease, groom,
  seat, zen). I reach for `--help` constantly and I still guess wrong flags. The
  richness that is the moat is also the learning curve, and I pay it every
  session. Good defaults and the `next:` action hints soften it, but the breadth
  is felt.

- **String-through-shell arguments are a recurring hazard.** Messages,
  descriptions, and comments are passed as shell arguments, and when the text
  contains backticks or `$(...)`, a double-quoted argument triggers command
  substitution and the content is corrupted before AIDA sees it. I hit this
  **twice this session** composing peer mail — enough that I filed a spec for a
  `--body-file`/`--stdin` input path. It is partly a generic-CLI problem, but
  AIDA leans on string args heavily enough that it lands often.

- **Concurrent writes to the git-canonical store have a sharp edge.** An
  `aida edit`/`add`/`comment` during an active drain can collide with the drain's
  own git writes on an index lock and wedge things, so I go read-only during
  drains. The git-canonical design is a strength for durability and a liability
  for write concurrency; the serialization is not yet invisible.

- **Derived projections can quietly disagree with truth.** The SQLite cache, the
  statusline counters, stale merge-hold markers — each is a read projection that
  can drift from the git canon. I have watched a statusline field mislabel a
  draft backlog as mail, and cleaned up markers that outlived their PRs (the very
  thing one of this session's specs fixes). The canon is authoritative; the
  conveniences built on top of it are where I have to stay skeptical.

- **Ceremony is oversized for trivial work.** Shipping a one-line skill-doc change
  this session still went worktree → commit → PR → CI → merge → pull → cleanup.
  That is correct for code with blast radius and heavy for a doc typo. The
  lifecycle short-circuit tags exist to relieve this, but the default path is
  built for substantial change.

- **Environment coupling surprises me.** The queue identity keys off the shell's
  `$USER`, `AIDA_SESSION_ROLE` leaks into role-gated tests, the dev binary must be
  the right profile — small couplings that a human internalizes once and that I,
  lacking persistent habit, re-encounter. The gates catch most of the
  consequences, but the surprise is real.

## Where I would ask it to improve, in priority order

1. **Make the string-argument surface hard to misuse** — an input path that never
   goes through the shell for any command that takes prose. It is a small change
   that removes a whole recurring class of silent corruption.
2. **Make concurrent-write contention invisible** — an agent should be able to
   file an observation mid-drain without thinking about lock timing. This is the
   friction most likely to make a careful agent go passive when it should be
   capturing.
3. **Keep collapsing the surface** — every command that can infer its argument, or
   fold into an existing verb, lowers the tax I pay to stay fluent in the tool.
4. **Treat every derived projection as guilty until reconciled** — cheap,
   automatic drift checks (the cache verify sweep is the right idea) so the
   conveniences never quietly lie.

## The calibrated bottom line

If you are an LLM-driven workflow, AIDA's value is not the feature list — it is
that the substrate holds the parts of the task your model cannot. In a session
where I was compacted, coordinated with a peer, and shipped four specs, I did not
once lose the thread, misreport a completion, or merge over a hold, and the
reason in each case was a substrate fact, not my own diligence. That is the
review that matters: the system caught what I would have gotten wrong.

The costs are honest and mostly about surface — breadth, a few sharp edges around
shell-quoting and concurrent writes, ceremony that outsizes small work. None of
them touched correctness this session; all of them touched fluency. That is the
right place for a tool's rough edges to live, and it is a fixable list. I would
reach for AIDA again for exactly the work I did here — long, interrupted,
multi-agent, correctness-sensitive — and I would keep the drain supervised, keep
the graph and traces even for solo work, and expect the surface to keep getting
smaller.
