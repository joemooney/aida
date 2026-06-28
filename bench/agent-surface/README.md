# AIDA agent-surface benchmark (CLI vs MCP vs TOON)

trace:SPIKE-73

Reproduces the [AXI](https://github.com/kunchenguid/axi) MCP-vs-token-efficient-CLI
benchmark (`bench-github/`) on **AIDA's own surfaces**, to settle a load-bearing
strategic question: AIDA's README calls the MCP server *"the highest-leverage
surface,"* but AXI's published data puts MCP at **87% success / $0.148 / 6 turns**
versus a token-efficient CLI at **100% / $0.050 / 3 turns**. Is MCP over-weighted
for AIDA too?

This harness measures cost / success / turns / tokens for the same agent tasks
run against:

| Condition | What the agent uses |
|---|---|
| `cli` | the current `aida` CLI (human-formatted: emoji + tables), via Bash |
| `mcp` | the AIDA MCP tools (`aida mcp-serve`), loaded upfront into context |
| `toon` | **placeholder** — the future token-efficient CLI output mode (TASK-964), not built yet |

## Methodology (ported from AXI)

Per `(condition x task x run)`:

1. **Fixture** — `seed_fixture.sh` builds a throwaway, `aida init`-ed project
   with known data (a queued item, a blocked-by chain, a status spread, an open
   advisor finding) so every task has a checkable answer.
2. **Run the agent** — `claude -p` with `--output-format stream-json`, cwd = the
   fixture. The condition's `preamble` (in `conditions.json`) tells the agent
   which surface to use; `--allowedTools` / `--disallowedTools` enforce it (the
   `mcp` condition disallows `Bash` so the agent *cannot* shell out to `aida`,
   and disallows `ToolSearch` so the ~69 MCP tool schemas load upfront — the
   realistic "MCP server exposes the graph" posture, and AXI's MCP condition).
3. **Parse** the stream-json: input/output tokens, `num_turns`, `total_cost_usd`,
   tool-call log, tool-result errors (`parse_claude_jsonl`, a port of AXI's
   `parseClaudeJsonl`).
4. **Grade** — an LLM-as-judge (`claude haiku`) reads the trajectory and the
   per-task `grading_hint` and returns `{"pass": bool, "reason": ...}`.
5. **Append** one row to `results/results.jsonl`; `report` aggregates.

### Conditions and tasks

- Conditions: `conditions.json` (preamble + allow/deny tool lists per surface).
- Tasks: `tasks.json` — 5 representative agent operations:
  `next_queue_item`, `show_spec_blocked_by`, `status_snapshot`,
  `find_finding`, `file_spec`.

### Why not match AXI's exact harness?

AXI's harness is TypeScript (pnpm workspace, `yaml`/`tsx` deps) and clones a real
GitHub repo per run. This port is **stdlib-only Python** (no install step) and
seeds a local AIDA project instead — the same shape (fresh setup -> stream-json
-> judge -> jsonl), retargeted at `aida` instead of `gh`. The token-accounting
and grading logic are direct ports.

One deliberate deviation: the surface instruction is prepended to the **prompt**
rather than relying solely on a `CLAUDE.md` drop, because `--setting-sources ''`
makes project-memory loading unreliable. The preamble text is near-identical
across conditions, so it does not bias the input-token comparison; what differs
is whether the MCP tool schemas are in context.

## Running it

```bash
cd bench/agent-surface

# one-time (or --force to rebuild): seed the fixture AIDA project (~1 min)
python3 run_bench.py seed

# a single cell
python3 run_bench.py run --condition cli --task next_queue_item --repeat 2

# the full matrix (default: all conditions x all tasks)
python3 run_bench.py matrix --condition cli,mcp --repeat 3

# regenerate the report from results.jsonl
python3 run_bench.py report
```

Defaults: agent model `sonnet` (matches AXI's Sonnet 4.6 family; bounded cost),
judge model `haiku`. Artifacts land under `results/<condition>/<task>/run<N>/`
(raw stream-json, judge output, grade.json) and `results/results.jsonl`.
`results/` and `fixture-project/` are git-ignored — they are regenerated.

Prerequisites: `claude` and `aida` on PATH (run `aida-on` for the dev build),
`git` configured. Each `claude -p` is a real billed call, so cost scales with
`conditions x tasks x repeat`.

## How to scale the run

The committed numbers below are a **small bounded run** (2 repeats). To turn
them into a publishable result:

1. **Raise `--repeat`** to 5-10 to shrink per-cell variance, and report the
   median + spread, not just the mean.
2. **Add tasks** to `tasks.json` — especially multi-step chains and an
   error-recovery task (AXI's third category), which is where MCP's per-call
   round-trips should hurt most. Each task needs a `grading_hint`.
3. **Add the `mcp-with-toolsearch` variant** — re-run `mcp` *without*
   `ToolSearch` in `disallowed_tools` to measure the deferred-tool-loading path
   (AXI found ToolSearch substantially narrows MCP's token penalty). This
   isolates "MCP is expensive" from "loading 69 schemas upfront is expensive."
4. **Fill the `toon` column** once TASK-964 ships: point its preamble at the
   token-efficient output mode and re-run the matrix. That third column is the
   actual decision input for "should the token-efficient CLI be the *primary*
   agent surface."
5. **Cross-model** — re-run with `--model haiku` and a non-Claude agent (the AXI
   harness also drives `codex`) to check the finding is not Sonnet-specific.

## Initial findings (SPIKE-73)

**Run:** 2026-06-28 · agent model `sonnet` · judge `haiku` · conditions `cli` +
`mcp` · 5 tasks × 2 repeats = **20 agent runs**. Numbers are a small bounded run
— directional, not publication-grade.

| Condition | Success | Avg input tok | Avg cost | Avg turns |
|---|---|---|---|---|
| `cli` (emoji/tables) | 80% (8/10) | **44,178** | **$0.0309** | 2.2 |
| `mcp` (aida mcp-serve, upfront) | 80% (8/10) | **95,261** | **$0.0676** | 2.2 |

Cold-vs-warm spread (prompt caching across sequential runs): CLI cost
$0.0169–$0.0516; MCP cost $0.0319–**$0.2506** (cold). CLI input tokens are a flat
~44k; MCP ~80k–97k.

**Read — yes, MCP is over-weighted on cost/context, and buys nothing measurable
here.** For these five single-shot agent ops, MCP costs ~2.2× the input tokens
and ~2.2× the dollars of the CLI for the *same* success and *same* turn count.
The entire penalty is the ~69-tool schema set the AIDA MCP server loads upfront:
a fixed ~51k-token tax on every call, however trivial. A *cold* MCP call cost
$0.25 vs the CLI's $0.05 (~5×); prompt caching collapses the dollar gap to ~2.2×,
but the context-window consumption (95k vs 44k) is not cacheable away — it halves
the budget left for real work before the task starts. MCP showed no success or
turn advantage (both tied 80% / 2.2 turns; each task is one tool call either
way). This challenges the README's "highest-leverage surface" framing: on the
cost/success/turns axes, MCP is the most expensive surface with no offsetting
win. Its leverage, if any, is structural (typed schemas, discoverability), not
efficiency. AXI's bet — a token-efficient CLI as the *primary* agent surface —
looks right for AIDA too; the `toon` column (TASK-964), once built, is the
deciding evidence.

**Provisional recommendation** (pending more repeats + the TOON column):
re-frame the README (MCP = leverage for *discovery/typing*, CLI = efficiency for
*routine ops*; don't sell MCP as cost-leverage — the data says the opposite);
prioritize TASK-964 as the **primary** agent surface, MCP as typed/discoverable
secondary; consider lazy MCP tool loading (`ToolSearch`) as the default server
posture since the upfront 69-schema load *is* the tax.

**Honest caveats (don't overclaim):** n=2 per cell — treat the ~2× ratio as the
signal, exact cents as noise. Single-call tasks favor the CLI (none force the
multi-round browse→filter→fetch pattern where AXI saw MCP balloon to 6 turns).
Two of the four failures are confounds, not surface deltas: `find_finding` failed
on **both** surfaces because `aida findings list` renders the finding ID
(`TASK-JM-001`) and its linked spec (`TASK-3`) in a layout the agent reversed —
a genuine CLI-output-clarity signal, not an MCP-vs-CLI delta; and one
`status_snapshot` "failure" is fixture drift (the `file_spec` task mutates the
shared fixture, inflating the count past the grading_hint). Caching makes
absolute cost order-sensitive; the token counts are the cache-independent
comparison.
