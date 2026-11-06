# North-star check — aida-monitor — 2026-10-05 (null snapshot)

<!-- trace:TASK-1523 | ai:claude -->

The first run of STORY-1425's two-arm reconstitution measure on aida-monitor,
per the OPERATOR DECISION on SPIKE-86 (Joe, 2026-09-26). This is the executable
check behind VIS-2 ("a project built with AIDA is far better placed to be
rebuilt than from commits and a README alone").

**Headline: the delta is not evaluable on this subject yet.** Both arms report
`unknown: no real traced tests to recall` for every probed spec. The recall
denominator — tests carrying `trace:SPEC-ID.ACn` — is empty across the entire
corpus, so the run publishes the coverage figures and the gap itself, not a
delta. VIS-2 remains **asserted, not yet measured**; what IS measured tonight
is exactly the capture gap STORY-1425's FLAW-2 coverage metric was designed to
expose.

## Pins

| Pin | Value |
|---|---|
| Subject | `~/ai/aida-monitor` |
| Subject code HEAD | `aadafa46101198f4d794a6e7a7a5fa4b6d53b4d6` (2026-09-19) |
| Subject store HEAD | `fbed31b886bea8d80e72a5648759e7f334d98309` |
| aida binary | 0.15.0, sha `dafb66432e+dirty`, built 2026-10-05 20:38 -07:00 |
| Model | none consumed — see "probe telemetry" below |
| Run date | 2026-10-05 20:58 -07:00 |

## Commands (repeatable)

```bash
cd ~/ai/aida-monitor
aida reconstitute STORY-3  --json --yes   # run 1 and run 2 (repeat)
aida reconstitute STORY-6  --json --yes
aida reconstitute STORY-10 --json --yes
```

Sample: a fixed seeded sample (STORY-3, STORY-6, STORY-10 — three of the seven
criteria-bearing specs), chosen because with an empty denominator every spec
returns the identical unknown pair; the full "all specs with acceptance
criteria" sample is deferred to the first evaluable run.

## Results

Per spec (identical for all three, and for the STORY-3 repeat run):

| Spec | Arm A (baseline) | Arm B (store) | Delta (B−A) |
|---|---|---|---|
| STORY-3 (run 1) | unknown — no real traced tests to recall | unknown — same | unknown |
| STORY-3 (run 2) | unknown — same | unknown — same | unknown |
| STORY-6 | unknown — same | unknown — same | unknown |
| STORY-10 | unknown — same | unknown — same | unknown |

Coverage (STORY-1487 figures, reported alongside recall, never combined):

| Metric | Value |
|---|---|
| Tests carrying any `trace:` marker / all tests | 0 / 1 (0%) |
| Specs with acceptance criteria | 7 / 10 (70%) |
| Criteria with a traced test | 0 / 33 (0%) |
| Commits with a SPEC-ID trailer | 1 / 4 (25%) |

Reproducibility: the repeat run of STORY-3 reproduced every score, coverage
and reason field exactly (the two JSON reports differ only in scratch-dir
UUIDs). With a null result, run-to-run variance is trivially zero.

## Pass condition

VIS-2 is supported when the aggregate delta (arm B − arm A) is positive and
exceeds the run-to-run variance measured by at least two repeated runs.
**Tonight's verdict: not evaluable** — no subject project carries
criterion-traced tests, so neither arm can score. This is a statement about
capture discipline, not about reconstitution quality.

Corpus scan behind that statement (2026-10-05):

| Candidate subject | Tests | `trace:SPEC.ACn`-traced tests |
|---|---|---|
| aida-monitor (decided subject) | 1 | 0 |
| aida-hub (STORY-1425's original candidate) | 336 | 0 |
| AIDA itself (labelled lower bound) | thousands | ~1 real (rest are fixture tokens) |

## Prerequisite for the first evaluable run

STORY-1527 (north-star subject readiness) defines what must be true before a
delta can be measured: specs with labeled ACn criteria, each with at least one
organically-written `trace:SPEC.ACn` test — organically, because tests
back-filled from spec text would inflate arm B's recall (the probe regenerates
tests from the same spec text) and void the comparison. When readiness is met,
re-run the commands above over all criteria-bearing specs and supersede this
snapshot.

## Probe telemetry (observed defects, filed separately)

The four store-probe agent runs each exited 0 in about a second with zero-byte
headless logs and empty scratch dirs (`regenerated: 0`), versus an 83KB log
for a real 2026-10-04 probe. With an empty denominator the report masks any
arm-B failure behind `no real traced tests to recall`, so whether the agent
ran is not distinguishable from the report alone. Filed as a finding
(promoted id visible via `aida findings list`); does not affect this
snapshot's numbers, which are locally computed.
