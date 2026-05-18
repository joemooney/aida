---
name: classify-memory-propagation
description: When writing a memory, classify it — AIDA-specific / generic / user-personal. Generic discipline should propagate to new projects via scaffolding.
propagation: scaffolding-pack
metadata:
  type: feedback
---

A project's memory directory captures discipline as it is discovered. Some of that discipline is specific to *this* project; some is generic — it applies to *any* project using AIDA. Generic discipline is most valuable when it propagates to new projects rather than being re-discovered each time.

**Why:** Memories only fire for sessions in the project where they were written. Generic discipline trapped in one project's memory dir is invisible to every other project — each one re-learns the same friction independently.

**How to apply:** When writing a new memory, classify it:

- References this project's internal code, file paths, architecture → **project-specific**; stays local.
- Describes a habit, vocabulary, workflow pattern, or design principle → **generic**; consider propagation.
- Describes a *user* preference (formatting taste, permission posture) → **user-personal**; stays local.

For generic memories, the propagation marker is the load-bearing step: add `propagation: scaffolding-pack` to the frontmatter immediately, right after the `description:` line. Intent is not enough — only the marker propagates. AIDA's starter discipline pack ships every memory carrying that marker.

Audit periodically: a quick scan for memory files missing the `propagation:` line finds generic discipline that has not been marked yet.

**Pattern to avoid:** write memory → index it → consider it done → accumulate dozens of generic memories that never reach another project.

Composes with [[advisor-role-responsibilities]] (memory curation is one of the six responsibilities).
