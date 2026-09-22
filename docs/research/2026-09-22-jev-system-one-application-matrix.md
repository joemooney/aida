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

System 1 models are best explored for **bounded categorization, ranking, and context filtering** where:
* The action is advisory, reversible, or filtered.
* Mistakes cleanly fall back to human inspection or conversational agent review.
* Deterministic checks (Rungs 1–3) pre-filter the problem space to minimize AI invocations.

---

## 2. Candidate Application Areas (Hypotheses)

The following matrix identifies 12 candidate application hypotheses in AIDA where Jev's fast, typed classification primitives (`choice`, `noul`, `score`) may provide value without requiring merge or store mutation authority. These represent unvalidated research directions that require labeled empirical validation.

| AIDA Subsystem / Area | Proposed Jev Role | Mechanism & Primitives | Why It May Fit Better (Hypotheses) |
| :--- | :--- | :--- | :--- |
| **1. Queue & Task Routing** | Classify a spec into implementation, docs, research, review, or advisor escalation. | `choice` over canonical roles: `implementer`, `reviewer`, `advisor`, `researcher`, `docs`, `escalate`. | Bounded classification; mistakes fall back to human routing. High-impact types remain barred from auto-dispatch. |
| **2. Requirement Metadata Hygiene** | Suggest requirement type, tags, feature group, or priority for unformatted drafts. | Multi-question `choice` & `score` on spec description. | Advises operator during grooming; no automatic canonical mutation occurs without confirmation. |
| **3. Duplicate / Related-Spec Detection** | Rank whether an incoming draft duplicates, supersedes, or extends an existing spec. | `choice` (`duplicate`, `extends`, `unrelated`, `supersedes`) over mechanically pre-filtered candidates. | Semantic similarity aids deduplication before specs enter the store; no automatic canonical mutation. |
| **4. Change-Impact Analysis** | Identify which specs, plans, documentation, or test suites are likely affected by a git diff. | `noul` query against candidate specs retrieved via AST/graph dependencies. | Functions as a retrieval and ranking aid for the implementer or reviewer, not a merge authority. |
| **5. Review-Finding Triage** | Cluster repeated findings across review rounds and identify likely duplicates or rephrasings. | `choice` (`duplicate_prior`, `new_issue`, `fixed`) comparing new finding text against past round notes. | Aims to identify repeated review feedback; must preserve all original findings without deletion. |
| **6. Finding Severity Classification** | Categorize review findings as `blocker`, `correctness`, `maintainability`, or `advisory`. | `score` / `choice` with defined rubric definitions. | Standardizes finding terminology; human reviewer or conversational agent retains final say. |
| **7. Review Prompt & Context Selection** | Select the most relevant acceptance criteria, prior findings, and related specs for a reviewer prompt. | `score` ranking relevance of ambient project rules to the current PR diff. | Context compression: aims to reduce context tokens in Claude/Codex seats without dropping mandatory rules. |
| **8. Traceability Assistance** | Detect likely missing or misplaced `// trace:<SPEC-ID>` comments in modified AST nodes. | `noul` verifying whether a modified function conceptually realizes the given spec ID. | Narrow proposition with immediate deterministic follow-up (diff inspection / grep). |
| **9. Stale-Document Detection** | Judge whether `AGENTS.md`, `OVERVIEW.md`, or architecture guides still reflect current source code. | Pairwise `choice` (`fresh`, `stale_terminology`, `contradicted`) on doc sections vs recent commits. | Escalates candidates for human or docs-lane confirmation; candidate flagging only. |
| **10. Session / Punt Classification** | Classify why an autonomous agent session was shelved or punted, and suggest the next lane. | `choice` over punt taxonomy (`environment_error`, `ambiguous_spec`, `ci_failure`, `agent_loop`). | Useful for fleet analytics and rework analysis without modifying runtime state. |
| **11. Reconstitution Comparison** | Compare regenerated test suites and behavioral descriptions against ground truth intent. | Calibrated `choice` (`matched`, `partial`, `divergent`) on test semantics (ADR-44). | Mentioned in earlier reports, but unvalidated in live evaluation; needs empirical testing. |
| **12. Operator-Facing Explanations** | Select a concise, human-friendly explanation category for a complex deterministic failure. | `choice` among predefined curated explanation templates based on error logs. | Jev chooses among predefined, vetted explanations; must cite the triggering deterministic evidence. |

---

## 3. Top Four Prioritized Near-Term Experiments

To establish defensible utility before expanding Jev's operational footprint, we prioritize four candidate experiments characterized by high potential operational value, low catastrophic risk, and measurable baselines.

```
                     ┌──────────────────────────────────────────────┐
                     │ 4 High-Leverage Near-Term Experiments        │
                     └──────────────────────┬───────────────────────┘
                                            │
        ┌───────────────────┬───────────────┴───────────────┬───────────────────┐
        ▼                   ▼                               ▼                   ▼
 1. Review Context   2. Duplicate Spec               3. Finding Triage   4. Queue Routing
    Selection           Detection                       & Clustering        & Lane Triage
 • Context select.   • Advisory hint                 • Preserves all     • Dispatches routine
 • Mandatory recall  • No auto-mutate                  findings            tasks to lane
 • Hard timeout      • Fallback search               • Annotates loops   • No auto-dispatch
                                                                           for high-impact
```

### Experiment 1: Review Prompt & Context Selection
* **Problem:** Conversational reviewers (Claude Code, Codex) suffer from context bloat. Stuffing entire project rules, full spec hierarchies, and past review history degrades judgment and causes timeout stalls.
* **Proposed Jev Role:** Pre-scan the PR diff and candidate guidelines to select the top $K$ relevant acceptance criteria and past review findings.
* **Critical Safety Metric (Mandatory Rule Recall):** Token reduction is insufficient on its own. The experiment must measure **Recall of Mandatory Acceptance Criteria and Critical Governing Rules** ($\ge 99.5\%$). A shorter prompt that omits a single load-bearing requirement or security invariant is an unacceptable regression.

### Experiment 2: Duplicate & Related-Spec Detection
* **Problem:** Human operators and autonomous agents frequently file duplicate bugs or overlapping tasks because searching hundreds of YAML specs via exact keywords misses synonyms or architectural overlap.
* **Proposed Jev Role:** When `aida add` runs, mechanically retrieve top candidate specs (via trigram / SQLite FTS) and invoke Jev `choice` (`duplicate`, `extends`, `unrelated`).
* **Interactive Latency & Non-Blocking Fallback:** Requirement capture (`aida add`) must never block on AI latency. The requirement YAML is persisted first. Advisory duplicate checks run asynchronously or with a short cutoff; on timeout or unavailability, `aida add` proceeds silently without stalling developer capture.


### Experiment 3: Review-Finding Clustering & Triage
* **Problem:** Autonomous drains often enter 3-to-5 round review loops where the reviewer raises one superficial finding per round, or re-raises a concern the implementer already addressed.
* **Proposed Jev Role:** Compare new reviewer findings against historical findings from prior rounds on the same PR to cluster duplicates and annotate recurrence.
* **Strict Invariant (Never Discard Evidence):** Jev must **never drop, suppress, or delete findings** based on its judgment. It may annotate, cluster, or group related findings, but the complete set of original findings and reviewer notes must remain fully visible to the human operator and reviewing agent.

### Experiment 4: Queue & Task Lane Routing
* **Problem:** `aida queue work` dispatches tasks to agents. Certain tasks require specialized tools (e.g. `docs`, `research`, `spike`, `keystone`), but operators rarely set explicit lanes on creation.
* **Proposed Jev Role:** Classify pending requirements into execution lanes (`implementer`, `researcher`, `docs`, `advisor`).
* **High-Impact "Do Not Auto-Dispatch" Gate:** Misrouting is not harmless if an autonomous agent is unleashed on the wrong task. High-impact types (`EPIC`, `ADR`, `GOAL`, security-sensitive, or novel architectural tasks) must **never be auto-dispatched** by Jev. Such tasks require an explicit human assignment or default to an advisory escalation lane.

---

## 4. Privacy, Payload Boundaries & Redaction

Several proposed applications transmit code diffs, requirement descriptions, review findings, or session traces to an external SaaS endpoint. The system must enforce strict operational boundaries:

1. **Opt-In Policy:** External evaluator calls must be strictly opt-in (`AIDA_JEV_API_KEY` or `TYPESAFE_API_KEY` explicitly set). If unset or if `AIDA_EVALUATOR_OFFLINE=1` is active, AIDA defaults to `MockEvaluator` or local models without network transmission.
2. **Secret & Credential Redaction:** All code diffs and context strings must pass through AIDA's credential sanitization filters (stripping API keys, JWTs, `.env` variables, and SSH keys) before transmission.
3. **Payload Truncation Caps:** Input context payloads are capped at 16 KB. If a diff or context exceeds this boundary, deterministic chunking or truncation must be applied rather than sending unbounded source trees.
4. **Offline Local Sovereignty:** Under ADR-55, all capabilities must retain an offline, local implementation (`LocalLlmEvaluator` or `MockEvaluator`) to protect repository sovereignty.

---

## 5. Experimental Evaluation Protocol

Each candidate experiment must be evaluated against an empirical ground-truth corpus before any mainline tooling adoption. See [`docs/research/2026-09-22-jev-system-one-measurement-protocol.md`](docs/research/2026-09-22-jev-system-one-measurement-protocol.md) for the complete causal measurement protocol, independent adjudication requirements, and pre-registered stop conditions.


```
       Input Space
            │
            ▼
┌───────────────────────┐
│ Rung 3 Deterministic  │ ──Excluded (Zero AI Cost)──► Bypass
│ Mechanical Pre-Filter │
└───────────┬───────────┘
            │ Candidate Subset (Use-Case Deadline)
            ▼
┌───────────────────────┐
│ Jev System 1 Evaluate │
└───────────┬───────────┘
            │
    ┌───────┴───────┐
    ▼               ▼
High Confidence    Low Confidence / Ambiguous / Timeout
    │               │
    ▼               ▼
[Annotate / Advise] [Abstain / Silent Deterministic Fallback]
```

For each experiment, the benchmark harness must report:

1. **Workload-Specific Coverage vs. Abstention:**
   * Measure the percentage of inputs the model confidently classifies vs. routes to `unknown`/`escalate`. 
   * Coverage targets must be justified per workload rather than assuming a blanket 70% threshold. (Recall: the historical review benchmark achieved only 4% autonomous decisions under safe thresholds).
2. **Precision, Recall & Class Breakdown:**
   * Detailed breakdown across classes, including mandatory constraint recall for context selection.
3. **Calibration & Empirical Thresholds:**
   * Compute Brier scores, Expected Calibration Error (ECE), and reliability curves to test whether output probabilities reflect empirical frequencies.
4. **Asymmetric Error Costs:**
   * Map the concrete cost of a False Positive vs. False Negative. For example, in duplicate detection, missing a duplicate adds slight graph clutter, whereas a false duplicate hint risks confusing an operator into abandoning valid work.
5. **Latency Distributions & Cutoff Rates:**
   * Measure P50, P90, P95, and Max latency, tracking the percentage of calls that exceed the declared use-case deadline.
6. **Deterministic Pre-Filtering Efficiency:**
   * Quantify how effectively mechanical filters (ripgrep, SQLite FTS, AST joins) narrow the input volume before invoking AI.


---

## 6. Governance Alignment (PRIN-5 through PRIN-8)

Every application of Jev must conform to AIDA's constitutional principles:

* **PRIN-5 (Fail-Closed):** Any API unavailability, network error, timeout, or low-confidence score must fail closed to human advisory or standard deterministic paths.
* **PRIN-6 (Currency & Provenance):** Any advisory output must bind its source payload hash, target commit SHA, and evaluator model version.
* **PRIN-7 (Dual Predicates):** Advisory recommendations never bypass mandatory CI or required review verdicts.
* **PRIN-8 (Determinism Ladder):** Jev remains classified as **Rung 3.5 (Calibrated Heuristic)**. All outputs presented to agents or humans must carry `heuristic: true` and cite the underlying deterministic evidence that triggered them.

## 7. Zero-Prerequisite & Offline Sovereignty Invariant

Jev is strictly an **optional acceleration plugin**, never a prerequisite for installing, compiling, testing, or operating AIDA:

1. **Zero Hard Dependency:**
   * AIDA builds, tests, runs, and passes all CI gates with zero environment variables, zero network access, and zero external API keys.
   * If `AIDA_JEV_API_KEY` or `TYPESAFE_API_KEY` is not present, no external calls are made.
2. **Deterministic Baseline When Jev Access is Absent:**
   * **`aida doctor --contradictions`:** Executes Slice 1 mechanical joins across requirements (e.g. Approved Vision older than Completed ChangeRequest, terminal plan Followups missing child, ADR overlaps). Reports all candidate conflicts with `model: "mechanical-join"` and `verdict: "candidate"` cleanly without error or disruption.
   * **`graded_review` (STORY-1424):** Runs Rung 2 deterministic bash checks. If all machine checks pass, residual prose criteria are **never auto-approved**; they cleanly escalate (`escalated_to_seat: true`) for the human operator or Phase 3 conversational reviewer seat (Claude Code / Codex).
   * **Candidate Advisory Features:** In keyless or offline environments, features immediately and silently fall back to deterministic baselines (full prompts, keyword search, default queue lanes).
3. **Local Sovereignty (ADR-55):**
   * AIDA is a git-canonical, self-sovereign development substrate. Tight coupling to any proprietary external SaaS API is strictly prohibited. `MockEvaluator` and `LocalLlmEvaluator` ensure complete local testing and air-gapped support.

---

## 8. Current Verification Status

* **Targeted Unit Tests:** 24 targeted unit tests pass across [`aida-cli-lib`](file:///home/joe/ai/aida-spike-87/aida-cli-lib):
  * 5 evaluator engine tests (`adr_55_evaluator_tests`)
  * 11 graded review tests (`story_1424_graded_review_tests`)
  * 8 contradiction sweep tests (`story_1426_contradictions_tests`, including explicit zero-evaluator offline fallback)
* **Remote CI Checks:** PR #2080 checks pass on Ubuntu (`CI/Build` and `merge-hold-gate`).
* **Governance Invariant:** No code has been merged to `main`; fast-pass auto-merging remains strictly disabled.

