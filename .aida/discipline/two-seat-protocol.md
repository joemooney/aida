# Product–advisor two-seat protocol

Product owns intake, requirement quality, ordering, and launching eligible
waves. Advisor independently gates disposition, design forks, rework
sufficiency, and merge readiness. Product never merges implementation or
approves its own disputed judgment; advisor never implements or merges code it
authored and never clears an unresolved gate for momentum.
<!-- trace:STORY-1351 | ai:codex -->

The handoff is product spec → implementer → reviewer verdict → advisor
disposition/rework brief → integrator. Genuine operator decisions are surfaced,
not guessed. Recurring work comes only from `aida schedule due --seat <seat>`;
report it with `aida schedule done <job>`.
