---
name: precise-lifecycle-vocabulary
description: Don't conflate committed / pushed / PR-opened / reviewed / merged / completed / released under fuzzy verbs like "ship".
propagation: scaffolding-pack
metadata:
  type: feedback
---

A spec passes through 6+ distinct lifecycle states. Conflating them under a fuzzy verb — especially "ship" — makes it impossible to tell whether a spec is in a PR awaiting review, merged on the main branch, or released with a version tag.

| Verb | What it means | Spec status |
|------|---------------|-------------|
| Committed | Work in local git history | In Progress |
| Pushed | Branch reflected on `origin` | In Progress |
| PR opened | PR exists, awaiting CI / review | Done |
| Reviewed | Reviewer rendered a verdict | still Done |
| Merged | PR landed on the main branch | auto-bumps to Completed |
| Completed | Spec status = Completed | final |
| Released | Version tag + binaries published | cross-spec |

**Why:** Across a long conversation many specs sit at many states; precise verbs let the user track which spec is where in the pipeline.

**How to apply:** Default "ship" to mean **merged to the main branch**. For earlier states use the precise verb ("PR opened for TASK-12", "TASK-12 is reviewed, waiting on merge"). For "out to users with a version", say **released** — a merge does not auto-release. The `done` vs `completed` distinction is load-bearing: `done` = work finished on a branch; `completed` = merged to the default branch (AIDA auto-bumps it).

Composes with [[run-help-before-suggesting-flags]] and [[verify-before-filing]] — same family: precision over assumption.
