# Expanding System One (Jev) Research: Lower-Risk, Reversible Applications in AIDA

**Date:** 2026-09-22  
**Spec References:** SPIKE-87, ADR-55, STORY-1424, STORY-1426, PRIN-5, PRIN-6, PRIN-7, PRIN-8  
**Author:** Antigravity (AI) & Codex (AI) in collaboration with Operator  

---

## 1. Executive Context: Shifting from Authority to Triage

The initial exploration of TypeSafe AI’s Jev focused on three high-stakes, ambitious use cases:
1. **Graded PR review & fast-pass auto-merge** (STORY-1424)
2. **Autonomous store-wide contradiction gating** (STORY-1426)
3. **Interactive intake quality-gate enforcement** (STORY-1427)

As established in the empirical SPIKE-87 benchmark (`docs/spikes/2026-09-22-spike-87-jev-system-one-benchmark.md`), granting Jev unreviewed merge authority or sole responsibility for store-wide semantic integrity is premature:
* **96% Escalation Rate:** Under conservative, safe thresholds ($p \ge 0.95$ approve, $p \le 0.20$ reject), only 4% of historical review records were decided, with the remaining 96% rightfully deferred to conversational reviewers.
* **Latency SLA Miss:** While median latency was fast (218 ms), P95 latency reached 550 ms (failing the <300 ms SLA) with tail outliers up to 5.2 seconds.
* **Positive Control Failure:** In the live benchmark run, Jev failed to detect the primary known semantic contradiction between `VIS-1` and `CR-6`.

Granting a System 1 classifier terminal authority over irreversible actions (like git merges or requirement deletions) exposes the system to catastrophic false-approval risks. 

### The Core Design Rule
> **Use Jev to decide *"which bounded path should handle this?"* before using it to decide *"is this safe to merge?"***

System 1 models excel at **bounded categorization, ranking, and context filtering** where:
* The action is advisory, reversible, or filtered.
* Mistakes cleanly fall back to human inspection or conversational agent review.
* Deterministic checks (Rungs 1–3) pre-filter the problem space to minimize AI invocations.

---

## 2. Landscape of Lower-Risk, High-Leverage Application Areas

The following matrix identifies 12 candidate application areas in AIDA where Jev's fast, typed classification primitives (`choice`, `noul`, `score`) provide immediate leverage without requiring merge or store mutation authority.

| AIDA Subsystem / Area | Proposed Jev Role | Mechanism & Primitives | Why It Fits Better Than Terminal Gating |
| :--- | :--- | :--- | :--- |
| **1. Queue & Task Routing** | Classify a spec into implementation, docs, research, review, or advisor escalation. | `choice` over canonical roles: `implementer`, `reviewer`, `advisor`, `researcher`, `docs`, `escalate`. | Bounded classification; mistakes fall back to human operator routing or lane default. |
| **2. Requirement Metadata Hygiene** | Suggest requirement type, tags, feature group, or priority for unformatted drafts. | Multi-question `choice` & `score` on spec description. | Advises the operator or agent during grooming; never mutates canonical YAML without human confirmation. |
| **3. Duplicate / Related-Spec Detection** | Rank whether a incoming draft duplicates, supersedes, or extends an existing spec. | `choice` (`duplicate`, `extends`, `unrelated`, `supersedes`) over mechanically pre-filtered candidates. | Semantic similarity aids deduplication before specs enter the store; zero risk of false store mutation. |
| **4. Change-Impact Analysis** | Identify which specs, plans, documentation, or test suites are likely affected by a git diff. | `noul` query against candidate specs retrieved via AST/graph dependencies. | Functions as a retrieval and ranking aid for the implementer or reviewer, not an authority. |
| **5. Review-Finding Triage** | Cluster repeated findings across review rounds and identify likely duplicates or rephrasings. | `choice` (`duplicate_prior`, `new_issue`, `fixed`) comparing new finding text against past round notes. | Directly attacks review-round churn ("one finding per round" loops) without altering verdicts. |
| **6. Finding Severity Classification** | Categorize review findings as `blocker`, `correctness`, `maintainability`, or `advisory`. | `score` / `choice` with defined rubric definitions. | Standardizes finding terminology; human reviewer or conversational agent retains final say. |
| **7. Review Prompt & Context Selection** | Select the most relevant acceptance criteria, prior findings, and related specs for a reviewer prompt. | `score` ranking relevance of ambient project rules to the current PR diff. | Context compression: saves expensive context tokens in Claude/Codex seats without losing critical rules. |
| **8. Traceability Assistance** | Detect likely missing or misplaced `// trace:<SPEC-ID>` comments in modified AST nodes. | `noul` verifying whether a modified function conceptually realizes the given spec ID. | Narrow proposition with immediate deterministic follow-up (file grep / diff check). |
| **9. Stale-Document Detection** | Judge whether `AGENTS.md`, `OVERVIEW.md`, or architecture guides still reflect current source code. | Pairwise `choice` (`fresh`, `stale_terminology`, `contradicted`) on doc sections vs recent commits. | Escalates candidates for human or docs-lane review; prevents silent architectural documentation rot. |
| **10. Session / Punt Classification** | Classify why an autonomous agent session was shelved or punted, and suggest the next lane. | `choice` over punt taxonomy (`environment_error`, `ambiguous_spec`, `ci_failure`, `agent_loop`). | Yields high-fidelity fleet analytics and rework analysis without modifying runtime state. |
| **11. Reconstitution Comparison** | Compare regenerated test suites and behavioral descriptions against ground truth intent. | Calibrated `choice` (`matched`, `partial`, `divergent`) on test semantics (ADR-44). | Replaces expensive conversational LLM scoring passes with low-cost, structured comparison. |
| **12. Operator-Facing Explanations** | Select a concise, human-friendly explanation category for a complex deterministic failure. | `choice` among predefined curated explanation templates based on error logs. | Jev selects among predefined, safe explanations; it is strictly barred from inventing unverified policy. |

---

## 3. Top Four Prioritized Near-Term Experiments

To establish defensible utility before expanding Jev's operational footprint, we prioritize four candidate experiments characterized by high operational value, low catastrophic risk, and measurable baselines.

```
                     ┌──────────────────────────────────────────────┐
                     │ 4 High-Leverage Near-Term Experiments        │
                     └──────────────────────┬───────────────────────┘
                                            │
        ┌───────────────────┬───────────────┴───────────────┬───────────────────┐
        ▼                   ▼                               ▼                   ▼
 1. Review Context   2. Duplicate Spec               3. Finding Triage   4. Queue Routing
    Selection           Detection                       & Clustering        & Lane Triage
 • Selects key rules • Prevents graph bloat          • Halts 1-finding   • Dispatches to right
 • Compresses prompt • Advisory merge hint             review loops        agent lane
 • Lowers token cost • Zero false deletions          • Classifies sever. • Safe fallback
```

### Experiment 1: Review Prompt & Context Selection
* **Problem:** Conversational reviewers (Claude Code, Codex) suffer from context bloat. Stuffing entire project rules, full spec hierarchies, and past review history degrades judgment and causes timeout stalls.
* **Jev Role:** Pre-scan the PR diff and candidate guidelines to select the top $K$ relevant acceptance criteria and past review findings.
* **Why It Wins:** Even an imperfect selection provides a better reviewer prompt than raw truncation or massive prompt dumps. Conversational reviewers retain full reasoning capacity.

### Experiment 2: Duplicate & Related-Spec Detection
* **Problem:** Human operators and autonomous agents frequently file duplicate bugs or overlapping tasks because searching hundreds of YAML specs via exact keywords misses synonyms or architectural overlap.
* **Jev Role:** When `aida add` runs, mechanically retrieve top 5 semantic candidates (via trigram / SQLite FTS) and invoke Jev `choice` (`duplicate`, `extends`, `unrelated`).
* **Why It Wins:** Advisory only. It outputs a helpful hint: *"Did you mean to extend TASK-412?"* If wrong, the operator simply ignores it. No specs are deleted or blocked.

### Experiment 3: Review-Finding Clustering & Triage
* **Problem:** Autonomous drains often enter 3-to-5 round review loops where the reviewer raises one superficial finding per round, or re-raises a concern the implementer already addressed.
* **Jev Role:** Compare new reviewer findings against historical findings from prior rounds on the same PR:
  - Cluster duplicates.
  - Classify severity (`blocker` vs `maintainability`).
  - Drop or downrank already-acknowledged advisory points.
* **Why It Wins:** Directly attacks the review tail without giving Jev authority to approve PRs.

### Experiment 4: Queue & Task Lane Routing
* **Problem:** `aida queue work` dispatches tasks to agents. Certain tasks require specialized tools (e.g. `docs`, `research`, `spike`, `keystone`), but operators rarely set explicit lanes on creation.
* **Jev Role:** Classify pending requirements into execution lanes (`implementer`, `researcher`, `docs`, `needs_human_refinement`).
* **Why It Wins:** Explicit `unknown/escalate` class. Misclassified tasks simply get reassigned or handled in the general implementer lane.

---

## 4. Rigorous Experimental Evaluation Framework

Each candidate experiment must be validated against an explicit evaluation protocol before merging into mainline toolchains.

```
       Input Space
            │
            ▼
┌───────────────────────┐
│ Rung 3 Deterministic  │ ──Excluded (Zero AI Cost)──► Bypass
│ Mechanical Pre-Filter │
└───────────┬───────────┘
            │ Candidate Subset
            ▼
┌───────────────────────┐
│ Jev System 1 Evaluate │
└───────────┬───────────┘
            │
    ┌───────┴───────┐
    ▼               ▼
High Confidence    Low Confidence / Ambiguous
    │               │
    ▼               ▼
[Advisory Action] [Abstain / Escalate to Human/Agent]
```

For each experiment, the benchmark harness must measure and report:

1. **Coverage & Abstention Rate:**
   * What percentage of inputs does the model confidently classify vs. route to `unknown` / `escalate`?
   * *Target:* $\ge 70\%$ coverage on routine cases, $\le 30\%$ abstention.
2. **Precision and Recall by Class:**
   * Per-class breakdown (e.g. for Finding Severity: precision and recall across `blocker`, `correctness`, `maintainability`, `advisory`).
3. **Calibration & Threshold Performance:**
   * Compute empirical calibration metrics: Brier score, Expected Calibration Error (ECE), and reliability diagrams across probability deciles.
4. **Asymmetric Cost of False Positives vs. False Negatives:**
   * Example (Duplicate Detection): A false negative (missing a duplicate) costs minor graph clutter; a false positive (falsely claiming duplication) risks discarding legitimate work. Thresholds must be tuned to the lower-cost error direction.
5. **Latency & Tail Metrics:**
   * Measure P50, P90, P95, and Max latency. Any task intended for interactive CLI use (e.g. `aida add`) must enforce a hard timeout of $\le 500\text{ ms}$ with graceful fallback.
6. **Deterministic Pre-Filtering Efficiency:**
   * Measure how effectively mechanical heuristics (keyword matches, diff paths, author filters) reduce the candidate volume *before* Jev is invoked, maximizing token economy and bounding latency.

---

## 5. Governance Alignment (PRIN-5 through PRIN-8)

Every new application of Jev must strictly conform to AIDA's governing principles:

* **PRIN-5 (Fail-Closed):** Any API unavailability, network error, or low-confidence score must fail closed to human advisory or standard fallback paths. Never default to an affirmative action on error.
* **PRIN-6 (Currency & Provenance):** Any advisory output must bind its source payload hash, target commit SHA, and evaluator model version. If the underlying file or spec changes, the recommendation is invalidated.
* **PRIN-7 (Dual Predicates):** Advisory recommendations never bypass mandatory CI or human approvals.
* **PRIN-8 (Determinism Ladder):** Jev remains classified as **Rung 3.5 (Calibrated Heuristic)**. All outputs presented to agents or humans must carry `heuristic: true` and the evaluated probability $p$. Deterministic checks (Rungs 1–3) must always run first.

---

## 6. Summary

By pivoting Jev from an unreviewed merge gatekeeper to a **structured triage, ranking, and context-selection engine**, AIDA leverages Jev's true strengths—speed, typed schemas, and cheap classification—while protecting the repository from catastrophic tail failures. The next phase of evaluation should implement the four prioritized experiments using the labeled evaluation protocol outlined above.
