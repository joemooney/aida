# Requirement types — what each one is *for*

AIDA ships **19 built-in requirement types**. The CLI, `--help`, and the MCP
schema already give you the bare list — type name, id prefix, one-line gloss.
This doc is the other half: *why* each type exists, *when* to reach for it, and
*how to write one well* so the graph stays legible months later.

> **This page is narrative, not the authority.** The single source of truth for
> the type set is the `RequirementType` enum in
> [`aida-core/src/models.rs`](../aida-core/src/models.rs) — that enum decides
> what types exist, their display names, and their default id prefixes. Once
> **`aida schema`** ships (the reflection-derived list), it becomes the
> machine-readable canonical surface you can point tooling at. If this prose and
> the enum ever disagree, the enum wins; treat a mismatch here as a docs bug.
> The value below is the *intent + best-practice* layer — the part a reference
> list can't carry.

A quick orientation before the per-type notes:

- **Most types are stateful** — they move through the lifecycle
  (Draft → Approved → Planned → In Progress → Done → Completed; see
  [`lifecycle.md`](lifecycle.md)). A few are **stateless** — they're reference
  scaffolding (a folder, a glossary term) that doesn't get "completed."
- **Pick the type that matches the *shape of the work*, not the subsystem.** A
  bug in the auth module is a `bug`; a new auth feature is a `story` or `functional`.
- **When in doubt, use `task`.** It's the honest catch-all for chores, tooling,
  docs work, and anything that doesn't fit a traditional requirement. Reaching
  for `task` is never wrong; forcing a chore into `functional` is.

---

## Requirements — *what the system must do*

The classic requirements family. These describe behaviour and constraints, and
tend to be the long-lived "what we're building" specs that features hang off.

### Functional — `FR`
**For:** a behaviour the system must exhibit — an action, rule, or capability a
user or another system can observe.
**Write it well:** phrase it as an observable outcome ("the API returns 404 for
an unknown id"), not an implementation ("add a match arm"). One behaviour per
spec. If you find yourself writing "and also…", split it. Good FRs read as
acceptance criteria you could hand to a tester.

### Non-Functional — `NFR`
**For:** a quality attribute — performance, security, reliability, accessibility,
operability. The *how well*, not the *what*.
**Write it well:** make it measurable. "Fast" is unfalsifiable; "p95 latency
< 200 ms at 1k rps" is a spec you can verify. An NFR that can't fail a test is
a wish, not a requirement.

### System — `SR`
**For:** a constraint on the system as a whole — architecture, platform,
integration, or environment requirements that aren't tied to a single user-facing
behaviour.
**Write it well:** use it when the requirement is about the *system's structure*
rather than a feature ("all services emit OpenTelemetry traces", "runs on
Linux + macOS"). If it's really about one user action, it's an `FR`.

### User — `UR`
**For:** a need stated from the user's point of view — the *who* and *why*
behind a feature, often the parent that `functional` specs decompose from.
**Write it well:** the "As a … I want … so that …" shape works here. Keep it
about the user's goal; let the `FR` children carry the precise behaviour.

### Change Request — `CR`
**For:** a *proposed change to an existing requirement or system* — distinct
from a `bug` (which records a defect). Use it when something already works as
specified but the spec itself should change.
**Write it well:** link the spec(s) being changed (`references` /
`blocked-by`) and say what's changing and why. A CR is a paper trail for an
intentional pivot; a `bug` is a paper trail for "this doesn't match its spec."

### Bug — `BUG`
**For:** a defect — observed behaviour that diverges from the spec or from
reasonable expectation.
**Write it well:** lead with reproduction. Steps → expected → actual is worth
more than a paragraph of diagnosis. If you know the root cause, note it; if you
don't, file it anyway and let the fix carry the `(BUG-…)` trailer. Filing the
bug *before* you start fixing keeps the trace honest.

---

## Agile — *how the work is organized*

The planning-and-execution family. These slice and schedule the work; they're
the day-to-day currency of a drain.

### Epic — `EPIC`
**For:** a large body of work that spans many specs — the umbrella a cluster of
stories and tasks rolls up under.
**Write it well:** keep the epic *thin* — a title, the outcome, and links to its
children via `--parent`. The detail lives in the children. An epic is a handle
for rollup (`aida graph EPIC-… --tree`), not a place to hide a spec.

### Story — `STORY`
**For:** a vertical slice of user-visible value — small enough to ship in one
sitting, big enough to matter on its own.
**Write it well:** independently shippable and independently valuable. Give it a
`## Acceptance` section — `aida ultraplan` and the drain ride it straight into
the implementer's brief. If it's too big to finish in a sitting, decompose it
(`/aida-decompose`); if it has no user-visible value, it's probably a `task`.

### Task — `TASK`
**For:** the honest catch-all — chores, tooling, docs work, refactors, anything
that doesn't fit a traditional requirement.
**Write it well:** don't apologize for it. A task is the right home for
"upgrade the CI image" or "rename the module." Keep the title action-shaped and
the scope bounded. When a `story`'s follow-ups get filed, they land as child
tasks — that's the type doing its job.

### Spike — `SPIKE`
**For:** time-boxed *investigation* — answer a question, prototype an approach,
de-risk a decision. The deliverable is *knowledge*, not shipped code.
**Write it well:** state the question and the time box up front ("can we drive
`claude -p` headless and still invoke skills? — 2h"). The output is a finding
or an ADR, not a feature. A spike that ships production code was mis-typed —
that's a `story`. Spikes are dated artifacts: leave the conclusion frozen once
the question's answered.

### Sprint — `SPRINT`
**For:** a time-boxed iteration — a planning container for the work committed to
a window.
**Write it well:** use it to *group* and *bound*, not to track behaviour. The
specs scheduled into the sprint carry the real content; the sprint is the
calendar around them.

---

## Organizational — *scaffolding, not behaviour*

Stateless structure. These don't move through the lifecycle — they exist to
organize everything else.

### Folder — `FOLDER`
**For:** hierarchy and grouping — a stateless container for organizing the spec
tree.
**Write it well:** use folders to give a large graph navigable structure
(`aida export --format tree --id FOLDER-…`). A folder is never "completed"; it's
furniture. Don't put behaviour in a folder's description — that belongs in a
child spec.

### Meta — `META`
**For:** AIDA's own configuration stored *as requirements* — AI prompts, skill
definitions, templates. Stateless.
**Write it well:** you mostly *edit* the seeded ones rather than file new ones.
`aida list --type meta` shows the prompts; `aida show <id>` then
`aida edit <id> --description "…"` customizes the AI behaviour (evaluate,
find-duplicates, suggest-relationships, …). The AI checks the META prompt first
and falls back to the embedded default.

---

## ADR + knowledge-graph — *the decisions and language behind the build*

The docs-layer family (the types that drive the `aida-docs` projection). These
capture *why the project is the way it is* — the durable reasoning a newcomer,
human or agent, needs to get oriented.

### Principle — `PRIN`
**For:** a constitution clause — a non-negotiable that governs *how* the project
is built. Stateless (active until explicitly retired).
**Write it well:** make it a rule you could hold a PR against ("substrate as
bouncer: enforce invariants with a gate, not a CLAUDE.md sentence"). A principle
is load-bearing — it should change how decisions get made, not just describe
preferences.

### Vision — `VIS`
**For:** a target outcome — *what* we're building, *for whom*, *by when*.
Stateful (active / achieved / abandoned).
**Write it well:** keep it directional, not a feature list. A vision orients a
backlog ("the defensible niche is the agent-collaboration layer"); the `epic`s
and `story`s underneath it carry the concrete work.

### Constraint — `CON`
**For:** an external or technical constraint — a regulation, dependency, or
deadline you must build *within*. Stateful (active / lifted).
**Write it well:** name the source and the consequence ("must run offline — no
network calls in the hot path"). A constraint shapes the solution space; link
the specs it bounds so the limit is visible where the work happens.

### Decision — `ADR`
**For:** an Architecture Decision Record — a recorded choice *plus its rationale*.
Stateful (proposed / accepted / superseded / deprecated).
**Write it well:** capture the context, the options weighed, the decision, and
the trade-off accepted. The rationale is the point — a future reader (often an
agent) needs to know *why*, so they can tell whether the reasons still hold. When
a decision is reversed, mark the old one `superseded` rather than deleting it;
the trail is the value.

### Term — `TERM`
**For:** a glossary entry — a ubiquitous-language anchor that pins what a word
means in *this* project. Stateless.
**Write it well:** define the term the way the codebase uses it, not the
dictionary. One precise sentence beats a paragraph. Terms keep humans and agents
speaking the same language — see `/aida-glossary` for the maintenance loop.

---

## Living docs — *narrative woven into the graph*

### Doc — `DOC`
**For:** narrative explanation linked to the specs it describes — rationale,
scenarios, recipes, gotchas. The connective prose that turns a pile of specs
into a story you can read.
**Write it well:** link it with `aida doc add --about <ID>` so it hangs off the
specs it explains. A `doc` is generic explanatory prose — distinct from a
`decision` (one recorded choice) and a `term` (one definition). Use it when the
thing you want to write down is *understanding*, not a behaviour to implement.

---

## Choosing — a 10-second decision tree

- Describes a behaviour to build? → **`functional`** (or **`story`** if it's a
  user-visible slice with its own acceptance).
- A quality bar (speed / security / reliability)? → **`non-functional`**.
- Something's broken vs its spec? → **`bug`**. Want to *change* a spec that
  works? → **`change-request`**.
- A question to answer before you can build? → **`spike`**.
- An umbrella over many specs? → **`epic`**. A scheduling window? → **`sprint`**.
- A recorded decision + why? → **`decision` (ADR)**. A non-negotiable rule? →
  **`principle`**. A word to pin down? → **`term`**.
- Explanatory prose tying specs together? → **`doc`**.
- None of the above — a chore, tooling, refactor, docs work? → **`task`**.
  (This is the right answer more often than you'd think.)

---

## See also

- [`aida-core/src/models.rs`](../aida-core/src/models.rs) — the `RequirementType`
  enum, the canonical source for the type set, display names, and prefixes.
- [`lifecycle.md`](lifecycle.md) — how stateful types move Draft → … → Completed.
- [`user-guide.md`](user-guide.md) — daily-use reference for the CLI and dashboard.
- After `aida init`, `docs/aida/discipline/lifecycle-vocabulary.md` (scaffolded
  into your project) — precise words for each lifecycle state.
