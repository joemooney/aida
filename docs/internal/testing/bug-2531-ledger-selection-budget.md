# Gate-ledger selection budgets

<!-- trace:BUG-2531 | ai:codex -->

Round 2 does **not** complete BUG-2531. Both review findings are accepted.
No approved scope split exists. The prior per-file discovery budget allowed
up to 64 GiB of payload work; that regression is removed in this round.

Discovery now shares one 64 MiB/200,000-event budget across all active
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

Finding 1 is therefore split: the aggregate I/O/parse regression is repaired;
bounded discovery of only relevant changes remains open. The new small-budget
regressions prove an over-budget unrelated malformed file is refused before
payload read/parse, and that unrelated files share the event limit. The large
history regression now expects an unsafe refusal rather than endorsing an
unbounded scan. Existing alias tests retain missing-index refusal coverage.

Finding 2 remains open: no automatic closed/merged rotation is implemented.
`store::ledger_file`, `read_file`, append/import and checked direct reads still
address the active directory. Selection rejects missing indexed changes.
Moving files alone breaks those readers; pruning indexes can hide refusals.
A complete implementation must preserve bindings to archived evidence, read
archived changes directly, and serialize revival with append and rotation
under the ledger-wide lock. Closure must be established independently of a
verdict's untrusted `closed_by_merge` field. Retention and rotation regressions
cannot honestly pass without that mechanism. TASK-3098 is a design follow-up,
not authorization to drop this requirement. The explicit BUG-2531 punt keeps
both unfinished parts visible and vetoes completion/publication.

The existing doctor capacity warning remains at 32 MiB, using metadata only.
The 4,096-change/16,384-directory-entry limits remain; they are not retention.

Validation command:

- `aida lane job run -- cargo test --profile agent -p aida-cli-lib --lib -- bug_2531`

Results are recorded in the final handoff. No completion credit is claimed.

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
