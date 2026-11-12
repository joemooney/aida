# Gate-ledger selection budgets

<!-- trace:BUG-2531 | ai:codex -->

Cross-label verdict selection derives a spec-to-change inventory from strictly
parsed change ledgers. It cannot rely exclusively on `by-spec`: appends can
succeed before index updates fail, and a missing index must not hide a refusal.

Discovery discards payloads after inspecting each file under the existing file,
line and event limits. The inventory has its own aggregate byte/event budget;
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

The fixtures cover more than 64 MiB of unrelated closed history, an absent
selected-spec index, exhaustion by relevant history, exact budget accounting
for a shared change, and the doctor's threshold boundary. The existing alias
suite also covers torn/malformed/misbound/linked evidence and missing indexes.

This is a partial remediation. Discovery still inspects all active change
payloads once; its I/O scales with active history. Automatic closed/merged
rotation requires an archive-aware retention contract for indexed selection,
direct change readers, audit/carry-over, append/import and reopened changes.
Moving ledgers and pruning indexes alone can lose refusal evidence. No archive
rotation or index pruning is performed by this slice.

Validation in the native lane on 2026-11-12:

- `rustfmt --edition 2021 --check` on the five changed Rust files: passed.
- `git diff --check`: passed.
- Targeted `bug_2531` lane job `j20261112T084821116-2700781-0`: exit 124,
  timed out after 900 seconds during slot wait/compilation; no tests executed.
- Alias-suite lane job `j20261112T085138412-2729924-0`: pending, waiting for a
  Cargo slot at handoff. No executed-test result is available: `untested=true`.

BUG-2531 was punted as a design fork for archive retention. A non-completing
Refs publication declaration was recorded; the punt remains a publication veto.
All checkout changes are uncommitted.
