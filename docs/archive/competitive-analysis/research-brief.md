# Competitive & Marketplace Deep-Dive — Reusable Multi-Agent Research Brief

**Type**: Reusable mission brief (hand this to any agent)
**Cadence**: Weekly deep dive (supersedes the quarterly default in `README.md` for the active race period)
**Mode**: Multi-agent, parallel lanes + adversarial red-team + synthesis
**Output**: A dated snapshot under `docs/competitive-analysis/YYYY-MM-DD-weekly-scan.md` + targeted updates to `ecosystem-watch.md` / positioning papers
**Evolves toward**: An `aida ultraplan`-assembled adversarial deep-research run (see §9). This brief is the manual scaffold of that capability.

> This brief is **self-contained**: an agent with no prior context can execute a lane from a cold start by following §3 (ground yourself) → your assigned lane in §4 → the output contract in §7 → the rigor rules in §8.

---

## 1. The mission, in one paragraph

Keep AIDA's strategic picture from rotting in a landscape that moves weekly. Each run answers five questions — **(1) what *is* AIDA right now, honestly; (2) who are the competitors and what did they ship this week; (3) where is the market going; (4) what should we adopt; (5) how do we adapt it to AIDA's substrate.** The output is a dated synthesis the operator and the master advisor can act on: a prioritized adopt/adapt/avoid list, refreshed positioning, and tripwires. It is **decision fuel, not a literature review.**

## 2. The five questions (every lane answers its slice of these)

1. **What is AIDA?** — State the current defensible core in one paragraph, *without* the marketing gloss. The durable thesis (verify it still holds): *a structured requirement graph on git, with stable IDs + typed relationships + code-to-spec traces, drained by an orchestrator with a spec-grounded escalation cascade, portable across AI vendors.* Does this week's evidence still support that, or has a competitor closed the gap?
2. **Who are the competitors?** — What shipped, what gained traction (stars/downloads/funding/launch), what changed in positioning. New entrants matter as much as incumbents.
3. **Where is the market going?** — The 6–12 week trajectory. Provider primitives (Anthropic/OpenAI), standardization (MCP, AGENTS.md, skill formats), the spec-driven-dev category, agent-orchestration frontier.
4. **What to adopt?** — Concrete capabilities a neighbor does better that AIDA should ride. Name the feature, the source, the evidence it's good.
5. **How to adapt it?** — The AIDA-substrate translation: how would this land on the graph/orchestrator/MCP surface, not as a bolt-on. This is where most "adopt" ideas die or get sharper.

## 3. Ground yourself first (REQUIRED reading before you scan)

Do not start from a blank page — you will re-derive what we already know and miss the deltas. Read, in order:

- `OVERVIEW.md` — what AIDA is and the Trojan-horse framing.
- `docs/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md` — **the current keystone synthesis**. Your job is to find what changed *since* this.
- `docs/competitive-analysis/2026-05-31-git-canonical-substrate-thesis.md` — the wedge, pressure-tested (and where it's dangerous).
- `docs/competitive-analysis/positioning.md` + the relevant `docs/positioning/vs-*.md` for your lane's competitors.
- `docs/competitive-analysis/signals-to-watch.md` — the standing tripwires; check each.
- `docs/competitive-analysis/README.md` §"Ecosystem Review Discipline" — the sources & tools list.

Then state, in 3 bullets, **what you expect to find** before you look. Surprises against that expectation are the highest-value findings.

## 4. Lanes (parallelize across agents — one agent per lane)

Each lane is independent and source-grounded. Pick the lane assigned to you; if running solo, do them in priority order. **Competitor lists are seeds, not limits — chase new entrants.**

### Lane A — Spec-driven-dev neighbors (the nearest competitors)
Scope: GitHub Spec Kit, Kiro, Karpathy-style `*.md` discipline, and any new "spec/PRD-as-source-of-truth" entrant. These are who a buyer compares AIDA to first.
Key question: has any of them added the *graph* (typed relationships, stable IDs, trace enforcement) or just markdown + AI? That gap is the moat — verify it's still open.
Sources: their repos/changelogs, docs, launch posts, HN/Reddit threads.

### Lane B — Agent orchestration & swarm frontier
Scope: Claude Code subagents + workflows, Claude Flow, Gastown, Claude Squad, Vibe-Kanban, Wit, and new multi-agent orchestrators. Plus provider-native coordination primitives (the biggest commoditization risk).
Key question: what did Anthropic/OpenAI ship that commoditizes part of AIDA's orchestrator/lease/worktree layer? What should we *stop building* and ride instead?

### Lane C — Memory, substrate & MCP/marketplace distribution
Scope: agent-memory libs (Mem0/LangMem/Graphiti/Cognee/Letta/Dreams), the MCP marketplace/registry landscape, AGENTS.md convergence, skill/plugin ecosystems. Distribution is the named real risk ("not differentiation, distribution").
Key question: where does AIDA get *discovered*? What packaging/marketplace move has the highest reach-per-effort this week?

### Lane D — Adversarial red-team (the one that keeps us honest)
Scope: **Argue AIDA loses.** Build the strongest case that AIDA is obviated, redundant, or about to be — by a provider primitive, a faster competitor, or a shift that makes the graph irrelevant. Steelman the "I could do this in 20 lines of bash / Claude already does this" reaction.
Key question: what is the single most credible path to AIDA being irrelevant in 6 months, and what early signal would tell us it's happening?
Output: the 3 strongest kill-shots, each with the tripwire that would confirm it.

> The synthesizer (the master advisor, or whoever runs last) reconciles A–D. **Disagreement between lanes is signal, not noise** — surface it, don't average it away.

## 5. The adversarial discipline (why multi-agent, not one pass)

- Each lane forms an **independent** view from sources — do not read other lanes' drafts before forming yours (avoids anchoring).
- Lane D exists to attack the conclusions the others want to reach.
- Synthesis **reconciles**, naming where lanes disagree and which evidence wins.
- This independent-then-reconcile structure is the manual seed of the §9 adversarial-deep-research capability.

## 6. Sources & how to use them

Release feeds (Anthropic Claude Code changelog, OpenAI/Goose releases, terminal-agent GitHub release/tag feeds) · star/traffic trackers (GitHub trending, npm/pip downloads) · community (HN, Reddit, X) for early momentum · standardization repos (MCP spec, `byronxlg/skillfold`, AGENTS.md). Prefer primary sources (the repo, the changelog, the spec) over commentary.

## 7. Output contract

Produce a **new dated file** `docs/competitive-analysis/YYYY-MM-DD-weekly-scan.md` (never overwrite an existing dated snapshot — they are immutable point-in-time records). Mirror the keystone structure:

1. **Integrated picture** (3–5 sentences: what changed this week).
2. **Per-lane findings** (A/B/C, each with source links).
3. **Adversarial read** (Lane D: the kill-shots + their tripwires).
4. **Commoditized vs differentiated** (what to ride vs defend — deltas only).
5. **Adopt / adapt / avoid** — a prioritized table: capability · source · why · how it lands on AIDA's substrate · effort. Each "adopt" should be fileable as a draft spec.
6. **Positioning line** — does the one-liner still survive? Revise if not.
7. **Tripwires** (update `signals-to-watch.md` if any fired or any new one emerged).
8. **Honest meta-read** — confidence level, what you couldn't verify, what to chase next run.

Then: update `ecosystem-watch.md` with the scan log entry, and open **draft specs** for the top adopt/adapt items (the master triages — do not self-approve or queue).

## 8. Rigor rules (non-negotiable — this is what separates intel from slop)

- **Source-verify every load-bearing claim.** Tag provenance: *verified (I read the source)* vs *inferred*. Never launder an inferred claim into a stated fact. Force yourself to link the source.
- **Don't overclaim.** Avoid "unsolved," "only," "nobody does this." State the *precise* open slice. Treat a competitor *investing* in something as **validation** of the direction, not just a threat.
- **Anchor "why the gap persists" on incentive, not capability.** "They can't" ages badly (they can, next release); "they won't, because their business model points elsewhere" ages well.
- **Question form, not just existence.** When AIDA already has something a competitor has, ask whether *our current shape* is still right — path-dependence ≠ correctness.
- **No silent truncation.** If you sampled or capped coverage (top-N repos, one forum), say so.

## 9. The evolution path (where this is going)

Today: manual lanes, hand-assigned, synthesized by the advisor. Next: `aida ultraplan` assembles the brief + lane context into a richer adversarial-deep-research prompt; lanes run as fanned-out agents; a synthesis + red-team pass runs automatically; the output lands as a draft snapshot for triage. The lane structure (§4) and the independent-then-reconcile discipline (§5) are deliberately designed to survive that automation — they become the workflow's stages.

## 10. Hand-off (paste-ready)

To dispatch an agent on a lane, paste the block in §10's child file `research-brief-dispatch.md` — or inline:

> **You are running Lane <X> of AIDA's weekly competitive deep-dive.** Read `docs/competitive-analysis/research-brief.md`, ground yourself per §3, execute Lane <X> in §4 under the §8 rigor rules, and produce your lane's section per §7. Source-link every load-bearing claim and tag provenance. File draft specs for adopt/adapt items; do not approve or queue them. Return your lane section as your final output.
