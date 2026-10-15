# Git-canonical knowledge substrate — competitive thesis (2026-05-31)

**Specs:** SPIKE-43 · **Status:** living (dated snapshot) · **Evidence:** web-current May 2026, graded inline
**Inputs:** operator strategic mandate (2026-05-31) + a competitive-intel research pass across AI memory libs, agentic coding tools, AI-native requirements/PM tools, and multi-agent orchestrators.

> This snapshot is frozen at time T per the immutability discipline. Supersede with a new dated file; do not retro-edit.

## The thesis under test (operator framing)

> *Git has emerged as the common standard for versioned state, so building the knowledge substrate ON git rides that standard. Competitors who deliberately avoid a git dependency may be missing the common infrastructure — AIDA's opening. AIDA also bets on multi-vendor (vendor-neutral) operation.*

## Headline verdict: partially true, category-dependent — and the naive form is dangerous

"Competitors avoid git, we ride it" is **true only in patches and FALSE against our nearest competitor.** Graded:

| Category | "Avoids git as the store"? | Why it matters to AIDA |
|---|---|---|
| **AI memory libraries** (mem0, Letta, Zep/Graphiti, Cognee) | **TRUE & strong** — uniformly DB / vector / temporal-graph | But it's a *different problem shape* (semantic recall, temporal fact-queries) git is genuinely bad at. **Not AIDA's fight.** Claiming a win here is a category error. |
| **Agentic coding tools** (Claude Code, Cursor, Windsurf, Aider, Continue, Cline) | **PARTIALLY** — they *operate in git*; shared rules go in git as prose; private auto-memory lives in app/home state outside git | The opening is AIDA's **structure**, not its git usage — they're already in git, just as unstructured prose with no IDs/graph/traces. |
| **AI-native requirements/PM** (GitHub **Spec Kit**, indie "files-in-git PM", Karpathy-md; Linear/Jira) | **FALSE at the AI-native end / CONTESTED** — Spec Kit + indie tools already do git-canonical specs *with AIDA's exact rationale* ("the filesystem is the API," "versioned alongside the repo"). Only legacy SaaS (Linear/Jira) avoids git, and they aren't contesting the substrate role. | **This is where leaning on "they avoid git" is most dangerous.** GitHub itself ships files-in-git specs. |
| **Multi-agent orchestrators** (claude-flow, agent teams) | **TRUE for a sound reason** — live hot-loop coordination needs fast multi-writer (SQLite / message bus); git's commit-per-write is too coarse mid-task | **AIDA agrees** — AIDA itself uses a SQLite cache + file handshakes for coordination, git only as durable writer-of-record. |

## The real wedge (survives contact with the evidence)

The defensible position is **NOT** "git vs DB." It is the **structure layer on top of files-in-git that no competitor combines**:

> **A stable-ID, typed-relationship, trace-enforced knowledge graph on git, queryable by agents via MCP, with a rebuildable cache for speed.**

- **Memory libs** have structure (graphs) but in opaque DBs for a different workload.
- **Coding tools** are in git but store unstructured prose (no IDs, no typed edges, no code↔spec traces, no query layer). The `AGENTS.md` convergence is the "Karpathy floor" AIDA already sits above.
- **Spec Kit** (nearest competitor) is git-canonical specs but **immutable, branch-frozen, numbered dirs** — no stable cross-cutting IDs, no relationship graph, no trace-to-code, no MCP graph resource. Its model is "spec → plan → tasks → implement, then freeze." AIDA's IDs + typed relationships + trace enforcement + cache + MCP graph is the genuine delta. *(Confidence: med — Spec Kit lacking these is the linchpin of our differentiation; VERIFY before building positioning on it — see Follow-ups.)*

## Multi-vendor is the strongest, truest claim

Git-canonical is the *mechanism* that makes vendor-neutral knowledge actually portable:

- App-local memory — Cursor Memories, Windsurf memories, Claude auto-memory — is **structurally single-vendor**: it lives in that vendor's app/home state and cannot be shared across tools ("your colleague's Cursor doesn't have what you discovered").
- AIDA's substrate is plain git files + an open MCP interface, so **any** agent (Claude, Codex, Cursor, Antigravity) reads/writes the same store with no vendor API. Vendor-neutral peers (Aider, Continue, Spec Kit) validate the demand; none pair it with the structured graph.

This is a **sharper, more durable wedge than "competitors avoid git"** — and it's anchored on *incentive* (single-vendor runtimes structurally won't make their memory portable; it's against their lock-in interest), which ages better than any capability claim.

## Honesty correction (must propagate to positioning)

AIDA already concedes the DB advantages **by design**: it builds a **SQLite cache** (for query speed git can't serve) + **file handshakes** (for coordination git's commit model is too coarse for) on top of git. So the architecture is **git-canonical writer-of-record + cache for queries + structure no files-in-git competitor has** — not git purity. Positioning docs and the I3-style framing must NOT claim "we use git, they use DBs" — that's imprecise and false against Spec Kit. Lead with the sophistication.

## Where the bet wins / loses

**Wins:** solo/small teams in git who distrust SaaS lock-in; regulated/air-gapped/privacy teams (data never leaves the repo); multi-vendor agent shops switching between Claude/Cursor/Codex (git = neutral ground); anyone wanting auditable spec↔code↔commit provenance forever; the Spec-Kit-curious who want *structure + a query graph* on files-in-git.

**Loses:** large orgs wanting hosted system-of-record + dashboards/SSO/RBAC/live collab (Linear/Jira); real-time multi-agent hot-loop coordination (SQLite/bus — AIDA doesn't use pure git here either); pure conversational recall (mem0/Letta/Zep); buyers unwilling to absorb the orphan-branch / cache-rebuild / two-leg-sync conceptual + ops surface (a real adoption tax).

## Follow-ups (dispatched / to dispatch)

1. **Spec Kit depth dive (HIGH — linchpin).** Does Spec Kit have *any* cross-spec relationships, stable IDs, or trace-to-code roadmap? AIDA's differentiation rests on it lacking the graph/ID/trace layer. Verify before positioning on it.
2. **Cognee / memory-lib git-export watch** — any move toward git-sync would further erode the (already narrow) "they avoid git" claim.
3. **`AGENTS.md` convergence trajectory** — if vendors standardize a richer structured `AGENTS.md`, does it commoditize part of AIDA's prose layer?
4. **Linear/Jira repo-native moves** — any SaaS PM shift toward git-backed specs changes the map.
5. **Internal honesty audit** — sweep positioning docs (`docs/positioning/`, I3 framing) to remove "git vs DB purity" language; replace with the git-canonical + cache + structure + multi-vendor line above.

**Bottom line:** Reframe the wedge from *"competitors avoid git, we ride it"* to **"the ecosystem is converging on files-in-git as the neutral substrate — AIDA is the only one putting a stable-ID, typed-relationship, trace-enforced, MCP-queryable knowledge graph on top of it, which is also what makes it genuinely multi-vendor while app-local memory stays vendor-locked."**
