# Shift discovery and cron budget

<!-- trace:BUG-2626 | ai:codex -->

The scheduler now gathers its queued candidates and parked re-drive candidates
before discovering holds. `held_specs` resolves just those labels through
BUG-2531's certified write-ahead bindings and by-spec hints. It validates the
bound change ledgers strictly, then uses the existing hold fold and legacy
marker read-through. It no longer calls the exhaustive `list_holds` surface.
Old unindexed ledgers are covered by the BUG-2531 bootstrap; a missing binding
with surviving evidence uses that reader's full-read recovery. A missing
certificate or unsafe/missing bound evidence blocks every requested spec.
The generic hold listing remains exhaustive for its other callers.

This uses the existing binding contract: hold writers put the hold's spec on
the event before binding and appending; the bootstrap binds pre-existing event
specs. Hand-restored ledgers require the BUG-2531 heal, as documented there.
The normal certified path reads no unrelated change ledger or ledger-root
inventory. Corruption recovery can still scan; it does not silently drop holds.
Legacy marker enumeration remains, so work can scale with legacy holds and
with the requested specs' bound change histories.

Quiet lease cleanup skips the unshipped-work diagnostic report. Interactive
`scan_reapable` still adds that report. All lease ownership probes remain in
`scan_reapable_with`; no ownership-liveness allowance moves or grows.

## Regression evidence (AC2 and AC3)

`bug_2626_tick_discovery_scales_with_relevant_changes` runs the production
`gather_probes` (queue, parked specs, held_specs and other tick probes), plus
quiet cleanup, twice each with 8 and 512 unrelated ledgers. Each invocation
opens fresh discovery. It asserts equal checked payload bytes, lossy-reader
bytes, parse attempts and checked directory entries across all four runs.
The exhaustive hold-candidate loop also counts its directory entries, so
restoring that call fails the guard. The relevant hold starts as an old ledger
without a hint or binding and is bootstrapped. The test asserts it stays held,
then sees a new release immediately; missing bound evidence or a missing
certificate holds fail closed. These are work counters, not a wall-clock
assertion subject to CI machine load.

Run through the launcher:

```
aida lane job run -- cargo test --profile agent -p aida-cli-lib --lib -- bug_2626
```

Also run:

```
python3 -m unittest tests.test_ownership_liveness_ratchet tests.test_measure_shift_tick
```

## Required host evidence (AC1, still pending)

No production timing is claimed by this lane. Its Git metadata is read-only;
the host must commit, deploy and measure the resulting binary. A timing of the
currently installed binary would not measure these edits. The earlier
lock-contended 0.09-second result is not acceptance evidence.

After deployment, the host can use `scripts/measure-shift-tick.py` as a cron
wrapper for the existing enabled night-shift invocation. Supply the actual
primary checkout, absolute deployed binary and its source SHA:

```
python3 /path/to/deployed/scripts/measure-shift-tick.py \
  --root /home/dirtlab/ai/aida --binary /absolute/path/to/aida \
  --deployed-sha <deployed-source-sha>
```

The wrapper runs a real `aida shift tick --json` under a 120-second timeout;
it can perform the normal tick's cleanup and launches. Do not run it as an
extra experimental tick on a host where that behavior is unwanted. Its JSON
records UTC time, exact argv/root/binary, host-attested source SHA, binary hash,
ledger count/bytes, by-spec count, output, exit code and elapsed seconds.
Disabled, dry-run, lock-contended, refused hold-discovery, malformed-output, failed and timed-out runs
do not pass. `tests.test_measure_shift_tick` checks the false-success cases
with a fake executable; it is not production timing evidence.

Attach a cron log containing `accepted: true`, with representative live-ledger
counts and elapsed time below 120 seconds, before closing BUG-2626. Until then
AC1 and the corresponding review finding remain unresolved; AC2/AC3 code and
regression validation alone do not establish completion.

## Lane validation, 2026-11-16

- Final scaling guard: launcher `j20261116T152219142-2-0`, 1 passed,
  0 failed (test execution 0.50 seconds; this is fixture timing, not AC1).
- Scheduler suite: `j20261116T152234204-2-0`, 110 passed, 0 failed.
- Lease-reaping matrix: `j20261116T152008412-2-0`, 18 passed, 0 failed.
- Python ownership-liveness ratchet and cron-probe checks: 3 passed.
- Rust formatting check on all changed Rust files and `git diff --check`: passed.

An earlier scheduler run failed the new marker fixture because it serialized
schema-1 legacy data as a typed JSON marker. Setting the fixture's schema to
2 corrected it; the final guard and scheduler suite above passed afterward.
The ownership-liveness inventory is unchanged. Production cron timing remains
pending the host deployment and measurement described above.
