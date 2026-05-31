# SPIKE-46 store interop prototype

This directory contains a tiny non-AIDA Python prototype that reads and writes
the git-canonical `.aida-store` object format directly.

The purpose is empirical: prove the multi-vendor thesis in the smallest useful
shape. A non-Rust/non-AIDA tool should be able to read the store broadly, but
write only through a bounded conformance shape.

## Prototype

```bash
python3 docs/architecture/spike-46-store-interop/store_interop_prototype.py read-report
python3 docs/architecture/spike-46-store-interop/store_interop_prototype.py write-status SPIKE-46 in-progress
```

`read-report` parses every `objects/**/*.yaml` file in the real `.aida-store`
and reports whether key fields are reachable.

`write-status` performs a bounded status flip:

- update the top-level `status`
- update or insert top-level `modified_at`
- append one `history` entry recording the status change
- reparse the patched object with the same Python loader

By default `write-status` writes to a temporary sandbox copy of the selected
real object and prints a unified diff. Pass `--apply` only when intentionally
mutating the real store.

## Finding

Read interop is easy. A stock YAML parser plus a small adapter for AIDA's
fallback-tolerant `!Custom` relationship tag can parse the live object corpus
and reach the fields agents need.

Write interop is bounded. Generic YAML re-emission is the wrong contract
because it rewrites formatting and field presentation. The viable vendor
contract is operation-shaped: a tool may implement narrow mutations that
preserve the rest of the object byte-for-byte. The prototype status flip is
one such operation.

For a publishable broader write contract, use
`docs/architecture/on-disk-serialization-surface.md` as the source inventory:
sorted collections, fallback-tolerant relationship types, and optional-field
omission are the key emitter constraints.

## Empirical run

Run on 2026-05-31 against this repository's live `.aida-store`:

```text
Objects parsed: 1635/1635
Objects with id/spec_id/status/req_type reachable: 1635/1635
Relationship kinds observed: BlockedBy, Child, Custom:implemented-by,
  Custom:implements, Custom:sprint_assignment, Custom:sprint_contains,
  Parent, References
```

The sandboxed bounded-write demo against `SPIKE-46` produced a narrow diff:

- `status: InProgress` -> `status: Completed`
- `modified_at` updated
- one `history` entry appended with `field_name: status`

No real store object was mutated during the default demo; the patch was written
to a temporary copy and reparsed successfully.

trace:SPIKE-46 | ai:codex
