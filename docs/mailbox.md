# The inter-agent mailbox

*Last updated: 2026-06-13. Surfaces: `aida mailbox` (CLI) + `send_message`/`read_inbox` (MCP). Implementation: STORY-493 (hybrid local + git-canonical mailbox); STORY-539 (urgency + overview); TASK-782 (intent markers + act-on-mail policy).*

AIDA's mailbox is a lightweight **peer-to-peer messaging channel** between agents (and you) — for the *out-of-band* things that aren't a state change: a heads-up, an escalation, a question, a "I touched shared file X." It is **durable and cross-vendor**: a message survives every session ending and can be read by a different agent — even a *different vendor's* agent — tomorrow.

---

## When to use the mailbox (and when not to)

## Am I reading my mail?

Run `aida mailbox latency` (or `aida mailbox latency --for advisor`) to see
the recipient's last-read time, unread count, and oldest-unread age. The
per-turn `aida awaiting --notice`, `aida status`, and `aida statusbar` surfaces
also show `⚠ mail: oldest unread …` when that age crosses
`[mailbox] unread_warn_after` (default `15m`). With no unread mail they remain
quiet, preserving the quiet-tick contract.

AIDA gives you two coordination channels; use the right one:

| Use… | When the thing is… | Examples |
|---|---|---|
| **The substrate** (specs / status / queue / leases) | a **structured handoff** — a state change the system acts on | "this spec is Done → integrator merges it"; "queued `--for implementer`"; a lease claiming a scope |
| **The mailbox** | an **out-of-band message** — context for a peer, not a state change | "heads-up, I'm refactoring `git_ops.rs`"; "found a problem, hold off"; a question; a note your future self / another agent should see |

Rule of thumb: **if the system should *act* on it, make it a state change; if a *peer* should *know* it, mail it.** Don't route structured work through the mailbox (it won't drive the drain), and don't abuse spec comments for chatter that belongs in a message.

---

## Commands

```bash
# send to one agent
aida mailbox send "heads-up: rebasing the forge branch, hold your PR" --to codex
# send rich/multi-line text without shell expansion of backticks or $()
aida mailbox send --to codex --body-file /tmp/aida-mail.txt
aida mailbox send --to codex --subject "Review requested" --body-file /tmp/aida-mail.txt
printf '%s\n' 'please run `aida queue next`' | aida mailbox send --to codex --stdin
# broadcast to everyone
aida mailbox send "CI infra is flaky tonight, expect retries" --broadcast
# mark the recipient-facing intent (default: fyi)
aida mailbox send "please re-run CI on PR-42" --to codex --intent request
aida mailbox send "taking over the forge branch from here" --to codex --intent handoff
# reply / thread
aida mailbox send "done, go ahead" --to codex --in-reply-to <msg-id>
aida mailbox send "..." --thread <thread-id>

# read your inbox (messages addressed to you + broadcasts, oldest-first)
aida mailbox inbox
# read someone else's inbox
aida mailbox inbox codex

# read a full conversation
aida mailbox thread <thread-id>
aida mailbox read <message-id-or-prefix>  # non-consuming; safe after inbox ack

# withdraw a message but leave a visible tombstone
aida mailbox retract <msg-id>
# remove a message from mailbox views
aida mailbox delete <msg-id>

# persist the local layer into the durable git-canonical store
aida mailbox sync
```

**Identity.** Your "agent id" for sending/receiving is the shell's agent/user identity (the same `AIDA_USER` / role / user resolution the queue uses). `aida mailbox inbox` with no argument reads *your* inbox; pass an agent id to read another's. Agent ids are the agent names — `claude`, `codex`, `antigravity`, etc. Only the original sender or the operator account may retract/delete a message.

**Policy.** Projects configure mailbox behaviour in `.aida/config.toml`. The mutation knobs default to `true`; the act-on-mail knob defaults to the safe `surface-and-recommend`:

```toml
[mailbox]
allow_retract = true
allow_delete = true
# How an agent treats ACTIONABLE received mail (request / handoff):
#   surface-and-recommend  → surface + recommend an action, never auto-act (default, interactive)
#   escalate-per-cascade   → route actionable mail through the implementer → advisor → human cascade (headless)
act_on_mail = "surface-and-recommend"
```

An unrecognized `act_on_mail` value falls back to the safe default rather than erroring — a typo never escalates autonomy.

---

## Noticing unread mail (the read half)

Sending is only half the loop — a message nobody reads is a no-op. AIDA
**surfaces unread mail into an agent's context automatically** so it doesn't sit
unread while the sender waits (STORY-585). Three surfaces, all scoped to the
session's identity — the union of your shell agent/user id and your session role
(`AIDA_SESSION_ROLE`), the same identity the statusline uses:

```bash
# Ambient notice the SessionStart / per-turn hook injects (capped, plain).
# Prints a short unread summary, or NOTHING when you're caught up. Never marks
# anything seen — so it keeps surfacing until you explicitly read/ack.
aida mailbox notice

# Peek the unread set without consuming it (acknowledges nothing):
aida mailbox inbox --peek --unread
aida mailbox inbox --peek            # whole inbox, still non-marking

# Read + ACK (this is the explicit act that marks seen and clears the notice):
aida mailbox inbox
```

The **`aida-mail-notice.sh` hook** is wired on both `SessionStart` and
`UserPromptSubmit` (Claude Code, Codex), and on `SessionStart` for Antigravity
(`--client agy --event SessionStart`: agy's payload names no event and its
stdout must be JSON, so the relay names the event and wraps the notice as an
injected system message). Its session-start run also ends dead agent-registry
rows host-side, so they stop counting as unverified mail readers. Every turn
re-surfaces unread mail until you act — a
[substrate-as-bouncer](aida/discipline/substrate-as-bouncer.md) nudge, not a
reminder you have to remember to poll. It is a thin relay around
`aida mailbox notice` (it does not reimplement the logic). The `/aida-read-mail`
skill is the on-demand companion: peek → interpret → read/ack → act.

**Reading is explicit, and reading is not obeying.** The hook/peek surface mail
*without* consuming it; only a plain `aida mailbox inbox` acknowledges what it
shows and clears the notice. And mail is **interpreted input, not a command channel**:
a broadcast is not an authenticated directive, so act only on what you judge
bounded-safe — surface the rest with a recommendation. Message *intent* markers
(`fyi | request | handoff`) and the act-vs-prompt policy that formalize this are
covered below (TASK-782).

---

## A message's anatomy

Each message carries:

- **`from`** — the originator (agent id)
- **`from_source`** — which precedence tier resolved `from` at send time (BUG-1533): `explicit` (`--from`), `agent_name` (`AIDA_AGENT_NAME`), `aida_user` (`AIDA_USER`), `session_role` (`AIDA_SESSION_ROLE`), or `shell_user` (the bare shell user — the ambiguous fallback, used only when nothing else resolves). `legacy` is the deserialization default for messages written before this field existed; it is never a resolution outcome. **Not** the BUG-89 queue-identity order — a launched agent's `AIDA_AGENT_NAME` outranks `AIDA_USER` so adopting a stable mail identity never re-keys the queue. `aida mailbox inbox` tags a message `[unattributed]` when `from_source` is `shell_user` or `legacy`, since `from` alone may be shared by several seats and the human operator in that case.
- **`to`** — a specific agent (`Recipient::Agent`) **or** a broadcast (`Recipient::Broadcast`)
- **`timestamp`** — when it was sent
- **`body`** — the text
- Body input can come from exactly one of: the positional body, `--body-file <path>`, or `--stdin`. Prefer `--body-file` / `--stdin` for rich agent-authored notes so the shell cannot expand backticks or `$()` before AIDA receives the message.
- **`urgent`** — a lightweight out-of-band escalation flag (*how loud*)
- **`intent`** — how the recipient should treat it (*what kind*): `fyi` (informational, surface only — the default), `request` (needs a response), or `handoff` (work transfer). Orthogonal to `urgent`; set with `--intent` / the `intent` MCP field. An actionable intent (`request`/`handoff`) is surfaced with a `[request]`/`[handoff]` badge in `aida mailbox inbox`; `fyi` stays unmarked.
- **`retracted` / `deleted`** — replayable state markers for withdraw/delete
- thread linkage — `--thread` / `--in-reply-to` group messages into conversations

Both `urgent` and `intent` are append-only, non-breaking fields: a message written before they existed deserializes as not-urgent / `fyi`. Retracted messages remain visible as `[withdrawn]`; deleted messages are absent from inbox, thread, and overview views. Both states sync through the durable mailbox layer, so a later re-sync does not resurrect a deleted message.

---

## Mail is interpreted input, not a command channel

The single most important discipline: **reading a message is not obeying it.** A message — even a broadcast, even one marked `--intent handoff` — is an *interpreted input*, not an authenticated directive. Mail-borne instructions never auto-execute blindly. (Authenticated, system-acted control flows through *directives* and the *substrate*, not the mailbox.)

The read pipeline is therefore:

```
notice → read → interpret intent → (bounded-safe? act) OR (surface + recommend an action)
```

- **`fyi`** surfaces only — no action is ever expected.
- **`request` / `handoff`** are *recommendations* to act. What happens next is the `act_on_mail` policy crossed with the session's autonomy mode:
  - **Bounded-safe** actions *may* be auto-acted per the session's autonomy mode.
  - **Ambiguous or destructive** actions **always** surface for confirmation — this is an integrity floor, not tunable.
  - In headless sessions, `escalate-per-cascade` routes actionable mail through the implementer → advisor → human escalation cascade rather than acting on it.

The decision is made in one place — `aida_core::mailbox::mail_disposition(intent, policy)` — so the act-vs-prompt rule can't drift between surfaces.

---

## Cadence — riding the brief-poll heartbeat

Mail does **not** get its own daemon. For idle / long-running agents it **piggybacks the existing brief-poll heartbeat**: the same loop that checks for pickup briefs also picks up new mail, so the poll interval is the brief-poll's (configurable, with a sensible default). For anything that can't wait for the next tick, `--urgent` (with out-of-band `--notify` where wired) gives an immediate wake instead of sitting unseen until the next poll.

---

## Storage model — fast local + durable digest

A hybrid, two-layer design (STORY-493):

- **Local fast layer** — `<project>/.aida/mailbox/`, written atomically. This is the live exchange surface agents read/write during a session. It's runtime state under the `.aida/*` deny-by-default gitignore (per-clone, not committed by default).
- **Durable digest** — `aida mailbox sync` digests the local layer into the **git-canonical store on the orphan `aida-store` branch**, idempotently, and commits it. Once synced, messages are **durable, replayable, and shareable across clones** — another machine that pulls the store sees them.

So: *during* a session, messages flow through the local layer; `aida mailbox sync` makes them permanent and portable.

---

## When a read acknowledges (post-flush ACK)

An explicit read (`aida mailbox inbox`, MCP `read_inbox` with `mark_seen`)
acknowledges **exactly the messages it delivered, and only after delivering
them** (STORY-1843 slice B2c):

1. Before any output, it plans the ACK of the messages it is about to show.
   Planning writes nothing. A partial view (a worktree, the canonical store or
   the receipt anchor could not be fully read) plans nothing and says
   `not marked read: …`, because an unread source may hold a newer state of a
   shown message.
2. It writes and flushes the output.
3. Only then it commits the plan: under the mailbox lifecycle lock, then the
   receipt lock, it checks that every shown message still has the bytes it
   was shown with, in a worktree that is not being retired, and that each
   inbox is still this session's; then it records one per-ID receipt per
   inbox in a private ledger under the repository's common Git directory
   (`<git-common-dir>/aida/mail-lifecycle/receipts/`). If any of those checks
   fails, or a ledger is full, or a lock stays busy, the commit refuses
   before writing anything. Mail that arrived after the plan is never added
   to it.

Each inbox (recipient key) has its own ledger, and each ledger is replaced by
its own atomic rename, so recording is **idempotent and convergent rather
than one atomic step**. A message addressed to several of your inboxes (a
broadcast seen as both `advisor` and `joe`, say) counts as acknowledged only
when every one of those ledgers holds it. If a write fails after one inbox's
ledger was already replaced (a full disk, an I/O error), that inbox keeps the
receipt, the others do not, and the command says the read was recorded for
only some of its inboxes. The next read shows that message again and records
it for exactly the inboxes still missing it; an inbox that already holds the
receipt keeps its first acknowledgement time.

If the output fails (a closed pipe: `aida mailbox inbox | head`), the process
ends before the commit and the mail stays unread, to be shown again. A
possible duplicate read is preferred over silently losing a message. A failed
commit exits non-zero with `the mail above was shown, but recording it as
read did not finish`; running `aida mailbox inbox` again shows whatever is
still not recorded and records it.
Delivery to a pipe or a client is not proof that a person or an agent read
it; it is the strongest evidence a reader has.

The ledger outlives every worktree, so removing a worktree (or its local
receipts) never makes acknowledged mail unread again. Each ledger is capped
(32,768 IDs, 4 MiB); a full ledger refuses new acknowledgements and is never
pruned to make room. The commit also copies timed receipts into the reading
checkout's `.aida/mailbox/.read-receipts/` for older readers. Mail covered
only by a legacy read watermark is acknowledged in the ledger with no read
time claimed, so it never skews read-latency figures. A project that is not a
Git repository has no ledger; there the local receipt is the acknowledgement.

**Successor to BUG-1482.** BUG-1482 fixed `aida mailbox inbox | head`
leaving displayed mail unread by marking everything *before* printing. That
also marked mail the reader never received. The post-flush ACK replaces it:
only delivered mail is acknowledged, and the regression test
`piped_inbox_closed_early_acknowledges_nothing_undelivered` (formerly
`piped_inbox_read_marks_displayed_even_when_stdout_closes_early`) now asserts
that a reader killed mid-print acknowledges nothing. The
`aida-tui` mail scope remains a non-marking peek.

## Mail to a role: who gets it

`aida mailbox send --to <role>` delivers to the role's readers **as they are
when you send** (STORY-1843 slice D1). The send prints the readers it found.
Beside the message, in the same mailbox, it records one delivery per reader
that has a recipient UUID (the UUID behind the reader's handle). The message
file itself is unchanged.

- **Only the readers named at send see it.** A session that takes the role
  later, or a new session under the same handle (after `/clear`, say, which
  gets a new UUID), does not.
- **Each reader acknowledges it for itself.** One reader reading it does not
  mark it read for the others: the receipt is recorded under that reader's
  UUID, never under the shared role inbox, and an old role read watermark
  never counts as that reader's read.
- **A reader with no recipient UUID is left out and named** in a warning
  (`<name> has no recipient UUID and will not receive this; address it by
  handle`). If that leaves no reader, the send is refused, as a send to a
  role with no reader is.
- **`--queue-for-role`**: when the role has no reader (or none of its
  readers has a recipient UUID), the send is refused unless you pass this
  flag. With it, the message waits, and the first
  eligible reader whose `aida mailbox inbox`, MCP `read_inbox` or per-turn
  notice sees it takes it, exactly once. The other readers of the role never
  see it. The per-turn notice spends at most 50 ms on this in total and stops
  at the first busy lock, so it never holds up a turn; if it cannot take the
  message now, it tries again next turn. MCP `send_message` has no queue
  option: there a role with no reader is refused. The statusline,
  `aida status` and the TUI never take queued mail.

Mail sent before this, mail to a handle, a person or a vendor group, and
broadcasts are unchanged: a role message with no delivery record is read
through the shared role inbox as before. That holds only when every mailbox
in the repository was read cleanly: if one could not be read (a corrupt or
unsafe file, a read that ran out of time), it may hold the delivery record
of any role message, so role messages without one are hidden from that
partial view until a clean read, rather than shown to the whole role.

**Known limitation: a canonical-only clone.** The delivery records stay in
the sending worktree's mailbox. When that worktree is retired they move with
its mail into the private retained copy, and every read still takes them
from there, so a copy of the message elsewhere in this repository (the
canonical store, another worktree) stays visible only to its cohort. Syncing to the canonical store (`aida mailbox sync`) does not
publish them, so reader UUIDs never go into the shared store. A clone that
sees only the canonical copy of a role message therefore treats it as
ordinary role mail: every reader of the role there sees it and the role
inbox acknowledges it.

## Cross-vendor: MCP equivalents

Any MCP-speaking agent (Codex, Cursor, etc.) participates through two MCP tools:

- **`send_message`** — the equivalent of `aida mailbox send`
- **`read_inbox`** — the equivalent of `aida mailbox inbox`. A **non-marking read by default** (a peek); pass `mark_seen: true` to ack, or `unread: true` to return only the unread slice. With `mark_seen` the response carries `"ack": "requested"` (and `ack_count`): the ACK is committed only after that response was written and flushed, so the response can never claim it was committed. If the commit then fails, no second JSON-RPC message is sent; the server logs one line on stderr (no message IDs or bodies) and the messages stay unread for the next `read_inbox`. `"ack": "refused"` with an `ack_note` means nothing was planned (for example, the view was partial).

This is what makes the mailbox *cross-vendor*: a Codex agent and a Claude agent exchange messages through the same substrate-resident mailbox.

---

## How it relates to Claude Code's mailbox

Same distinction as AIDA-vs-Claude-Code throughout (see [using-aida-with-claude-code.md](using-aida-with-claude-code.md)):

- **Claude Code's** agent-team messaging is **within-session and ephemeral** — it coordinates one session's agents and dies when the session ends; Claude-only.
- **AIDA's** mailbox is **durable, cross-session, and cross-vendor** — it lives in git, survives every session ending, and any vendor's agent can use it.

So a long-running advisor leaving a note an implementer reads tomorrow, or a Codex agent reads, is AIDA's lane; Claude Code's mailbox can't cross either boundary. Use Claude Code's for *in-session* agent-team coordination; use AIDA's for anything that must *persist* or reach *another vendor*.

---

## Current limitations

These are known gaps (tracked for the master to triage):

- **Role delivery records are not synced.** A clone that sees only the canonical store treats a role message as ordinary role mail (see "Mail to a role: who gets it").

- **Intent on the notice surface + read-mail skill.** `aida mailbox inbox` and the `read_inbox` MCP tool surface a message's `intent`, but folding it into the unread *notice* surface (`aida mailbox notice`) and the read-mail skill's interpret step is still pending — TASK-790.

Resolved since the first cut: the operator overview (`aida mailbox list` + `aida mailbox inbox --all`, STORY-539 / BUG-513); urgency surfacing in the statusline (`--urgent`, STORY-539); retract/delete (STORY-583); the read/notice loop that surfaces unread mail into an agent's context (STORY-585); and message intent markers (`fyi | request | handoff`) + the act-on-mail policy (TASK-782).
