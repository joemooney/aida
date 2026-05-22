---
name: Memory-pack hygiene — schedule audits; memories can become OBE
description: The memory pack is a living substrate, not a write-only log. Run explicit audits on a defined cadence to retire / revise / re-tag memories. Five OBE categories, six self-reflection questions, three outcomes.
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
Memories are point-in-time captures of advisor judgment. They go stale. A pack that only grows is one that increasingly contradicts itself, references dead code, and burdens fresh-loaded advisors with prescriptions for problems that no longer exist. **An audit cycle is part of the discipline, not optional polish.**

## Five categories of OBE (overcome-by-events)

1. **Capability landed.** The memory captured a workaround for an absent feature. When the feature ships, the prescription is obsolete. The principle may survive; the *action* changes.
2. **Empirical claim contradicted.** The memory's worked example or mechanism-claim was wrong, or later evidence refuted it. Retire or revise.
3. **Underlying tool / dependency drift.** Memories cite behaviour of Claude Code, `gh`, `git`, AIDA itself. When those change, mechanism-claims age. The principle often survives; the citation needs refreshing.
4. **Duplicate or superseded.** A more refined memory subsumes an older one; without explicit synthesis bridging them, one is incoherent. Retire or merge.
5. **Project pivoted.** Strategic positioning / vocabulary / roadmap memories assume a direction. When the direction moves, those memories misdirect the fresh-loaded advisor.

## Six self-reflection questions per memory

- **Does the worked example still apply?** If the incident the `Why:` block cites no longer represents system behaviour, the evidence is stale even if the principle holds. Revise the example or retire.
- **Is the prescription verifiable today?** Run the diagnostic the memory implies — does the system still exhibit what the memory warns about?
- **Does it contradict a more recent memory without explicit synthesis?** One is wrong-or-stale; reconcile.
- **Is it cited via `[[wikilinks]]` from anywhere?** Zero in-links = pile-item. Either link in (load-bearing for downstream principle) or consider retirement.
- **How old is `originSessionId`?** Trace lineage. Long-untouched memories from old sessions are review-flagged.
- **For `propagation: scaffolding-pack` memories:** is the prescription valid in a *fresh project* with none of this project's accumulated state? A memory that depends on AIDA-repo-internals shouldn't scaffold.

## Three pack-level questions

- **Clusters without synthesis** — N memories touching the same concept without a meta-memory tying them together = fragmentation. Synthesise or accept tension.
- **Sync drift** — for scaffolding-pack memories, master copies in `aida-core/templates/memories/` should match local copies. Drift = new projects inherit older discipline than this project has.
- **Coverage holes** — recurring frictions in the session with no memory yet. The pack should grow to *match* observed reality.

## Audit triggers

When to run the cycle:

- **Keystone spec ships** (e.g., STORY-306 merges → audit anything touching advisor-routing, manual-relay, escalation).
- **Release boundary** (`make release-minor` → audit the substrate before promoting).
- **N weeks since last full audit** — calendar trigger. The longer the gap, the harder the cleanup.
- **Sense of accumulating friction** — if recent sessions repeatedly re-discover the same insight, the substrate isn't recording it well.

## Three outcomes per memory after audit

For each memory: **retire** (delete, with a note in the index entry of why), **revise** (update the example, prescription, or tags), or **preserve** (still load-bearing as-is).

## Composes with

- [[feedback_propagate_generic_discipline_via_scaffolding]] (writing-time classification; this memory is the audit-time recheck)
- [[feedback_capture_over_concentration]] (capture is one half; retiring stale captures is the other)
- [[feedback_refinements_must_be_acceptance_criteria]] (refinements go in acceptance, not comments — same principle applied to memories: revisions live in the file, not in a parallel note)
