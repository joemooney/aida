# Cost observability: evidence, reusable parts, and the rework tail

**Date:** 2026-09-19  
**Research spike:** SPIKE-83  
**Decision owner:** advisor (this report makes no product or design decision)

## Executive finding

AIDA already has enough local evidence to attribute Claude Code and Codex usage to a message, session, vendor, and—through AIDA's lease/session manifests—to a spec and orchestration run. It does not yet have a durable round/phase join in `usage.jsonl`, and Antigravity per-message usage could not be established from the artifacts inspected. The existing drain summary is not a reliable cost ledger: 57 of 92 drain-summary records contain zero tokens, while the nonzero records contain 538,786,359 tokens.

The supplied 2026-09-18 12:00–2026-09-19 10:00 study inputs imply a rework tail of **67.8% of rounds**, approximately **37.4M of 55.1M tokens**, and approximately **17.2 of 25.3 throughput-hours**. This evidence favors examining the review round-trip before model substitution. That prioritization is a decision for the advisor.

## Method and reproducibility

All local inspection was read-only. Key commands:

```bash
aida show SPIKE-83 --full
rg -n "usage.jsonl|cumulative_tokens|tokens_per_spec" aida-cli-lib/src
jq -c 'select(.event=="drain_summary")' ~/.aida/usage.jsonl
find ~/.claude/projects -type f -name '*.jsonl'
find ~/.codex/sessions -type f -name '*.jsonl'
rg -n 'session_id|claude_session_id|spec_id' /home/joe/ai/aida/.aida/sessions
find ~/.config/Antigravity -type f
```

The time window is interpreted in the operator's local timezone (America/Phoenix). Arithmetic in Part 4 deliberately uses the supplied measured inputs: 40 shelved rounds, 19 merged specs, 0.75 merges/hour, and approximately 2.9M tokens/merged spec.

## Part 1 — what AIDA records today

### The zero-record cause

The exit record is assembled in `aida-cli-lib/src/lib.rs`, `finalize_drain_summary()`. It computes `cumulative_tokens` by calling `sum_headless_log_tokens(drain_root, started)`. That function:

1. reads only `<drain_root>/.aida/headless-logs/*.jsonl`;
2. includes only files whose filesystem `modified()` time is `>=` the drain's in-memory `SystemTime` start;
3. parses each included file with `drain_caps::tokens_from_log()`; and
4. silently contributes zero for a missing/unreadable directory, unreadable file, empty/non-JSON log, or a stream whose usage shape is not recognized.

`tokens_from_log()` in `aida-cli-lib/src/drain_caps.rs` recognizes the Claude-style usage keys `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, and `cache_read_input_tokens`; it prefers a terminal `type: "result"` line and otherwise takes the maximum parseable line total.

Therefore the precise zero condition is: **at finalization, the sum of parseable Claude-shaped usage in JSONL files under the resolved drain root that have an mtime at or after the drain start is zero** (or `drain_root` is `None`). A fully interactive drain necessarily meets it. So do vendor streams with a different schema, logs placed elsewhere, missing/truncated logs, and timestamp/root misses.

This explains how stdout and persistence can disagree: live cap/progress output can call the driver's `cumulative_tokens()` during execution, while the exit summary independently rescans a filtered directory. They are separate observations, not a single counter carried into persistence.

### Population and correlation

On 2026-09-19, filtering `~/.aida/usage.jsonl` to `event == "drain_summary"` yielded 92 valid records:

| Slice | Records | Zero-token records | Recorded tokens |
|---|---:|---:|---:|
| All outcomes | 92 | 57 (62.0%) | 538,786,359 |
| `drained` | 23 | 18 | 52,548,682 |
| `drained-with-shelved` | 38 | 18 | 283,218,342 |
| `failed` | 5 | 1 | 56,495,492 |
| `max-reached` | 20 | 17 | 27,682,842 |
| `mismatched` | 6 | 3 | 118,841,001 |

Nine records had no role and all nine were zero; 83 were advisor-role records and 48 were zero. Zeros do **not** uniquely correlate with outcome or shelving: every outcome class has both zero and nonzero records except the small role-null slice. `drain_summary` has no vendor field, so vendor correlation cannot be tested from that file. The record has drain-level outcome, role, iterations, shipped/shelved counts, elapsed time, diff statistics, and one aggregate token number; it has no spec ID, session ID, phase, vendor, seat, round ID, or per-model/token-class breakdown.

### Known-window blind spot

For the specified 22-hour window, ten persisted drain summaries total 167,943,259 tokens, 47 iterations, 16 shipped and 25 shelved; one empty zero-iteration drain recorded zero. The operator's independently analyzed work window reports approximately 55.1M tokens for 19 merged specs (19 × 2.9M). These are not reconcilable populations: the summary rows aggregate whole drains and include work outside the 19-spec cohort, while the study figure is cohort-normalized. The available `.aida/drain-logs/` files stop before this window, so the literal stdout totals for these ten drains are not retained there and a defensible stdout-minus-JSONL delta cannot be reconstructed after the fact.

That inability is itself the blind spot: `usage.jsonl` cannot select the 19 specs or their phases/rounds, and ephemeral stdout cannot serve as a ledger. The report does not substitute unlike totals to manufacture a percentage.

## Part 2 — usable sources already on disk

| Vendor/source | Observed shape | Complete enough? | Existing correlation | Per-message attribution without API? |
|---|---|---|---|---|
| Claude Code `~/.claude/projects/<slug>/*.jsonl` | assistant `message.usage`: input, cache-create, cache-read, output, thinking; message/session/model IDs | Yes for the sampled window: 2,938/2,938 assistant rows had usage. Duplicate message representations exist, so deduplicate by message/request identity. | Transcript `sessionId`; AIDA manifests store `claude_session_id` and `spec_id`; headless filenames also carry spec/session | **Yes** |
| Codex `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` | `event_msg` / `token_count`; `total_token_usage` and `last_token_usage` include input, cached input, cache-write input, output, reasoning output, total; session metadata contains model/cwd | Yes for responses that emit `token_count`; use `last_token_usage`, not repeated cumulative totals | Rollout/session ID and cwd; AIDA phase/session metadata and headless filename provide the spec/run join | **Yes** |
| `codex exec --json` stream captured by AIDA | Codex emits token-count events, but the inspected AIDA headless logs in the window contained zero `token_count` events and zero Claude `result` events | The vendor emits usage, but today's captured AIDA stream is not a proven durable copy of it | Potential session/phase filename join | **Not from the current captured files; yes if the existing emitted event is retained** |
| Antigravity `~/.config/Antigravity` | Editor caches and logs; no demonstrated per-message token record | Unknown | No proven session/spec join | **No, not established from artifacts already kept** |

Local artifacts estimate consumption; vendor APIs are still needed for authoritative billing reconciliation, credits, negotiated rates, delayed adjustments, and organization/project totals.

## Part 3 — prior art to reuse

### Vendor usage and cost APIs

- **Reuse:** authoritative time-bucketed usage/cost endpoints for reconciliation. OpenAI explicitly distinguishes granular Usage from the Costs endpoint and recommends Costs for invoice reconciliation ([OpenAI Usage API](https://platform.openai.com/docs/api-reference/usage)). Anthropic exposes organization usage/cost reporting through its Admin API ([Anthropic Usage and Cost API](https://docs.anthropic.com/en/api/usage-cost-api)).
- **Adapt:** join vendor buckets to locally known vendor, model, project/API key, and time window; retain a reconciliation status rather than overwriting local message facts.
- **Do not:** require an API call for every message or treat an API aggregate as phase/spec attribution. It is a reconciliation source, not the local causal trace.

### OpenTelemetry GenAI semantic conventions

- **Reuse:** the stable semantic vocabulary and types for model/provider and token usage; the registry covers GenAI usage attributes and reasoning-token detail ([OpenTelemetry GenAI attributes](https://opentelemetry.io/docs/specs/semconv/registry/attributes/gen-ai/)).
- **Adapt:** map Claude cache-create/cache-read and Codex cached/reasoning fields without discarding the vendor-native payload.
- **Do not:** build an exporter or tracing backend in the next slice merely to obtain naming compatibility. Use the names in the local normalized record first.

### ccusage and related local analyzers

- **Reuse:** parser and deduplication behavior, especially Claude `message.usage`, Codex `last_token_usage`, session aggregation, and model normalization. The current ccusage project supports both Claude and Codex local JSONL and is MIT licensed ([ccusage repository](https://github.com/ccusage/ccusage)); its cost modes already separate recorded cost from recalculation ([ccusage cost modes](https://github.com/ccusage/ccusage/blob/main/docs/guide/cost-modes.md)).
- **Adapt:** isolate the minimum parser fixtures/logic behind an AIDA normalization boundary; validate against AIDA's observed files before vendoring or depending on a package.
- **Do not:** recreate a general usage dashboard, billing-block UI, or pricing engine. Those are mature adjacent capabilities and outside the attribution gap.

### Published price maps

- **Reuse:** LiteLLM's machine-readable model price/context table as upstream data ([LiteLLM model price map](https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json)). ccusage demonstrates a useful operational pattern: pin a snapshot and update it through a tested scheduled change rather than silently using mutable network data.
- **Adapt:** a small versioned rates-data snapshot with exact raw-model aliases, effective dates, source URL/revision, currency, input/output/cache categories, and explicit unknown/unpriced state.
- **Do not:** compile prices into branching application code or silently fuzzy-match unknown models. Rates change independently of releases and estimates must remain auditable.

## Part 4 — decision-changing arithmetic

### Cost per merged spec

Measured input: approximately **2.9M tokens per merged spec**. For 19 merged specs:

```text
19 × 2.9M = 55.1M tokens
1 / 0.75 merges/hour = 1.333 hours per merged spec
19 / 0.75 = 25.33 throughput-hours
```

Dollar cost cannot be responsibly collapsed to one number without the model and token-class mix: cache reads, cache writes, uncached input, and output have different rates. The correct result from today's evidence is **2.9M tokens and 1.33 throughput-hours per merged spec**, with dollars deferred until the normalized mix is joined to dated rates.

### Rework tail

Treating the 40 shelved rounds plus the 19 ultimately merged rounds as the observed round population:

```text
tail share = 40 / (40 + 19) = 67.80%
tail tokens ≈ 55.1M × 67.80% = 37.36M
productive-round tokens ≈ 17.74M
tail throughput-time ≈ 25.33h × 67.80% = 17.18h
productive-round time ≈ 8.15h
```

This is a proportional estimate, not a claim that every round has identical usage. Per-round/phase records are needed to replace it. Even with that limitation, a two-thirds tail is large enough that the evidence favors review round-trip work (TASK-1289/1290/1291) as the first optimization to evaluate, ahead of model choice. The advisor should decide whether to act on that signal.

### Cache-hit ratio

For the observed message with 2 uncached input tokens and 868,439 cache-read input tokens:

```text
cache-read share of read + uncached input
= 868,439 / (868,439 + 2)
= 99.99977%
```

This is a one-message sample and excludes cache creation and output tokens, so it is not a fleet-wide hit rate. It does establish that, for that turn, context reuse utterly dominated uncached input. If representative, prompt/context stability and avoiding unnecessary cache invalidation are stronger levers than trimming the two uncached tokens.

## Proposed next slice (for advisor approval)

The smallest next slice would be a read-only normalizer, not a new collector or schema migration:

1. Parse existing Claude and Codex transcript records using tested, ccusage-informed deduplication.
2. Emit a local derived table keyed by vendor session/message plus AIDA run/spec/phase/round joins; retain raw token classes and raw model name.
3. Use OpenTelemetry-compatible attribute names while preserving vendor extensions.
4. Join a pinned, dated rates-data snapshot for estimates; represent missing rates explicitly.
5. Reconcile aggregates asynchronously against vendor usage/cost APIs and store the variance/provenance.
6. Measure the 40-round cohort directly, then return the phase/round token and wall-clock distribution to the advisor before choosing review-loop, cache-stability, or model-routing work.

Open questions requiring advisor decisions: whether the derived table belongs in the existing local cache or a separate artifact; whether vendor reconciliation is in the next slice; and whether TASK-1289/1290/1291 should precede observability implementation based on the 67.8% estimate.
