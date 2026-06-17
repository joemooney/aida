# Coordinating Multi-Vendor AI Agent Fleets

### A Theory of AIDA — design-science findings from using a home-grown system as a probe into multi-vendor agent coordination (AIDA is the instrument, not the subject)

**Status:** Draft basis for a research paper · **Date:** 2026-06-16 · **Genre:** Design-science / autoethnographic systems research · **Spine:** Multi-vendor fleet coordination (apex of a layered coordination stack) · **Frame:** AIDA as research probe — the deliverable is knowledge about the problem plus a roll-your-own cost/benefit, *not* a case for AIDA

> This is a working draft meant to be handed to collaborators and revised. Claims are tagged with an **evidence grade** (see §2) so reviewers can see exactly how much weight each carries. Nothing here is asserted as a controlled empirical result; the contribution is the artifact and the design principles extracted from building it.

---

## Abstract

As large language models commoditize code generation, the binding constraint on software production moves *off* the model and *onto* the coordination of many agents — across roles, across time, and, increasingly, across **vendors**. We report on AIDA, a system built to make a fleet of confident, forgetful, concurrent, multi-vendor coding agents collaborate against a shared, durable model of intent. AIDA is git-canonical: every requirement is a YAML object on an orphan branch, related agents read and write it through a typed graph and an MCP server, and code links back to it through inline trace comments. We frame AIDA's machinery as a **coordination stack** and advance the position that, as lower layers of this stack are absorbed by model vendors, durable value migrates upward — and that the topmost layer, *cross-vendor fleet coordination*, is structurally under-provided for incentive rather than capability reasons. **AIDA is the instrument, not the subject:** we use a home-grown system as a probe to surface the real requirements, minefields, and faulty assumptions that appear only once you build, and to weigh the cost/benefit of roll-your-own — for which our honest verdict is *open*. We extract eight design propositions about the problem space (not arguments for AIDA), each embodied in a running mechanism and triangulated against market observation. We are explicit about the threats to validity of single-system, single-operator, alpha-stage design research, and we state what a multi-team study would have to measure to confirm or refute each proposition.

---

## 1. The shift: code is getting cheap, coordination is getting expensive

The first-order effect of capable coding agents is obvious — code gets written faster. The second-order effect is the interesting one: when any unit of code is cheap to (re)generate, the *code* stops being the scarce, durable artifact. What becomes scarce is an **addressable, queryable model of intent** that survives the churn — what was decided, why, what depends on what, which code satisfies which requirement, and what is still open. A human team carried that model implicitly, in heads and in review. A fleet of agents cannot: agents are **confident** (they assert), **forgetful** (each session is a fresh context), and **concurrent** (many act on the same artifacts at once). And the fleet is increasingly **multi-vendor** — a Claude implementer, a Codex sibling, a Gemini reviewer — none of which shares the others' memory, conventions, or governance.

AIDA is a home-grown attempt at the missing substrate — and we treat it as a **probe**, not a product to defend. This paper is not a tool description, nor a case for AIDA; it uses what building (and dogfooding) AIDA taught us to map *which layer of the agent-coordination problem holds durable value*, what a home-grown solution can and cannot reach, and whether rolling your own is worth the cost. The artifact is the instrument; the deliverable is the knowledge.

> **AIDA as probe, not product.** Throughout, read every mechanism as evidence about the *problem*, not as a feature pitch. Where AIDA succeeds it marks what is possible; where it strains or fails — a scaffolding bug that polluted its own `main` branch mid-writing (BUG-570), weeks lost to a market-discovery blind spot — it marks a minefield or a true cost of rolling your own. Both directions are data. The right answer the probe points to may well be "do not build this," or "the tool is not AIDA."

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

**The central research question.** Value in this stack is migrating upward: as model vendors solve L0 and begin to absorb L1–L2 inside their own walled gardens (project memory, skills, sub-agents, internal task graphs), the durable, defensible value moves to the layers a single vendor is *disincentivized* to provide well — the cross-vendor layers L4–L5. For a large corporation deciding where to invest as the ground shifts, the open question is **how far up the stack value has already moved, how fast, and whether building any of it yourself is worth the cost.** We do not claim certainty about the answer — and we explicitly do not claim AIDA is the thing to build. We claim the stack is the right way to *ask*; that the incentive structure makes L4–L5 the layers a single vendor will under-serve; and that whether *you* should roll your own there is a cost/benefit question we leave open (§12).

---

## 2. Method and evidence grades

This is **design-science research** (Hevner): the artifact is the primary contribution, and the knowledge claims are design principles extracted from building and operating it. It is also **autoethnographic** — AIDA is built *using* AIDA; the dogfood is the experiment, and the authors are the subjects. This is a legitimate genre (systems "experience reports," design science, reflective practice), but it has sharp limits, which we state up front rather than bury (§10).

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

**The formal core.** The merge converges because each field's merge function is a *join* in the CRDT sense — commutative, associative, and idempotent — so two replicas that have observed the same set of updates compute the same value regardless of order or duplication. The append-only arrays are grow-only sets (G-Sets), where union is the join and `merge(a,a)=a`, `merge(a,b)=merge(b,a)`, and `merge(merge(a,b),c)=merge(a,merge(b,c))` hold trivially. The scalar fields are last-writer-wins registers (LWW-Registers): the join is "take the value with the greater `(HLC-timestamp, node-id)` pair," and the hybrid logical clock supplies the *total* order that makes that join deterministic across nodes without a coordinator. What git's native three-way merge cannot do is recognize these field semantics — it diffs lines, so two appends to one file collide and an LWW field edited on both sides conflicts; the schema-aware driver replaces git's line-merge *for these files* with the per-field join. **The reusable recipe, beyond AIDA:** model each field as the CRDT whose semantics match its use — event stream → G-Set (or OR-Set when deletes must survive), current value → LWW- or MV-Register — and supply a structured merge driver that applies the per-field join instead of a textual diff. The cost is that *every* field must be classified; the unclassified default (whole-object LWW) silently drops concurrent edits — which is the trap the tag G-Set's lost-removal only partially illustrates.

---

## 6. L4 — Autonomy and role orchestration (P3, P6)

**P3 (Substrate-bounded autonomy) — grade (M)(D). The most counter-intuitive claim.** The quality ceiling of an autonomous decision is set by the **richness of the substrate the deciding agent boots from, not by the model's raw capability** — because, in a fleet, each escalation/resolution step is typically a *cold boot* (a fresh process with no memory of the live session that produced the problem). Therefore the highest-leverage investment for better autonomy is *substrate enrichment*, not prompt engineering or a bigger model.

**Mechanism in AIDA.** When a headless implementer hits a design fork it *punts* (parks the spec `NeedsAttention`); the orchestrator routes the punt to a headless **advisor tier** — a fresh `claude -p` invocation that has only what the substrate carries, not what the live session knew. We observed directly that this advisor is a cold boot, which reframed our roadmap: the levers that matter are (1) enriching the substrate the cold-boot reads, (2) fork-from-live snapshots, (3) a persistent advisor entity — *all substrate moves*, none a model move.

**After red-team — scoped restatement.** The cold-boot premise is partly *self-imposed* (it follows from spawning a fresh `claude -p`, which fork-from-live and a persistent advisor would dissolve), and "substrate richness" and "model capability" are not independently variable — a more capable model needs less substrate to reach the same decision. So P3 is not a law of agent autonomy. Its defensible, actionable form is **scoped**: *under stateless escalation, substrate enrichment is the cheapest and most reliable autonomy lever — cheaper than chasing model upgrades you do not control.* The unscoped "substrate dominates capability" version likely holds at today's model tier and weakens as models improve; we assert only the scoped claim. See §9.

**P6 (Punt-and-continue role cascade) — grade (M)(D).** Reliable autonomous throughput needs **role separation** (implementer → advisor → human) with **punt-and-continue** semantics: a blocker parks exactly one unit of work and routes it to the right role; it never halts the line. The fleet's throughput is then bounded by the rate of genuinely-human decisions, not by the first blocker.

**Mechanism in AIDA.** The resilient drain (EPIC-28): a shelvable phase failure parks one spec and continues; dependents skip; the run exits with a distinct code so a supervisor can triage the parked set. The three-mode autonomy ladder separates *is a human present?* from *what should be escalated?* as orthogonal axes.

---

## 7. L5 — Cross-vendor fleet coordination (P8)

**P8 (Incentive-persistent gap) — grade (M)(K). The apex claim.** The durable, cross-vendor agent-coordination layer is under-provided for **incentive** reasons, not capability reasons. Each model vendor can build excellent *within-vendor* coordination (memory, sub-agents, task graphs, skills) and is strongly incentivized to make it *sticky* and *non-portable* — that is the lock-in. A coordination layer that is deliberately **vendor-neutral and durable** — a Claude agent and a Codex agent and a human sharing one mailbox, one role queue, one lease table, one intent graph — is precisely what no single vendor is incentivized to build well. The gap is therefore **structurally persistent**: it does not close as models improve, because it is not a capability gap.

**Mechanism in AIDA.** The store and the inter-agent surface are vendor-agnostic by construction: a file-based mailbox (`send_message`/`read_inbox`), briefs, directives, findings, leases, and roles, all exposed identically over CLI and MCP so any tool that speaks either can join the fleet. The strategic stance we converged on: **ride the vendor's native coordination *within* a vendor session; own the cross-vendor, durable substrate.** Within-vendor, do not compete with the platform; across vendors and across time, be the layer the platform will not build.

**Market triangulation (K) — June 2026 snapshot.** *(Dated and treated as immutable per §10's moving-target caveat; from a multi-source landscape scan, confidence varying by source.)* The evidence corroborates P8a with unusual clarity:

- **The interop standards converge — but deliberately stop short of coordination state.** MCP (agent↔tool; donated to the Linux Foundation's Agentic AI Foundation, Dec 2025), A2A (agent↔agent; Google → Linux Foundation; *explicitly stateless* — discovery, delegation, message-passing, "without sharing internal memory, state, or tools"), and ACP (IBM; *merged into A2A* in 2025) are consolidating under neutral governance. None standardizes a durable, multi-vendor-readable *coordination record* (shared queue / role / lease / intent state); MCP's experimental Tasks primitive is a single-requestor call-now-fetch-later pattern, not shared state. Independent analyses and the protocols' own specs/roadmaps agree the coordination-state layer is the open, un-standardized slice.
- **Every first-party vendor coordination feature is single-vendor.** Claude Code's Agent Teams (file-based, local, Claude-only), OpenAI Codex sub-agents, Gemini/Jules, Cursor's multi-agent mode, Copilot's fleet, Amp, Devin-manages-Devins — none lets a *different* vendor's agent join its coordination primitive. The host layer (e.g. VS Code's multi-agent sessions) runs rival agents side-by-side but with *no unified cross-vendor task list*. This is exactly P8a's prediction: vendors build coordination, and build it single-vendor.
- **The exact niche — a portable, program-owned, cross-vendor coordination record — is occupied only by small OSS.** GNAP (git-native agent protocol: roster / tasks / runs / messages as JSON in a plain git repo, "if it can `git push`, it can participate"), and the Beads + Gas Town pair (a DAG work-store plus a 20–30-agent fleet orchestrator over git worktrees). This both *validates the approach* and **tempers any uniqueness claim**: AIDA is not alone, git-native cross-vendor coordination is a small-but-real movement rather than a singular insight, and the honest comparison is against these neighbors, not a vacuum. (Beads notably moved its store to Dolt rather than plain git — a reminder that "git-canonical" is a deliberate choice, not the default.)

The pattern holds: capable teams keep building the lower layers and the single-vendor case; the neutral cross-vendor *record* is left to a handful of OSS projects — consistent with P8a's incentive argument. (Discovery-method finding, retained: we missed our nearest competitors for weeks by searching our *own* vocabulary inside a fixed category. Competitive discovery in a fast-moving field must be multi-modal — sweep awesome-lists, watch known builders, search the problem's many names, follow star-velocity — not keyword-match your framing. A methodological contribution in its own right, and the reason GNAP/Gas Town above are in this draft at all.)

**Implication for the build-vs-buy decision (not an AIDA pitch).** For a large organization the question is not "which model" — that commoditizes — but "what owns the fleet." As L0–L2 get absorbed upward into vendor platforms, L4–L5 is where a *home-grown* layer could still buy something a vendor will not provide: spanning the vendors an enterprise inevitably mixes, surviving any single vendor's deprecations, and hosting governance/audit/policy that cannot be outsourced to a walled garden. Whether that justifies *building* it — versus waiting for a neutral incumbent, or accepting a vendor's native coordination — is the open cost/benefit question of §12, not a foregone conclusion.

**After red-team — split the claim.** P8 as stated bundles a strong mechanism with a contingent business bet; the honest paper separates them and asserts only the first strongly:

- **P8a (strong, (M)): a model vendor is disincentivized to build *portable* cross-vendor coordination**, because portability dissolves the lock-in that within-vendor coordination exists to create. This is sound and is what the (M) grade covers.
- **P8b (contingent, (D)(K)): therefore an independent, durable layer is defensible *by AIDA*.** P8b does *not* follow from P8a. P8a establishes the gap exists; it does not establish a small player can hold it. P8b rests on three bets that must be named, not assumed: (i) vendor *heterogeneity persists* (enterprises do not consolidate on one vendor for procurement/governance simplicity); (ii) emerging interop standards do *not* commoditize the coordination *record* down to a thin protocol — *currently favorable* (as of June 2026 MCP/A2A/ACP have converged under the Linux Foundation but explicitly stop at tool-calling and delegation, leaving coordination state open), but the A2A↔MCP interop working group and NIST's agent-standards initiative are the things to watch; (iii) AIDA *out-executes the non-vendor incumbents* — and as of mid-2026 they are **visibly moving on L5**: Microsoft's Agent 365 "control plane" (with cross-vendor registry sync to AWS/Google agents in preview), Temporal positioning as an "agentic control plane," control-plane startups being acquired, and OSS neighbors (GNAP, Gas Town) already in the exact niche. The red-team hole — *a neutral incumbent, not you, captures it* — is not hypothetical; it is materializing. The paper asserts P8a, argues P8b, and treats (i)–(iii) as the explicit, falsifiable conditions the bet rests on. See §9.

---

## 8. Orthogonal: the surface (P7)

**P7 (Trojan-horse disclosure) — grade (D)(K). The weakest claim, and the one most about the artifact rather than the problem.** We record it as an *observation about home-grown-tool adoption*, not a recommendation. For a tool whose value is *emergent* (a graph, a coordination fabric), a deliberately shallow surface that defers depth-disclosure to use seemed to land better than one advertising its depth — the "I could do this in 20 lines of bash" first reaction was acceptable because the value (IDs, graph, traces, the merge model, the cascade) is only legible after living in it. We flag it precisely because it is *least* generalizable: it may simply describe one operator's taste, and it is the finding most entangled with promoting the artifact — which this paper explicitly is not trying to do.

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

## 11. Faulty assumptions this probe falsified

The clearest argument for *building* rather than analyzing is the set of beliefs that fail only once the system is real. Each below was held at some point and falsified by the dogfood; several became the findings above. They cluster in two places — the gap between *what we assumed an LLM would do* and what it did, and the unglamorous infrastructure seams — and neither is visible from analysis alone.

| Assumption (believed) | What building falsified it | Becomes |
|---|---|---|
| Rules and prompts can govern a capable agent | Agents routed around CLAUDE.md / memory rules confidently; only programmatic gates held the line | P1 (substrate-as-bouncer) |
| A more capable model reduces the need for coordination infrastructure | The hardest autonomy failures were context-starvation at cold-boot, not reasoning limits; a bigger model does not help a fresh process that lacks the substrate | P3 (scoped) |
| A git-backed multi-writer store means merge hell | False *with* a schema-aware structured merge (append-union + LWW); but naive line-merge on appends still conflicts — the append *shape* alone was a trap | P4 |
| Requirements tracking can be bolted on after the code | ID instability and missing traces made retrofitting the graph expensive; it must be load-bearing from the first commit or agents cannot name things unambiguously | P2 |
| Cross-vendor coordination is a capability problem the next model solves | It is an incentive problem; no model release moves it | P8a |
| Scaffolding / init is harmless and idempotent | An `aida init` on a fresh clone silently committed a regenerable scaffold dump and polluted the shared `main` branch (BUG-570, found mid-writing). Home-grown infra has minefields exactly where attention lapses | a direct cost-of-RYO datapoint (§12) |
| I can find my competitors by searching my own framing | We missed our two nearest competitors for weeks; discovery had to go multi-modal — a faulty assumption about the *research method itself*, not the product | a method finding |

The pattern is the justification for the probe: these were not knowable by reasoning about the problem; they required walking the ground. That value accrues *whether or not the artifact survives.*

## 12. The cost/benefit of roll-your-own (verdict: open)

We state plainly that we do **not** have a verdict on whether a home-grown coordination layer is worth building. The probe is still measuring. Here is the ledger as it stands.

**Costs (observed).**
- *Churn against shifting vendor surfaces.* During the build, vendor-native primitives (sub-agents / agent teams, MCP, skills, project memory) appeared and moved *under* the home-grown layer, repeatedly turning built features into redundant or competing ones. You build on sand, and the sand is being poured by better-resourced teams.
- *The unglamorous substrate tax.* An ID dispenser, a hybrid logical clock, a structured three-way merge driver, role-gating, cross-platform CI debt, scaffolding/init correctness (BUG-570) — none of it the interesting part, all of it required before the interesting part works.
- *The dogfood maintenance tax.* The system must work well enough to build itself, every day, before it can teach you anything.

**Benefits (observed).**
- *Control of the bouncer.* You can place gates and invariants (P1) exactly where a vendor will not, because the incentives differ.
- *Vendor-neutrality and durability.* Nothing in the substrate is hostage to one vendor's roadmap or deprecations.
- *The learning itself.* You cannot map the minefield (§11) without walking it. Much of this paper did not exist as analyzable knowledge until the system forced it into the open. If the goal is *understanding the problem*, building is a uniquely high-bandwidth instrument — even if the artifact is later discarded.

**The asymmetry that shapes the (still-open) verdict.** Cost and benefit are *layer-dependent*. Lower-stack cost (L0–L2) is being eroded as vendors solve it for free, so building there increasingly duplicates work that arrives anyway. Upper-stack (L4–L5) is where home-grown still uniquely buys something — but it is also where the largest red-team hole sits (a neutral incumbent, not you, may capture it; §9, P8b). So the honest lean, held loosely, is *against* rolling your own at the bottom and *toward at-most-experimenting* at the top — but we assert neither as a conclusion.

**What would settle it.** The §13 ablations — especially the cross-vendor portability test and the multi-team field study — plus the simple passage of time: watch whether vendor surfaces converge on the upper layers within roughly a year. Until then, the verdict is open, and saying so is the finding.

A companion decision framework — `2026-06-16-layered-evaluation-framework.md` — operationalizes this open verdict into a per-layer **build / buy / ride-native / wait** rubric with lock-in safeguards, a cross-cutting scoring checklist, and the *substrate-as-organizational-asset* thesis (the shared intent+coordination graph as a compounding, non-purchasable competitive advantage).

## 13. What a real study would measure

To move claims from (M)(D) to confirmed empirical results:

1. **Multi-team, multi-vendor field study.** Several teams, each running a mixed Claude/Codex/Gemini fleet, half with AIDA's substrate and half with unstructured markdown+git. Measure rework rate, "done-but-not-in-diff" incidence, time-to-context for a cold-boot agent, and merge-conflict rate on shared state.
2. **Gate vs rule ablation (P1).** The same invariant enforced as a CLAUDE.md rule vs a programmatic gate; measure violation rate per 100 agent-actions. P1 predicts a large, robust gap.
3. **Substrate-richness ablation (P3).** Hold the model fixed; vary only how much context the cold-boot advisor's substrate carries; measure resolution quality. P3 predicts substrate dominates model tier.
4. **Calibration longitudinal (P5).** Track predictive accuracy of a learning vs static substrate over months; P5 predicts crossover where the learner overtakes.
5. **Cross-vendor portability test (P8).** Introduce a new vendor agent into a running fleet with zero bespoke integration; measure time-to-productive. P8's value rests on this being near-zero.

## 14. Related work

*Foundations.* The conflict-light store (§5) is an applied instance of CRDTs (Shapiro et al.) — G-Sets, LWW-Registers, OR-Sets — embedded in git rather than a bespoke runtime. The method frame is design-science research (Hevner) and the software-engineering *experience-report* tradition. The shared-store-plus-messages coordination pattern is a modern, vendor-neutral *blackboard* architecture. The floor it builds on is Karpathy-style "structured markdown as context"; AIDA's claim is that the typed graph + ID stability + enforcement loop is the layer *above* that floor.

*Interop standards (June 2026).* MCP (agent↔tool, under the Linux Foundation's Agentic AI Foundation), A2A (agent↔agent discovery/delegation, explicitly stateless), and ACP (merged into A2A) standardize how agents call tools and pass messages — *not* a shared coordination record. They are the protocol layer *beneath* the coordination-state layer this paper concerns (see §7).

*Spec-structuring neighbors.* GitHub spec-kit (vendor-neutral, git-portable, but sequences a single agent), AWS Kiro (single-vendor, spec-driven IDE), OpenSpec, Backlog.md — these structure specs/tasks but do not coordinate fleets; they are L1/L3 neighbors, not L5.

*Nearest neighbors — portable cross-vendor coordination.* **GNAP** (git-native agent protocol: roster/tasks/runs/messages as JSON in plain git; cross-vendor; no lease mechanism) and the **Beads + Gas Town** pair (a DAG work-store plus a fleet orchestrator over git worktrees; Beads now Dolt-backed) occupy AIDA's exact niche and are the most direct comparison. AIDA's distinguishing choices are the typed *relationship graph* (vs flat issues/tasks), lease/role gating, and code-to-spec traces — but the family resemblance is real, and the paper's claims should be read against these, not in a vacuum.

*Orchestration / control planes.* LangGraph, CrewAI, the Microsoft Agent Framework, Temporal, and Microsoft Agent 365 orchestrate or govern multi-agent work but hold coordination state in a service or process, not a portable program-owned record. They are the L4 orchestration neighbors and, increasingly (§9, P8b-iii), the incumbents most likely to contest L5.

*Execution orchestrators.* A large class of parallel-execution tools — Claude Squad, Conductor Build, Code Conductor, Axel, SPECTRE, and others — fan agents out across isolated git worktrees with a review/merge dashboard. Of nine surveyed (June 2026), eight maintain *no* durable coordination model; they are L4 execution fan-out, not L1/L3/L5. *graft* is a related but distinct runtime lock-manager — OS-style claim-before-write leases at file-resource granularity (state in an in-memory bus), preventing concurrent-edit conflicts — complementary to intent coordination, not a substitute for it.

*Identity and authorization.* *Authenticated Delegation* (South et al., arXiv:2501.09674, 2025) extends OAuth/OIDC with agent-ID and delegation tokens so a third party can verify an agent acts for a named principal within granted permissions. It is a trust substrate a coordination layer can sit *atop* — complementary, and a natural pairing for a cross-vendor fleet that must prove who authorized what.

---

## 15. What the probe produced — distinctive value, honestly assessed

The probe stance forces a specific question: not "is AIDA good," but *what value, if any, did building it encapsulate that is genuinely distinctive rather than commodity — and how much of that survives contact with the current landscape (June 2026)?* We sort by defensibility and concede where neighbors match, because conceding is the point.

**The honest correction, first.** Our initial read was that AIDA *fuses* a typed relationship graph + code-to-spec traces + lifecycle gates into a coordination substrate no neighbor holds whole. A landscape scan falsifies the strong form: the **Gas Town / Beads stack** (Yegge) holds most of that fusion — Beads is a typed-relationship issue graph (`relates_to` / `duplicates` / `supersedes` / parent-child + blocker-aware "ready" detection; ~24.6k stars at scan time), Gas Town adds seven coordination roles and enforced merge gates (the *Refinery* queue), it is cross-vendor and durable. AIDA is therefore **not unique in the fusion**; it is one member of a small family — with Gas Town/Beads the *more mature* member by traction — distinguished only by narrower choices. (Dated snapshot, treated as immutable per §10.)

**Tier 1 — what survives as genuinely distinctive (narrowed).**
- **Code-to-spec inline traceability** (`// trace:SPEC-ID`, machine-checkable from either direction): found in *none* of the dozen-odd tools surveyed (Gas Town only indirectly, via issue IDs in commits). The satisfaction relation living in the source itself is the sharpest surviving differentiator.
- **Git-YAML-canonical store** (one plain YAML file per spec; SQLite a rebuildable cache) vs the family's *database-as-source-of-truth* (Beads requires Dolt). A real architectural fork: AIDA's record is readable with `git` and a text editor, works anywhere git works, needs no database engine — the "program-owned, inspectable, portable" property in its strongest form.
- **Spec-lifecycle *authority* gates** — who may advance Draft→Approved→… (advisor-only transitions, merge-driven completion, trailer auto-complete) — as distinct from merge-time *verification* gates (Refinery's CI queue). AIDA gates the *authority to change intent state*; the family gates *what merges*. Different, rarer enforcement point.
- **The MCP requirement-graph surface** (~60 tools exposing the typed graph to any agent): not matched by the fan-out tools; Beads exposes a CLI/SQL graph of a different shape.

**Tier 2 — distinctive but contested (the family holds it too).** The typed relationship graph itself, role/lease coordination, and durable cross-vendor coordination are all present in Beads/Gas Town and *more mature there*. AIDA's versions differ in shape — a typed *requirement* graph (19 node types) vs an issue graph; an advisor/implementer/reviewer/integrator *seat* model vs Mayor/Polecat/Refinery — but the category is shared, not owned.

**Tier 3 — commodity (where most of the field lives).** Parallel execution + worktree isolation + dashboard (Claude Squad, Conductor Build, Code Conductor, Axel — 8 of 9 surveyed); spec-structuring as portable markdown (Kiro, SPECTRE, spec-kit); runtime file-lock arbitration (graft). Useful, replicable, not where durable value sits.

**Tier 4 — the value that does not depend on the artifact.** The transferable knowledge (P1–P8). Even granting that Gas Town/Beads matches much of AIDA's surface, the *findings* — substrate-as-bouncer, substrate-bounded autonomy, the conflict-light recipe, calibration-over-accuracy, the incentive-persistent gap — are the probe's durable output and generalize across the whole family.

**Landscape map** (June 2026 snapshot; `Y` / `~` partial / `N`; star counts point-in-time):

| Tool / family | Typed graph | Code↔spec trace | Lifecycle-authority gate | Roles / leases | Git-canonical (plain files) | Cross-vendor | Durable coordination model |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| **AIDA** | Y | **Y** | **Y** | Y | **Y** (git-YAML) | Y | Y |
| Gas Town + Beads | Y | ~ (issue IDs) | Y (merge queue) | Y (7 roles) | N (Dolt DB) | Y | Y |
| GNAP (git-native) | N | N | N | N | Y | Y | ~ (primitive sync) |
| graft (lock-manager) | ~ (runtime DAG) | N | ~ (resource locks) | ~ (file leases) | N (in-mem bus) | ~ (Claude-first) | runtime only |
| Kiro (spec IDE) | N | N | ~ (workflow seq) | N | Y (md) | N | N (single agent) |
| Kilo Code | N | N | N | N | N (ephemeral) | Y (model-level) | ~ (subtask) |
| Claude Squad / Conductor | N | N | N | N | Y (worktrees) | Y | N (fan-out) |
| spec-kit / SPECTRE | N | N | ~ (phase seq) | N | Y | mixed | N |

**The synthesis (revised).** AIDA's encapsulated value is *not* a unique architecture — the landscape contains a more-mature sibling. It is (a) a narrow set of distinctive choices within a real family — code-to-spec traceability, git-YAML purity, lifecycle-authority gating, the MCP graph surface — and (b) the knowledge the build produced (Tier 4). For the central research question, the load-bearing landscape fact is that **the typed-graph + roles + gates + cross-vendor coordination model is now independently held by at least two efforts (AIDA, Gas Town/Beads) plus thinner ones (GNAP)** — which *strengthens* P8a (the niche is real and being filled by OSS, not by model vendors) while *sharpening* P8b's third condition: "out-execute the incumbents" must now also mean out-executing Gas Town/Beads, which has Yegge's distribution and an order of magnitude more traction. The probe's honest verdict on AIDA-the-artifact tilts further toward "a useful instrument that taught us the problem," not "the thing to bet on" — exactly the distinction the paper exists to keep clear.

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

*Working draft. To revise with: collaborators' challenges to the propositions, real numbers from any of the §13 ablations, and updated market snapshots (dated, immutable).*
