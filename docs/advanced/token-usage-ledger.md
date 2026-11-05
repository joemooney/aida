# Local token usage ledger

`aida usage rebuild` creates a project-local derived ledger at
`.aida/derived/token-ledger-v1.sqlite`. The directory and database are private
to the local user where the platform supports Unix permissions. The ledger is
gitignored runtime state, not requirements truth, and can be deleted and
rebuilt at any time.

```bash
aida usage rebuild
aida usage rebuild --json
aida usage show TASK-1427
aida usage show TASK-1427 --group-by phase,vendor,round --json
aida usage show TASK-1427 --cost
aida usage show TASK-1427 --cost --group-by phase,vendor,model,round --json
```

The collector reads already-retained Claude Code and Codex JSONL artifacts. It
does not call either vendor and never copies prompts, responses, tool inputs,
environment variables, or credentials. Provenance is limited to session/event
identifiers, timestamps, content-free source identity, model, and numeric usage
fields. Input, cache creation/write, cache read, output, and reasoning/thinking
remain separate instead of being collapsed into one lossy total.

Claude's repeated representations of one assistant message are deduplicated by
vendor session, message, and request identity. For Codex, a
`token_usage_record`'s stable response ID (scoped to its thread/session) is the
preferred per-response identity. Older `last_token_usage` events are a bounded
fallback for requests without a correlated stable record. Cumulative
`total_token_usage` vectors
only validate fallback checkpoints, suppress repeats, and detect reset
segments; their numeric values are never used as record identities.
Conflicting duplicates contribute no tokens and appear in coverage diagnostics.
In mixed-version files, a legacy representation is suppressed only when its
session, usage, and timestamp or ordinal correlate to a stable response;
distinct legacy requests remain measured with an explicit fallback reason.

Attribution is deliberately conservative. A vendor session must have one exact
AIDA manifest/session match before a spec can be assigned. Timestamps may then
narrow a manifest item or lifecycle phase interval, but timestamp proximity by
itself is never a join. Missing and ambiguous dimensions remain null with a
distinct reason for each of run, spec, seat, phase, and round in the database.
Phase rounds come from the signed `PhaseEntered.attempt` value, including
non-consecutive retries; missing, malformed, or competing phase evidence is
kept null with an explicit reason rather than inferred from event order.
`usage show` prints measured totals together with unknown/unattributed coverage.
Each token class also retains its own known-record and missing-record counts: an
absent class is unknown, a partially observed class is partial coverage, and a
measured numeric zero remains a genuine zero.
Query output reports candidate, measured, non-measured, and unknown-dimension
coverage scoped to the requested spec, plus reason counts for each of run,
spec, seat, phase, and round. Ledger-global collector diagnostics are labeled
separately and must not be interpreted as coverage for the requested spec.

These values are local telemetry, not invoice reconciliation. Deleted or
unsupported transcripts reduce coverage. Historical `drain_summary` token
values predate this schema and are lower bounds because their zero values may
mean collection failure; they are never added to per-message totals. Dollar
pricing is optional and derived. `--cost` reads the bundled, versioned
`data/token-rates/v1.toml` snapshot without making a network call. It matches an
exact provider, raw model (or an explicitly listed alias), and the half-open
effective interval `[effective_from,effective_to)`. Unknown models, missing or
invalid timestamps, ambiguous matches, and token classes without their own rate
remain explicitly unpriced; measured zero remains priced zero.

Rates are decimal USD per one million tokens with at most six fractional digits.
AIDA parses them as integer pico-USD per token, accumulates with checked integer
arithmetic, and rounds only displayed USD values to six decimals using
round-half-even. JSON retains the exact `cost_pico_usd` integer. Raw measured
tokens and ledger provenance are never overwritten.

The rate snapshot records source URL, revision/retrieval date, license, currency,
token unit, and effective dates. Updating it is a reviewed data change: add a new
dated interval/table version rather than rewriting historical prices. Query
output always identifies the table version and source revision. The estimate is
local and is not a vendor bill. Vendor invoice reconciliation, negotiated
discounts or credits, taxes/currency conversion, and real-time budget enforcement
remain out of scope.

Schema version 1 and collector provenance are stored in the SQLite `metadata`
table. Rebuild writes a complete temporary database and atomically replaces the
prior ledger, so repeated ingestion cannot append or double-count records.
Unknown future event fields are ignored; malformed, truncated, and unsupported
events are counted separately from measured zero. Source discovery and JSONL
parsing are streaming and bounded; oversized/non-regular inputs and symlinked
ledger destination components fail closed rather than following
attacker-controlled paths.
