# Gate-ledger selection budgets

<!-- trace:BUG-2531 | ai:codex -->

The binding advisor ruling of 2026-11-12 13:51 UTC approves two slices of
BUG-2531. PR #3872 is slice 1 and must use a `(Refs BUG-2531)` trailer.
Both P2 findings from the 13:30Z review are accepted for slice 2 of this same
spec; they do not require new draft tasks. Review slice 1 for budget enforcement,
regressions and tests. This supersedes the earlier unapproved-split/punt narrative
below; existing substrate punt or hold signals still require host resolution.

Discovery now shares one 128 MiB/200,000-event budget across all active
change ledgers, including torn-line retries. A metadata size check refuses
before payload I/O when a file cannot fit the remaining budget. Reads of a
concurrently growing file are limited to the smaller of the file cap and
remaining budget, plus one overflow-detection byte. Event charging precedes
JSON deserialization. Budget exhaustion is unsafe evidence, never absence.
The inventory and selected-payload budgets remain separately bounded. Selected
changes are cached across labels and charged once to the selection budget.

This is a resource-safety repair, not selective discovery: an unrelated active
history over the discovery budget still blocks a small selection. Missing or
incomplete `by-spec` hints cannot safely authorize skipping an active ledger,
since `store::write_events` persists and syncs events before attempting index
updates, whose failures are warnings. A selective reader needs an authoritative
binding inventory with an append/index failure protocol, migration of existing
ledgers, and snapshot validation. Until that mechanism exists, scanning is
bounded and fails closed rather than silently trusting incomplete hints.

Slice 2 must run in a new lane and branch:

- Finding 1: discover only ledgers relevant to the requested change/spec without
  trusting lossy by-spec hints. Missing/stale hints must not hide refusals, and
  unrelated history beyond the discovery ceiling must not block selection.
- Finding 2: merge/close must automatically archive the change ledger while
  preserving indexed refusal evidence, direct reads, and reopened-change
  append/revival. Include retention regressions. The manual 2026-11-12 01:35
  archival must never be needed again.

No slice-2 implementation is added to #3872. The raised discovery ceiling is a
mitigation, not completion. Selected payloads retain their separate 64 MiB budget.

The existing doctor capacity warning remains at 32 MiB, using metadata only.
The 4,096-change/16,384-directory-entry limits remain; they are not retention.

Validation command:

- `aida lane job run -- cargo test --profile agent -p aida-cli-lib --lib -- bug_2531`

Results are recorded in the final handoff. No completion credit is claimed.

The following is historical round-2 evidence, not current validation or scope:

Round-2 handoff validation (2026-11-12):

- `rustfmt --edition 2021 --check` on both changed Rust files: passed.
- `git diff --check`: passed.
- Initial test job `j20261112T101412211-2-0`: compiling at handoff;
  superseded by the final-source run after adding measured-work counters.
- Final-source test job `j20261112T101543446-2-0`: queued behind the initial
  compilation at handoff. No executed-test result is available: `untested=true`.
  Resume with `aida lane job wait j20261112T101543446-2-0`.
- BUG-2531 punt recorded successfully; store change queued for host commit.
  Non-completing Refs publication declaration recorded; punt veto remains.
  Checkout edits remain uncommitted as required by the native-seat contract.


Round-5 host handoff:

- Publication subject: `[AI:codex] fix(gate): retain approved discovery mitigation and scope split (Refs BUG-2531)`.
- PR #3872 title must end in `(Refs BUG-2531)`.
- PR body text: “Slice 1 of BUG-2531, per joe's binding 2026-11-12
  13:51 UTC ruling. Raises the aggregate discovery ceiling to 128 MiB while
  retaining the 200,000-event ceiling and separate selected-payload budget.
  Both P2 findings from the 13:30Z review are accepted as slice 2 of this same
  spec, on a new branch/lane: relevant-only discovery without trusting lossy
  hints, and automatic merge/close archival preserving refusal evidence,
  direct reads and reopened-change revival. This PR does not complete BUG-2531.”
- Native seat leaves edits uncommitted for the host to commit and publish.
- Formatting and `git diff --check` passed. Final-source executed-test evidence
  is pending job `j20261113T021212493-2-0`; earlier runs are superseded.
- Host must verify the destination: this checkout HEAD is preserved WIP
  `622c7081ad`, not reviewed PR head `78dfea63a192`. Do not replace unrelated
  PR changes with this older checkout. Apply the slice-1 scope record and
  preserve the reviewed mitigation when publishing.
