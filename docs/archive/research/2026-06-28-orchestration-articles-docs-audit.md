# Docs Audit — Orchestration Articles Applied To AIDA Research

**Date:** 2026-06-28  
**Inputs:** GitHub "What is AI agent orchestration?", Developers Digest "How to Coordinate Multiple AI Agents", recursive scan of `docs/**/*.md`.

## Useful Takeaways

The two articles do not change AIDA's core research question, but they improve the vocabulary and production checklist.

- Mainstream framing now treats agent orchestration as a **control plane**: state store, policy engine, guardrails, observability, cost controls, HITL, and recovery.
- Pattern vocabulary is useful for explaining AIDA without inventing terms: sequential, concurrent/fan-out, group chat, handoff/pipeline, hierarchical delegation, blackboard/shared state, consensus, and dynamic/magentic orchestration.
- The strongest connection to AIDA is the **blackboard pattern**: agents coordinate by reading and writing shared state. AIDA's research twist is that the blackboard is durable, program-owned, typed, traceable, and cross-vendor rather than a runtime object inside one framework.
- Production concerns should be explicit proof points: checkpoint/resume, retry/fallback, circuit breakers, structured handoffs, token/cost attribution, and per-agent traces.

## Docs Updated

- `docs/research/2026-06-16-research-proposal-multi-vendor-coordination.md`
  - Added control-plane vocabulary from the GitHub article.
  - Replaced the over-broad "no current practice" wording with a narrower claim that acknowledges Beads/Gas Town, GNAP, Goosetown, and messaging tools.
  - Added production orchestration economics and production-control metrics to the research proof points.

- `docs/research/2026-06-16-layered-evaluation-framework.md`
  - Narrowed the gate language to match the gate-vs-rule ablation results: own load-bearing gates; do not gate every prompt rule without field evidence.
  - Added observability and cost-control criteria.

- `docs/competitive-analysis/marketplace-roster.md`
  - Updated the dated roster note to point at the 2026-06-26 refresh.
  - Corrected the stale "cross-vendor + free + self-hosted + durable is unoccupied" claim after the Gas Town OSS cross-vendor update.

- `docs/research/2026-06-28-multi-vendor-coordination-deck.md`
  - Added orchestration frameworks as a market lane.
  - Added runtime controls to the proposed substrate and production metrics to the research plan.

- `docs/research/2026-06-28-multi-vendor-coordination-research-notes.md`
  - Added the two requested articles as explicit sources.
  - Added a short extraction of reusable material for presentation framing.

## Remaining Improvement Candidates

- Add a durable "orchestration pattern mapping" section to the theory paper: AIDA as durable blackboard + gated handoff/pipeline + selective fan-out.
- Revisit older competitive-analysis snapshots only if they are still presented as current. Many are dated snapshots and should remain immutable, but current index/roster pages should point readers to the newest refresh.
- Add production metrics to future ablation / field-study designs: cost per spec, retry count, circuit-breaker trips, resume success rate, and trace completeness.
- Consider adding a one-slide visual in the deck: "runtime orchestration vs program-owned coordination record" with the blackboard pattern as the bridge.
