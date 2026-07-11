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
| `mcp` | the AIDA MCP tools (`aida mcp-serve`), all ~69 schemas loaded upfront |
| `mcp-toolsearch` | MCP with on-demand schema loading (`ToolSearch`) instead of upfront |
| `toon` | the token-efficient CLI output mode (`AIDA_AGENT_OUTPUT=1`), TASK-964 — **shipped** |

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
- Tasks: `tasks.json` — 6 representative agent operations:
  `next_queue_item`, `show_spec_blocked_by`, `status_snapshot`,
  `find_finding`, `file_spec`, and `chained_followup` — the multi-round
  browse→filter→fetch→write pattern where MCP's per-call round-trips should hurt
  most (categorized `chained` for the single-call-vs-chained report split).

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

Prerequisites: `claude` and `aida` on PATH (run `aida dev activate` for the dev build),
`git` configured. Each `claude -p` is a real billed call, so cost scales with
`conditions x tasks x repeat`.

## Scaling / remaining work

The committed run is the full **72-cell** matrix (4 conditions × 6 tasks × 3
repeats — see Findings). The `mcp-toolsearch` variant and the `toon` column are
now both built and measured; remaining directions to harden it further:

1. **Raise `--repeat`** to 5-10 to shrink per-cell variance, and report the
   median + spread, not just the mean.
2. **Add an error-recovery task** (AXI's third category) — the multi-round
   browse→filter→fetch→write chain (`chained_followup`) is in; an explicit
   error-recovery task is not yet. Each task needs a `grading_hint`.
3. **Cross-model** — re-run with `--model haiku` and a non-Claude agent (the AXI
   harness also drives `codex`) to check the finding is not Sonnet-specific.

## The TOON column — measured (TASK-964)

trace:TASK-964

The `toon` condition is now built: `AIDA_AGENT_OUTPUT=1` (or any non-TTY caller)
makes `aida` emit **TOON** — a compact tabular encoding (`name[N]{fields}:` +
one comma-joined line per row; single specs as `key: value` head fields + a
relationships table) with a minimal default schema. The human TTY emoji/table
path is byte-identical (`AIDA_AGENT_OUTPUT=0` forces it). Both bench conditions
now pin the env var, so `cli` measures the pre-TOON baseline and `toon` measures
the new agent surface.

The full `claude -p` matrix is a real billed run (re-run it with
`python3 run_bench.py matrix --condition cli,toon --repeat 5`); the **direct,
cache-independent** measurement below is the format's token delta on identical
command output — the comparison the SPIKE-73 caveats name as the signal. Bytes
measured on this repo's live store (~945 specs); tokens estimated at ~4 chars/tok.

| Command (same data) | CLI human (~tok) | TOON (~tok) | Reduction |
|---|---|---|---|
| `aida list --all` (all 945 rows) | ~90,600 | ~71,600 | **21%** |
| `aida show <spec> --no-git` | ~276 | ~147 | **47%** |
| `aida queue list` (actionable head) | ~410 | ~67 | **84%** |
| `aida status` | ~84 | ~31 | **62%** |
| `aida list` (default, TOON also caps to 30) | ~37,500 | ~845 | 98%† |

†The default-`list` row conflates two effects — TOON's encoding *and* the
TASK-970 agent default row-cap (30 rows). The honest TOON-format-only number is
the `list --all` row (both render the same 945 rows): **~21%** purely from
dropping the table chrome, padding, and per-row glyphs. The focused single-task
reads (`show` / `queue list` / `status`) — which are what each SPIKE-73 agent
task actually issues — drop **47–84%**.

**Read.** TOON beats the human CLI on every surface, and wins biggest exactly
where it matters: the multi-row *browse* pattern (`list`, `queue list`) where
JSON/MCP and the emoji table balloon. The honest nuance is that SPIKE-73's
"CLI = 44k input tokens / task" is dominated by fixed harness + system-prompt
overhead, not `aida`'s output: a single-spec lookup is already ~80–410 tokens in
either mode, so TOON shaves hundreds-of-tokens per focused task and tens-of-
percent per browse — it does **not** move single-call tasks below the ~44k floor
because that floor is mostly not `aida` output. The decisive win is
**context-window consumption on result-heavy reads** (where MCP's 95k came from)
plus the strictly-smaller bytes on every call. This supports making the
token-efficient CLI the *primary* agent surface; re-run the billed matrix for the
cost/success/turns columns before publishing.

## Findings (SPIKE-73)

The committed evidence is the full **72-cell** matrix (4 conditions × 6 tasks × 3
repeats), reproduced independently — June 2026-06-29 and July 2026-07-11 (current
models + current AIDA). Authoritative per-run numbers and the full
find→fix→validate narrative live in
[`results/replication-2026-07-11.md`](results/replication-2026-07-11.md); the raw
reports are `results/report.md` (June) and `results/report-2026-07-11*.md` (July).

**The core result replicates: MCP costs ~2× the CLI, with no offsetting win.**

| Surface | Cost vs `cli` (across runs) | Success | What it is |
|---|---|---|---|
| `cli` | 1.0× (baseline) | ~100% | human-formatted CLI |
| `mcp` | **1.98× / 2.18× / 2.31×** | ~100%† | all ~69 schemas upfront |
| `mcp-toolsearch` | ~1.4–1.8× | 100% | on-demand schema loading |
| `toon` | ~0.8–1.0× (parity) | ~100% | token-efficient CLI output |

- The **~2× MCP premium is the whole finding.** The AIDA MCP server loads ~69
  tool schemas upfront — a fixed ~51k-token tax on every call, however trivial. A
  *cold* MCP call ran ~5× the CLI; prompt caching collapses the dollar gap to
  ~2×, but the context-window consumption (~95k vs ~44k input tokens) is not
  cacheable — it halves the budget before real work starts.
- MCP shows **no success or turn advantage** on these tasks. Its leverage, if
  any, is structural (typed schemas, discoverability), not efficiency.
- **On-demand schema loading (`mcp-toolsearch`) narrows but doesn't erase the
  premium** (~1.4–1.8×) at full success — the upfront load *is* most of the tax.
- **TOON (`AIDA_AGENT_OUTPUT=1`) sits at or below CLI cost**, biggest on
  result-heavy browse reads (see the direct byte deltas above).

**This is why the CLI (with TOON) is AIDA's primary agent surface and MCP is the
typed option, not the default** — the conclusion AXI reached first, confirmed here
for AIDA.

†The success column is the *post-fix* rate. The benchmark's deterministic grading
surfaced two artifacts along the way, both fixed: **BUG-717** (MCP `status_unified`
counted META/system specs the CLI excludes — reported 11 vs 5 on a fresh fixture,
failing the `status_snapshot` MCP cells; fixed, and the re-run took those 0/3 →
3/3) and a **harness false-negative** on `chained_followup` (per-*condition*
reseeding let runs 2+ see the prior run's follow-up spec, so the agent correctly
declined a duplicate and the grader mis-scored it a failure — now reseeded
per-*run*). Neither was an agent/CLI/AIDA efficiency effect; the cost finding is
independent of both.

**Honest caveats:** n=3 per cell — treat the ~2× ratio as the signal, exact cents
as noise. Single-call tasks favor the CLI; the one multi-round task
(`chained_followup`) is where MCP's round-trips could pay off but didn't measurably
here. Caching makes absolute cost order-sensitive; the token counts are the
cache-independent comparison.
