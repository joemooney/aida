---
title: "Coordinating Multi-Vendor AI Agent Fleets"
subtitle: "What must agents share to stay coherent, safe, and auditable across vendors, sessions, teams, and time?"
author: "Joe Mooney"
date: "June 2026"
---

# Why This Matters Now

## Code generation is no longer the bottleneck

Agents are becoming:

- **Concurrent**: many agents working at once
- **Forgetful**: each session starts cold
- **Heterogeneous**: Claude, Codex, Gemini, Copilot, goose, Cursor, Devin
- **Long-lived**: software programs outlast vendors and staff

The scarce artifact becomes **shared intent**: what exists, why, who owns it, what depends on it, and which code satisfies it.

::: notes
Make the shift explicit: as code gets cheaper, coordination gets more expensive. The practical problem is not "can an agent write code?" It is "can 5, 20, or 100 agents change the same program without losing intent?"

Timing: 1 minute.
:::

---

# The Market Is Solving Coordination

## But mostly inside vendor or tool boundaries

| Lane | Examples | What it proves | Limit |
|---|---|---|---|
| Vendor-native coordination | Claude Code Agent Teams; Codex subagents | Coordination is a real platform problem | Usually one vendor's agents and runtime |
| Durable OSS coordination | Beads + Gas Town; GNAP; Goosetown | The portable-record idea is real | Fragmented, young, or different model |
| Messaging substrate | agmsg; tap; Agent Mail | Cross-agent messaging is commoditizing | Message bus, not a requirement record |
| SDD / spec tooling | Spec Kit; Kiro; OpenSpec; BMAD | Specs-as-artifacts is mainstream | Often per-feature, not a maintained graph |
| Standards | MCP; A2A; AGENTS.md | Transport and context are standardizing | They stop short of a durable coordination record |
| Orchestration frameworks | LangGraph; CrewAI; AutoGen/AG2 | Patterns, checkpoints, memory, tracing are maturing | Usually app/runtime state, not program-owned intent |

::: notes
This is the most important correction to make professionally: the space is not empty. Beads/Gas Town and GNAP matter. The right claim is narrower: no one has clearly assembled the exact bundle of program-owned requirement graph, trace enforcement, lifecycle, approval authority, and vendor-neutral coordination.

Timing: 1.5 minutes.
:::

---

# The Open Slice

## A program-owned coordination record

The gap is not "multi-agent coordination."  
The gap is **coordination the program owns**.

It must be:

- **Vendor-neutral**: any agent can participate
- **Durable**: survives sessions, tools, and vendor changes
- **Inspectable**: reconstruct who did what, when, and under what authority
- **Traceable**: connect requirement -> code -> review -> merge
- **Portable**: readable without a hosted control plane

**Core hypothesis:** vendors are incentivized to coordinate their own agents well, but disincentivized to make that coordination portable across competitors.

::: notes
This slide is the thesis. Say it plainly: vendors can absolutely build coordination. The incentive problem is portability. A vendor-owned coordination record strengthens lock-in; a program-owned coordination record weakens it.

Timing: 1 minute.
:::

---

# Proposed Substrate

## The minimum shared state to test

```text
Typed requirement graph
  stable IDs | relationships | lifecycle

Code-to-spec traces
  source comments | commit trailers | merge linkage

Coordination layer
  queue | leases | roles | briefs | reviews | escalations

Runtime controls
  checkpoints | retries | circuit breakers | cost traces

Program-owned store
  versioned | auditable | exportable | offline-capable
```

Agents coordinate by reading and writing the same durable substrate, not by relying on one vendor's live session memory.

::: notes
Keep this concrete but not implementation-heavy. The audience should understand that the proposal is to test the minimum state: identity, relationships, traces, lifecycle, and handoffs. AIDA embodies one version of that substrate, but the research question is larger than AIDA.

Timing: 1 minute.
:::

---

# What We Already Know

## Early findings from the probe

| Finding | Evidence so far | Interpretation |
|---|---|---|
| Typed graph pays selectively | Relational queries ~80x faster at 500 specs; stable IDs prevent rename reference rot | Use graph where relationships matter; grep still wins for text |
| Cross-vendor onboarding can be near-zero | Codex joined a running AIDA-style fleet via stock CLI in ~50s, no bespoke integration | Promising, but n=1 vendor/task |
| Multi-vendor competition is QA | Vendors converged on design; quality varied on edge cases | Compete to catch variance, not to expect design diversity |
| Gates need discipline | Controlled ablations hit 100% rule-following; gates were idle | Gate high-risk field conditions, not every rule |

::: notes
This slide makes the presentation feel empirical rather than speculative. Be careful with scope: these are pilots and design-science evidence, not broad statistical proof. The credibility comes from naming the limits.

Timing: 1.5 minutes.
:::

---

# Research Plan

## Measure coherence, calibration, escalation, and ownership

1. **Adoption ladder**  
   Solo -> inter-team -> multi-program -> disconnected/offline

2. **Vendor-neutrality tests**  
   Add a new agent vendor; swap a provider mid-program; export/re-import the record

3. **Coherence metrics**  
   Does the fleet stay aligned as agents, sessions, and tools multiply?

4. **Governance metrics**  
   Can we reconstruct authority, decisions, trace links, and failures from the record alone?

5. **Production metrics**  
   Retry loops, resume success, per-agent cost, and trace completeness

6. **Build-vs-buy decision**  
   Does owning the substrate beat lighter alternatives enough to justify the tax?

::: notes
This is where you connect to the research proposal's proof points. Emphasize measurable proof points over roadmap promises. The proposal is attractive because it is falsifiable: if the substrate does not surface more reuse, reduce rework, or preserve optionality, do not scale it.

Timing: 1 minute.
:::

---

# Recommended Pilot

## One-quarter, decision-grade experiment

Compare three matched arms:

- **S: Substrate-backed**  
  Shared typed graph, traces, lifecycle, coordination record

- **W: Write-it-down**  
  Shared unstructured docs/wiki; no typed graph

- **C: Control**  
  Existing per-team tools and vendor-native coordination

Primary metrics:

- Synergy events per team-month
- Median lead time before impact
- Rework-hours avoided
- False-positive rate from graph-surfaced candidates

::: notes
This is the executive close: measure before betting. The W arm is important because it separates "the graph helped" from "writing anything down helped." The recommended pilot is not org-wide adoption; it is a cheap falsification attempt.

Timing: 1.25 minutes.
:::

---

# Decision Frame

## Own the record; ride the runtime

**Ride native** where vendor tools are strongest:

- model execution
- within-session teams
- subagents
- IDE/cloud ergonomics

**Own or preserve** what compounds:

- intent graph
- stable IDs
- traceability
- lifecycle history
- cross-vendor coordination record

**Decision rule:** proceed only if the substrate surfaces materially earlier cross-team reuse, dependencies, or conflicts than the control arms.

::: notes
End with a clear, pragmatic stance. The proposal is not anti-vendor. It says use vendor runtimes aggressively, but do not let them become the system of record for program intent and coordination if multi-vendor, long-lived work matters.

Timing: 45 seconds.
:::

---
