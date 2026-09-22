# Benchmark findings: TypeSafe AI Jev on AIDA review and store artifacts

**Date:** 2026-09-22

**Spec:** SPIKE-87

**Mode:** live Jev API; no simulated answers were admitted into these results

## Result

The live run evaluated 50 real `.aida/review-verdicts/PR-*.json` records and 30
deterministically selected real records from the git-canonical `.aida-store`.
The harness preserves each source identifier, response, fixture label and measured
latency when `AIDA_JEV_EVIDENCE_OUT` is set.

```
historical verdicts                 50
store pairs                         30
latency p50                     218.25 ms
latency p90                     322.53 ms
latency p95                     550.09 ms
latency max                    5198.42 ms
projected cost / 1,000             $0.08
decided-verdict concordance        100.0%
escalation rate                     96.0%
false approvals                        0
false rejections                       0
pair fixture accuracy               96.7%
VIS-1 / CR-6 detected                 no
```

The strict intake-latency criterion, P95 under 300 ms, **failed**. The measured
P95 was 550.09 ms. The named VIS-1/CR-6 positive control also **failed** in this
run even though aggregate pair-fixture accuracy was 96.7%. These failures make
Jev unsuitable as an unreviewed merge bypass on this evidence. Its useful role
is a fast advisory/escalation layer behind deterministic checks, with a
conversational reviewer retained for ambiguity and service/model failures.

The 100% concordance number applies only to the 4% of historical verdicts on
which the threshold policy made a decision. A 96% escalation rate means it is
not evidence that Jev can replace the current reviewer.

## Method

For review records, the input is the recorded reviewed SHA, summary and findings.
A `noul` question asks whether the record establishes merge readiness. The tested
policy is:

- auto-approve only when probability is at least 0.95 and confidence at least 0.90;
- auto-reject only when probability is at most 0.20 and confidence at least 0.85;
- otherwise escalate.

For store pairs, VIS-1/CR-6 is the positive control. The other 29 inputs are
real, reproducibly selected records used as fixture-labeled negative controls;
the label is not a claim that the corpus has been exhaustively adjudicated.
The model chooses `compatible`, `supersedes`, or `contradicts`.

The cost figure is a projection from the stated price and assumed input size,
not a billing measurement. All accuracy and calibration findings are heuristic
and carry their confidence, per PRIN-8.

## Reproducibility and failure semantics

`scripts/benchmark_jev.py --sample 50 --json` runs live when it finds a key.
Any authentication, HTTP, timeout, schema or parse error terminates the live run
as unavailable (exit 2); it never substitutes a local keyword simulator while
retaining the `live` label. Set `AIDA_JEV_OFFLINE=1` for an explicitly labeled
simulation. Set `AIDA_JEV_EVIDENCE_OUT=<path>` to retain the full raw evidence.
Insufficient sample sizes exit 3.

## Recommendation

Adopt ADR-55's `EvaluatorEngine` abstraction, provenance and conservative
tri-state routing, including an offline local-engine implementation. Keep
deterministic executable criteria first, require green CI independently, bind
every heuristic result to the reviewed SHA and question-payload hash, and fail
closed to the conversational reviewer. Do not enable Jev fast-pass merging from
this benchmark; re-evaluate after latency and positive-control performance are
demonstrably improved.

## Expanded scope: lower-risk, reversible applications

The benchmark established that granting Jev unreviewed merge authority or sole
responsibility for store-wide semantic integrity is premature. However, several
lower-risk, advisory applications fit Jev's System One architecture significantly
better because their outcomes are bounded, reversible, and fall back to human or
conversational review.

Governing design rule: **use Jev to decide "which bounded path should handle this?"
before using it to decide "is this safe to merge?"**

Detailed application matrix and experimental protocol:
`docs/research/2026-09-22-jev-system-one-application-matrix.md`.

### Candidate application areas (hypotheses)

| AIDA area | Possible Jev use | Why it may fit better (hypotheses) |
| :--- | :--- | :--- |
| **Queue and task routing** | Classify a spec into implementation, docs, research, review, or advisor escalation | Bounded classification; mistakes fall back to human routing. High-impact types remain barred from auto-dispatch. |
| **Requirement metadata hygiene** | Suggest type, tags, feature, priority, or duplicate candidates | Advises operator during grooming; no automatic canonical mutation occurs without confirmation. |
| **Duplicate/related-spec detection** | Rank whether a new requirement duplicates or extends an existing one | Semantic similarity aids deduplication before specs enter the store; no automatic canonical mutation. |
| **Change-impact analysis** | Identify which requirements, plans, docs, or tests are likely affected by a diff | A retrieval/ranking aid, not a merge authority. |
| **Review-finding triage** | Cluster repeated findings across review rounds and identify likely duplicates | Aims to identify repeated review feedback; must preserve all original findings without deletion. |
| **Finding severity classification** | Categorize findings as blocker, correctness, maintainability, or advisory | Standardizes finding terminology; human reviewer or conversational agent retains final say. |
| **Review prompt/context selection** | Select the most relevant acceptance criteria, prior findings, and related specs for a reviewer | Context compression; aims to reduce context tokens in Claude/Codex seats without dropping mandatory rules. |
| **Traceability assistance** | Detect likely missing or misplaced trace:<SPEC-ID> comments | Narrow proposition with immediate deterministic follow-up (diff inspection / grep). |
| **Stale-document detection** | Judge whether AGENTS/OVERVIEW/docs still describe the current architecture | Escalates candidates for human or docs-lane confirmation; candidate flagging only. |
| **Session/punt classification** | Classify why a task was shelved or punted and suggest the next lane | Useful for fleet analytics and rework analysis without modifying runtime state. |
| **Reconstitution comparison** | Compare regenerated tests/docs to intended behavior | Mentioned in earlier reports, but unvalidated in live evaluation; needs empirical testing. |
| **Operator-facing explanations** | Select a concise explanation category for a deterministic failure | Jev chooses among predefined, vetted explanations; must cite the triggering deterministic evidence. |

### Prioritized near-term experiments

1. **Review-context selection** — choose relevant acceptance criteria and prior findings before a Claude/Codex review. Measure mandatory rule recall (safety critical).
2. **Duplicate/related-spec ranking** — advisory candidate overlap hints during intake; capture persists first and never blocks, with silent deterministic fallback on cutoff.

3. **Finding clustering and triage** — cluster repeated findings across rounds to reduce review churn; strictly preserves all original findings without discarding evidence.
4. **Queue routing** — advisory classifier with explicit "unknown/escalate" class; bars auto-dispatch for high-impact or ambiguous spec types.

Each experiment should evaluate workload-justified coverage/abstention, class precision/recall,
calibration/threshold performance, asymmetric error costs, latency/cost, and
deterministic pre-filtering efficiency.

External evaluation remains optional: Jev is not a prerequisite for compiling,
testing, or running AIDA. Without Jev credentials, the contradiction sweep and
graded-review paths fall back to deterministic candidates (`model:
"mechanical-join"`) or conversational reviewer escalation. Secret redaction, a
16 KB evaluator payload cap, and an explicit `AIDA_EVALUATOR_OFFLINE` switch are
**proposed safeguards, not current implementation**; they require separate
implementation and tests before expanding external evaluator use.
The test suite validates 24 targeted unit tests across `adr_55`, `story_1424`, and `story_1426`.


