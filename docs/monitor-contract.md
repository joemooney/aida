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
`aida pr ship` does, and since `1.2.0`, a recorded review verdict
(`aida review record`) and a disposition or `execution_mode` change
(`aida edit --status`/`--mode`, including groom/approve/reject, which shell
out to `aida edit`). **Events written before `1.2.0` still miss the review
and disposition/mode paths** — a review verdict recorded, or a spec approved,
rejected, deferred, or re-classified by execution_mode outside a drain phase,
emitted nothing before this version. Any seat count or activity count a
consumer computes from an event log that spans back before `1.2.0` is
therefore a **lower bound**, not a complete account. `seat` (present on
`MergeHoldChanged`, `ReviewVerdictRecorded`, `DispositionChanged`,
`ExecutionModeChanged`, and, when known, on `PrMerged`) is `null`/absent on
every event recorded before this field existed, and on drain-phase events,
whose actor is instead identified by `run_uuid`. // trace:BUG-1423 | ai:claude // trace:TASK-1450 | ai:claude

## Migration notes

- `1.3.0` — the new `GateHeld` event kind records a gate that REFUSED or
  HELD (`gate`, optional `pr`, `reason`, optional `actor`, plus `seat`):
  the merge-hold clear floor, the `aida pr ship` hold-release refusals, a
  stale approval, a review in progress, a refused fresh pickup (BlockedBy
  named distinctly), a closure hold, and an ambiguous-id refusal. A human
  `aida merge-hold clear` now also emits `MergeHoldChanged { placed: false }`,
  so floor refusals can be counted against releases
  (`aida history --kind gate-held`). `GateHeld` is not actionable (absorbed by
  `aida watch`). Before `1.3.0` no refusal was recorded: any refusal count
  over an older log is zero by omission, not by measurement. Additive only.
  (STORY-1436) // trace:STORY-1436 | ai:claude
- `1.2.0` — `aida review record` now emits `ReviewVerdictRecorded` (spec, PR,
  verdict, reviewed sha); `aida edit --status`/`--mode` now emit
  `DispositionChanged`/`ExecutionModeChanged` with before/after values
  (covers groom/approve/reject, which shell out to `aida edit`). All three
  are seat-tagged. Additive only — no covered field changed shape. (TASK-1450)
- `1.1.0` — `aida pr ship` now emits `PrMerged` (parity with the drain merge
  phase) and the new `MergeHoldChanged` event kind for a coordination-seat
  hold placed/lifted; `Event` gained an additive `seat` field. Additive only —
  no covered field changed shape. (BUG-1423)
- `1.0.0` — initial contract: eleven polling surfaces and the events follow
  feed. The promised subset is recorded in `monitor-contract-fixtures/`.
