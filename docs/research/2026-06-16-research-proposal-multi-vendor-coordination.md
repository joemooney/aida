# Research Proposal — Coordinating Multi-Vendor AI Agent Fleets

**Status:** Draft · **Date:** 2026-06-16 · **Author:** Joe Mooney · **Companion to:** the theory paper (`2026-06-16-coordinating-multi-vendor-agent-fleets.md`) and the decision framework (`2026-06-16-layered-evaluation-framework.md`)

> A concise, proposal-shaped statement of the research agenda: the problem, why current practice does not serve it, the approach, who it serves, and a checklist of measurable proof points. The long-form argument and evidence grades live in the theory paper; this is the agenda in brief.

---

## 1. What is the project trying to do, and why is it hard?

Software is beginning to be written by **fleets** of autonomous AI agents rather than by individual humans or single AI assistants. **Coordinating** them — keeping them aligned on shared intent, tracking what each has done, verifying their work, recovering from their failures — is already one of the most heavily invested problems in the industry: every major vendor is shipping coordination features (background sessions, agent "teams," shared task lists, orchestration loops). *That investment is the proof the problem matters.*

But it is converging on a single shape: coordination **owned by, and locked to, one vendor.** Whether it runs locally or in the vendor's cloud, the state is *the vendor's* — their format, their retention, their agents only. That suits the vendor; it fails a program that must mix providers, **own** its coordination record, and keep coordinating where a vendor cloud cannot or should not reach. The open slice — which no vendor is incentivized to close — is **coordination the program itself owns: vendor-neutral, portable, and inspectable.**

We are trying to discover the **minimal substrate** that fills that gap: one where *everything agents use to coordinate* — both shared declarative state (a typed requirement graph) and the messages they leave each other (assignments, briefs, reviews, escalations) — lives in one durable, program-owned store rather than a vendor's live runtime.

It is hard because every assumption classical coordination relies on is violated:

- **Non-determinism** — same input, different output; coordination cannot assume reproducibility.
- **Ephemerality** — sessions are short-lived; coordination state must outlive the agents.
- **Heterogeneity** — agents span vendors, with different capabilities and no shared protocol.
- **Long horizon** — programs live for years to decades; coordination must outlast vendor, staff, and tooling churn.
- **Boundaries** — work crosses team and subsystem lines, sometimes disconnected environments, with no shared live service.

**The research question:** *what is the minimum a fleet of agents must share to stay coordinated under all five constraints at once — and can we measure, even prove, that the fleet stays coherent and safe when it coordinates that way?*

## 2. How is it done today, and what are the limits of current practice?

Today, coordination is either **human** or **vendor-runtime** — and neither yet serves a large, long-lived, multi-team program.

- **Human coordination** (tickets, code review, tribal knowledge) does not scale to agent fleets operating continuously and faster than humans can supervise.
- **Vendor agent runtimes** are advancing rapidly — background sessions, session/lease supervisors, agent "teams" with shared task lists and mailboxes, unified agent registries, workflow orchestration, goal-completion loops. These work, and they improve monthly.

These runtimes are backed by serious cloud infrastructure — so "it only runs on one machine" is *not* the gap, and won't be; vendors can and do coordinate across machines through their own cloud. The durable gaps are the two a vendor is structurally not incentivized to close:

1. **Cross-tool / vendor-neutral** — a vendor coordinates *its own* agents. It has no reason to be the neutral coordination layer for a fleet that *mixes* providers' agents. A program that wants to combine providers — for capability, cost, or second-sourcing over a long lifecycle — needs coordination that sits *above* any single vendor, and no vendor will build it.
2. **Program-owned, inspectable record** — even when a vendor persists coordination state, that state is the vendor's: their format, their service, their retention, optimized for the active task. It is not a record the *program* holds, can audit end-to-end, and can keep beyond its relationship with that vendor.

No current practice coordinates an autonomous agent fleet across those boundaries with a record the program itself owns — and that gap widens exactly as the program gets larger and longer-lived.

## 3. What is the approach, and how is it new?

**Approach: coordination through a shared, version-controlled declarative substrate, studied as a research object.**

Every agent reads and writes a **shared graph of typed requirements** — stable identifiers, typed relationships, lifecycle state, and code-to-specification trace links. Agents coordinate through this substrate two ways: they **converge on shared declarative state** (the graph), and they **leave one another durable, substrate-resident messages** (work assignments, pickup briefs, review notes, escalation handshakes). The distinctive choice is that *both* live in the program's own version-controlled store, not in a vendor's ephemeral runtime — so even the messages between agents are owned, durable, and auditable after the fact.

The substrate is kept in the same kind of **version-controlled file store engineers already use for source code** (we call this *git-canonical* — the coordination state lives in git, not a live cloud service). That single choice gives, for free, the properties a long-lived program needs: portable (a copy on every machine, working connected or disconnected), owned by no AI vendor, and a complete, attributable, inspectable history over time.

This reframes coordination as a **measurable scientific question** rather than an engineering convenience. The new phenomenology to characterize:

- **Substrate-mediated coherence** — do heterogeneous agents coordinating through a shared, durable substrate (rather than one vendor's runtime) stay aligned on intent as the fleet grows and spans tools? Under what substrate richness — how much state, how structured the messages — does coherence hold or break?
- **Calibrated autonomous trust** — the substrate already records a self-assessed effort/complexity estimate at ship time. The *closed loop* is the research and does not yet exist: capture the actual outcome, compute the prediction-vs-reality error, and feed it back so self-assessment grows measurably more accurate — until autonomy rests on a track record, not hope.
- **Verifiable autonomous escalation** — a formal cascade (agent → reasoning-advisor → human) in which an agent that cannot safely decide is *designed to stop and escalate* rather than guess; measure how reliably it does so — its correctness and false-escalation rates.
- **Substrate-as-enforcement** — coordination guarantees expressed as *programmatic gates* the substrate enforces, versus *instructions* an agent may ignore; measure where each is necessary.

**Why this is research, not engineering:** the contribution is not the apparatus but the discovery of **the minimal building blocks of agent coordination** and the laws governing fleet coherence, calibration, and safe escalation over a vendor-neutral substrate. **Why now:** agents have just become capable enough for real autonomous coordination, and vendors are proving the *single-vendor* case — de-risking the agent layer and isolating the open problem (vendor-neutral, program-owned coordination). The apparatus exists and is dogfooded daily, so research starts against real fleets on day one.

## 4. Who cares?

**Large, long-lived, multi-team software programs care most.** Such programs spread work across many teams and subsystems, run for years, sometimes operate in disconnected or offline environments, and carry traceability and audit obligations. A vendor-neutral, version-controlled coordination substrate addresses what a single-vendor runtime is not positioned to:

- **Vendor-independent** — no lock-in to a single AI provider; the program can mix or change providers over a long lifecycle without losing its coordination state. No vendor offers this, for the incentive reason above.
- **Program-owned record** — the coordination state lives in the program's own version-controlled store, in its format, under its retention — not in a provider's cloud the program neither owns nor controls.
- **Traceable** — code-to-spec links with stable identifiers fit the requirements-traceability practice these programs already follow.
- **Reaches where vendor clouds cannot** — because coordination travels as files, a fleet can keep working in disconnected or offline environments a live cloud runtime cannot serve.

**In short: a company can run multi-vendor autonomous agent fleets without vendor lock-in, provided it can identify the right architecture.**

## 5. Proof points and milestones

A checklist of measurable proof points, grouped by what they demonstrate. They are deliberately more numerous and more specific than a phased calendar: each is an independent piece of evidence that can be claimed as soon as it lands, and the agenda advances by accumulating them — not by waiting on a multi-month plan that the monthly-shifting agent-runtime landscape would obsolete before its second phase.

### A. Adoption — does a real fleet run on the substrate at increasing scale and constraint?
- [ ] **Solo developer** runs a multi-agent fleet coordinating *only* through the shared substrate. (First real coordination data, smallest scale.)
- [ ] **Inter-team / inter-subsystem** fleet coordinates across boundaries — the first regime a single-vendor runtime is not built to serve.
- [ ] **Multi-program** coordination: two independent programs share one substrate and surface a cross-program dependency or reuse neither team filed by hand.
- [ ] **Disconnected / offline** fleet coordinates with no live central service, against a vendor-runtime baseline that depends on one. (The offline-operation claim.)

### B. Vendor-neutrality and lock-in
- [ ] A **new vendor's agent joins a running fleet with zero bespoke integration**; time-to-productive measured (target: near-zero).
- [ ] A **full provider swap mid-program** with no loss of coordination state — the program-owned record survives the change.
- [ ] Coordination state **exported in full fidelity** (history + typed relationships) to an open format and re-imported without loss.

### C. Measurement / phenomenology — the science
- [ ] **Intent-coherence convergence rate** measured as a fleet grows and spans tools: does coherence hold, and under what substrate richness does it break?
- [ ] **Trust-calibration accuracy improves over time** — the prediction-vs-reality error shrinks measurably as the closed loop runs.
- [ ] **Autonomous-escalation correctness + false-escalation rate** measured — the agent reliably stops-and-escalates rather than guessing.
- [ ] **Substrate-as-enforcement:** a gate-vs-rule comparison shows programmatic gates hold an invariant where instructions do not (violation rate per N agent-actions).

### D. Governance and record
- [ ] **End-to-end audit** of who/what/when/under-what-authority reconstructable from the program-owned record alone.
- [ ] **Code-to-spec traceability** links resolve bidirectionally across the whole fleet's output.

### E. Synthesis
- [ ] A written account of **the minimal building blocks of agent coordination** (the substrate floor), backed by the measurements above.
- [ ] The **roll-your-own cost/benefit** ledger resolved from evidence, or its remaining open conditions explicitly named.

**Phasing note (objectives, not a calendar).** The adoption proof points form a ladder of increasing scale and constraint — solo → inter-team → multi-program → disconnected — and each tier is not "productization" but the means by which a real fleet is observed at greater scale, generating the measurements the science depends on. Because the agent-runtime landscape shifts monthly, the science runs *continuously* and the tiers come online on overlapping, evidence-first schedules: each tier is an entry point where a sponsor can pick up and fund the next step on *measured results, not promise* — evidence early and continuously, not after a multi-year wait.

---

*Working draft. Companion to the theory paper and the decision framework; develop alongside them as proof points land.*
