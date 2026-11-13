# Gate-ledger selection budgets

<!-- trace:BUG-2531 | ai:codex -->

Cross-label verdict selection derives a spec-to-change inventory from strictly
parsed change ledgers. It cannot rely exclusively on `by-spec`: appends can
succeed before index updates fail, and a missing index must not hide a refusal.

Discovery discards payloads after inspecting each file under the existing file
and line limits. One scan-wide budget caps actual payload reads at 128 MiB
(plus at most one growth-detection sentinel byte) and JSON deserialization at
200,000 attempts, including unrelated ledgers and torn-line retries. Metadata
rejects a file larger than the remaining budget before reading it. Exhaustion
refuses the whole lookup; it never returns a partial inventory or an approval.
The inventory has its own aggregate byte/event budget;
directory-entry and change-count bounds still apply. Selection reads and caches
only changes discovered for the requested labels, charging those payloads and
the selected by-spec entries to the existing 64 MiB/200,000-event budget. A
change shared by several labels consumes payload budget once. Unsafe evidence
still refuses; the change does not turn malformed unrelated files into absences.

`aida doctor check review-verdicts` reports active change-ledger capacity at
32 MiB (50% of the selection byte budget). It probes file metadata without
reading payloads and offers no automatic healing.

Targeted regressions:

- `cargo test --profile agent -p aida-cli-lib --lib -- bug_2531`
- `cargo test --profile agent -p aida-cli-lib --lib -- bug_2403_alias_selection_tests`

The `bug_2531_discovery_bounds_actual_unrelated_reads_and_parses` regression
counts bytes actually returned by reads and actual deserialization attempts,
with an intentionally small injected scan budget and twenty unrelated refusal
ledgers without indexes. It verifies byte and event exhaustion independently;
resetting the budget per file makes it fail. The production ceiling permits
the existing >64 MiB fixture but prevents the former 64 GiB worst case.

The fixtures also cover more than 64 MiB of unrelated closed history, an absent
selected-spec index, exhaustion by relevant history, exact budget accounting
for a shared change, and the doctor's threshold boundary. The existing alias
suite also covers torn/malformed/misbound/linked evidence and missing indexes.

This is a partial remediation, without an approved scope split. Finding 1's
unbounded discovery-work regression is mitigated by a finite aggregate ceiling,
but discovery still scans active history and refuses above that ceiling. It
does not meet the requirement to read only relevant changes. Finding 2 remains
open: there is no automatic archival, and the 4096-change limit still applies.
Automatic closed/merged rotation requires an archive-aware retention contract for indexed selection,
direct change readers, audit/carry-over, append/import and reopened changes.
Moving ledgers and pruning indexes alone can lose refusal evidence. No archive
rotation or index pruning is performed by this slice.

Round 2 validation on 2026-11-12:

- `rustfmt --edition 2021 --check` on `checked.rs`: passed.
- `git diff --check`: passed.
- `python3 scripts/check-ci-gates.py --audit`: passed (86 gates classified).
- `make check-ci-fast`: stopped when it invoked a direct Cargo check, which
  conflicts with this lane's runner-only build rule; not a passed gate.
- `bug_2531` lane job `j20261112T104457008-2-0`: exit 124, timed out after
  900 seconds during compilation; no tests executed (`untested=true`).
- Alias-suite lane job `j20261112T104910700-4051952-0`: queued/running at
  handoff, without an executed-test result.

Both findings are accepted. The full relevant-only discovery criterion and
finding 2's automatic rotation remain open, without an approved scope split.
The proposed design is in
[the decision sketch](../../plans/2026-11-12-bug-2531-discovery-retention-sketch.md).
BUG-2531 must not be marked complete on the strength of this safety cap.
The native Refs publication declaration grants no completion credit; the
design-fork punt is a publication veto. Changes remain uncommitted for the host.
