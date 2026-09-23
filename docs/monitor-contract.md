# AIDA monitor contract

Contract name: `aida-monitor`  
Current version: `1.0.0`

This is a versioned, **read-only** interface for external dashboards. It grants
no write or dispatch capability. Consumers must never parse human output and
must depend only on fields listed by `aida contract --json`. A field that is
visible in JSON but absent from that manifest is not promised. If a consumer
needs another field, file an AIDA spec; do not read `.aida-store` directly.

The contract covers eleven polling surfaces and the single follow feed shown by
`aida contract --json`. Additive fields require a minor version bump. Removing
or renaming a covered field, or changing its type, requires a major version bump
and a migration note in this document. Patch releases may clarify docs or fix
behavior without changing the shape.

`aida watch --all --json` follows the local `.aida/events.jsonl` file and writes
one JSON object per newly appended event. It performs no network access. It
starts at end-of-file by default; add `--backlog` to replay existing events
before following, or `--once` to render the existing backlog and exit.

## Consumer example

This one-page shell consumer uses only contracted fields to display drain,
queue, and gate state. Each command remains independently pollable; the follow
feed can trigger a refresh without repeatedly polling while nothing changes.

```sh
drain=$(aida drain status --json)
queue=$(aida queue progress --json)
findings=$(aida findings list --json)

jq -n --argjson d "$drain" --argjson q "$queue" --argjson f "$findings" '
  {drain_running: $d.drain.running,
   in_flight: ($d.in_flight | length),
   queue: {queued: $q.queued, active: $q.in_progress, done: $q.done},
   gates_waiting: ($f.findings | length)}'

aida watch --all --json | jq -c '{at: .ts, event: .kind.event, spec: .spec}'
```

`QueueDrained` is the terminal boundary for a drain, not proof that every
optional diagnostic was measured. In particular, `excluded_from_batch: 0`
means the exclusion count was measured and was genuinely zero; an absent
`excluded_from_batch` means it was not measured (including legacy events and
chained-batch drains). Consumers must preserve that distinction instead of
coercing an absent field to zero. // trace:BUG-1425 | ai:codex

`.aida/events.jsonl` records what a drain does plus, since `1.1.0`, what
`aida pr ship` does. **Events written before `1.1.0` record drain activity
only** — a merge, review verdict, or supervised-merge-hold change performed by
a coordination seat (advisor, product) outside a drain phase emitted nothing.
Any seat count or merge count a consumer computes from an event log that spans
back before `1.1.0` is therefore a **lower bound**, not a complete account.
`seat` (present on `MergeHoldChanged` and, when known, on `PrMerged`) is
`null`/absent on every event recorded before this field existed, and on
drain-phase events, whose actor is instead identified by `run_uuid`. // trace:BUG-1423 | ai:claude

## Migration notes

- `1.1.0` — `aida pr ship` now emits `PrMerged` (parity with the drain merge
  phase) and the new `MergeHoldChanged` event kind for a coordination-seat
  hold placed/lifted; `Event` gained an additive `seat` field. Additive only —
  no covered field changed shape. (BUG-1423)
- `1.0.0` — initial contract: eleven polling surfaces and the events follow
  feed. The promised subset is recorded in `monitor-contract-fixtures/`.
