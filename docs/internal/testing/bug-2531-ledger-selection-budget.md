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

## Completeness slice: write-ahead bindings (advisor ruling 2026-11-12 22:55 MST)

The text above describes the earlier safety-cap slice. This slice replaces
the full scan in selection and completes BUG-2531; retention and archival
are TASK-3098.

- A writer persists `gate-ledger/bindings/<SPEC>/<CHANGE>` for every spec its
  events carry before it appends, under the append's locks. A failed binding
  write fails the append.
- `gate-ledger/bindings-complete.json` certifies that every change ledger is
  bound. Selection refuses without a supported marker and names the fix:
  `aida doctor --heal --yes --category gate-ledger-bindings`. That command
  scans every active change ledger under the exclusive ledger lock, writes
  the bindings and publishes the marker last, so an interrupted run leaves no
  marker and a rerun completes it. A ledger with no change ledger at all is
  certified by its first writer.
- Selection, ownership filtering and the effective view read the requested
  labels' bindings and only the change ledgers they name. The `by-spec`
  indexes stay hints: they may add a change, never hide one. A bound change
  whose ledger is missing, or a ledger that records a spec's reviews without
  a binding, refuses and names the fix.
- A selection by spec label reads the label's bindings alone unless it holds
  evidence that a binding existed and is gone. Two things count as evidence:
  the label's `by-spec` hint names a change the bindings do not, or the
  label's bindings directory exists without a hint (a writer, and the fix
  command, leave both; the directory outlives a deleted binding and marks the
  deletion). Only then does the selection read every change ledger strictly
  and take every change whose events carry the label, so a deleted binding
  does not hide a newer refusal. A label never recorded, such as the unused
  alias of a two-label selection, has neither directory nor hint and reads no
  change ledger; a hint that names fewer changes than the bindings is a lossy
  hint and reads only the bound ledgers. The full read retains no payload,
  runs under the bootstrap's work ceiling (4 GiB, 20 million events) rather
  than the selection budget, and is reused within a process while every
  change ledger keeps the size, inode and times it was read with. A change
  ledger it cannot read strictly refuses that selection, naming the file and
  the quarantine command. Limit: when a binding and the hint both lack the
  same change, the label looks exactly like one never recorded on that
  change, and only the deep doctor check (`--category gate-ledger-bindings`)
  reports it.
- The event format is 2. Readers accept 1 and 2. A writer refuses a ledger
  that holds events of a format newer than it writes.
- Binaries older than format 2 cannot write the ledger. Format 2 moved the
  ledger-wide lock to `.aida/gate-ledger.v2.lock`. Before its first write
  (and before the bootstrap certifies anything) a format 2 binary replaces
  the old lock path `.aida/gate-ledger.lock` with a dangling link, under the
  exclusive format 2 lock, and then waits for every older writer that
  already holds the old lock. An older binary opens (or creates) that path
  before every ledger write, so each of its writes now fails with "No such
  file or directory"; the link's target names the reason. Its readers are
  unaffected (its strict reader already refuses format 2 events).
- The fence is part of the completeness certificate. Selection refuses a
  valid marker beside an old lock path that is not the fence, and a writer
  that finds the fence down puts it back and withdraws the marker, because
  an older binary may have appended without binding in between. To run an
  older binary against the store again (a rollback), remove the link; after
  returning to a format 2 binary, run the fix command again.
- Every spec label an event carries is bound. A label usable as a file name
  is its own key; any other label (one with a separator, or one that reads
  as a change key) is bound under `~<sha256 of the normalized label>`. The
  ownership filter therefore finds the witnesses of every review copy, and
  no longer withholds a review whose label had no key.
- The fix command also repairs a ledger whose marker is valid. `aida doctor`
  reports, and the heal rebuilds, older writers not fenced out, a binding
  whose change ledger is gone, a bindings tree that is not plain directories
  of regular files named by changes and, when the category is asked for by
  name (as the fix command does), a change ledger that carries a spec no
  binding names. The rebuild withdraws the marker first, removes whatever
  is not a binding of an existing change ledger, writes the bindings the
  scan found and publishes the marker last.
- The shared discovery of a verdict listing (BUG-2543) is revalidated
  against what it read: the marker, the bindings of each spec it listed and
  the stat identity of each ledger it read.
- An unsafe or oversized ledger now refuses only the specs bound to it and
  the bindings rebuild, not every selection.

Deploy step: after a binary with this slice goes live, run the fix command
once on the live store before any review or train. Long-running processes of
the previous binary (a review watch, a deputy loop) can no longer write the
ledger from that point: restart them on the new binary. A contained reviewer
opens `.aida/gate-ledger.v2.lock` read-only, so the fix command (which
creates it) must have run on the host first.

Rework round 2 (review of #4128 at e1ad931):

- Ownership filtering resolves a label's changes exactly as its selection
  does (bindings, hint and, on evidence of a lost binding, the full read),
  so a deleted binding of a witness's change cannot let a foreign copy win.
  A shared inventory is reused only while each folded label keeps its
  bindings and its hint's stat identity, and while any full read it used is
  still current.
- An ownership read that fails (byte or event exhaustion included) poisons
  the inventory it was building: it leaves the scope, and the discovery
  continues on a fresh one, so a repeated lookup refuses as a fresh one does.
- The bootstrap rewrites every hint to exactly what it bound and removes a
  hint it bound nothing to; unbound `by-spec/<SPEC>.jsonl` ledgers stay.
- A writer that finds the fence down withdraws the marker, durably, before
  the fence goes in, so no read trusts the ledger during the older writers'
  drain or after an interruption right after the rename.
- Certification (the bootstrap's marker, and a first writer's) requires
  verified quiescence: no process may hold the old lock open, read from
  `/proc/<pid>/fd` (its live path or the replaced inode). An older writer
  paused between its open and its lock therefore blocks certification
  until it exits; the fix command is then rerun. Limits: processes of
  another user, or in another PID namespace, are not seen; without a
  readable `/proc` certification refuses; off Linux only the fence's lock
  passes protect.
- The unbound ledger refuses a newer event format before writing, like the
  change ledgers.

A hand-restored change ledger (moved back from the quarantine or the
archive) carries no bindings either: run the fix command after restoring one.

Regressions: `gate::ledger::bindings::tests` (bound-only reads counted by
bytes and parse attempts with unrelated history over the old ceiling, missing
and unsupported marker, bootstrap interruption and rerun, binding/append
boundary, missing or inconsistent bindings, newer-format refusal, shared
discovery after a new binding, the older-writer fence and its wait, a fence
taken down under a valid marker, the fix under a valid marker, a refusal
under a label without a key), plus the adapted
`bug_2543_shared_discovery_tests` and `bug_2542` tests.
