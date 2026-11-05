# Claude exact-session mailbox wake sketch

Status: proposed; needs master sign-off before implementation.
Owner: TASK-3072. <!-- trace:TASK-3072 | ai:codex -->

## Evidence and design fork

Inspected installed Claude Code 2.1.318 and current AIDA source on 2026-11-05.
Claude's [cross-session messaging documentation](https://code.claude.com/docs/en/cross-session-messaging#the-sessions-inbox-socket)
exposes `CLAUDE_CODE_MESSAGING_SOCKET` and a session token to hooks.
Its page documents auth framing but does not specify the posted message schema
or a machine-readable acceptance response. Busy sessions receive peer messages
between tool calls; idle sessions can start a turn. Inbound settings can hold
or refuse a message, so a socket write cannot establish delivery to the model.

The [channels documentation](https://code.claude.com/docs/en/channels)
offers an MCP push transport, with explicit enablement and provider restrictions.
That alternative overlaps approved SPIKE-40 and changes the MCP/launch contract.
Neither route is equivalent to starting `claude --resume`: installed CLI help
explicitly says background resume may copy an already running session.

Decision required: approve a socket adapter after establishing its supported
message/response contract, or approve a session-owned channel implementation
coordinated with SPIKE-40. Recommend the socket route if its contract can be
verified without inventing a private protocol. Do not infer a supported wire
schema from a successful connection or silently change inbound settings.

## Reusable helpers

- `mailbox_wake_run.rs`: reuse logical snapshot, batching, accepted-ID ledger,
  inflight uncertainty and explicit recovery. Keep the Codex transport intact.
- `mailbox_wake.rs`: preserve exact recipient audience and watcher authority.
- `agent_registry.rs`: SessionStart already records Claude session ID and
  process-start identity; bind endpoint registration to that exact row.
- `mail_wake.rs`: extend vendor capabilities only once transport is verified.
- `mail_backlog.rs` and mailbox hooks: retain operator and next-turn fallback.

## Proposed contract after approval

Capture endpoint binding only from the receiving session's own SessionStart;
never discover a target by name, latest-session lookup or socket globbing.
Validate session ID, PID/start identity, owner and private socket ancestry;
refuse dead, ambiguous, foreign, replaced or missing bindings. Keep tokens out
of the shared store, logs and public registry. Decide whether token capture is
needed at all for a Linux host watcher; never claim own-child authority on its
behalf. Retire bindings on session end and reject stale recovery identities.

Durable `kind=request` mail remains canonical. Wake carries only a bounded
notice telling the exact recipient to read its inbox. Accepted wake IDs dedup
subsequent passes; timeout after possible submission remains uncertain until
explicit recovery. Defined rejection can retry; transport success alone is
not read ACK or acted disposition. Preserve existing post-flush mailbox ACKs.

## Validation before claiming completion

Test exact-session isolation with two simultaneous sessions, busy and idle
delivery, inbound accept/hold/refuse, endpoint replacement, dead PID and PID
reuse, unsafe paths, batching, duplicate IDs, rejection retry, ambiguous
submission, recovery, restart and fallback. Use bounded fake transport tests
plus an explicitly scoped live transport probe; do not wake unrelated agents.
Run targeted Rust tests, formatting and diff checks. No production adapter or
new schema is implemented by this sketch; TASK-3072 must remain unfinished.
