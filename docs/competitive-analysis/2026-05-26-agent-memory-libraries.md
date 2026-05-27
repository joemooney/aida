# Agent Memory Libraries: Anatomy, Critique, and Where AIDA Fits

**Date**: 2026-05-26
**Source article**: ["agent memory: an anatomy"](https://example.com) — agent memory taxonomy + production-library critique, mid-2026
**Refresh trigger**: new agent memory library lands (n+1 of: Mem0/LangMem/Graphiti/Cognee/Letta/Zep/Anthropic Dreams), Anthropic publishes memory product update, AIDA ships a prospective/consolidation primitive that warrants re-evaluation
**Status**: Living analysis — refresh on triggers above

---

## TL;DR

The article cuts agent memory into a sharp three-part anatomy — **extractor**, **store**, **retriever** — and uses it to show that most "agent memory" libraries on the market are actually narrower than the word suggests: they're **autobiographical-semantic** memory systems with metadata tags for variety, dressed in cognitive-science vocabulary they don't structurally honor.

AIDA fits the framework but isn't reducible to "another memory library". Where Mem0/LangMem/Graphiti/Cognee hold autobiographical facts about a user in a vector store, **AIDA holds five distinct memory kinds across multiple substrates, with git as the single source of truth**. The article's hardest store question — *adjudication of contradictions* — is largely sidestepped by AIDA's substrate-as-bouncer design: status transitions are explicit, archive ≠ delete, and the git history of the orphan `aida-store` branch answers *"what did I believe last month?"* for free.

AIDA's **biggest gap** relative to the libraries reviewed is **offline consolidation** (Dreams / Letta-style scheduled reflection over accumulated material). The advisor curates memory manually today; a scheduled `aida reflect` pass would close the gap.

AIDA's **biggest unclaimed strength** is **prospective memory** — the article explicitly says *"no production library I've seen ships this"*; AIDA ships it via briefs + queue + lease + auto-filed followups + punt→advisor→resume.

---

## The article's framework

### Three anatomical parts

| Part | What it does | Key design choices |
|---|---|---|
| **Extractor** | Reads conversation, emits short abstracted facts (statements). | Eager-per-message vs lazy-end-of-session; what gets compressed away (coreference, temporal anchors, situated context). |
| **Store** | Holds the statements. Vector / relational / graph / hybrid. | **Adjudication**: when new statement contradicts old, overwrite / append / mark-superseded? *"A store that can't answer 'what did I believe last month' isn't a memory system. It's a snapshot with a timestamp on it."* |
| **Retriever** | Turns query into search, returns relevant statements. | Vector baseline + keyword + reranker. Time filter (skip stale). Presupposition check (block question that assumes stale fact). |

### Four kinds of memory (after Tulving 1972 + Tulving 1985 + Baddeley & Hitch 1974)

| Kind | Cognitive definition | Production-library reality |
|---|---|---|
| **Episodic** | Specific events tied to time + place ("I had coffee with X last Tuesday"). | Compressed away at extraction time — situated event becomes decontextualized fact. |
| **Semantic** | Facts about the world ("Berlin is in Germany"). | **What most libraries actually hold.** Narrower still: autobiographical-semantic — facts about the user. |
| **Procedural** | Knowing how to do things ("how to ride a bike"). Behavioral disposition, not retrievable as a fact. | Mostly mislabeled — same vector store + a `memory_type=procedural` tag. **Only LangMem** structurally honors it (evolving system prompt from scored trajectories). |
| **Prospective** | Remembering to do something in the future ("when X next happens, do Y"). | **No production library reviewed ships this.** Open territory. |
| _(Working)_ | Context window. Different machine. Out of scope for the article. | — |

### What the libraries actually are

> *"Most agent memory libraries are autobiographical memory systems with extra steps. The field's central problem is narrower than 'memory' — and clearer when you name it."*

---

## Library-by-library — verified against source

Verified May 2026 by reading each project's repo or docs directly. Quoted text is from the source where indicated.

### LangMem (github.com/langchain-ai/langmem)

**Shape**: Unified vector store via `create_manage_memory_tool()` + `create_search_memory_tool()`. *Plus* a separate prompt-optimizer mechanism for procedural memory.

**Per source**: *"Background memory manager that automatically extracts, consolidates, and updates agent knowledge."* But: the main memory tools use single embeddable store; the procedural-memory path is the prompt optimizer (no vector-store writes — evolves the system prompt from scored trajectories).

**Verdict**: The article's claim is verified — LangMem is **the only library reviewed that structurally separates procedural memory** (as behavioral disposition encoded in instructions, not retrievable fact).

### Mem0 (github.com/mem0ai/mem0)

**Shape**: Multi-level memory (User / Session / Agent state). Single vector index. Procedural memory exposed as `metadata.memory_type = "procedural"` label.

**Per source**: *"Single-pass ADD-only extraction. One LLM call, no UPDATE/DELETE. Memories accumulate; nothing is overwritten."* Multi-signal retrieval (semantic + BM25 + entity matching, parallel-scored).

**Verdict**: Confirmed — accumulate-only, no consolidation, procedural memory is a tag not a separate mechanism. The "memories accumulate; nothing is overwritten" admission is also *the* adjudication critique the article makes: this can't answer *"what did I believe last month?"* without scrolling raw history.

### Graphiti / Zep (github.com/getzep/graphiti)

**Shape**: Bitemporal context graph. Everything is an entity (node) with evolving summary, fact (edge) with validity window, episode (provenance). No procedural/semantic/episodic distinction at the API level.

**Per source**: *"A context graph is a temporal graph of entities, relationships, and facts — like 'Kendra loves Adidas shoes (as of March 2026).' Unlike traditional knowledge graphs, each fact in a context graph has a validity window."* No mention of offline consolidation; *"Incremental graph construction: New data integrates immediately without batch recomputation."*

**Verdict**: Confirmed — unified graph, no kinds-of-memory exposure, no offline reflection. The validity window IS a serious answer to adjudication — closer in spirit to the article's preferred "mark superseded" choice than to either Mem0's overwrite/accumulate or LangMem's vector-similarity replacement.

### Cognee (github.com/topoteretes/cognee)

**Shape**: Knowledge graph + vector + session cache hybrid. Four primitives: `remember`, `recall`, `forget`, `improve`. Ontology grounding + multimodal ingestion (PDFs, Slack, Notion, images, audio).

**Per source**: Continuous improvement via the `improve` operation runs during the `remember` pipeline (not offline). Unified knowledge graph — no separate memory types at the API level.

**Verdict**: A different cut from the libraries above — *knowledge-graph-first RAG* more than agent-conversation-memory. The `forget` primitive is the only library reviewed that exposes deletion as a first-class verb. No offline consolidation.

### Letta — sleep-time compute (letta.com/blog/sleep-time-compute)

**Shape**: **Two-agent architecture.** Primary conversational agent has NO memory-editing tools — only the sleep-time agent can modify memory blocks. Memory rewrites happen asynchronously; primary agent reads from the (continually-updated) memory blocks during conversation without blocking.

**Per source**: *"Memory management to happen asynchronously. Memory formation in MemGPT is incremental, so memories may become messy and disorganized over time. Sleep-time agents...can continuously improve their learned context."*

**Verdict**: Real offline consolidation, but configured-frequency rather than truly idle-triggered. The architectural commitment — primary agent CAN'T edit memory — is novel and worth noting: it forces all consolidation through the asynchronous channel rather than letting synchronous writes interleave with reflection.

### Anthropic Dreams (platform.claude.com/docs/en/managed-agents/dreams)

**Shape**: **Async pipeline.** Input: existing memory store + 1-100 session transcripts. Output: NEW memory store (input never modified). Operator-initiated, claude-opus-4-7 or sonnet-4-6, custom `instructions` field.

**Per source**: *"Dreams let Claude clean that up. A dream reads an existing memory store alongside past session transcripts, then produces a new, reorganized memory store: duplicates merged, stale or contradicted entries replaced with the latest value, and new insights surfaced. The input store is never modified, so you can review the output and discard it if you don't like the result."*

**Verdict**: This is **the article's preferred offline-consolidation pattern in production**. Three properties worth claiming as the design floor:

1. **Non-destructive** — input store immutable, output is a NEW store.
2. **Reviewable** — operator can compare before adopting.
3. **Instructable** — `instructions` field steers what gets curated (*"Focus on coding-style preferences; ignore one-off debugging notes"*).

### Sebastian Lund — "Ultimate Guide to LLM Memory" (fastpaca.com)

**Shape**: Four layers cut by *prompt composition* (what fills the context window): **working** / **episodic** / **semantic** / **document**.

**Key claims**:
- Reject database mental model; memory IS constrained prompt assembly.
- **Positional weighting**: stable context early, dynamic context late.
- **Budget allocation**: per-layer token ceilings.
- **Layered failure modes**: semantic/episodic failures cause friction; working/document failures demand explicit errors over hallucination.
- "Start minimal" — add layers as capability needs justify them.

**Verdict**: A *different axis* from the article's framework — orthogonal and composable. The article cuts by *kinds of memory before any engineering*; Lund cuts by *what fills the prompt at runtime*. Together they describe the system from both ends.

---

## AIDA against the framework

AIDA isn't on either list of "memory libraries" because AIDA isn't framed as a memory library — but applied to the framework, AIDA holds five distinct memory kinds across multiple substrates.

### AIDA's five memory kinds

| Kind | AIDA substrate | Mechanism |
|---|---|---|
| **Semantic-project** | The spec graph — YAML files under orphan `aida-store` branch + SQLite cache + FTS5 index. | `aida add`, `aida edit`, `aida list`, `aida search`, `aida show`. Stable SPEC-IDs. |
| **Procedural** | Discipline pack (`docs/aida/discipline/*.md`) + CLAUDE.md + AGENTS.md + skills (`.claude/skills/`) + hooks (`.claude/hooks/`). | Context injection at session-start, not retrieval-as-fact. Same mechanism as LangMem's prompt-optimizer — *behavioral disposition encoded in instructions* — but scaffolded into every project. |
| **Episodic** | Git history of `aida-store` branch + `aida history --events` ledger + punt ledger (STORY-325) + calibration substrate (STORY-347) + usage telemetry (`~/.aida/usage.jsonl`). | Each transition / decision / observation is a timestamped record. Git is the time-travel substrate. |
| **Prospective** | Briefs (`aida brief <agent> <spec>`) + queue + lease + auto-filed followups at Done (TASK-96) + punt→advisor→resume handshake (STORY-306). | *"When agent X next picks up condition Y, here's what to know."* The article's specific gap. |
| **Autobiographical-user** | `~/.claude/projects/<slug>/memory/` — `feedback_*.md`, `user_*.md`, `project_*.md`, `reference_*.md` files with frontmatter. | What the article describes — facts about the user held on their behalf. |

### The three anatomical parts in AIDA

**Extractor** — *Deliberate, not LLM-driven.* Three pathways:

1. **Human/agent explicit filing**: `aida add` for specs, `aida findings add` for observations, manual trace-comment writes, hand-written memory-pack files.
2. **Marker parsing on auto-bump**: the scanner reads `(SPEC-ID)` trailers in commit subjects; the `aida show` Git-linkage section harvests `trace:SPEC-ID` from code. No LLM extraction needed.
3. **Lazy end-of-session safety net**: `/aida-capture` skill sweeps the chat for missed requirements.

This is structurally different from Mem0's "automatic transcription on every message" — AIDA's extraction is **operator/agent-deliberate**, more friction at capture time, fewer false positives, no compression of situated context (the commit preserves *when/who/what code*; the spec preserves *the abstracted fact*; the trace comment binds them).

**Store** — Git-canonical hybrid. One YAML per spec under orphan `aida-store` branch (substrate-of-truth) + SQLite cache (relational + FTS5 + graph) for read speed. No vector embeddings today — keyword + relational + graph instead. The cache rebuilds when the orphan branch's HEAD SHA changes; the cache cannot diverge from the substrate.

**Adjudication** — AIDA's answer to *"how do I handle contradictions?"* is **mark superseded, never overwrite**:
- Spec status transitions explicit (`Approved → In Progress → Done → Completed → Released`).
- Archive flag orthogonal to status (`aida archive <ID>` — STORY-441 explicit: *archive ≠ deletion*; YAML, audit trail, requirement graph all survive).
- Memory-pack files: when wrong, retire-with-documented-reason, not silent edit.
- Git history of the orphan branch answers *"what did I believe last month?"* directly.

This is closer to Graphiti's validity-window pattern than to Mem0's accumulate-only model.

**Retriever** — `aida search` (FTS5 keyword), `aida list` (relational filters), graph traversal (parent/child/subsumes/depends-on), MCP tools (`list_requirements`, `show_requirement`, `search_requirements`) exposing the same retrievers to coding agents.

---

## Where AIDA correctly anticipates the article's critiques

| Article critique | AIDA's design choice |
|---|---|
| *"A store that can't answer 'what did I believe last month' isn't a memory system."* | Git history of the orphan branch is the answer — query any past state of the substrate by SHA. |
| Forgetting-as-mistake (biological constraint, not feature) | `aida archive` preserves audit trail. `feedback_memory_pack_hygiene.md` codifies the audit cycle: three outcomes (retire / revise / preserve), preserve is default. |
| Procedural-as-label vs procedural-for-real | Discipline pack + CLAUDE.md + skills genuinely change behavior via context injection at session start — the LangMem mechanism, scaffolded by `aida init`. Not a vector-store label. |
| Emotional salience: structural absence in text-only agents | No fake affect. Substrate-side salience instead: priority (high/medium/low), severity (major/minor/cosmetic), status (NeedsAttention, In Progress, Done) — explicit attention as data, agent-judged at filing time. |
| Consolidation via promotion path | Findings (kind:observation) with recurrence ≥ 3 promote to substrate-actionable spec (STORY-467). This is genuine episodic→semantic compression with operator judgment in the loop — small Dreams. |
| Adjudication: overwrite / append / mark-superseded | AIDA chose **mark-superseded** uniformly (status transitions, archive flag, retire-with-reason). The article's preferred choice. |

---

## Where AIDA could learn — concrete, scoped opportunities

### 1. Offline consolidation (Dreams / Letta pattern)

**Gap**: AIDA has no scheduled pass over accumulated material. The advisor curates memory manually today (per `feedback_dialog_role_responsibilities`: "curate memory" is one of six advisor responsibilities).

**Proposed**: `aida reflect` — operator-initiated batch pass that:
- Sweeps findings for recurrence-pattern candidates (auto-promote candidates whose count crossed threshold).
- Sweeps memory pack for OBE candidates (per `feedback_memory_pack_hygiene` triggers).
- Sweeps specs for dead Approved-90d branches.
- Sweeps trace comments for orphans (SPEC-ID archived or deleted).
- **Non-destructive**: produces a *proposed reorganization manifest*, not direct edits. Operator reviews + accepts/discards (the Dreams non-destructive pattern).

**Worth a SPIKE** to validate cost + value before committing to a full feature.

### 2. Time-travel queries (substrate exposure)

**Gap**: The orphan branch *has* the history; the CLI doesn't surface it.

**Proposed**: `aida show SPEC-1 --at 2026-04-01` — answer *"what did this spec look like a month ago?"* directly from the git history of the orphan branch. Cheap, single-verb. Closes the article's "snapshot with a timestamp" critique fully.

### 3. Findings auto-decay → mark-superseded instead

**Gap**: Findings auto-decay after 30 days no recurrence. That's a small forgetting rule that conflicts with the rest of AIDA's "mark superseded, never delete" stance.

**Proposed**: keep the finding; mark it `decayed` after 30 days; expose `aida findings list --include-decayed` for forensic queries. Cheap behavioral change; preserves the audit trail.

### 4. Vector retrieval as an opt-in layer

**Gap**: Keyword (FTS5) + relational + graph retrieval cover most cases, but *"the finding about session leases from a couple weeks ago"* is exactly the case keyword retrieval misses.

**Proposed**: optional embeddings layer (`aida cache embed --model <model>`) that augments keyword/graph search with similarity rerank. Substrate-of-truth stays YAML — embeddings are derived projections, rebuildable. No commitment to a particular vendor.

### 5. Vocabulary import for `glossary.yaml` and discipline pack

**Gap**: AIDA documents memory surfaces ad-hoc. Readers landing on AIDA from one of the memory libraries don't have the shared vocabulary.

**Proposed**: extend `aida-core/templates/docs/aida/discipline/glossary.yaml` with the article's taxonomy — `kind-episodic`, `kind-semantic`, `kind-procedural`, `kind-prospective`, `kind-autobiographical-user` — each pointing at the AIDA substrate that holds it. Cheap, alignment with cognitive-science vocabulary, sharpens the framing for users.

---

## AIDA's structural differentiators (worth claiming)

Three properties from this framework that **AIDA has and the libraries don't**:

1. **Git-canonical substrate** — Mem0/LangMem/Graphiti/Cognee all use a database as source-of-truth; AIDA uses the git orphan branch. Free time-travel queries, free audit trail, free distributed replication, free conflict resolution via merge. The database (SQLite cache) is a derived projection.

2. **Prospective memory primitives** — *"no production library I've seen ships this"*. AIDA does — briefs are *when-X-next-happens triggers*, queue + lease + role routing solve *which-agent-acts-on-this*, auto-filed followups on Done (TASK-96) propagate *what-needs-to-happen-after-this*, punt→advisor→resume (STORY-306) closes *what-do-we-do-when-the-implementer-gets-stuck*.

3. **Stable IDs** — vector stores key by embedding; AIDA keys by `SPEC-ID`. Refactoring is safe (rename the function, the `trace:SPEC-1` stays bound). Cross-agent handoffs are safe (Codex and Claude see the same TASK-X). Releases are safe (`(SPEC-ID)` trailer scans years-old commits without re-embedding).

---

## Where AIDA's design differs philosophically

The article's libraries all assume **automatic memory extraction during conversation** is the right ergonomic — friction-free capture, in exchange for some loss of fidelity at the extractor.

AIDA's design assumes **deliberate filing with substrate-as-bouncer** — more friction at capture time (you have to run `aida add` / `aida findings add` explicitly), in exchange for:
- No false-positive memories cluttering the substrate.
- Operator/agent judgment at the moment of filing (severity, priority, type, relationships).
- No "memory drift" from over-eager LLM extraction.
- Clean separation between observation (episodic-tier, in `aida findings`) and fact (semantic-tier, in spec graph).

This is the Karpathy-style *"structured markdown queryable by Claude"* baseline + the AIDA discipline layer on top.

The friction has a cost — first-user "where do I file this thought?" UX is harder than Mem0's "the agent does it for you". The win is that what's filed is **load-bearing**, not noise.

---

## Refresh signals

Trigger a refresh of this analysis when:

- A new agent memory library lands (current set: Mem0 / LangMem / Graphiti / Cognee / Letta / Zep / Anthropic Dreams). Especially: any library that ships prospective memory as a first-class primitive.
- Anthropic Dreams graduates from research preview to GA (currently `dreaming-2026-04-21` beta header required).
- Letta's sleep-time compute pattern gets adopted by another library — that's the "consolidation as architectural commitment" pattern spreading.
- AIDA ships `aida reflect` or equivalent — then this doc shifts from "AIDA has a gap" to "AIDA's reflect command + Dreams + Letta = the consolidation pattern".
- Cognitive-science vocabulary stops being purely cited and starts being structurally honored across multiple libraries — that's when the field matures.

## Related

- [`positioning.md`](positioning.md) — AIDA's defensible-niche statement.
- [`vs-karpathy-md.md`](../positioning/vs-karpathy-md.md) — *"do you need a library at all?"* (this analysis is the next-question-up: if you accept you need persistent memory, what shape?).
- [`2026-05-16-market-snapshot.md`](2026-05-16-market-snapshot.md) — May 2026 landscape including agent coordination tools.
- `aida-core/templates/docs/aida/discipline/machinery-glossary.md` — AIDA's machinery vocabulary (orchestrator, phase, drain, lease, etc.).
- `aida-core/templates/docs/aida/discipline/glossary.yaml` — structured single-source vocabulary (44 terms, expanded with article's memory-kind taxonomy per opportunity #5 above).
