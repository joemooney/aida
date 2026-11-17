# Blocked-seat operator alerts

<!-- trace:BUG-2631 | ai:codex -->

`aida notify check` checks live role-bearing sessions for confirmed permission
prompts, Claude cross-session waiting/blocked status and failed advisor handshakes, even when the blocked session cannot run
another hook. The project schedules this check every five minutes. The host
scheduler must be running; a stalled model's own turns are not the timer.

```toml
[seat_health]
blocked_after_mins = 15
push_commands = ['notify-send -- {title} "$(cat)"']

[[schedule.jobs]]
name = "seat-health-guard"
command = "notify check"
every = "5m"
enabled = true
```

The threshold defaults to 15 minutes (positive minutes, maximum one week).
Alerts repeat after 15, 30, 60, then 120 minutes until the block disappears.
Cross-session waiting age starts at its first observed check; subsequent checks
retain that timestamp until a successful observation clears the wait. A failed
cross-session query retains prior waiting evidence. The query is capped at two
seconds.

A clear notice identifies the prior block when it is no longer observed; a
terminated session also clears its block. Successful delivery updates a shared,
locked ledger under `.aida/seat-health/`; overlapping checks skip the busy lock.
Failed delivery retries on the next check. If one channel fails after another
succeeds, retrying may repeat delivery to the successful channel.

Every configured transport is attempted: the existing `[notify] command` and
all `seat_health.push_commands`, with identical commands deduplicated. Add the
host's ntfy or other push command to this list to enable additional channels.
Commands receive the full alert on stdin; `{title}` and `{rule}` substitutions
are shell quoted. They are authorized only by the reviewed default-branch
configuration, so an unmerged lane cannot install push commands. Each command
has a five-second timeout. Health alerts bypass ordinary notification quiet
hours and rate limits because they have their own backoff. With no configured
transport, no delivery is recorded; `aida agent ls` still exposes the evidence.

Alerts identify the session handle, role, terminal, duration and prompt text
when the permission notification captured it. Unread counts use the same
subject-bound logical mailbox reader as inbox inspection, including role mail.
Counts are unknown when inspection is incomplete. The PR count covers the local
review watcher's pending/skipped changes across the project, not assignment to
a particular advisor; without watcher evidence it is unknown. It is not a full
forge backlog census.

`aida agent ls` shows current blocks to peers (including an optional
`operator_input` field on JSON rows). Non-advisor handshake hook notices also
repeat the evidence, with an instruction to escalate instead of re-pinging.
A failed handshake is labeled as such: it does not prove that a permission
prompt exists. Claude waiting status is reported as waiting for operator input.
Generic idle process states do not prove an input block. Permission markers for live sessions survive the old
30-minute expiry; orphaned markers retain the expiry cleanup backstop.

<!-- trace:BUG-2639 | ai:codex -->
Stopped relay rotations also enter this alert path from their durable fault
records, even with no live child. Their recorded stop time drives the same
threshold and delivery backoff. A successful successor binding for the same
handle resolves the fault and triggers a clear notice; ending the failed
controller alone does not clear it. The original diagnostic remains in history.
