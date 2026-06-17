# Coordinating Multi-Vendor AI Agent Fleets

### A Theory of AIDA — design-science findings from building (and dogfooding) an intent-substrate for agentic software development

**Status:** Draft basis for a research paper · **Date:** 2026-06-16 · **Genre:** Design-science / autoethnographic systems research · **Spine:** Multi-vendor fleet coordination (the apex of a layered coordination stack)

> This is a working draft meant to be handed to collaborators and revised. Claims are tagged with an **evidence grade** (see §2) so reviewers can see exactly how much weight each carries. Nothing here is asserted as a controlled empirical result; the contribution is the artifact and the design principles extracted from building it.

---

## Abstract

As large language models commoditize code generation, the binding constraint on software production moves *off* the model and *onto* the coordination of many agents — across roles, across time, and, increasingly, across **vendors**. We report on AIDA, a system built to make a fleet of confident, forgetful, concurrent, multi-vendor coding agents collaborate against a shared, durable model of intent. AIDA is git-canonical: every requirement is a YAML object on an orphan branch, related agents read and write it through a typed graph and an MCP server, and code links back to it through inline trace comments. We frame AIDA's machinery as a **coordination stack** and advance the position that, as lower layers of this stack are absorbed by model vendors, durable value migrates upward — and that the topmost layer, *cross-vendor fleet coordination*, is structurally under-provided for incentive rather than capability reasons, and is therefore the defensible layer for an independent system or a large corporation to own. We extract eight design propositions, each embodied in a running mechanism and triangulated against eighteen months of market observation. We are explicit about the threats to validity of single-system, single-operator, alpha-stage design research, and we state what a multi-team study would have to measure to confirm or refute each proposition.

---

## 1. The shift: code is getting cheap, coordination is getting expensive

The first-order effect of capable coding agents is obvious — code gets written faster. The second-order effect is the interesting one: when any unit of code is cheap to (re)generate, the *code* stops being the scarce, durable artifact. What becomes scarce is an **addressable, queryable model of intent** that survives the churn — what was decided, why, what depends on what, which code satisfies which requirement, and what is still open. A human team carried that model implicitly, in heads and in review. A fleet of agents cannot: agents are **confident** (they assert), **forgetful** (each session is a fresh context), and **concurrent** (many act on the same artifacts at once). And the fleet is increasingly **multi-vendor** — a Claude implementer, a Codex sibling, a Gemini reviewer — none of which shares the others' memory, conventions, or governance.

AIDA is our attempt to build the missing substrate. This paper is not a tool description; it is an argument about *which layer of the agent-coordination problem holds durable value*, structured as a stack and grounded in what building AIDA taught us.

### The coordination stack

```
        ┌───────────────────────────────────────────────────────────┐
  L5    │  CROSS-VENDOR FLEET COORDINATION   (mailbox, roles, leases) │  ← apex / the bet
        ├───────────────────────────────────────────────────────────┤
  L4    │  AUTONOMY & ROLE ORCHESTRATION    (cascade, punt-continue)  │
        ├───────────────────────────────────────────────────────────┤
  L3    │  COLLABORATIVE STATE              (conflict-light git store) │
        ├───────────────────────────────────────────────────────────┤
  L2    │  ENFORCEMENT & GOVERNANCE         (substrate-as-bouncer)     │
        ├───────────────────────────────────────────────────────────┤
  L1    │  INTENT SUBSTRATE                 (graph, IDs, traces, MCP)  │
        ├───────────────────────────────────────────────────────────┤
  L0    │  MODEL / AGENT CAPABILITY         (vendor turf, commoditizing)│
        └───────────────────────────────────────────────────────────┘
```

**The central research question.** Value in this stack is migrating upward: as model vendors solve L0 and begin to absorb L1–L2 inside their own walled gardens (project memory, skills, sub-agents, internal task graphs), the durable, defensible value moves to the layers a single vendor is *disincentivized* to provide well — the cross-vendor layers L4–L5. For a large corporation deciding where to invest as the ground shifts, the open question is **how far up the stack value has already moved, and how fast.** We do not claim certainty about the answer; we claim the stack is the right way to ask, and that L5 is the structurally safest bet.

---

## 2. Method and evidence grades

This is **design-science research** (Hevner): the artifact is the primary contribution, and the knowledge claims are design principles extracted from building and operating it. It is also **autoethnographic** — AIDA is built *using* AIDA; the dogfood is the experiment, and the authors are the subjects. This is a legitimate genre (systems "experience reports," design science, reflective practice), but it has sharp limits, which we state up front rather than bury (§9).

Each proposition carries an **evidence grade**:

- **(M) Mechanistic** — argued from first principles about how LLM agents behave; the claim says *why* it must hold, independent of how often we have seen it.
- **(D) Dogfood** — observed repeatedly while building AIDA with AIDA; real but autoethnographic and uncontrolled.
- **(K) Market** — triangulated against external behavior (what competitors build, what users adopt); demand-side evidence, not internal.

A strong claim is one that holds on more than one grade. The strongest here — substrate-as-bouncer (P1), substrate-bounded autonomy (P3), conflict-light state (P4) — are **(M)+(D)**.

---

## 3. L1 — The intent substrate (P2)

**P2 (Intent as the durable layer) — grade (M)(D).** As code commoditizes, the bottleneck shifts from *generating* code to *maintaining an addressable model of intent*: stable identifiers, typed relationships, and bidirectional code↔spec traces. The model, not the code, is the thing worth version-controlling carefully.

**Mechanism in AIDA.** Every requirement is a YAML object (`objects/TYPE/000/SPEC-ID.yaml`) on the orphan `aida-store` branch, with a stable ID that never moves. Requirements carry typed relationships (`blocks`, `blocked-by`, `parent`, `depends-on`) that form a graph queryable with `aida graph <ID> --impact`. Code links back through inline `// trace:SPEC-ID` comments, making the satisfaction relation *machine-checkable from either direction*. An MCP server exposes the whole graph to any agent as tools and resources, so the model is not a document an agent must be told to read — it is an API the agent queries.

**Why it matters for a fleet.** A stable ID is the only thing a forgetful agent and a concurrent sibling can both name without ambiguity. The graph is what lets a cold-booting agent reconstruct *context* it never held. The trace comment is what lets a reviewer verify "done" against code rather than against a claim (a failure mode we hit repeatedly — agents mark work done that the diff does not contain).

**What would falsify it.** If, in practice, agents coordinate just as well over unstructured markdown + git history as over a typed graph, the typing is dead weight. Our dogfood evidence says they do not — but a multi-team study is required to separate "the graph helped" from "the discipline of writing things down helped."

---

## 4. L2 — Substrate as governance (P1, P5)

**P1 (Substrate-as-bouncer) — grade (M)(D). The sharpest claim in the paper.** *Natural-language instructions do not reliably constrain a capable LLM agent.* A rule in a prompt, a CLAUDE.md, or a memory is advisory: the agent is free, confident, and will route around it when its in-context reasoning disagrees. To **guarantee** an invariant against such an agent you must place a **programmatic gate** in the execution substrate — a bouncer that refuses the action, not a sign that asks politely.

**Mechanism in AIDA.** Status transitions are gated, not documented: `approved`/`planned` are advisor-only and the CLI *refuses* the transition from a non-advisor identity; `completed` is merge-driven and cannot be hand-set; a `(SPEC-ID)` commit trailer is what auto-completes a spec, so "done" is a property of git ancestry, not an assertion. The merge-gate assigns short IDs deterministically rather than trusting an agent to pick a free one. Each of these started life as a *rule* an agent ignored, and only became reliable once it became a gate.

**P5 (Calibration over accuracy) — grade (M)(D).** A self-correcting substrate (record a prediction now → compare against ground truth on landing → tune) outperforms a more-accurate-but-static one for agent-driven planning. A 60%-accurate estimator that learns beats an 80%-accurate one that never updates, because the fleet runs continuously and compounding correction dominates one-shot accuracy.

**Mechanism in AIDA.** Calibration mode emits two verdicts per autonomous decision (a cold-boot driver and a fork-from-live shadow), recorded as findings and reviewed with `aida findings calibration --stats`, deliberately to mine the gap between what the substrate predicted and what happened.

**Why these are the governance layer.** Together they say: you do not *configure* an agent fleet's behavior, you *fence* it and then *measure* it. This is the layer most directly threatened by vendors (each is building its own in-house gates and memory) — but only *within* its own walls, which is exactly the seam the upper layers exploit.

---

## 5. L3 — Conflict-light collaborative state (P4)

**P4 (Conflict-light state in plain git) — grade (M)(D).** Multi-agent collaborative state can be kept conflict-light *without a CRDT library and without a database* by splitting each object into two kinds of field and merging them with a schema-aware driver: **append-only event streams** merged by union (a grow-only set, keyed by entry id) and **scalar registers** merged last-writer-wins under a total order (a hybrid logical clock). This is a practical, library-free CRDT, embedded in ordinary git.

**Mechanism in AIDA.** `conflict.rs::merge_spec_three_way` is a structured three-way merge driver for one spec's YAML. `history:`, `comments:`, and `processing_record:` are **union by id** (G-Set semantics — order-independent, idempotent, never conflict). `tags:` is a set union. Scalar fields (`status`, `title`, `priority`, …) are **LWW on `modified_at`**, with the HLC (`hlc.rs`) providing the cross-node total order that makes LWW deterministic. The append-only *shape* is what makes the union trivial to define; the *structured driver* is what actually buys conflict-freedom — two raw appends to one file still collide under git's line merge, so the shape alone is not enough.

**The honest limitation, as a worked example of the method.** The tag G-Set loses concurrent removals (a tag deleted on one node reappears after merge, because union cannot encode a delete). The principled fix is an OR-Set (observed-remove set). We filed this as an *exploratory* item rather than a mandate — illustrating the paper's own stance that not every theoretical gap is worth closing, and that knowing *which* to leave open is itself a finding.

---

## 6. L4 — Autonomy and role orchestration (P3, P6)

**P3 (Substrate-bounded autonomy) — grade (M)(D). The most counter-intuitive claim.** The quality ceiling of an autonomous decision is set by the **richness of the substrate the deciding agent boots from, not by the model's raw capability** — because, in a fleet, each escalation/resolution step is typically a *cold boot* (a fresh process with no memory of the live session that produced the problem). Therefore the highest-leverage investment for better autonomy is *substrate enrichment*, not prompt engineering or a bigger model.

**Mechanism in AIDA.** When a headless implementer hits a design fork it *punts* (parks the spec `NeedsAttention`); the orchestrator routes the punt to a headless **advisor tier** — a fresh `claude -p` invocation that has only what the substrate carries, not what the live session knew. We observed directly that this advisor is a cold boot, which reframed our roadmap: the levers that matter are (1) enriching the substrate the cold-boot reads, (2) fork-from-live snapshots, (3) a persistent advisor entity — *all substrate moves*, none a model move.

**After red-team — scoped restatement.** The cold-boot premise is partly *self-imposed* (it follows from spawning a fresh `claude -p`, which fork-from-live and a persistent advisor would dissolve), and "substrate richness" and "model capability" are not independently variable — a more capable model needs less substrate to reach the same decision. So P3 is not a law of agent autonomy. Its defensible, actionable form is **scoped**: *under stateless escalation, substrate enrichment is the cheapest and most reliable autonomy lever — cheaper than chasing model upgrades you do not control.* The unscoped "substrate dominates capability" version likely holds at today's model tier and weakens as models improve; we assert only the scoped claim. See §9.

**P6 (Punt-and-continue role cascade) — grade (M)(D).** Reliable autonomous throughput needs **role separation** (implementer → advisor → human) with **punt-and-continue** semantics: a blocker parks exactly one unit of work and routes it to the right role; it never halts the line. The fleet's throughput is then bounded by the rate of genuinely-human decisions, not by the first blocker.

**Mechanism in AIDA.** The resilient drain (EPIC-28): a shelvable phase failure parks one spec and continues; dependents skip; the run exits with a distinct code so a supervisor can triage the parked set. The three-mode autonomy ladder separates *is a human present?* from *what should be escalated?* as orthogonal axes.

---

## 7. L5 — Cross-vendor fleet coordination, and why it is the bet (P8)

**P8 (Incentive-persistent gap) — grade (M)(K). The apex claim.** The durable, cross-vendor agent-coordination layer is under-provided for **incentive** reasons, not capability reasons. Each model vendor can build excellent *within-vendor* coordination (memory, sub-agents, task graphs, skills) and is strongly incentivized to make it *sticky* and *non-portable* — that is the lock-in. A coordination layer that is deliberately **vendor-neutral and durable** — a Claude agent and a Codex agent and a human sharing one mailbox, one role queue, one lease table, one intent graph — is precisely what no single vendor is incentivized to build well. The gap is therefore **structurally persistent**: it does not close as models improve, because it is not a capability gap.

**Mechanism in AIDA.** The store and the inter-agent surface are vendor-agnostic by construction: a file-based mailbox (`send_message`/`read_inbox`), briefs, directives, findings, leases, and roles, all exposed identically over CLI and MCP so any tool that speaks either can join the fleet. The strategic stance we converged on: **ride the vendor's native coordination *within* a vendor session; own the cross-vendor, durable substrate.** Within-vendor, do not compete with the platform; across vendors and across time, be the layer the platform will not build.

**Market triangulation (K).** Competitor investment validates the *demand* without closing the *gap*: spec-kit and Kiro invest in spec-structured workflows; Beads/Gas Town in agent-issue tracking — all largely single-vendor or single-tool. The fact that capable teams keep building the lower layers, and keep *not* building the neutral cross-vendor layer, is consistent with P8's incentive argument. (We also record a **discovery-method finding**: we missed our two nearest competitors for weeks by searching our *own* vocabulary inside a fixed category. Competitive discovery for a fast-moving field must be multi-modal — sweep awesome-lists by category, watch known builders, search the problem's many names, follow star-velocity — not keyword-match your own framing. This is a methodological contribution in its own right.)

**Why this is the corporate bet.** For a large organization the question is not "which model" — that commoditizes — but "what owns the fleet." As L0–L2 get absorbed upward into vendor platforms, the organization that controls L4–L5 controls the part that (a) spans the vendors it will inevitably mix, (b) survives any single vendor's deprecations, and (c) is the natural home for governance, audit, and policy that an enterprise cannot outsource to a vendor's walled garden.

**After red-team — split the claim.** P8 as stated bundles a strong mechanism with a contingent business bet; the honest paper separates them and asserts only the first strongly:

- **P8a (strong, (M)): a model vendor is disincentivized to build *portable* cross-vendor coordination**, because portability dissolves the lock-in that within-vendor coordination exists to create. This is sound and is what the (M) grade covers.
- **P8b (contingent, (D)(K)): therefore an independent, durable layer is defensible *by AIDA*.** P8b does *not* follow from P8a. P8a establishes the gap exists; it does not establish a small player can hold it. P8b rests on three bets that must be named, not assumed: (i) vendor *heterogeneity persists* (enterprises do not consolidate on one vendor for procurement/governance simplicity); (ii) emerging interop standards (MCP, A2A, ACP) do *not* commoditize the coordination layer down to a thin protocol; (iii) AIDA *out-executes the non-vendor incumbents* — GitHub/Microsoft, the IDEs, LangChain, and the "agent control plane" startups — who are equally incentivized to own L5 and have distribution AIDA lacks. The paper asserts P8a, argues P8b, and treats (i)–(iii) as the explicit, falsifiable conditions the bet rests on. See §9.

---

## 8. Orthogonal: the surface (P7)

**P7 (Trojan-horse disclosure) — grade (D)(K).** For a tool whose value is *emergent* (a graph, a coordination fabric), a deliberately **shallow surface that defers depth-disclosure to use** outperforms a surface that advertises its depth. The "I could do this in 20 lines of bash" reaction on first sight is acceptable — even intended — because the value (IDs, graph, traces, the merge model, the cascade) is only legible after the user has lived in it. Surface complexity is the anti-pattern; quiet depth is the asset. This is a product/HCI claim and the weakest-graded here, but it shaped every interface decision and is worth stating because it cuts against the instinct to demo the architecture.

---

## 9. Strongest objections and restatements

We red-teamed the two load-bearing propositions — P3 (the most counter-intuitive) and P8 (the apex bet) — and let the strongest objections rewrite the claims. The result is two precision edits: both *narrow* the assertion to ground we can defend, surrendering the indefensible part before a reviewer takes it. This is the paper's own stance applied to itself (state the precise open slice; do not overclaim).

### P3 — substrate-bounded autonomy

| Objection | Bite |
|---|---|
| Cold-boot is self-imposed | Each resolution is a cold boot only *because* AIDA spawns a fresh `claude -p`; fork-from-live and a persistent advisor (both on the roadmap) dissolve the premise. The trend in vendor platforms (persistent memory, long sessions) is *against* the premise. |
| Substrate ⊥ capability is false | A more capable model needs *less* substrate for the same decision, so the two trade off and cannot be varied independently; the effect likely shrinks as models improve. |
| Difficulty confound | Richer substrate correlates with better-specified (intrinsically easier) specs, so an ablation risks measuring problem difficulty, not substrate. |
| Necessary ≠ dominant | Substrate may set the *floor* while the model sets the *ceiling*; "substrate is necessary" is not "substrate is the binding constraint." |

**Restatement (asserted):** *Under stateless escalation, substrate enrichment is the cheapest, most reliable autonomy lever.* The unscoped "substrate dominates capability" form is held only weakly and time-bounded to the current model tier.

### P8 — incentive-persistent cross-vendor gap

| Objection | Bite |
|---|---|
| Newness, not incentive | The gap may simply be young and close on its own; "structural persistence" cannot yet be distinguished from immaturity. |
| A gap AIDA can't hold | Even if no *model vendor* builds L5, GitHub/Microsoft, the IDEs, LangChain, and "agent control plane" startups are equally incentivized and have distribution AIDA lacks. The gap existing ≠ a small player capturing it. **(Largest hole.)** |
| Interop commoditizes the layer | MCP / A2A / ACP show vendors *do* have interop incentives; if coordination standardizes at the protocol layer, a mailbox/roles/leases substrate is standardized away. |
| Multi-vendor is transient | Enterprises may consolidate on one vendor for procurement/governance simplicity, shrinking the multi-vendor premise. |
| Native > neutral on UX | A neutral layer is often lowest-common-denominator; users may prefer a vendor's better-integrated native coordination. |
| Self-serving framing | P8 is the claim most aligned with "therefore build AIDA" — exactly the one its authors should most distrust (the §10 self-evaluation threat bites hardest here). |

**Restatement (split):** assert **P8a** (vendors are disincentivized to build *portable* coordination) strongly on the (M) grade; argue **P8b** (an independent durable layer is defensible *by AIDA*) as contingent on three named, falsifiable bets — heterogeneity persists, interop does not commoditize the layer, AIDA out-executes the non-vendor incumbents. The business case lives entirely in P8b and its three conditions, not in P8a.

---

## 10. Threats to validity

- **N=1 system, N≈1 operator.** Every (D) claim is autoethnographic. We cannot separate "AIDA's design helped" from "writing intent down at all helped," nor from "this particular operator's discipline helped."
- **Alpha maturity.** The system is pre-1.0; some mechanisms (e.g., the cold-boot advisor) are recent and lightly exercised.
- **Self-evaluation.** The authors designed the propositions and the system; confirmation bias is unmitigated by independent review.
- **Market grade (K) is observational.** Competitor behavior is consistent with P8 but does not prove the incentive mechanism; an alternative explanation (the gap is simply newer) cannot yet be excluded.
- **Moving target.** The stack's value distribution (§1) is shifting during the study; any snapshot of "how far up value has moved" dates quickly. We treat dated artifacts as immutable observations, not standing claims.

## 11. What a real study would measure

To move claims from (M)(D) to confirmed empirical results:

1. **Multi-team, multi-vendor field study.** Several teams, each running a mixed Claude/Codex/Gemini fleet, half with AIDA's substrate and half with unstructured markdown+git. Measure rework rate, "done-but-not-in-diff" incidence, time-to-context for a cold-boot agent, and merge-conflict rate on shared state.
2. **Gate vs rule ablation (P1).** The same invariant enforced as a CLAUDE.md rule vs a programmatic gate; measure violation rate per 100 agent-actions. P1 predicts a large, robust gap.
3. **Substrate-richness ablation (P3).** Hold the model fixed; vary only how much context the cold-boot advisor's substrate carries; measure resolution quality. P3 predicts substrate dominates model tier.
4. **Calibration longitudinal (P5).** Track predictive accuracy of a learning vs static substrate over months; P5 predicts crossover where the learner overtakes.
5. **Cross-vendor portability test (P8).** Introduce a new vendor agent into a running fleet with zero bespoke integration; measure time-to-productive. P8's value rests on this being near-zero.

## 12. Related work (pointers to develop)

- Karpathy "software 2.0/3.0" and structured-markdown-as-context — AIDA positions the typed graph + ID stability + enforcement loop as the layer *above* this floor.
- CRDTs (Shapiro et al.): G-Sets, LWW-registers, OR-Sets — §5 is an applied, git-embedded instance.
- Design-science research (Hevner) and software-engineering experience reports — the method frame.
- Spec-driven agent tooling (spec-kit, Kiro) and agent-issue trackers (Beads) — the L1–L3 neighbors; §7 argues the open layer is above them.
- Multi-agent systems / blackboard architectures — the file-based mailbox + shared store is a modern, vendor-neutral blackboard.

---

## Appendix A — Proposition ↔ layer ↔ mechanism ↔ grade

| P | Layer | Claim (one line) | Mechanism | Grade |
|---|---|---|---|---|
| P2 | L1 | Intent model is the durable scarce artifact | graph + stable IDs + traces + MCP | (M)(D) |
| P1 | L2 | Govern agents with gates, not instructions | gated transitions, merge-gate, trailer-completes | (M)(D) |
| P5 | L2 | Self-correcting substrate beats static accuracy | calibration mode + findings | (M)(D) |
| P4 | L3 | Conflict-light state = append-union + LWW, schema-aware merge | `merge_spec_three_way`, `hlc.rs` | (M)(D) |
| P3 | L4 | *Scoped:* under stateless escalation, substrate is the cheapest autonomy lever | headless advisor cold-boot + punt routing | (M)(D) |
| P6 | L4 | Punt-and-continue role cascade for throughput | EPIC-28 resilient drain, autonomy ladder | (M)(D) |
| P8a | L5 | Vendors are disincentivized to build *portable* coordination | vendor-neutral mailbox/roles/leases over CLI+MCP | (M) |
| P8b | L5 | *Contingent:* a durable layer is defensible by AIDA (rests on 3 named bets) | same, + the §9 conditions | (D)(K) |
| P7 | surface | Defer depth-disclosure to use | TUI-as-product | (D)(K) |

---

*Working draft. To revise with: collaborators' challenges to the propositions, real numbers from any of the §10 ablations, and updated market snapshots (dated, immutable).*
