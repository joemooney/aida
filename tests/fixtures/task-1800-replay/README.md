# TASK-1800 replay fixture

<!-- trace:TASK-2067 | ai:claude -->

`audited-cases.bundle` lets CI run the mandate trace-attribution zero-loss
gate (TASK-2068). The full replay (`make task-1800-replay`) needs the 50
pinned squash-merged PR heads, which CI's clone does not have.

## What it holds

For each of the 18 cases in
[`docs/testing/task-1800-semantic-audit.json`](../../../docs/testing/task-1800-semantic-audit.json)
the bundle has two refs, `refs/task-1800/<pr>/base` and
`refs/task-1800/<pr>/head`. Each is a root commit whose tree holds only the
paths the real `base..head` diff changes, with the real blobs and modes. A
`git diff-tree` of the synthetic pair therefore lists the same paths and
blobs as the real pair, so the production collector sees the same evidence.
Commit metadata is fixed, so each commit ID depends only on the real diff.

`task_1800_audited_cases_replay_from_the_vendored_bundle`
(`aida-cli-lib/src/tests/task_1800_trace_replay_tests.rs`) fetches the bundle
into a temporary repository under `diff.renames` unset, `false` and `copies`.
It checks each case's scoped digest, fallback set and changed paths against
`docs/testing/task-1800-trace-exclusions.json` and the audit, re-hashes every
audited blob, and fails if any true-touch or unresolved pair is lost.

## Regenerate or verify

Both need the pinned PR heads in the local clone:

```bash
scripts/task-1800-replay-fixture.py           # rewrite the bundle
scripts/task-1800-replay-fixture.py --check   # rebuild and compare commit IDs
```

`--check` proves that the committed bundle matches a rebuild from the frozen
heads, ref for ref and commit ID for commit ID. With the same git version the
rebuilt file is also byte-identical.
