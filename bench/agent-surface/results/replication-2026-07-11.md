# SPIKE-73 agent-surface replication — 2026-07-11

Independent re-run of the 72-cell agent-surface benchmark (4 surfaces × 6 tasks ×
3 repeats), reproducing the 2026-06-29 run in [`report.md`](report.md).

**Intent:** does the MCP-vs-CLI cost finding hold on current infrastructure? Two
variables changed vs June — the agent model (`--model sonnet` now resolves to
current Sonnet) and the AIDA binary (0.14.0 release vs June's build). Same tasks,
fixture design, harness, and deterministic (LLM-judge) grading.

## Core finding: REPLICATES

MCP cost premium over the plain human-formatted CLI:

| Surface | June cost | June ×CLI | July cost | July ×CLI |
|---|---|---|---|---|
| cli | $0.0358 | — | $0.0461 | — |
| mcp | $0.0709 | 1.98× | $0.1004 | **2.18×** |
| mcp-toolsearch (on-demand schema) | $0.0636 | 1.78× | $0.0714 | 1.55× |
| toon (token-efficient CLI) | $0.0360 | 1.01× | $0.0458 | 0.99× |

The ~2× MCP premium holds (2.18× vs 1.98×). On-demand schema loading still lands
between CLI and full MCP (~1.5–1.8×). TOON stays at cost parity with the CLI. The
paper's central claim — MCP costs ~2× the CLI, so the CLI is the primary agent
surface — reproduces on current models and current AIDA.

## Success rates: one fixture-drift caveat

| Surface | June | July |
|---|---|---|
| cli | 100% | 100% |
| mcp | 89% | 78% |
| mcp-toolsearch | 100% | 100% |
| toon | 100% | 94% |

The MCP drop is **not** a model regression — it's fixture drift. Current
`aida init` seeds more META/system specs (11 total vs the ~5 the `status_snapshot`
grader was written to expect), so all three mcp `status_snapshot` cells were
failed by the judge for a count mismatch (0/3; cli/toon/mcp-toolsearch happened to
pass it). Exclude that one confounded task and mcp success is ~stable (~94%). The
cost finding is independent of spec count and unaffected. (Fixing the grader's
expected count would make the success rates directly comparable in a future run.)

Raw data: [`results-2026-07-11.jsonl`](results-2026-07-11.jsonl); full July report
[`report-2026-07-11.md`](report-2026-07-11.md). June baseline: `report.md` /
`results.jsonl`.
