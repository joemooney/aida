# Automatic conflict-resolution lane sketch

Status: proposed; requires master sign-off before production implementation.
<!-- trace:STORY-2710 | ai:codex -->

The existing rework pickup already performs a host merge and routes fresh
conflicted pickups to Claude in `queue_cmd/work.rs::apply_rework_conflict_route`.
It refuses a resumed non-Claude thread. Its conflict prompt currently instructs
the child to resolve, commit and push without an explicit compile gate.
`review_watch_update.rs::attempt` instead records a refused forge update and
routes it to the advisor. These are separate entry points; a forge refusal is
not evidence that a local host merge was attempted.

## Proposed control flow

1. Keep live-author and unknown-liveness protections in review-watch. Only a
   positively observed conflict on an open, same-repository PR with an exact
   head, branch, base and unambiguous backing spec may request a conflict lane.
   Network/auth failures retain the existing retry and advisor behavior.
2. Have the watcher request an ordinary fresh queue-work pickup pinned to Claude
   and the existing PR branch. Reuse its lease, vendor admission, containment,
   host-merge and rollback checks. The watcher must not run a local merge or
   Cargo itself. Revalidate the PR head before admission; a changed head is
   rediscovered rather than repaired using stale evidence.
3. Reserve dispatch under shared state locking before spawning. Persist an
   exact forge/repository/PR/head key and invocation identity. A confirmed spawn
   keeps the reservation until observed completion; an ambiguous spawn retains
   capacity and cannot trigger another lane. Failed pre-spawn setup can retry
   only after cleanup is confirmed. Use the same reservation for watcher and
   rework requests so concurrent entry points cannot launch duplicate repairs.
4. In the admitted Claude lane, repeat the host merge against the pinned base.
   A clean merge uses the ordinary publication path. A real conflict enters the
   shared repair policy below. Never convert an existing Codex resume thread
   into a Claude resume: request a fresh admitted lane after releasing its seat.
5. Before publication, require no unmerged entries or conflict markers and a
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

Resolve authoritative source conflicts first. Restore only registered generated
outputs to a parseable starting state, run their generators once in dependency
order, and stage only validated declared outputs. Refuse generator failures,
unexpected writes or unresolved source dependencies; never accept a ledger by
unioning conflict sides or enlarging an allowance merely to pass a guard.

Mechanical source repair permits independent appended tests/document sections
when both sides remain intact and identifiers are unambiguous. Changes to the
same behavior, contradictory documentation or overlapping declarations stop the
lane. Preserve diagnostics and the original conflict paths for advisor triage.

## Refusal delivery and state decision requiring sign-off

Persist a refusal notice keyed by repository/PR/original head, including paths,
reason, lane invocation and validation result. Deliver through ordinary advisor
mail routing and mark accepted delivery in the same existing notice protocol.
Polling must not resend an accepted notice for that head; a new head may notify
again. A send/receipt crash window has the mailbox protocol's existing semantics,
so strict exactly-once delivery must not be claimed without an idempotent send.

Approve extending shared watcher state for conflict dispatch reservations, or
choose a separate shared repair journal usable by direct rework as well. The
recommended choice is a shared repair journal with a versioned schema, while
watcher state retains only its reference. This avoids making direct queue-work
depend on the watcher's private state format. This is an orchestrator semantics
and persisted-state change covered by the repository sketch-first gate.

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
