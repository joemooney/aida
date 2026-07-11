# SPIKE-73 agent-surface replication — 2026-07-11

Independent re-run of the 72-cell agent-surface benchmark (4 surfaces × 6 tasks ×
3 repeats), reproducing the 2026-06-29 run in [`report.md`](report.md).

**Intent:** does the MCP-vs-CLI cost finding hold on current infrastructure? Two
variables changed vs June — the agent model (`--model sonnet` now resolves to
current Sonnet) and the AIDA binary (0.14.0 vs June's build). Same tasks, fixture
design, harness, and deterministic (LLM-judge) grading.

## Core finding: REPLICATES (three runs now)

MCP cost premium over the plain human-formatted CLI:

| Surface | June | July v1 | July v2 (post-fix) |
|---|---|---|---|
| cli | — | — | — |
| **mcp** | **1.98×** | **2.18×** | **2.31×** |
| mcp-toolsearch (on-demand schema) | 1.78× | 1.55× | 1.37× |
| toon (token-efficient CLI) | 1.01× | 0.99× | 0.83× |

The ~2× MCP premium holds across all three runs — different model generations,
different AIDA versions, independent runs. On-demand schema loading stays between
CLI and full MCP; TOON stays at/below CLI cost parity. The paper's central claim —
MCP costs ~2× the CLI, so the CLI is the primary agent surface — reproduces.

## What v1 surfaced: a real product bug (BUG-717), now fixed

The first re-run (v1) showed mcp success dropping 89%→78%. That was **not** a model
regression — the deterministic grader caught a genuine **MCP-vs-CLI parity bug**:
the MCP `status_unified` tool counted META/standing-artifact specs in its "Total
requirements," while the CLI (`aida status`/`list`) excludes them. On a fresh
`aida init` fixture the MCP surface reported 11 requirements where the CLI reports
5, so all 3 mcp `status_snapshot` cells were failed for reporting the (correct,
but CLI-inconsistent) count.

Fixed in **BUG-717** — `status_unified` now reuses the CLI's own
`is_standing_artifact_type` predicate. The v2 re-run (fixed binary) validates it:

| Surface | June | July v1 | July v2 (fixed) |
|---|---|---|---|
| cli | 100% | 100% | 89% |
| mcp | 89% | 78% | **100%** |
| mcp-toolsearch | 100% | 100% | 100% |
| toon | 100% | 94% | 100% |

- **mcp `status_snapshot`: 0/3 → 3/3** — the fix, confirmed by the benchmark that
  found the bug (self-test via dogfood).
- **mcp overall: 78% → 100%.**
- **The v2 cli 89% is a benchmark false negative — a defect in the harness, now
  fixed. It should not be read as "≈100%".** Both misses are `chained_followup`
  (browse→filter→fetch→file-a-follow-up). In each, the agent correctly identified
  the target *and then correctly declined to file a duplicate* — because run 1's
  follow-up spec was still in the fixture (the harness reseeded per-*condition*,
  not per-*run*). The grader demands a brand-new spec every run, so it scored the
  correct decline as a failure. Nothing in the agent, the CLI, or AIDA failed;
  the benchmark manufactured the duplicate and then penalized the right response.
  Fixed: the harness now reseeds before every `chained_followup` run
  (`RESEED_PER_RUN_TASKS`) so each run measures a clean create.
- Corollary: the `chained_followup` **success** numbers in *all three* runs are
  unreliable pre-fix — June/v1 passed 3/3 only because those models happened to
  create a duplicate rather than decline it. The metric measured "naive-create vs
  smart-decline," which is noise; the harness fix makes it measure the actual
  write capability. (The **cost** numbers are unaffected — tokens are spent the
  same whether the agent creates or declines.)

## Takeaway

Two independent reproductions agree on the core cost finding, and the exercise
turned up (and fixed) a real surface-parity bug the paper's own methodology was
built to catch. Data: [`report-2026-07-11.md`](report-2026-07-11.md) (v1),
[`report-2026-07-11-fixed.md`](report-2026-07-11-fixed.md) (v2), with raw
`results-2026-07-11*.jsonl`. June baseline: `report.md`.
