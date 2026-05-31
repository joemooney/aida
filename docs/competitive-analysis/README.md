# AIDA Competitive Analysis & Market Positioning

**Last updated**: 2026-05-22  
**Refresh Cadence**: Quarterly (scheduled) / Signal-Triggered (see [signals-to-watch.md](signals-to-watch.md))

The AI dev-tooling and agent coordination landscape moves with extreme velocity. This directory serves as the **living, time-stamped record** of where AIDA sits in this ecosystem—documenting our defensible niche, assessing competitors, and tracking landscape developments.

---

## Directory Structure & Scope

We organize our market intelligence into three functional layers:

1.  **Strategic Positioning**: Defining AIDA's vertical value proposition in relation to horizontal platforms.
2.  **Point-in-Time snapshots**: Dated landscape scans capturing the exact state of competitor tools at a specific historical moment.
3.  **Category Summaries**: Multi-project, deep-dive architectural analyses focusing on specific coordinate paradigms.

---

## Index

| File / Directory | Type | Scope & Context |
|---|---|---|
| **[2026-05-31-round2-moat-gaps-moves.md](2026-05-31-round2-moat-gaps-moves.md)** | **Current Synthesis** | **The competitive picture as of 2026-05-31**: integrates four parallel deep-dives (Spec Kit, multi-agent frontier, AGENTS.md/MCP convergence, beyond-software use cases) into the commoditized-vs-differentiated split, the prioritized capability roadmap (P1 checkpointing … P5 drain legibility + AGENTS.md-generator + ReqIF option), use-case verdicts (pursue ADRs / watch compliance+memory / avoid ELN-wiki-PKM), the positioning line, and the tripwires. **Verdict: the durable core is the structured graph on git drained by an orchestrator with a spec-grounded escalation cascade, portable across vendors; the real risk is distribution, not differentiation.** |
| **[2026-05-31-git-canonical-substrate-thesis.md](2026-05-31-git-canonical-substrate-thesis.md)** | Strategic Thesis (round-1) | **The git-canonical wedge, pressure-tested**: grades the "competitors avoid git, we ride it" thesis as category-dependent and dangerous in its naive form; reframes the wedge to the stable-ID + typed-relationship + trace-enforced graph on git that no competitor combines, with multi-vendor portability as the strongest claim. Superseded on specifics by the round-2 synthesis above. |
| **[positioning.md](positioning.md)** | Strategic Statement | **AIDA's Defensible Niche**: Synthesizes AIDA's 8 core architectural pillars and details our horizontal-vertical symbiosis with Anthropic's platform. |
| **[2026-05-16-market-snapshot.md](2026-05-16-market-snapshot.md)** | Market Snapshot | **May 2026 Competitor Profiles**: Technical breakdown of 10 target systems (Claude Flow, Gastown, Claude Squad, Vibe-Kanban, Wit, Skillfold, wshobson, barkain). |
| **[2026-05-26-agent-memory-libraries.md](2026-05-26-agent-memory-libraries.md)** | Category Analysis | **Agent Memory Libraries**: Anatomy (extractor/store/retriever) + 4-kinds taxonomy applied to Mem0/LangMem/Graphiti/Cognee/Letta/Dreams. Verified against source. Identifies AIDA's 5-kind memory framework + git-canonical differentiator + offline-consolidation gap. |
| **[2026-05-26-marketplace-research.md](2026-05-26-marketplace-research.md)** | Market Snapshot / Roadmap | **Marketplace and MCP Distribution Scan**: Current extension-marketplace signals and a ranked roadmap for packaging AIDA as the agent-coordination substrate. |
| **[2026-05-26-ai-coding-agents.md](2026-05-26-ai-coding-agents.md)** | Category Analysis | **AI Coding Agents (Cline/Aider/Plandex/Goose/Continue)**: Framework lens (task-model / state / linkage / autonomy / multi-agent) applied to single-agent CLI coding tools. Source-verified. Identifies AIDA's structured spec graph + git-canonical substrate + lifecycle-as-substrate as differentiators; pulls 5 opportunities to learn from neighbors (Aider auto-commit, Plandex sandbox diff, Continue declarative checks, Goose compile target, Cline coordinator+specialist). |
| **[tui-prior-art.md](tui-prior-art.md)** | Architectural Spike | **Terminal User Interfaces**: An empirical UX study of six agent TUIs (CMux, Vibe Tree, Conductor, etc.) informing AIDA's child-hosting PTY design. |
| **[skillfold-spike.md](skillfold-spike.md)** | Architectural Spike | **Skillfold Compatibility**: A deep-dive gap analysis evaluating the compilation of AIDA's skill templates to declarative skillfold YAML. |
| **[category-summaries/](category-summaries/)** | Directory | **Ecosystem Lens Breakdowns**: Category-level summaries analyzing architectural shifts. |
| ├─ **[coordination-protocols.md](category-summaries/coordination-protocols.md)** | Category Summary | **Agent Concurrency**: Compares declarative compilers (Skillfold), lock daemons (Wit), and AIDA's git-native advisory leases. |
| ├─ **[swarm-orchestrators.md](category-summaries/swarm-orchestrators.md)** | Category Summary | **Agent swarms**: Analyzes WASM swarms (Claude Flow) and economic swarms (Swarm-Protocol) against AIDA's requirements graph. |
| ├─ **[parallel-session-managers.md](category-summaries/parallel-session-managers.md)** | Category Summary | **Workspace Isolation**: Surveys Git worktree multiplexers and database backends, cross-referencing our TUI prior-art study. |
| └─ **[agent-libraries.md](category-summaries/agent-libraries.md)** | Category Summary | **Ecosystem Extensibility**: Surveys agent command marketplaces and delegation prompt wrappers against AIDA's unified runtime. |
| **[signals-to-watch.md](signals-to-watch.md)** | Watch List | **Refresh Triggers**: Outlines the specific market events (e.g. Anthropic Teams release, star milestones) that trigger a directory scan. |
| **[2026-03-17-landscape-scan.md](2026-03-17-landscape-scan.md)** | Market Snapshot | **Historical Baseline**: Our foundational broad landscape scan, mapping early features and competitors. |
| **[claude-code-plugin-ecosystem.md](claude-code-plugin-ecosystem.md)** | Market Snapshot | **Claude Code Marketplace**: An early scan analyzing the unexploited discoverability channel of the plugin registry. |

---

## The Ecosystem Review Discipline

To ensure AIDA maintains its architectural edge, we follow a rigorous, continuous scanning discipline. This prevents our strategic competitive analysis from rotting and directly guides our product roadmap.

### 1. Cadence
- **Scheduled Scans**: Quarterly comprehensive reviews to completely re-evaluate the developer-agent landscape.
- **Signal-Triggered Scans**: Immediate evaluations triggered when critical events specified in [signals-to-watch.md](signals-to-watch.md) occur (e.g., a competitor reaching 10k stars, major provider releases).
- **Release Verification Hook**: Every minor or major version bump (via `scripts/release.sh`) prompts/verifies that an ecosystem scan has been conducted or acknowledged.

### 2. Who Scans (The Ecosystem Watch Captain)
The active scan is led by the **Ecosystem Watch Captain** (a role mapped to the 7th responsibility of the AIDA **Advisor**). The Captain is responsible for executing the scan checklist, documenting findings in [ecosystem-watch.md](ecosystem-watch.md), and updating the individual positioning papers.

### 3. Tools & Sources
- **Release Feeds**: Anthropic Claude Code changelogs, OpenAI/Goose releases, and major terminal-agent GitHub release/tag feeds.
- **Star & Traffic Trackers**: GitHub trending, npm/pip download telemetry, and developer community forums (Hacker News, Reddit, Twitter/X) to detect early-stage momentum.
- **Standardization repos**: The Model Context Protocol (MCP) spec repository, `byronxlg/skillfold` changes, and collaborative workspace protocols.

### 4. Assessment Matrix
Every evaluated tool, feature, or platform update must be classified into one of four action quadrants:
- **Compete**: The feature directly challenges AIDA's vertical value. We must document the positioning gap and pivot our engineering roadmap if necessary.
- **Complement**: The capability is adjacent to AIDA's core (e.g., a better local PTY or specialized shell subagent). We partner, wrap, or pair with it rather than rebuilding it.
- **Integrate**: The tool represents an emerging standard (e.g. MCP-as-bus or Skillfold schemas) that AIDA should adopt natively to compose with other systems.
- **Ignore**: The capability is out-of-scope, low-momentum, or closed-ecosystem. We track but do not act.

### 5. The Feedback Loop
Scanning is not just documentation; it is a direct pipeline to product engineering. Any identified gap or strategic pivot is immediately filed into AIDA's active requirements database as a `TASK` or `BUG` (using `/aida-req` or `aida queue add`). Every item logged in [ecosystem-watch.md](ecosystem-watch.md) should ideally resolve to a requirement card, creating a paper trail from market intelligence to shipped code.

---

## Sibling: `docs/positioning/`

While this directory is a wide-angle, time-stamped view of the wider landscape, our sibling directory **[docs/positioning/](../positioning/)** answers the immediate question: *"Should I use AIDA or X?"* in the form of sharp, paired comparisons (e.g., `vs-ultraplan.md`, `vs-claude-code-subagents.md`). Positioning is the argument; competitive-analysis is the evidence.

---

## The Living-Doc Discipline & Contribution Guide

To prevent competitive intelligence from rotting, we enforce strict documentation hygiene:

1.  **Durable Time-Stamps**: Every file carries a prominent `Last updated` line.
2.  **Appends Over Overwrites**: Do not silently delete or overwrite old competitor descriptions. If a competitor ships a major update, append a new dated entry (e.g., *Update: 2026-05-22*). The diff between dates represents crucial trajectory signal.
3.  **Human-Readable Text**: Keep markdown files clean and optimized for human reading. Avoid referencing internal specification IDs or database tags in the body text (keep them strictly in the metadata or conclusion blocks if necessary).
4.  **How to Contribute**: If you detect context drift or discover a new competitor:
    *   Open a branch (`docs/competitive-analysis-update`).
    *   Modify the relevant category summary or append a dated entry.
    *   Commit using the standard scope: `docs(competitive): update positioning vs X`.
    *   Run `aida pr ship` to verify and merge.
