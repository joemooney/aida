# Committed regeneration rate roundtrips

<!-- trace:SPEC-447 | ai:codex -->

## Scope and conclusion

SPEC-447 requests investigation and a bounded repair design. The remaining
control validator uses exact floating equality in `score/controls.rs`:
`require_execution` compares the recorded rate with check-derived integer
division. With the locked serde_json 1.0.149 default parser, valid JSON for
`1/11` and `43/44` parses to a neighboring float. Consequently a valid
committed stub can block subsequent scoring in `ControlGate::load`; a
roundtripped ordinary trial can also fail `ControlGate::apply`.

This change adds executable characterization and consensus regression coverage.
It does not change production eligibility policy. Consensus already has the
separately authorized finite `8 * f64::EPSILON` consistency check from
STORY-1679; SPEC-447's earlier comment records that boundary. The optional
display reader has its own tolerant, count-derived presentation policy.

## Reproduction

The control fixture constructs three matching, completely passing reference
records, a generated-noop stub, and an ordinary trial. It uses the production
pretty-JSON serializer, commits records under `regen/trials/<uuid>.json`, then
deletes their working files. The production committed-blob reader still loads
the records. Original in-memory controls and trials pass validation; persisted
ones exhibit the following behavior:

| Passed / total | Original f64 | Parsed f64 | Strict stub load / trial apply |
| --- | --- | --- | --- |
| 1 / 11 | 0.09090909090909091 | 0.09090909090909093 | Aggregate refusal |
| 43 / 44 | 0.9772727272727273 | 0.9772727272727272 | Aggregate refusal |
| 1 / 3 | 0.3333333333333333 | Same | Accepted |
| 2 / 3 | 0.6666666666666666 | Same | Accepted |
| 1 / 2 | 0.5 | Same | Accepted |
| 0 / 11 | 0.0 | Same | Accepted |

Trial application is isolated from stub loading using a valid zero-pass stub,
so the two refusals are established independently. Completely passing reference
rates remain exactly 1.0. A stub passing every check remains ineligible because
it leaves no discriminating denominator.

A scalar survey serializes/parses all 131,840 count pairs with denominators
1 through 512. Of these, 13,710 change, with a maximum observed difference of
one ULP (one neighboring representable floating value). This is measured
coverage, not a universal parser error guarantee. Enabling serde_json's
`float_roundtrip` feature is expected to eliminate this drift and flip the
survey's `changed > 0` assertion. TASK-2842 should evaluate it as a candidate
repair before adding a tolerance: exact float roundtrips could retain the
existing zero-tolerance comparator. Check feature unification and rerun the
committed fixtures and authority-gate refusal matrix before choosing between
that dependency-wide parser change and the bounded comparator below. Enabling
the feature remains outside this investigation.

The consensus fixture commits three matching ordinary trials for each of the
above fractions plus 11/11. It exercises analysis and the command handler.
Materially wrong rates, mismatched passed/total counts, and mixed oracle,
bundle or corpus provenance each refuse before draft filing. Direct NaN and
infinities refuse; their JSON null representation is rejected by deserialization.

These are synthetic execution records in real temporary Git repositories.
They establish persisted-evidence validation behavior, not successful live
confined installer/oracle execution or new scoring authority.

## Proposed strict-gate repair (not applied)

Keep the bounded comparison local to strict aggregate consistency. Do not
call the optional display validator or normalize a record before validating it.

1. Derive `passed` and `total` from the exact checks. Require a nonempty set and
   exact equality of the stored integer counts to those values.
2. Compute `expected = passed as f64 / total as f64`.
3. Require the stored rate to be finite and in `[0, 1]`. For expected endpoints
   0 and 1, require numeric equality, accepting signed zero as numeric zero.
4. For interior rates, permit at most two ULPs from `expected`. For nonnegative
   f64 values, compare `stored.to_bits().abs_diff(expected.to_bits()) <= 2`.
   The two-ULP proposal covers the measured one-ULP cases with a small margin;
   it is a chosen acceptance bound, not a guarantee for every decimal parser.
   Unlike a fixed absolute epsilon it stays narrow near zero.
5. Continue calculating discriminating scores from check counts. Preserve the
   original raw stored aggregate as evidence; never use the tolerated rate to
   choose exclusions, reference success, or completion.

Leave exact names, reference repeat identities/counts, 100% reference passes,
latest-batch selection, oracle/bundle/corpus binding, generated-noop stub
identity, confinement, guard hash/hits, fresh roots, flags, exits, survivors,
path identity and committed-only reads unchanged. No file-format change or
additional authority follows from numeric consistency.

A follow-up repair should turn the characterization refusals into positive
committed-fixture regressions, test one/two/three-ULP boundaries and exact
endpoints, reject missing aggregates, wrong counts, NaN/infinities, out-of-range
and materially wrong rates, and rerun the existing independent provenance and
sandbox gate refusal matrix. Keep consensus's existing authorized policy
separate unless its owner explicitly requests convergence. A live confined
control/scoring run would provide additional integration evidence, but must
not be claimed from the synthetic fixtures here.

## Validation commands

```sh
cargo test -p aida-cli-lib --lib committed_json_fraction --offline
cargo test -p aida-cli-lib --lib json_fraction_roundtrip_error_survey --offline -- --nocapture
cargo test -p aida-cli-lib --lib regen::score::controls::tests --offline
cargo test -p aida-cli-lib --lib regen::consensus::tests --offline
cargo fmt --all -- --check
```

## Recorded local results

The two committed-fraction tests pass. The scalar survey reports the counts
above. Running the compiled library test binary with
`regen::score::controls::tests` passes 15 tests; two existing end-to-end tests
fail during setup because they hardcode `tempdir_in("/var/tmp")`, and `/var/tmp`
is read-only in the native sandbox. Repeating that group with these two tests
skipped passes all 15 admitted tests:

- `handle_score_refuses_absent_or_failed_committed_controls_without_scored_record`
- `handle_score_persists_installer_failures_and_flagged_family_score_after_real_controls`

The repair proposal is filed for triage as draft
`SPEC-b6b1278890774ef993d2ebc327862470-011`. This investigation does not mark that
repair complete.

All 18 consensus tests pass. `cargo fmt --all -- --check` and `git diff --check`
pass. The control survey also passes in the full control-group run.
