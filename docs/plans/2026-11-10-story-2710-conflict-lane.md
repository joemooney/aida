# Automatic conflict-resolution lane sketch

Status: merge mechanics ruled by advisor on 2026-11-10 at 22:22 UTC.
Shared versioned repair journal approved by advisor on 2026-11-11 at 01:28 UTC.
Story implementation is incomplete.
<!-- trace:STORY-2710 | ai:codex -->

The existing rework pickup already performs a host merge and routes fresh
conflicted pickups to Claude in `queue_cmd/work.rs::apply_rework_conflict_route`.
It refuses a resumed non-Claude thread. Its conflict prompt currently instructs
the child to resolve, commit and push without an explicit compile gate.
`review_watch_update.rs::attempt` instead records a refused forge update and
routes it to the advisor. These are separate entry points; a forge refusal is
not evidence that a local host merge was attempted.

## Binding host-staged merge boundary

The advisor ruling supersedes the original Claude-only proposal and spec wording.
The host fetches and merges `origin/main` in the seat clone before launch. A real
conflict retains `MERGE_HEAD`, unmerged index entries and working-tree markers.
The host restores registered regenerable outputs from main and regenerates them
before launch. The configured implementer vendor edits only working-tree files;
its brief lists conflict paths. In particular, Codex `.git` stays read-only.
Neither a vendor reroute to Claude nor a writable Codex `.git` is permitted.

After resolution the host publication path scans for opening or closing conflict
markers, stages and commits the files, completing the pending merge, then pushes
through existing publication vetoes. AC1 uses the configured implementer vendor.
AC6 requires an end-to-end Codex fixture with read-only `.git`; AC7 requires
publication refusal for either remaining marker in a conflicted path. The former
FAQ-150 step 6 and BUG-2456 Claude routing are retired by the ruling, but their
production code and documentation still need migration.

## Proposed control flow

1. Keep live-author and unknown-liveness protections in review-watch. Only a
   positively observed conflict on an open, same-repository PR with an exact
   head, branch, base and unambiguous backing spec may request a conflict lane.
   Network/auth failures retain the existing retry and advisor behavior.
2. Have the watcher request an ordinary fresh queue-work pickup using the
   configured implementer vendor and the existing PR branch. Reuse its lease, vendor admission, containment,
   host-merge and rollback checks. The watcher must not run a local merge or
   Cargo itself. Revalidate the PR head before admission; a changed head is
   rediscovered rather than repaired using stale evidence.
3. Reserve dispatch under shared state locking before spawning. Persist an
   exact forge/repository/PR/head key and invocation identity. A confirmed spawn
   keeps the reservation until observed completion; an ambiguous spawn retains
   capacity and cannot trigger another lane. Failed pre-spawn setup can retry
   only after cleanup is confirmed. Use the same reservation for watcher and
   rework requests so concurrent entry points cannot launch duplicate repairs.
4. The host merges the pinned base before launching the admitted lane. A real
   conflict stays staged in the seat with `MERGE_HEAD` and the original path
   list retained; the lane resolves only working-tree files. A clean merge
   uses the ordinary publication path. Resumes retain their original vendor.
5. Before host staging, require no remaining conflict markers in the listed
   paths. Unmerged index entries are expected until host `git add`; require a
   successful `cargo check --profile agent -p aida-cli-lib --tests` on the exact
   merged tree. Any subsequent source change invalidates that check. Push only
   to the same PR branch with expected-head protection; preserve ordinary
   publication vetoes. This does not grant merge, review or completion credit.

## Generated-file and source policy

Keep a compiled, closed table of output paths, authoritative inputs and fixed
argv regeneration commands. Initially register the exec-context ledger with
`python3 scripts/check-exec-context.py --write-ledger`, and enumerate template
mirror outputs from the existing template sync mechanism using
`make sync-templates`. Inspect the Makefile's symlink behavior before choosing
the exact output set; do not treat all of `.claude/` as generated. Other ratchet
ledgers enter this table only with reviewed generators and source dependencies.

At host preparation, restore registered generated outputs from the pinned main
commit and regenerate them in dependency order. If a generator needs conflicted
authoritative sources, refuse with those paths rather than generating a partial
ledger. Run generators inside the host-owned sandbox; never execute a generator
from a lane-modified checkout with host privileges. Validate declared outputs. Refuse generator failures,
unexpected writes or unresolved source dependencies; never accept a ledger by
unioning conflict sides or enlarging an allowance merely to pass a guard.

Mechanical source repair permits independent appended tests/document sections
when both sides remain intact and identifiers are unambiguous. Changes to the
same behavior, contradictory documentation or overlapping declarations stop the
lane. Preserve diagnostics and the original conflict paths for advisor triage.

## Approved shared repair journal

Persist a refusal notice keyed by repository/PR/original head, including paths,
reason, lane invocation and validation result. Deliver through ordinary advisor
mail routing and mark accepted delivery in the same existing notice protocol.
Polling must not resend an accepted notice for that head; a new head may notify
again. A send/receipt crash window has the mailbox protocol's existing semantics,
so strict exactly-once delivery must not be claimed without an idempotent send.

The binding advisor ruling selects one shared repair journal per repository
under `.aida/`, with a schema `version` field and entries keyed by repository,
PR and original head. Each entry retains the dispatch actor, timestamp and lane
identity, ambiguous-spawn reservation, host-recorded original conflict paths,
exact-tree validation evidence and refusal-notice delivery state. An unknown
spawn outcome blocks redispatch for that head until its lane is proven dead.
Watcher state holds only a reference; direct rework must not depend on the
watcher's private state format. Writes use the store writer lock or a journal
lock with the same discipline.

Both the lane brief and publication marker gate must use those host-recorded
original conflict paths, retained across staging retries. A changed-tracked-path
scan is only an optional extra guard. No additional state-contract signoff is
required. The production implementation remains outstanding.

## Validation after approval

Use injected spawn/forge/mail seams to test conflict-only dispatch, live or
unknown ownership refusal, moving heads, concurrent watcher/rework admission,
ambiguous receipts and failed-setup recovery. Exercise real temporary Git merge
fixtures for generated-ledger regeneration, independent appended source changes
and semantic refusal. Assert compile failure prevents push and checks bind the
published tree. Verify repeated refused passes send one accepted notice per
head, with conflicting paths, and a new head permits another notice. Preserve
the watcher no-Cargo/no-local-merge ratchet. Execute targeted Rust tests via the
lane job runner; no implementation or executed-test evidence is claimed here.

## Round 2 implementation evidence and remaining gate

The independent AC7 safety slice adds a bounded, read-only sandbox probe before
`commit_pending` can mutate the index. It scans changed tracked working-tree
paths against HEAD, including unmerged entries and staged resolutions on retry,
for either opening or closing markers. Deleted resolutions are allowed. Literal
NUL-delimited pathspecs handle unusual names without shell interpolation. Probe
errors refuse publication. This deliberately does not make `write-tree` accept
an unmerged index or enable automatic conflict dispatch.

P1 remains open: AC1-6 need production dispatch, host regeneration, merged-tree
compile gating, per-head refusal notices and their regression coverage. AC7 has
a fixture regression, but does not alone establish the full story. No story
approval or completion is requested for this slice. At the end of round 2,
the shared journal state contract still awaited a ruling. The advisor approved
it on 2026-11-11 at 01:28 UTC; that design gate is now satisfied. Production
implementation, rather than further state-contract approval, remains required.

Validation for this slice: lane job `j20261110T222603765-2-0` ran
`cargo test --profile agent -p aida-cli-lib --lib -- story_2710_marker_gate`:
**1 passed, 0 failed, 0 ignored**. The real Git fixture exercises opening-only
and closing-only markers, unmerged and staged resolutions, index/HEAD/MERGE_HEAD
preservation, deletion and filenames with newlines, brackets and shell-looking
text. `rustfmt --edition 2021 --check aida-cli-lib/src/seat_clone.rs` and
`git diff --check` pass. This test does not exercise an end-to-end read-only
Codex launch or host publication; AC6 remains unverified. The lane recorded a
Refs-only native publication declaration and punted P1; the punt remains a
publication veto pending triage. All checkout edits are left uncommitted.

## Round 3 bounded regression fix and outstanding findings

The marker probe now first checks `MERGE_HEAD`. Ordinary publication, including
staged edits to tracked guides with pre-existing literal opening and closing
marker examples, bypasses the conflict-only scan. Probe failures still refuse.
The real Git fixture covers both unstaged and staged ordinary edits alongside
its existing pending-merge and staged-retry refusals.

This is not the approved production slice. P1 remains open in full: shared
journal reservation and dispatch, configured-vendor rework, retained host merges,
registered regeneration, exact-tree compile validation, refusal delivery and
end-to-end read-only Codex evidence are not implemented. P2's original-path
journal integration also remains open: pending merges still use the prior
changed-path scan and cannot complete host staging through `write-tree`. This
round fixes the ordinary-publication false positive only. The advisor ruling
is accepted; no further approval of the state contract is requested. An explicit
punt records the incomplete findings rather than requesting story completion.

Round 3 validation: lane job `j20261111T013744670-2-0` executed
`cargo test --profile agent -p aida-cli-lib --lib -- story_2710_marker_gate`:
**1 passed, 0 failed, 0 ignored** against the final source. Rustfmt check and
`git diff --check` pass. A Refs-only native publication declaration is recorded;
`aida punt` succeeded for the unfinished P1 and P2 integration and remains a
publication veto. No Git commit or push was performed in this seat.

## Round 3 follow-up: explicit conflict-path boundary, findings punted

The marker helper accepts an explicit original conflict-path list. It no longer
reconstructs conflicts from changed tracked paths. The real Git regression
retains that same list across unmerged and staged retries and permits marker
examples in an unrelated changed guide during a merge. Missing, empty, absolute
or parent-traversing path evidence refuses. Ordinary publication remains allowed
without conflict evidence.

Publication does not yet load a shared repair journal, so `commit_pending`
passes no evidence and explicitly refuses every pending merge before staging.
This replaces the prior incidental `write-tree` refusal; it does not implement
the approved host-staged merge flow. The test supplies host paths directly and
does not establish persistence, lane-brief integration or end-to-end acceptance.

P1 and P2 remain open and are not disputed. A successful `aida punt` names both
findings and records that journal persistence/reservation, watcher/direct-rework
integration, configured-vendor routing, retained host merges, regeneration,
exact-tree compile evidence, refusal notices and lane briefs remain unfinished.
This is preparatory code, not an approved scope reduction or story completion.
The journal ruling is accepted without another signoff request. The punt remains
a publication veto. The native-seat edits are left uncommitted for host review.

Follow-up validation: lane job `j20261111T023415446-2-0` executed
`cargo test --profile agent -p aida-cli-lib --lib -- story_2710_marker_gate`:
**1 passed, 0 failed, 0 ignored**. Rustfmt check and `git diff --check` pass.
The test establishes only the explicit-path helper behavior. A Refs-only native
publication declaration is recorded; the punt remains in force. The required
host codegraph client was absent at its supplied absolute path; `aida doctor
check codegraph` also reported the tool missing. Session-manifest writing was
refused by the seat filesystem boundary; no permission expansion was attempted.
