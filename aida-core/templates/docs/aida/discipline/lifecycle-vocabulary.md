# Lifecycle vocabulary

A spec moves through many states between "someone started typing code" and
"users have it." Fuzzy verbs — especially **"ship"** — collapse those states
together and make it impossible to tell, from a sentence, whether a spec is
in a PR awaiting review, merged on the main branch, or released with a
version tag. Use the precise verb for the state.

## The lifecycle

```
   in-progress          done                    completed        released
   ───────────►  ─────────────────────►  ──────────────►  ─────────────►
   committed     pushed   PR-opened  reviewed   merged    auto-bumped   tagged
   (local git)   (origin) (gh PR)    (verdict)  (main)    (Completed)   (binaries)
```

## The states and their verbs

| Verb | What it means | AIDA spec status |
|------|---------------|------------------|
| **Committed** | Work exists in local git history | still In Progress |
| **Pushed** | The branch is reflected on `origin` | still In Progress |
| **PR opened** | A PR exists, awaiting CI / review | Done |
| **Reviewed** | A reviewer rendered a verdict (approved / rejected) | still Done — waiting for merge |
| **Merged** | The PR landed on the main branch | should auto-bump to Completed |
| **Completed** | The spec's status is Completed in AIDA | final state |
| **Released** | A version tag exists and binaries are published | cross-spec — aggregates many merges |

## The off-mainline state: Needs Attention

Not every spec travels the line cleanly. **Needs Attention** is an
*off-mainline* status: a spec that was **In Progress** but is now **paused**
because an autonomous agent hit a design-fork it could not safely resolve and
**punted** (`aida punt` / `/aida-punt`) rather than guess.

- Reached only from **In Progress** — punting is the "I was working this and
  hit a fork" move.
- Carries a structured reason: an obstacle **category** (`design-fork`,
  `ambiguous-spec`, `missing-context`, `blocked-dependency`, `other`), a
  human-readable **detail**, and an optional **lean** (the agent's
  best-guess-if-forced, kept separate from the fork itself).
- Excluded from normal queue pickup, but surfaces in `aida findings` for
  human / advisor triage — a Needs Attention spec *is* a punt awaiting triage.
- Resolved out only to **Approved**, **In Progress**, or **Rejected**.

Punting is a discipline, not a failure: a punted fork is a recorded decision
point. Guessing past it produces a silent wrong implementation; punting keeps
the wrong call from being made at all.

## How to apply

- Default **"ship"** to mean **merged to the main branch** — the
  developer-facing "out the door."
- For earlier states, use the precise verb: "PR opened for TASK-12", "TASK-12
  is reviewed, waiting on merge", "TASK-12's PR merged".
- For "out to users with a version number," say **released** — distinct from
  merged, because a merge does not auto-release.
- `lifecycle:*` tags change which orchestrator phases run; they do not change
  the spec-state vocabulary. A spec still becomes **Done** when the PR opens
  and **Completed** when a referenced commit lands on main. See
  [`machinery-glossary.md`](machinery-glossary.md#lifecycle-short-circuit-tags)
  for the phase-skip contract.

Better phrasing:

- ~~"TASK-12 shipped"~~ → "TASK-12's PR merged" / "TASK-12 is on main"
- ~~"shipped to a PR"~~ → "PR opened for TASK-12"
- ~~"v1.2 shipped"~~ → "v1.2 was released" (binaries published)

## Why precision matters

Across a long conversation many specs sit at many different states. Precise
verbs let the user track which spec is where in the pipeline — exactly the
workflow-state awareness AIDA is meant to provide. `done` vs `completed` is
the load-bearing distinction: `done` means "work finished on a branch";
`completed` means "merged to the main branch." AIDA auto-bumps
`done → completed` when a commit referencing the spec lands on the default
branch, so you rarely set `completed` by hand — let the merge promote it.
