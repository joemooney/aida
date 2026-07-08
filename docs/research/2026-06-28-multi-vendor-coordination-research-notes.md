# Research Notes — Multi-Vendor Coordination Presentation

Companion to `2026-06-28-multi-vendor-coordination-deck.md`.

## Confidence Model

This presentation is grounded in the proposal and the newer competitive scans, with public-source checks for the highest-risk claims.

- Swept AIDA research and competitive docs:
  - `docs/research/2026-06-16-research-proposal-multi-vendor-coordination.md`
  - `docs/research/2026-07-08-coordinating-multi-vendor-agent-fleets.md`
  - `docs/research/2026-06-16-layered-evaluation-framework.md`
  - `docs/research/2026-06-26-agent-coordination-market-landscape.md`
  - `docs/competitive-analysis/marketplace-roster.md`
- Re-checked primary/public surfaces for Beads, Gas Town, GNAP, Goosetown, goose, agmsg, tap, Claude Code Agent Teams, Codex subagents, MCP/AAIF, and A2A.
- Reviewed GitHub's April 2026 orchestration primer and Developers Digest's 2026 multi-agent coordination guide for mainstream production concerns and pattern vocabulary.
- Treated star counts, benchmarks, and adoption claims as directional unless primary-source and dated.
- Separated shipping products, protocols, research papers, and positioning claims.
- Main residual uncertainty: whether a neutral incumbent or standard captures the durable coordination-record layer before a home-grown substrate matures.

## Competitor / Neighbor Map

| Lane | Projects | Presentation treatment |
|---|---|---|
| Vendor-native coordination | Claude Code Agent Teams, Codex subagents, Cursor agents, Copilot agents, Devin | Evidence that coordination matters; mostly not program-owned or cross-vendor as a shared record |
| Durable OSS coordination | Beads + Gas Town / Gas City, GNAP, Goosetown | Critical neighbors; prevent any "AIDA is unique because graph/coordination" overclaim |
| Messaging substrate | agmsg, tap, Agent Mail / AgentMail, AMP-like protocols | Commoditizes mailbox/message bus; does not by itself provide a requirement graph or lifecycle |
| Spec / issue / intent tools | Spec Kit, Kiro, OpenSpec, BMAD, Backlog.md, Claude Task Master, Linear, Jira/Rovo, Miyabi, Intent, Augment Cosmos | Shows specs-as-artifacts and typed work graphs are mainstream; AIDA's claim must be narrower |
| Standards and context | MCP, A2A, AGENTS.md, goose under AAIF | Transport/context standardization is a tailwind, but not a durable coordination record |
| Research | ReqToCode | Code-to-requirement traceability is now being formalized; still not a shipping competitor with AIDA-style trace comments |

## Useful Material From The Two Requested Articles

- GitHub's orchestration article is useful for executive vocabulary: orchestration is a control layer for agent creation, responsibility assignment, communication, conflict resolution, escalation, state management, policy-as-code, auditability, cost control, and HITL checkpoints.
- Its pattern taxonomy is presentation-friendly: sequential for safety/auditability, concurrent for independent work, group chat for exploration, handoff for approval-heavy chains, and "magentic" / dynamic orchestration for changing conditions.
- Developers Digest adds the implementation vocabulary AIDA should map against: fan-out/fan-in, pipeline, hierarchical delegation, blackboard shared state, handoff chain, and consensus.
- The production checklist is directly useful for AIDA's research proof points: explicit state, checkpoint/resume, retry with context, fallback agents, circuit breakers, structured output validation, token/cost controls, and per-agent traces.
- The blackboard pattern sharpens AIDA's substrate framing: AIDA is a durable, program-owned blackboard where the shared state is not just runtime scratch state but typed requirements, lifecycle, traces, and coordination messages.

## Key External Sources Checked

- Beads: <https://github.com/gastownhall/beads>
- Gas Town: <https://github.com/gastownhall/gastown>
- GNAP: <https://github.com/farol-team/gnap>
- Goosetown: <https://github.com/block/goosetown>
- goose: <https://github.com/aaif-goose/goose>
- agmsg: <https://github.com/fujibee/agmsg>
- tap: <https://github.com/HUA-Labs/tap>
- Claude Code Agent Teams: <https://code.claude.com/docs/en/agent-teams>
- Codex subagents: <https://developers.openai.com/codex/subagents>
- MCP / AAIF: <https://www.anthropic.com/news/donating-the-model-context-protocol-and-establishing-of-the-agentic-ai-foundation>
- A2A / Linux Foundation: <https://www.linuxfoundation.org/press/linux-foundation-launches-the-agent2agent-protocol-project-to-enable-secure-intelligent-communication-between-ai-agents>
- GitHub orchestration primer: <https://github.com/resources/articles/what-is-ai-agent-orchestration>
- Developers Digest coordination guide: <https://www.developersdigest.tech/blog/how-to-coordinate-multiple-ai-agents>

## Defensive Positioning

Avoid:

- "AIDA is the only typed graph." Beads, Linear, and Jira make this false.
- "AIDA is the only durable coordination layer." Beads/Gas Town and GNAP make this false.
- "The space is empty." It is not.
- "Programmatic gates beat rules." The controlled ablation program did not support that broad claim.

Say instead:

> The precise open slice is a program-owned coordination record that combines a typed requirement graph, machine-checkable code-to-spec traces, lifecycle/history, pre-work authority, and cross-vendor access without making a vendor runtime or hosted control plane the source of truth.

That is the research claim to test, not a settled product claim.
