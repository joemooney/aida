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
