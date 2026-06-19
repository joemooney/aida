---
description: "Interrogate the human to author acceptance criteria for a spec the questions sweep flagged as under-specified — the human-decision PRODUCER that turns \"parked: missing acceptance\" into \"burndown-ready\"."
---
# Clarify an Under-Specified Spec

Interrogate the human to author acceptance criteria for a spec the questions
sweep flagged as under-specified — the human-decision PRODUCER that turns
"parked: missing acceptance" into "burndown-ready".

## Instructions

Follow the workflow in `.claude/skills/aida-clarify.md`:

1. Resolve the target spec(s) from `$ARGUMENTS` (one spec, or the swept set
   when launched via `aida questions clarify`).
2. Skip any non-clarifiable spec: visions/folders/meta/principles/terms,
   already-built or held-for-review specs, or specs with an active lease.
3. Per spec, read it + its graph context (parent, siblings, trace markers, the
   sweep's flag reason).
4. Interrogate with a SMALL, targeted question set — lead with definition-of-done,
   primary caller/environment (TTY vs headless, agent/role, MCP vs CLI),
   output/shape, explicit out-of-scope. Structured choices where the fork is
   enumerable; free-text where it is authoring.
5. Surface any conflict between the human's answers and the spec's existing
   body and RECONCILE — never silently override.
6. Draft a `## Acceptance` section, show it for approval/edit, bind it via
   `aida edit`, and clear the gating parking tag now that sign-off happened.
7. Re-run the sweep / pickability check and report whether the spec is now
   autonomous-ready — and what still blocks it if not.

This is interactive (`claude "/aida-clarify ..."`, not headless). It pairs with
`aida burndown run` as its human-side complement: clarify produces the pickable
specs the drain burns down.
