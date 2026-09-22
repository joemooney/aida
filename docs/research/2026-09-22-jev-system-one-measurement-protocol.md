# Objective Measurement & Causal Evaluation Protocol for System One (Jev) in AIDA

**Date:** 2026-09-22  
**Spec References:** SPIKE-87, ADR-55, STORY-1424, STORY-1426, STORY-1427, PRIN-5, PRIN-6, PRIN-7, PRIN-8  
**Author:** Antigravity (AI) & Codex (AI) in collaboration with Operator  

---

## 1. Deconstructing the Evaluation into Three Distinct Questions

Previous evaluation outlines conflated statistical prediction, workflow velocity, and operational cost into single aggregate scores. A scientifically rigorous evaluation must decouple these into three distinct research questions with separate estimands, datasets, and experimental regimes:

```
┌───────────────────────────────────────────────────────────────────────────────────┐
│ 1. MODEL PREDICTION: Is Jev's judgment accurate and calibrated on bounded tasks?   │
│    (Evaluated via independently adjudicated offline corpora)                      │
├───────────────────────────────────────────────────────────────────────────────────┤
│ 2. WORKFLOW CAUSALITY: Does a specific Jev intervention improve an AIDA workflow? │
│    (Evaluated via single-intervention, randomized, stratified live trials)        │
├───────────────────────────────────────────────────────────────────────────────────┤
│ 3. OPERATIONAL TRADEOFF: Is the workflow gain worth latency, privacy, and cost?    │
│    (Evaluated via shadow mode telemetry, fault injection, and billing audits)     │
└───────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Core Methodological Invariants

### 2.1 Independent Adjudication Over Historical Consensus
* **Problem:** Historical review records (`.aida/review-verdicts/*.json`) and spec metadata reflect the subjective biases, scraping errors, and heuristics of past human/LLM reviewers. Treating them as unimpeachable ground truth risks penalizing correct Jev predictions or rewarding shared biases.
* **Invariant:** Offline evaluation must use an **independently adjudicated benchmark sample** stratified by task type, size, and complexity. Borderline cases and all negative/positive controls must be adjudicated by human consensus with documented rubrics.

### 2.2 Strict Decoupling of Experimental Regimes
* **Historical Replay:** Strictly limited to evaluating predictive capability (class recall, calibration curves, hypothetical context selection). It **cannot** claim reductions in drain turnaround, stall frequency, or financial savings.
* **Passive Shadow Mode:** Measures real-world operational health: network latency distributions under load, timeout frequency, schema parse failures, API availability, and secret redaction verification. Shadow mode never alters workflow state.
* **Randomized Live A/B Trials:** Required for any claim of workflow velocity, ping-pong reduction, or drain throughput.

### 2.3 Single-Intervention Rollout (Unbundling Interventions)
* **Problem:** Bundling context pruning, duplicate spec hints, and finding clustering into an autonomous batch creates confounding, preventing causal attribution.
* **Invariant:** Interventions must be evaluated in an isolated, staged progression. We begin with **Experiment 1 (Review-Context Selection)** in isolation before introducing subsequent interventions.

### 2.4 Safety Gates vs. Statistical Claims
* **Problem:** Stating "False Approval Rate = 0.0%" as a statistical fact from a finite sample is mathematically invalid.
* **Invariant:** Safety is enforced as a **deterministic release gate**:
  1. An independently adjudicated safety corpus ($N \ge 100$) where observed false approvals must be zero, reporting the upper one-sided 95% Clopper-Pearson confidence bound ($p < 0.03$).
  2. A **Fault-Injection Test Suite** verifying that Jev fails closed under injected network partitions, timeouts, HTTP 500s, malformed JSON schemas, missing credentials, context cancellations, and stale commit SHAs (PRIN-5).

### 2.5 Phase-Level Timing Isolation
* **Problem:** Measuring end-to-end drain turnaround ($T_{\text{merged}} - T_{\text{pickup}}$) is heavily confounded by queue wait times, CI runner contention, git merge-locks, and external network latency.
* **Invariant:** Workflow evaluations must isolate **Phase 3 Review Time** ($T_{\text{verdict}} - T_{\text{review\_spawn}}$), reviewer idle time, and human intervention time.

### 2.6 Full Baseline Prompt Denominator & Mandatory Rule Recall
* **Problem:** Defining context compression simply as "tokens passed to Jev vs all rules" rewards dangerous omissions.
* **Invariant:**
  * **Compression Denominator:** Must encompass the complete baseline prompt: system prompt, ambient discipline rules, full spec hierarchies, complete PR diff, prior round findings, and retry prompts.
  * **Mandatory Rule Recall (Safety Constraint):** The primary safety gate for context pruning is **Recall of Mandatory Acceptance Criteria and Governing Rules** ($\ge 99.5\%$). A prompt compression that drops an essential invariant is an unacceptable failure regardless of token savings.

### 2.7 Telemetry & Cost Grounding
* In accordance with AIDA’s cost-observability research (`docs/spikes/2026-09-19-cost-observability.md`), raw token counts from autonomous drain logs can omit message joins or record zero-token artifacts.
* All financial cost claims must be explicitly labeled as **estimated model pricing projections** until reconciled against real invoice billing telemetry.

---

## 3. Initial Isolated Study: Review-Context Selection (Experiment 1)

To establish rigorous causal proof without risking repository stability, we formulate the initial empirical trial around a single, low-risk, high-leverage application: **Review-Context Selection**.

```
                   PR Diff + Candidate Guidelines
                                 │
                                 ▼
                   ┌───────────────────────────┐
                   │ Rung 3: AST/Diff Pre-Filter│ (Deterministic extraction)
                   └─────────────┬─────────────┘
                                 │ Candidate set
                                 ▼
                   ┌───────────────────────────┐
                   │ Jev Scoring (2–5s cutoff) │ (Ranks relevance to diff)
                   └─────────────┬─────────────┘
                                 │
                 ┌───────────────┴───────────────┐
                 ▼                               ▼
       Pass Selection (Top K)          Timeout / Low Confidence
                 │                               │
                 ▼                               ▼
        Pruned Reviewer Prompt          Full Baseline Prompt
                 │                      (Safe Fallback)
                 ▼                               │
        ┌────────────────────────────────────────┴───────┐
        │ Conversational Reviewer (Claude / Codex Seat)  │
        └────────────────────────────────────────────────┘
```

### 3.1 Causal Hypotheses & Estimands
* **Primary Workflow Outcome:** Phase 3 Reviewer Duration ($T_{\text{review}}$).  
  *Hypothesis:* Pruning ambient rules and non-relevant criteria reduces reviewer deliberation time by $\ge 20\%$ with a 95% confidence interval excluding zero.
* **Primary Safety Gate:** Mandatory Criterion Recall ($R_{\text{mandatory}}$).  
  *Requirement:* $R_{\text{mandatory}} \ge 99.5\%$ on adjudicated acceptance criteria. Zero omission of security or merge-hold rules.
* **Secondary Efficiency Outcome:** Reviewer Prompt Token Volume ($V_{\text{tokens}}$).  
  *Hypothesis:* Reduces input tokens passed to the reviewer seat by $\ge 35\%$.

### 3.2 Trial Design & Randomization
* **Randomization Unit:** Individual PR review invocations.
* **Stratification:** Stratified by:
  1. Diff size (Small: $< 50$ lines, Medium: $50–300$ lines, Large: $> 300$ lines).
  2. Spec type (`BUG`, `TASK`, `STORY`, `REF`).
* **Treatment Arm (Jev Context Selection):** Deterministic extraction of touched AST nodes and modified files $\rightarrow$ Jev ranks candidate criteria and ambient rules $\rightarrow$ Reviewer prompt includes top relevant rules + all mandatory criteria.
* **Control Arm (Status Quo):** Reviewer receives full ambient project guidelines and unpruned requirement text.
* **Statistical Power:** Powered to detect a 15% difference in review duration at $\alpha = 0.05$ and $\beta = 0.80$ (estimated sample size $N \approx 80$ stratified PR reviews).

### 3.3 Pre-Registered Stop Conditions
The automated trial must immediately halt and revert to the control baseline if:
1. **Safety Violation:** Any mandatory acceptance criterion or project invariant is omitted from the pruned prompt in an adjudicated run.
2. **Reviewer Quality Regression:** Any statistically significant increase in post-merge bug filings or review-finding disagreements.
3. **Availability / Latency Failure:** Jev API error or timeout rate exceeds $5.0\%$ over a rolling 20-call window.

### 3.4 Deadline Policy: Use-Case Specific Deadlines, Not a Global 400ms SLA
A global 400 ms SLA is an unrealistic benchmark ambition that does not reflect AIDA’s actual operational architecture. AIDA's conversational reviewer seats take 30–90 seconds and CI builds take 10–15 minutes. 

The governing deadline policy is:
> **Each Jev use case declares its own deadline. Advisory interactive features must never delay the primary operation; on deadline expiry they use the deterministic/full-context fallback. Latency is measured and reported by percentile, but is not treated as a universal AIDA requirement.**

* **`aida add` (Interactive Capture):** Non-blocking. The requirement YAML is saved first. Advisory duplicate or classification checks run asynchronously or with a short cutoff; on timeout they are skipped silently without stalling developer capture.
* **Review-Context Selection:** Precedes a 30–90 second Claude/Codex review session. A provisional 2–5 second deadline is completely acceptable. On expiry, it silently falls back to the full baseline prompt.
* **Queue & Task Routing:** 2–5 seconds is acceptable for batch dispatch; on timeout it defaults to the standard lane.
* **Shadow Telemetry:** Background execution; no user-facing deadline needed.
* **Safety-Sensitive Gates:** Never compromise safety to chase latency; fail closed or escalate.

---

## 4. Fault-Injection & Fail-Closed Test Suite

Per PRIN-5 and ADR-55, Jev integration must be verified against simulated failure modes:

| Test Case | Injected Fault | Expected Behavior |
| :--- | :--- | :--- |
| `test_fault_injection_timeout` | Simulated API stall exceeding deadline (e.g. 5s) | Aborts at use-case deadline; cleanly falls back to unpruned baseline prompt. |
| `test_fault_injection_http_500` | HTTP 500 Internal Server Error | Fails closed; routes to standard reviewer seat with error logged. |
| `test_fault_injection_malformed_json` | Truncated / invalid JSON response | Fails closed; does not parse partial payload. |
| `test_fault_injection_missing_key` | Unset API credentials | Immediate local fallback to `MockEvaluator` / local baseline; no network traffic. |
| `test_fault_injection_stale_sha` | Head commit advances during call | Invalidates Jev recommendation; re-evaluates against new SHA (PRIN-6). |
| `test_fault_injection_secret_leak` | API token injected into mock diff | Sanitization filter redacts token before payload construction. |

---

## 5. Summary


By replacing bundled claims with **isolated estimands, stratified independent adjudication, phase-level timing metrics, and pre-registered safety stop conditions**, AIDA can evaluate Jev with scientific credibility. The immediate priority is executing the isolated **Review-Context Selection trial** under the fault-tolerant safeguards defined above.
