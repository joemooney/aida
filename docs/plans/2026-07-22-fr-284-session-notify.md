# FR-284 NOTIFY — tell a live interactive session its spec is done + safe to exit

- **Date:** 2026-07-22
- **Specs:** FR-284 (NOTIFY slice); ADR-23 (channel), ADR-24 (chain posture); child TASK-1179 (CHAIN, filed)
- **Status:** Implemented (guided keystone session)
- **Complexity:** Low-medium (one new pure predicate + a best-effort delivery leg folded into the existing reap pass)

## Approach

FR-284 has three pieces. REAP (TASK-1177) shipped: a supervisor pass reaps a session whose
spec is Done/Completed **and** branch merged **and** process **exited**. That predicate
correctly leaves an *interactive* session alone — it sits alive at its prompt after its spec
merges, so "process exited" is false. NOTIFY closes that half: the same pass that would reap it
were it dead instead sends it a one-time "safe to exit" FYI.

```
reap scan (read-only)  ──► for each lease, gather ReapFacts once
                             │
        ┌────────────────────┼─────────────────────────────┐
        ▼                    ▼                              ▼
  classify_session_reap  session_should_notify        (skip reasons)
   = Reap → tear down     = finished+merged+clean
                            +unlocked+ALIVE → NOTIFY
                             │
                             ▼
        notify_finished_live_sessions (execute, best-effort)
          mailbox FYI (intent=fyi) → Recipient::Agent(current_user_id)
          once-per-session sentinel under .aida/session-notices/<lease-id>
```

The NOTIFY predicate is exactly the reap predicate **with the liveness bit flipped**: a session
is notifiable iff it would be reapable were its process not still running. The two partition the
finished+merged+clean case on one bit, so a session is never both reaped and notified.

## Decisions (ratified forks, guided session)

- **ADR-23 — channel:** mailbox FYI (`intent: fyi` = surface-only) addressed to the operator
  handle (`current_user_id`, the identity the per-turn `aida awaiting` notice reads its *own*
  inbox as), deduped per session. Rejected: a bespoke worktree-local sentinel (new surface the
  design didn't name) and a brief (semantically "here's work", not an FYI).
- **ADR-24 — chain posture:** CHAIN's first increment is print/suggest handoff, never
  auto-launch. Filed as child **TASK-1179**; not in this PR.
- **Scope:** NOTIFY only this PR; CHAIN deferred to TASK-1179.

## Files (build order)

1. `aida-cli-lib/src/session_reap.rs` — `session_should_notify` (pure predicate), `NotifyRow` +
   `ReapReport.notifiable`, `session_notice_path` (dedup sentinel), `notify_finished_live_sessions`
   (delivery leg), and the wiring in `run_session_reap` (fires before the reap decision, on every
   real pass; `--dry-run` previews). Post-merge hook (`pr_cmd.rs`) already calls `run_session_reap`,
   so NOTIFY rides it with no change there.
2. `aida-cli-lib/src/tests/fr_284_session_notify_tests.rs` — the NOTIFY predicate matrix +
   sentinel-path confinement.
3. Docs: `docs/session-lifecycle.md`, `docs/cli/06-roles-sessions.md`, `CLAUDE.md`.

## Critical files

- `session_reap.rs` — the whole slice lives here; the delivery leg is best-effort (a mailbox
  write failure degrades to a skip, never fails a reap pass or a landed PR).

## Reusable helpers (not reimplemented)

- `classify_agent_worktree` / `AgentWorktreeFacts` — the shared worktree-GC merge/dirty gate.
- `aida_core::mailbox::{Message, Recipient, Intent}` + `mailbox_store::write_message` — the exact
  Message-construction path `aida mailbox send` uses.
- `current_user_id` — the operator-handle resolver the awaiting headline keys on.

## Risks + gotchas

- **Cross-user routing (accepted):** the FYI is addressed to the reaper's `current_user_id`. In
  the common single-operator, same-machine case that equals the target session's handle. A
  cross-user session on one host would route to the reaper's inbox instead — a non-destructive miss,
  documented in the code.
- **Idempotency:** sentinel content is the spec id, so a worktree lease reused for a *different*
  spec re-notifies; the sentinel is dropped only after the message lands, so a write failure retries.
- **HARD BOUNDARY preserved:** detect-and-notify only — no terminal scraping, no force-close.

## Tests

`fr_284_session_notify_tests.rs`: `finished_merged_but_live_session_is_notified`,
`squash_merged_live_session_is_notified_on_the_forge_signal`,
`an_exited_session_is_reaped_not_notified`, `unfinished_spec_is_not_notified`,
`unmerged_live_session_is_not_notified`, `dirty_live_session_is_not_notified`,
`locked_live_session_is_not_notified`,
`squash_merged_live_session_with_extra_unique_commits_is_not_notified`,
`notify_and_reap_are_mutually_exclusive_over_liveness`,
`session_notice_path_is_confined_to_the_notices_dir`.

## Verification

```bash
env -u AIDA_SESSION_ROLE cargo test -p aida-cli-lib --lib session_reap   # 25 pass (16 reap + 9 notify)
cargo fmt --all -- --check
aida session reap --dry-run --json                                       # notifiable[] present, no panic
```

## Followups

- **TASK-1179 (CHAIN):** after a reap, suggest (opt-in: execute) the next-queued-spec handoff.
  Detect-and-suggest, never auto-launch first (ADR-24).

## Related

- TASK-1177 (REAP slice, shipped), FR-284 (parent), ADR-23, ADR-24, TASK-1179.
- `docs/session-lifecycle.md`, `docs/architecture/autonomy-and-escalation.md`.
