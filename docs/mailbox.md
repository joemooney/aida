# The inter-agent mailbox

*Last updated: 2026-06-09. Surfaces: `aida mailbox` (CLI) + `send_message`/`read_inbox` (MCP). Implementation: STORY-493 (hybrid local + git-canonical mailbox).*

AIDA's mailbox is a lightweight **peer-to-peer messaging channel** between agents (and you) — for the *out-of-band* things that aren't a state change: a heads-up, an escalation, a question, a "I touched shared file X." It is **durable and cross-vendor**: a message survives every session ending and can be read by a different agent — even a *different vendor's* agent — tomorrow.

---

## When to use the mailbox (and when not to)

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
# broadcast to everyone
aida mailbox send "CI infra is flaky tonight, expect retries" --broadcast
# reply / thread
aida mailbox send "done, go ahead" --to codex --in-reply-to <msg-id>
aida mailbox send "..." --thread <thread-id>

# read your inbox (messages addressed to you + broadcasts, oldest-first)
aida mailbox inbox
# read someone else's inbox
aida mailbox inbox codex

# read a full conversation
aida mailbox thread <thread-id>

# persist the local layer into the durable git-canonical store
aida mailbox sync
```

**Identity.** Your "agent id" for sending/receiving is the shell's agent/user identity (the same `AIDA_USER` / role / user resolution the queue uses). `aida mailbox inbox` with no argument reads *your* inbox; pass an agent id to read another's. Agent ids are the agent names — `claude`, `codex`, `antigravity`, etc.

---

## A message's anatomy

Each message carries:

- **`from`** — the originator (agent id)
- **`to`** — a specific agent (`Recipient::Agent`) **or** a broadcast (`Recipient::Broadcast`)
- **`timestamp`** — when it was sent
- **`body`** — the text
- thread linkage — `--thread` / `--in-reply-to` group messages into conversations

There is **no priority/urgency field today** (see [Current limitations](#current-limitations)).

---

## Storage model — fast local + durable digest

A hybrid, two-layer design (STORY-493):

- **Local fast layer** — `<project>/.aida/mailbox/`, written atomically. This is the live exchange surface agents read/write during a session. It's runtime state under the `.aida/*` deny-by-default gitignore (per-clone, not committed by default).
- **Durable digest** — `aida mailbox sync` digests the local layer into the **git-canonical store on the orphan `aida-store` branch**, idempotently, and commits it. Once synced, messages are **durable, replayable, and shareable across clones** — another machine that pulls the store sees them.

So: *during* a session, messages flow through the local layer; `aida mailbox sync` makes them permanent and portable.

---

## Cross-vendor: MCP equivalents

Any MCP-speaking agent (Codex, Cursor, etc.) participates through two MCP tools:

- **`send_message`** — the equivalent of `aida mailbox send`
- **`read_inbox`** — the equivalent of `aida mailbox inbox`

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

- **No mailbox overview.** You can read one agent's inbox, but there's no `aida mailbox list` (agents with mail + unread counts) and no `aida mailbox inbox --all` operator-wide view. The mailbox is currently write-and-hope-they-read.
- **No priority/urgency.** Messages are purely chronological; there's no way to mark one urgent or surface it (e.g. in the statusline).

Both are proposed as a follow-up; see the mailbox-UX spec in the backlog.
