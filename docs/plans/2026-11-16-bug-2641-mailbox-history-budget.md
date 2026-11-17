# BUG-2641: mailbox history compaction and unread discovery

Status: proposed; lifecycle sign-off required before implementation.

<!-- trace:BUG-2641 | ai:codex -->

## Verified problem

`mailbox_logical::read_source` charges all message payloads against a shared
32 MiB budget. Exhaustion discards that source's accumulated records.
`AckPlan::prepare_records` refuses a snapshot that is not ACK-eligible;
`story1843_b2c_partial_view_plans_nothing` explicitly protects this behavior.
`archive_mailbox_older_than`, also used by `mailbox gc`, writes archive markers
containing the original payload. It does not reduce the scanned bytes.
Existing lifecycle retirement preserves whole worktree sources, rather than
compacting individual records from an active mailbox.

## Proposed contract for approval

1. Preserve archived payloads outside active message discovery. Publish durable
   disposition evidence before reclaiming active payloads, under the existing
   lifecycle locking discipline. A crash before reclamation leaves duplicates;
   a crash after reclamation must leave recoverable payload and disposition.
   Canonical digest, sync, replica reconciliation, and archived lookup must honor
   the same disposition, so sync cannot resurrect compacted history.
2. Reclaim only mail with durable acknowledgement by every required delivery,
   or an explicit authorized archive disposition. Do not infer expiry from
   process absence or a dead handle. Unread broadcast/group/queued deliveries
   remain protected until their delivery obligations are resolved. A separate
   expiry policy needs an explicit retention period and authority decision.
3. Separate history materialization from unread discovery. An exhausted history
   budget may omit old bodies, but must not discard already validated unread
   records. Preserve per-file, directory-entry, and deadline safeguards; reaching
   those safeguards must still report incomplete discovery honestly.
4. Permit an ACK only with positive record-scoped completeness evidence: all
   possible replicas and their dispositions, ownership bindings, and receipt
   sources for that record were validated. Unknown or conflicting replicas,
   unreadable receipts, and incomplete source inventories refuse the ACK.
   History truncation alone may cease to be an ACK veto once that evidence exists.
   Merely allowing `AckError::Partial` or increasing the byte cap is insufficient.
5. Retain output-flush-before-ACK, source identity/fingerprint revalidation,
   lifecycle generation checks, and per-key receipt publication semantics.
   Handshake acknowledgement follows successful message ACK publication.
6. Make `aida mailbox gc` perform actual safe compaction, then name that command
   in the history-budget warning. Preserve archived inspection and report when
   protected unread mail prevents reclamation.

## Implementation and validation sequence

- Add and review the disposition/archive persistence and recovery contract first;
  then implement compaction and sync/reconciliation consumers together.
- Add record-scoped discovery evidence and ACK validation without weakening
  refusal for corrupt receipts, foreign sources, conflicting replicas, changed
  bytes, or unknown source inventories.
- Regress an over-32-MiB mailbox with reader unread mail: display full bodies,
  flush, ACK, and verify durable receipts on the next read. Include unread mail
  placed after the history that exhausts the old budget.
- Verify compaction reduces active scanned bytes, preserves archived lookup,
  survives interruption at each publication boundary, and does not resurrect
  mail after canonical sync or discovery from another worktree.
- Verify another recipient's unread mail and unbound queued deliveries survive
  compaction. Exercise multiple recipients and partial receipt publication.
- Verify one full advisor inbox read ACKs all delivered hellos, and handshake
  doctor agrees; failed output or ACK publication must leave hellos pending.
- Run targeted core lifecycle/logical tests and CLI mailbox/handshake tests via
  the host lane job runner, followed by formatting and source guards.

## Current handoff

No implementation or test execution is claimed. The prior spec sketch raised
this same lifecycle fork; the subsequent reapproval recorded no design decision.
Master sign-off on the proposed contract, or a replacement contract, is required
by the repository's Sketch-First Protocol. The configured reviewed codegraph
client `/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph` is absent; source
inspection used `rg`. Host deployment is required to restore graph navigation.
