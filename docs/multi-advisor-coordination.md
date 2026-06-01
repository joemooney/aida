# Multi-advisor coordination

*Strategic doc. 2026-05-20. Output of SPIKE-10.*

> One advisor per project, project-scoped substrate, parent-only one-way propagation — these are AIDA's current assumptions. They hold while a project has one developer and one strategic context. They break when (a) the project's pack grows past ~100 memories and cross-subsystem noise dilutes per-session focus, or (b) a sibling project is initiated from a parent and the parent's strategic context doesn't transfer with `aida init`. This doc captures the four gaps, the mechanisms considered, and the decisions on which sub-problems become STORYs.

## Verdict

**Two implementation tracks, two STORYs each, sequenced by current need.**

- **Track A — focus/scoping** (more immediately useful as the pack grows past ~30 memories): STORY-A1 (subsystem-tagged memories + `--focus` loading), STORY-A2 (`aida advisor handoff` verb).
- **Track B — ongoing relationship** (less time-critical until a second project exists): STORY-B1 (`aida advisor mentor` pattern), STORY-B2 (bidirectional substrate promotion).

Track A files immediately as approved. Track B files as approved but lower priority — file now to lock the design, implement when a second project genuinely exists. Track A also files a small follow-up SPIKE on retroactive tagging of the existing pack.

## Near-term: the functional two-advisor split — logical / physical (2026-05-31)

Before the subsystem-advisor model below (SPIKE-10's Track A/B) is needed, a solo
operator can already run **two advisor seats split by workflow stage** — a
different, simpler axis than subsystem scoping, and the fastest near-term way to
move faster. The cleanest mental model is **database design's logical/physical
split**:

| Seat | Layer | Owns | Posture |
|------|-------|------|---------|
| **`product` advisor** | **Logical design** — aspirational, implementation-independent | The **WHAT** + why: shapes desired functionality with the operator, captures it as well-formed backlog (acceptance + rationale), **proposes** into the queue | Subordinate / proposer — does not drive execution or redefine buildability |
| **`advisor` (master)** | **Physical design** — where the rubber meets the road | The **HOW** + buildability: drives the agreed queue, oversees drains/agents, triages, promotes; holds architecture coherence | **Master** — holds the veto + arbitrates; the single master from the principle above |

This does not violate "one master advisor": the `advisor` seat *is* that single
master (product owner, coherence holder). The split decouples **logical intake**
from **physical execution** so they run in parallel — the win is gated entirely
on **handoff quality** (specs complete enough that execution drives them without
round-tripping the operator).

**The veto is reality-grounded, not rank-grounded.** The physical layer
(`advisor`) overrides the logical (`product`) not because it's senior but because
**physical reality constrains the aspiration**. A legitimate veto therefore
always carries a **constraint reason** ("can't be realized as specified because
X"); a veto with no physical reason is just the bottleneck wearing a hat. The
operator is the **DBA above both layers**, arbitrating only the genuine
logical-vs-physical conflicts the master can't reconcile.

### Coordination protocol — a logical↔physical consistency invariant

Keep the physical (what's being built) faithful to the logical (what was
intended); drift is a design *bug* to reconcile, and physical reality feeds
*back* to reshape the logical (bidirectional + iterative, unlike a one-way DB
compile).

1. **Sync is continuous via the substrate — there is no deferred "big sync."**
   Both seats read the same specs / queue / findings / comments / shared
   `~/.claude` memories. The physical seat *records* every non-trivial decision
   (a decision-comment, a finding); the logical seat stays converged by reading.
   A deferred sync event is the trap where divergence hides.
2. **The physical seat decides *reversible* architecture forks and proceeds** —
   recording the call and filing an **arch-flag finding** (`aida findings add`,
   `kind:arch-flag` + `for:product`) as a *priority signal*: "physical hit a
   constraint that diverges from the logical — review promptly."
3. **Irreversible / high-blast-radius forks → BLOCK** and gate on the logical
   seat first (sync-before-decide), punt-style. The seam is **reversibility**,
   not "ask vs don't" — the Type A/B/C calibration.
4. **The arch-class bar** that triggers a flag/block = the master-sign-off list
   (file formats, tool contracts, conventions, orchestrator behavior, EPIC-shaped
   work). Routine implementation choices stay autonomous — flag *everything* and
   you've rebuilt the bottleneck with extra steps.
5. **The empowerment is only as safe as the logical seat's review cadence.**
   `product` clears `kind:arch-flag` findings at the **start of each session**.
   If that cadence lapses, continuous-sync silently degrades to "product fell
   behind and divergence accumulated" — the return-path mirror of forward-path
   handoff quality.

This is the same pre-PR-flag / sketch-first discipline used for sibling agents
flagging architecture to the master — applied advisor-to-advisor. It's proven,
not new. The subsystem-advisor tracks below remain the *later* evolution.

### What actually coordinates through the substrate — and what does not (verified 2026-05-31)

"Coordinate through the substrate" is only as strong as what *is* substrate.
Verified against the storage layer:

| Surface | Storage | Shared across clones/machines? |
|---|---|---|
| Specs, comments, history, relationships, tags, **status** | orphan `aida-store` branch (`objects/…yaml`) | **Yes** — git-canonical |
| **Findings** | *derived* from spec status/tags (a view, not a file) | **Yes** |
| **Default queue** | `.aida-store/registry/queues/<user_id>.yaml`, **committed to the orphan branch**, sharded per user | **Yes**, after `aida db sync` — per-user shard |
| `--global` queue | `~/.aida/queue/<role>.yaml` (HOME) | **No** — machine-local |
| **Leases** | `.aida/sessions/<id>.toml` (gitignored) | **No** — machine-local |
| Punt/calibration ledger, drain-state, logs | `.aida/*` (gitignored) | **No** |

Two consequences the protocol must respect:

1. **The default queue *is* shared, but sharded by `user_id`.** Distinct users
   never collide (separate `<user>.yaml` files, clean rebase, mutually visible
   after sync). The hazard is **two machines sharing one `user_id`** — most
   easily the `default` fallback (BUG-89) when `$USER`/`$AIDA_USER` are unset
   (CI, containers) — which makes both write the *same* file and conflict at
   sync. Distinct `user_id` per machine is a hard requirement for shared-branch
   collaboration. (Tracked: TASK-618.)
2. **Leases are machine-local**, so the "is someone already working on this?"
   guard is single-machine. Across machines the only shared "taken" signal is
   the spec's **status flip to in-progress** — git-canonical but
   *eventually-consistent*: a second person can pick up the same spec in the
   window before they pull the first's flip. Cross-machine coordination must
   therefore ride **spec status (pull-before-pickup)**, not leases. (Tracked:
   TASK-619.)

So for two advisors on **one machine, one user** (the common solo case) the
queue is a fully shared coordination surface. For **distinct machines/users**,
coordinate on spec status + tags + findings + comments, treat the per-user
queue as advisory, and pull before picking up.

## Gaps + mechanisms

### Gap 1 — Subsystem-advisor within a monolithic repo

**Problem.** Every advisor session today loads the entire memory pack. At 30 memories that's fine; at 200+ it's noise that dilutes attention. An orchestrator-focused session shouldn't load TUI memories; a CLI-focused session shouldn't load MCP-server memories. The need will hit before the pack reaches 100 — the texture of "this memory feels off-topic for what I'm doing" already appears occasionally at 30.

**Mechanisms considered.**

- **A. Subsystem tag in frontmatter** — `subsystem: orchestrator` (or omitted for universal memories). Advisor sessions take a `--focus orchestrator` flag and load only universal + scope-matching memories. Backward-compatible: untagged memories are universal by default; today's pack stays valid until each memory is retroactively classified. Lowest-risk; ships in a week.
- **B. Hierarchical memory dirs** — `~/.claude/projects/<slug>/memory/subsystems/orchestrator/*.md`. Physical separation. Cleaner long-term but invasive (changes MEMORY.md index conventions, breaks existing tooling).
- **C. Relevance-scored loading** — memories scored against active context (current spec, recent commits). Top-N loaded. AI-mediated; non-deterministic; over-engineered for current scale.

**Recommendation: A.** Ship subsystem-tagging + `--focus`. Revisit B at ~100 memories. C is research, defer indefinitely.

**Subsystem identification.** Manual tagging per memory. Universal discipline (advocate-not-passive, capture-over-concentration, etc.) stays untagged. AIDA-specific subsystems initially: `orchestrator`, `cli`, `tui`, `mcp-server`, `web-dashboard`. Per-project; not a global registry.

**Memory can carry multiple subsystem tags** (e.g., `subsystem: orchestrator, mcp-server` when a memory is relevant to both). Loading filter is OR-match.

### Gap 2 — Sibling-advisor initiation as a verb

**Problem.** `aida init --with-memories` propagates the generic discipline pack (scaffolding-pack memories). It doesn't propagate the parent's *strategic* context — the project-specific decisions, scope-relevant substrate, explicit latitude (what the sibling decides vs what's already decided), or the vision in broader context. When a parent AIDA initiates a sibling project, the sibling boots without the parent's accumulated thinking on what the sibling is *for*.

**Existing partial mechanism.** Manual handoff briefs (`aida-tutor/docs/2026-05-18-advisor-handoff.md`). Effective when the parent writes them; ad-hoc, not a verb, easy to skip.

**Mechanism.** `aida advisor handoff --to <project-path> --focus <topic>` generates a checked-in brief at `<project-path>/docs/<date>-advisor-handoff.md` with five sections:

1. **Parent identity** — parent project name, parent advisor's lineage, generation date.
2. **Vision in broader context** — one paragraph framing why the sibling exists in the parent's worldview. (Operator-written.)
3. **Decided things** — bulleted commitments the sibling should NOT relitigate. Defaults from parent's project: spec-graph architecture, punt → advisor → escalate pattern, file-handshake conventions. (Auto-populated from parent's substrate + operator-pruned.)
4. **Scope-relevant substrate slice** — memories filtered by `--focus <topic>` from the parent's pack, plus universal discipline. (Auto-assembled.)
5. **Explicit latitude** — what the sibling has freedom over vs parent's decided territory. (Operator-written; default text: "you decide implementation strategy; parent reserves architectural patterns.")

**Design points.**

- The brief is checked-in markdown the sibling reads at boot. Proven pattern.
- Generated semi-automatically: AIDA assembles sections 1, 3, 4 from substrate; operator fills 2 + 5.
- Versioned: `aida advisor handoff --update` regenerates against current parent state, surfacing what's changed since the previous brief for operator review.
- Pairs with `--focus`: a handoff brief is by definition scope-focused; the focus tag drives the substrate slice.

### Gap 3 — Mentor relationship over time

**Problem.** Initiation is one-shot. Real mentoring is ongoing. The parent's strategic context evolves; the sibling discovers things the parent should know; substrate audits should cross-pollinate. Without an ongoing mechanism, the relationship degrades to the static handoff brief.

**Mechanism.** `aida advisor mentor --child <project-path>` runs a parent-side mentor session:

1. **Read sibling's recent activity** — last N specs, last N memory writes, last N PR merges. Parameterizable window (default: since last mentor session, or 2 weeks).
2. **Run pack hygiene audit** on sibling's pack (per `feedback_memory_pack_hygiene`). Surface OBE candidates, coverage holes, fragmentation.
3. **Surface lessons the sibling missed** — specs the sibling shipped that contradict parent's recorded principles; opportunities the sibling didn't see.
4. **Identify candidate-for-parent memories** — sibling memories tagged for upward promotion (see Gap 4).
5. **Write mentor brief** at `<child>/docs/<date>-mentor-brief.md` capturing the above.
6. **Optionally update parent substrate** — if sibling's discoveries apply more broadly, parent advisor writes them into the parent pack.

**Cadence.**

- **Manual-trigger initially** — parent runs `aida advisor mentor` when context warrants. Lowest-friction; lets the parent decide based on signal.
- **Calendar cadence later** — weekly review, monthly deep-audit. File once we have evidence of when manual-trigger gets skipped.
- **Event-triggered later** — every time sibling ships a major thing. File once we have a "major" signal (keystone-ship marker).

**Recommendation:** ship manual-trigger only. Calendar + event triggers are speculative until we have a second project to validate against.

### Gap 4 — Bidirectional substrate propagation

**Problem.** Scaffolding flows parent → child via `propagation: scaffolding-pack`. When a child advisor discovers universally-applicable discipline, there's no upward path. Discipline gets stuck.

**Mechanism.** Bottom-up proposal flow with parent governance:

1. **Sibling advisor flags candidate memories** — frontmatter `propagation: candidate-for-parent` (in addition to or instead of `propagation: scaffolding-pack`). Marks "I believe this applies to the parent's worldview too."
2. **`aida advisor candidates --child <name>`** lists pending candidates on the parent side. Surface during mentor sessions.
3. **Parent decides per-candidate:**
   - **Promote** — add to parent pack with appropriate tags (often `propagation: scaffolding-pack` so it flows to other children too).
   - **Decline** — leave in child only; record decline reason as a comment.
   - **Refine** — rewrite for parent context, then promote. The original child memory may or may not be retired.
4. **Audit trail** — promotion / decline tracked in `~/.aida/advisor-candidates.toml` or per-memory metadata.

**Governance.** Parent decides. Child cannot write directly to parent pack. This is the right asymmetry: a child advisor without the parent's broader context shouldn't unilaterally affect the parent's worldview. The proposal flow is bottom-up; the decision is top-down.

**Risk.** Dormant candidates. If parent rarely runs mentor sessions, candidates sit unreviewed. Mitigation: `aida advisor candidates --age` surfaces candidates older than threshold; parent has visibility on the backlog.

## Composition + sequencing

The four gaps interact:

- **Gap 1 + Gap 2** share the **scoping concept**: subsystem tagging powers both `--focus` (single-project) and `--focus <topic>` on handoff (cross-project). Implement as Track A.
- **Gap 3 + Gap 4** share the **ongoing relationship**: mentor sessions are where candidate-promotion happens. Implement as Track B.

**Track A is more immediately useful** because the project's pack grows monotonically and noise will hit before a second project exists. Track A's value compounds with every memory write.

**Track B is more strategically important long-term** but premature without a sibling. File the design now; implement when a second project genuinely exists.

## STORYs to file

### Track A (file now, implement next quarter)

**STORY-362: Subsystem-tagged memory pack + `--focus` loading**
- Memory frontmatter `subsystem:` tag (single value or comma-separated list).
- Advisor session honors `--focus <subsystem>` flag.
- Loading filter: universal memories (untagged) + scope-matching memories.
- TUI / status line surfaces active focus.
- Backward-compatible: existing pack works unchanged (untagged = universal).
- Retroactive-tag tooling: `aida memory audit --tag` walks the pack and prompts per-memory.

**STORY-363: `aida advisor handoff` CLI command**
- `aida advisor handoff --to <project-path> --focus <topic>` generates a brief.
- Template under `aida-core/templates/advisor-handoff.md` (embedded via build.rs).
- Sections: parent identity (auto), vision (operator), decided things (auto-suggested + operator-pruned), substrate slice (auto-filtered by --focus), latitude (operator).
- `--update` regenerates against current parent state, marks diffs.
- Document the workflow in `docs/multi-advisor-coordination.md` (this file) and `docs/aida/discipline/`.

### Track B (file now, lower priority — implement when second project exists)

**STORY-364: `aida advisor mentor --child <project-path>`**
- Reads sibling's recent activity (specs / memories / PRs).
- Runs pack hygiene audit.
- Writes mentor brief at `<child>/docs/<date>-mentor-brief.md`.
- Manual-trigger only initially.

**STORY-365: Bidirectional substrate propagation — `propagation: candidate-for-parent`**
- Memory frontmatter tag.
- `aida advisor candidates --child <name>` lister.
- `aida advisor promote --candidate <id> --tag <target-tag>` action.
- `aida advisor decline --candidate <id> --reason <text>` action.
- Audit-trail in `~/.aida/advisor-candidates.toml`.

### Follow-up SPIKE

**SPIKE-12: Pack-scale hygiene threshold**
- At what memory-pack size does subsystem-scoping become mandatory (vs nice-to-have)?
- Empirical: synthetic packs of 50, 100, 200, 500 memories — observe advisor-load latency, response quality, cross-topic noise.
- Verdict feeds whether STORY-362's `--focus` defaults stay opt-in or become opt-out at scale.

## Open questions flagged

1. **Subsystem registry per project** — should `aida init` scaffold a list of subsystems, or do they emerge from memory tags organically? Default: organic; project's `CLAUDE.md` documents the active set.
2. **Memory-pack indexing under `--focus`** — `MEMORY.md`'s flat index list works at 30 memories; at 200 with multiple focus modes, does the index need restructuring? Punt to SPIKE-12.
3. **Latitude semantics** — "explicit latitude" in handoff brief: is it free-form prose, or a structured allow/deny list? Default: free-form for v1; structured if friction emerges.
4. **Candidate age-out** — do unreviewed candidates auto-expire after N mentor sessions? Default: no auto-expire; surface as backlog with age. Operator decides.
5. **Cross-project memory references** — should a sibling memory be able to link `[[parent-memory-name]]` to a parent memory it conceptually inherits from? Worth supporting; design TBD.

## Revisit triggers

- **Pack reaches 100 memories** — re-evaluate whether Gap 1 mechanism A still suffices or whether Gap 1 mechanism B (hierarchical dirs) is needed.
- **A sibling AIDA project is initiated** — implement Track A's STORY-363 (handoff verb) and start using it. Validate Track B mechanisms against real usage.
- **Two sessions of context-switch dissonance** — when an advisor session feels diluted by cross-subsystem noise twice in close succession, that's the signal to ship STORY-362 immediately.
- **First child memory the user wants to promote up** — file STORY-365 if not already implemented.

## Related

- **EPIC-1** — agent collaboration; substrate of which is the memory + discipline pack
- **EPIC-26** — TUI is the product; TUI is where focus + advisor session would manifest
- `feedback_propagate_generic_discipline_via_scaffolding` — existing parent → child one-way propagation; this doc explores the bidirectional + subsystem variants
- `feedback_memory_pack_hygiene` — audit cycle; mentor sessions apply the same questions cross-advisor
- `feedback_dialog_role_responsibilities` — advisor role definition; multi-advisor extends to N advisors
- `feedback_advocate_not_be_passive` — the advocacy directive; multi-advisor needs each advisor to maintain it independently
- **SPIKE-9** (sibling SPIKE, completed today) — MCP-as-bus; provides cross-agent coordination transport that complements multi-advisor coordination (especially for cross-machine / cross-agent scenarios)
- **SPIKE-11** (sibling SPIKE, completed today) — fork-from-live advisor; orthogonal direction (one-advisor-richer-context vs N-advisors-coordinating)
- **2026-05-18 user observation** captured in the SPIKE-10 brief: *"a project may want to remain a monolithic repo. We will need a way for an advisor to mentor/train/initiate an advisor for some subsystem or other sibling project."*
