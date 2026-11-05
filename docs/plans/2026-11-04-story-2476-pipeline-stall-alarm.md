# Pipeline stall alarm sketch

Status: proposed; needs master sign-off before implementation/PR.
Owner: STORY-2476. <!-- trace:STORY-2476 | ai:codex -->

## Contract and decisions needed

The description requires an alarm after 30 minutes with zero implementers,
no ship-ready PR, and blocked PRs or queued work. It explicitly requires silence
while any lane runs or a PR is ready. The operator's subsequent comments add
under-cap lanes, ready PRs without a train, no merge for 90 minutes, unread
advisor mail, rework exits without new commits, vendor wake adapters, action
tracking, and escalation after repeated nudges.

Proposed interpretation: keep the original pipeline-idle alarm's suppression
rules, and treat the added conditions as separate alarm kinds. Master must
confirm this interpretation: a ready PR would suppress the original alarm but
trigger the added ready-without-train alarm. Similarly, one running lane would
suppress pipeline-idle but could trigger under-cap. Do not silently choose one
contract or mark the full story complete after implementing only its first part.

## Existing evidence and reusable helpers

- `aida-cli-lib/src/supervise_cmd.rs`: `run_watch_pass` is the oversight tick;
  `nudge_pass` already resolves a live advisor and uses the mailbox, with
  operator-notification fallback. Its dry-run path must remain side-effect free.
- `aida-cli-lib/src/status_prs.rs`: reuse the open-PR snapshot and check rollup.
  `idle_eligible.rs` already composes current-head local review verdicts, merge
  holds, live reviewers, mergeability and CI. Its idle-age filter must not hide
  a newly ready PR from the suppression check.
- Session leases and registry process identities provide project-scoped lane
  liveness. Reuse those identities; the prototype's global `pgrep -fc` patterns
  can count unrelated projects and miss other launch forms.
- `mailbox_view.rs` and `aida-core` mailbox lifecycle ACKs provide logical
  cross-worktree snapshots, partial-read coverage and post-output-flush read
  receipts anchored in common Git. Read ACKs do not prove action.
- `mailbox_wake_run.rs`: `watch_row` rejects non-Codex vendors explicitly.
  Reuse its existing batching, acceptance ledger and uncertain-submission
  recovery. Do not add a second Codex delivery channel.
- `docs/agents/codex-mail-wake.md` documents exact-thread wake, opt-out,
  authority checks and unsupported Antigravity live wake.
- Inspected host prototype:
  `~/.local/share/aida/ops/advisor-scripts/stall-check.sh`. Its default-branch
  commit age is not necessarily last-merge age; preserve that distinction.

## Proposed detector and persistence

Implement a vendor-neutral observation/evaluation module, invoked by oversight
watch. Obtain bounded snapshots once per pass: PR state and named failing
checks, project leases/active processes, queues, and advisor logical mail.
Unknown or incomplete evidence must not establish zero live lanes, zero ready
PRs, an empty mailbox, or a successful action. Reset the corresponding timer
on an observation gap so an unobserved interval cannot satisfy persistence.

Use a permanent nonblocking lock and atomic replacement for disposable,
versioned per-project runtime state at the main worktree. Track alarm kind,
first continuously observed time, last successful mail time, episode/message
identity and nudge count. Default pipeline-idle threshold: 30 minutes. Expose
thresholds through the existing advisor config only after schema agreement.
Reset episodes when their predicate clears; serialize concurrent ticks.

On a due alarm, send one bounded digest via the existing mailbox writer with
`kind=request`, addressed through live advisor resolution. Include each PR's
current-head review blocker, failing check names, conflict and stale lease
evidence; include queue rework or existing session-end/release commands with
exact targets. Advice never releases leases, launches lanes or merges PRs.
Retain pending delivery on failure; don't claim a nudge was sent. Avoid sending
another copy every tick or counting the detector's own unresolved notification
as a new unread-mail episode.

Unread advisor mail needs the logical mailbox audience/inspection policy;
never synthesize an audience merely from the string `advisor`. Add oldest-age
to the existing hook notice without acknowledging mail from a passive read.

## Action receipts and vendor delivery boundary

Proposed action lifecycle: explicit recipient disposition tied to a message ID,
with outcome (acted, declined, or deferred), timestamp, and evidence/reason.
Reading, wake acceptance, a reply, or process exit alone must not imply acted.
Store this as an additive lifecycle record using the existing trusted receipt
anchor and authorization checks, rather than rewriting canonical message YAML.
Master must approve the record/CLI contract, treatment of legacy messages,
and which dispositions pause escalation before this is implemented.

For Codex, existing mailbox wake delivers the request unchanged. For Claude
and Antigravity, first establish a supported exact-session, live-delivery
primitive and its busy-session/idempotence semantics. Do not substitute a
new resumed process for injection into the stalled session. Until verified,
report unsupported delivery truthfully and retain hook/mail fallback; this
does not meet the expanded live-delivery acceptance. Master must choose whether
vendor capability work blocks this story or is split into linked requirements.

Repeated unresolved alarms should use the existing operator notifier, with a
configured nudge cap and cooldown; neither wake queue acceptance nor read ACK
resets the unresolved episode. Threshold/default and opt-out policy need the
same sign-off as the action lifecycle.

## Implementation and validation after sign-off

1. Detector/state/config and oversight integration, including dry-run output.
2. Actionable mailbox digest, deduplication/retry and unread-age hook display.
3. Approved action receipts and unresolved-request escalation.
4. Verified vendor adapters (or explicit linked dependency split).

Test the real tick wiring with an injected clock, forge, processes and mailbox:
29/30-minute boundary; lane-start/ready-PR suppression; queue-only stalls;
current-head versus stale verdicts; named red checks/conflict/stale lease
commands; unknown/partial snapshots; episode resets; concurrent ticks;
failed send/retry; deduplication; unread versus read-but-unacted requests;
self-alarm exclusion; escalation cap; and dry-run with no writes or sends.
Vendor tests must distinguish queue acceptance, read ACK and acted disposition,
and refuse uncertain/foreign/dead session targeting. Run targeted Rust tests
and `cargo fmt --all -- --check` for implementation changes.

No production changes have been made by this sketch.
