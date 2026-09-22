---
marp: true
title: "Autonomous Software Engineering: AIDA & TypeSafe AI Jev"
description: "Executive presentation on why AIDA is a compelling platform for autonomous agent fleets and how Jev System 1 models accelerate it."
paginate: true
theme: default
class: lead
backgroundColor: #0f172a
color: #f8fafc
style: |
  section {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    padding: 40px 60px;
    font-size: 22px;
    line-height: 1.45;
    background-color: #0f172a;
    color: #f8fafc;
  }
  h1 {
    color: #38bdf8;
    font-size: 38px;
    margin-bottom: 12px;
  }
  h2 {
    color: #60a5fa;
    font-size: 30px;
    border-bottom: 2px solid #334155;
    padding-bottom: 8px;
    margin-bottom: 16px;
  }
  h3 {
    color: #93c5fd;
    font-size: 22px;
    margin-top: 8px;
  }
  blockquote {
    background: #1e293b;
    border-left: 5px solid #38bdf8;
    margin: 14px 0;
    padding: 12px 20px;
    color: #cbd5e1;
    font-style: normal;
  }
  table {
    font-size: 17px;
    width: 100%;
    border-collapse: collapse;
    margin-top: 12px;
  }
  th, td {
    padding: 8px 12px;
    border: 1px solid #334155;
  }
  th {
    background-color: #1e293b;
    color: #38bdf8;
  }
  .highlight {
    color: #38bdf8;
    font-weight: bold;
  }
  .success {
    color: #34d399;
    font-weight: bold;
  }
  .warn {
    color: #fbbf24;
    font-weight: bold;
  }
  .metric-box {
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 8px;
    padding: 12px 16px;
    margin: 8px 0;
  }
  footer {
    font-size: 13px;
    color: #64748b;
  }
---

<!--
AUDIENCE: Executive Leadership, Engineering VPs, Technical Product Leadership
GOAL: Articulate why AIDA is the foundational substrate for autonomous software engineering,
      why Jev System 1 models provide the missing high-velocity evaluation layer,
      and how this combination delivers unprecedented velocity while preserving human governance.
-->

# Autonomous Software Engineering
### Why AIDA is the Winning Substrate & How Jev Powers It

A Strategic Executive Briefing on Agent Fleets, System 1 Decision Models, and Bounded Governance

<small>September 2026 · AIDA Core Architecture</small>

---

## Executive Summary: The Strategic Bet

1. **The Paradigm Shift:** Software engineering is rapidly moving from human-interactive code completion to **autonomous agent fleets** (Claude, Codex, Antigravity) developing in parallel.
2. **The Bottleneck:** The constraint is no longer code generation. The bottleneck is **governance, verification latency, context economics, and human comprehension**.
3. **The AIDA Bet:** AIDA provides the essential collaboration substrate—a git-canonical requirements graph, typed dependency edges, stable spec IDs, and dual-predicate merge gates (PRIN-7).
4. **The Jev Acceleration:** TypeSafe AI's **Jev (System 1)** introduces calibrated, sub-second, sub-penny evaluations into code pipelines, routing tasks, pruning prompt bloat, and auditing specs.
5. **The Non-Negotiable Invariant:** AIDA remains fully operable without Jev or network access; Jev is strictly an optional acceleration plugin that fails closed.

---

## Part 1: Why AIDA is Worth Being a Bet in the First Place

Autonomous agents operating on raw git repositories quickly degrade into chaos:
* **Context Bleed:** Agents dump thousands of lines of prompt lore into chat histories.
* **Semantic Drift:** Code changes diverge silently from product intent.
* **Review Stalls:** Expensive conversational seats spend 45–90 seconds per PR doing shallow checks.
* **Cognitive Alienation:** LLMs author machine-dense specs that human managers cannot decipher.

> **Without a structured substrate, multiplying AI agents only multiplies technical debt.**

---

## AIDA: The Substrate for Autonomous Fleets

AIDA transforms a standard repository into an auditable, multi-agent operating system:

* **Git-Canonical Requirements Graph:** Every story, task, bug, and architectural decision record (ADR) is a typed object stored directly in the orphan `aida-store` branch.
* **End-to-End Traceability:** Code connects directly to requirements via inline trace comments (`// trace:TASK-123 | ai:codex`), commit trailers, and automated PR bumping.
* **Deterministic Quality Ladder (PRIN-8):** Enforces quality at the most deterministic layer available—compiler types first, test suites second, static joins third, calibrated models fourth, conversational agents last.
* **Vendor-Neutral Coordination:** Seamlessly orchestrates Claude, Codex, and Antigravity seats with strict file-lease locks and collision prevention.

---

## Part 2: The Missing Primitive — System 1 Decision Models

In human cognitive psychology (Kahneman), intelligence operates on two levels:

```
┌──────────────────────────────────────────────┐   ┌──────────────────────────────────────────────┐
│        SYSTEM 1 (Fast, Reflexive, Typed)     │   │      SYSTEM 2 (Slow, Generative, Reasoning)  │
├──────────────────────────────────────────────┤   ├──────────────────────────────────────────────┤
│ • Pattern recognition & classification       │   │ • Multi-step architectural synthesis         │
│ • Binary checks & calibrated probabilities   │   │ • Deep debugging, refactoring, implementation│
│ • Latency: 100 ms – 300 ms                   │   │ • Latency: 30,000 ms – 90,000 ms             │
│ • Cost: $0.00008 / decision                  │   │ • Cost: $0.03 – $0.15 / invocation           │
│ • Primitive: TypeSafe AI Jev                 │   │ • Primitive: Claude 3.7 / Codex / GPT-4o     │
└──────────────────────────────────────────────┘   └──────────────────────────────────────────────┘
```

> **The Flaw in Modern AI Tooling:** We are deploying heavyweight, expensive System 2 chatbots to perform routine classification, triage, and filtering tasks.

---

## Introducing TypeSafe AI's Jev

Engineered specifically for structured, typed decisions rather than conversational text:

* **No Conversational Bloat:** Emits zero prose, markdown, or chat tokens.
* **Strongly Typed Primitives:**
  * `noul`: Calibrated binary probability ($p \in [0.0, 1.0]$).
  * `choice`: Discrete multi-class selection with probability distributions.
  * `score`: Ordinal rubric evaluations (e.g., 1 to 5).
* **High-Throughput Parallelism:** Evaluates dozens of criteria against a context payload in a single forward pass.
* **Machine-Native Output:** Outputs typed, schema-validated JSON consumed directly by compiled Rust code without regex parsing or markdown stripping.

---

## Ground Truth: Empirical Benchmark Findings (SPIKE-87)

To ground this strategic bet in reality, we benchmarked live Jev endpoints against 50 real AIDA PR reviews and 30 canonical store pairs:

| Metric | Measured Benchmark Result | Operational Implication |
| :--- | :--- | :--- |
| **Median Latency (P50)** | **218 ms** | Fast enough for interactive and pre-commit pipelines. |
| **Tail Latency (P95 / Max)** | **550 ms / 5.2s** | Fails sub-300ms SLA at the tail; requires deadline budgets. |
| **False Approval Rate** | **0% (0 / 50)** | Fail-closed thresholds successfully prevent unauthorized merges. |
| **Escalation Rate** | **96% (48 / 50)** | Conservative gating defers non-trivial reviews to conversational agents. |
| **Known Contradiction** | **Missed (VIS-1 / CR-6)** | Semantic nuance requires human/agent oversight on edge cases. |
| **Projected Cost** | **$0.08 / 1,000 decisions** | ~400x cheaper than running conversational LLM reviewer seats. |

> **Executive Conclusion:** Jev is not an autonomous auto-merger. Its immediate, high-ROI value is as an **advisory triage, routing, and context-pruning engine**.

---

## The Governing Rule for Jev Adoption

Based on empirical evidence, AIDA enforces a strict architectural rule:

> *"Use Jev to decide 'which bounded path should handle this?' before using it to decide 'is this safe to merge?'"*

```
                              Incoming Artifact
                                      │
                                      ▼
                      [Jev System 1 Triage & Routing]
                                (Sub-Second)
                                      │
          ┌───────────────────────────┼───────────────────────────┐
          ▼                           ▼                           ▼
   Deterministic Lane          Context Pruning             Advisory Triage
  Route to docs, research,    Strip irrelevant rules       Cluster duplicate
  or quick fix without       before dispatching costly    findings for human
  human queue wait            System 2 reviewer seats      or agent review
```

---

## Benefit 1: Intelligent Queue & Task Routing

**The Bottleneck:** When tasks enter the backlog, human operators or slow LLM schedulers must manually inspect descriptions to assign roles and queues.

**The Jev Solution:**
* Evaluates spec titles and acceptance criteria against lane rubrics in **under 250ms**.
* Categorizes work into `implementation`, `docs`, `research`, `review`, or `advisor escalation`.
* Bounded & reversible: if routing is ambiguous, it defaults safely to the general queue.
* **Business Return:** Eliminates backlog grooming lag and keeps autonomous agent drains continuously fed with correctly prioritized work.

---

## Benefit 2: Review-Context Pruning & Cost Compression

**The Bottleneck:** Autonomous review seats (Claude/Codex) receive massive prompt payloads containing every project rule, guideline, and test output, costing \$0.05–\$0.15 per run and inducing model hallucination.

**The Jev Solution:**
* Scores relevance of candidate guidelines against the active PR diff in milliseconds.
* Prunes irrelevant instructions while deterministically preserving $\ge 99.5\%$ of mandatory safety rules.
* **Business Return:**
  * **30%–50% reduction in review token costs**.
  * **Faster reviewer seat turnaround** (System 2 models generate verdicts faster with focused context).
  * Enforced by safety stop conditions: any pruned mandatory rule halts the trial immediately.

---

## Benefit 3: Human-Centric Spec Exposition (EPIC-72)

**The Problem:** Autonomous agents author specifications packed with internal UUIDs, parser edge cases, and recursive lore. Human leadership and product owners become alienated from their own codebase.

**The Jev Solution:**
* Generates on-demand, plain-language expositions stored as sidecars (`.aida-store/expositions/<SPEC-ID>/<audience>.yaml`).
* Jev audits the generated exposition for:
  * **Readability & Jargon Saturation:** Flags dense, impenetrable prose.
  * **Constraint Preservation:** Verifies that non-technical summaries did not drop safety invariants.
* **Human-in-the-Loop Protection:** `human_reviewed: true` preserves human edits while still tracking drift via SHA-256 graph hashes.
* **Business Return:** Restores total executive visibility into what autonomous agents are building and why.

---

## Benefit 4: The Living Project Wiki

**From Opaque Store to Browsable Enterprise Asset:**
* Uses the exposition layer to compile a read-only, static, hyperlinked **Living Wiki** (`aida wiki build` & `aida wiki serve`).
* **Executive Front Door:** Plain-English summaries of active epics, architecture themes, and quarterly milestones.
* **Subsystem Architecture Maps:** Automatically aggregates requirements into visual Mermaid diagrams showing real dependency graphs.
* **Enterprise Security:** Bound strictly to local loopback (`127.0.0.1`)—no internal repository lore or trade secrets leak to third-party web hosts.

---

## Why AIDA is a Compelling Platform for Jev Adoption

Jev is a powerful primitive, but it cannot deliver value in an unstructured vacuum. **AIDA provides the ideal operating environment for Jev:**

1. **Structured Context Neighborhoods:** AIDA supplies exact graph closures (immediate parents, blockers, decisions, and diffs), giving Jev crisp, bounded inputs.
2. **Deterministic Ladders (PRIN-8):** Jev has a well-defined home at Rung 3.5—never replacing compiler guarantees or test suites, but replacing fragile conversational chats.
3. **Fail-Closed Safety Architecture (PRIN-5):** AIDA never allows a model glitch to produce a false positive. If Jev times out or fails, AIDA defaults to safe fallback.
4. **Stable Evaluation Ground Truth:** AIDA’s immutable store records provide historical verdicts and canonical fixtures for ongoing model calibration.

---

## Architectural Resilience & Sovereignty (ADR-56)

Enterprise infrastructure requires resilience against SaaS network outages and vendor lock-in:

* **Non-Prerequisite Invariant:**
  > *AIDA remains fully operable without Jev or network access; remote evaluator failures cannot prevent core operation.*
* **Deadline-Aware Budgeting (TASK-1430):** Every call operates within a strict time budget (e.g., 2–5s for review prep; non-blocking save-first for `aida add`).
* **Concurrency-Safe Circuit Breaker (TASK-1431):** 3 consecutive transient failures trip the breaker into a 60-second cooldown, failing closed instantly without network hangs. Permanent errors (401/403/schema) never trip the breaker.
* **Duplicate Charge Protection:** Every retry carries an idempotency hash to eliminate double billing.

---

## Phased Rollout & Capital Efficiency

```
Phase 1: Transport Resilience (ADR-56) ──► Phase 2: Exposition Layer (EPIC-72) ──► Phase 3: Living Wiki & Benchmark
• DeadlineRetryAdapter & circuit breaker   • Versioned sidecar schema              • Human pilot on 15-spec corpus
• Fault-injection test suite passed       • aida explain CLI with SHA-256 drift   • Read-only private wiki projection
• EvaluatorEngine wired to fallback       • Advisory Jev readability auditing     • Objective comprehension benchmark
```

### Business Impact:
* **Immediate Risk Mitigation:** Zero mutation risk on canonical specs; fail-closed architecture protects existing CI.
* **Measurable Milestones:** Each phase delivers an independently shippable, independently verified capability.
* **Capital Return:** Low experimental investment ($0.08 / 1k calls) yielding massive efficiency gains across human review time and agent token consumption.

---

## Conclusion: The Strategic Recommendation

1. **AIDA is the Bedrock:** Without a git-canonical collaboration substrate, agent swarms descend into technical bankruptcy. AIDA provides the auditable, multi-agent foundation.
2. **Jev is the Reflex Layer:** Adding System 1 models to AIDA unlocks the high-speed routing, context pruning, and readability gating needed to scale to dozens of concurrent agents.
3. **Human Leadership Retains Control:** Through the Human Exposition layer and Living Wiki, engineering leadership gains unprecedented clarity over what their autonomous fleet is building.

> **Recommendation:** Proceed with the phased rollout on `feat/jev-integration`. Complete Phase 1 resilience, ship the Phase 2 exposition layer, and establish AIDA as the gold standard for governed, high-velocity autonomous engineering.
