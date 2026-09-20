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

## Migration notes

- `1.0.0` — initial contract: eleven polling surfaces and the events follow
  feed. The promised subset is recorded in `monitor-contract-fixtures/`.

