# Cross-vendor inter-agent mailbox — live demo evidence (2026-06-14)

**Specs:** TASK-816 · **Status:** evidence (dated snapshot, immutable) · **Type:** empirical validation
**Validates:** [`2026-05-31-git-canonical-substrate-thesis.md`](2026-05-31-git-canonical-substrate-thesis.md) §"Multi-vendor is the strongest, truest claim" — specifically the assertion that *"any agent (Claude, Codex, Cursor, Antigravity) reads/writes the same store with no vendor API."*

> Frozen at time T per the immutability discipline. Supersede with a new dated file; do not retro-edit.

## What was demonstrated

A "hello world" inter-agent message was routed through AIDA's file-based, git-canonical mailbox to agents from **three different vendors**. Each agent — running only the `aida` CLI against the same store, with no shared vendor runtime, no daemon, and no network service — **detected** the message, **acted** on it (wrote a proof file), and **replied** back to the sender. Every reply round-tripped into the operator's inbox.

| Vendor (tool) | Agent identity | Detected | Acted (proof file) | Replied → joe |
|---|---|---|---|---|
| **Claude** (Claude Code) | `demo-bot` | ✅ | ✅ | ✅ |
| **Codex** (OpenAI Codex CLI) | `codex` | ✅ | ✅ | ✅ |
| **Antigravity** (Google) | `agy` | ✅ | ✅ | ✅ |

This is the multi-vendor substrate claim shown, not asserted: AIDA's coordination surface is just plain files in a git-canonical store plus the `aida` CLI / MCP interface, so any tool that can run a shell command participates as a first-class, addressable peer. App-local memory (Cursor Memories, Windsurf memories, Claude auto-memory) is structurally single-vendor and cannot do this — the value here is anchored on *incentive* (single-vendor runtimes won't make their state portable; it's against their lock-in interest), which ages better than any capability claim.

## Scope of the claim (precise, not over-claimed)

- **Proven:** the **inter-agent coordination surface** (mailbox send/inbox) operates identically across Claude, Codex, and Antigravity against one shared git-canonical store. All three drove the real `aida` CLI; the messages, acks, and replies are ordinary substrate state.
- **Strongly indicated but not separately exercised here:** the broader requirement-graph read/write surface (`list`/`show`/`add`/`edit`, MCP graph resource). The same CLI/MCP entry point serves it, so the same vendor-neutrality applies, but this demo did not file specs from each vendor.
- **Not claimed:** real-time multi-agent hot-loop coordination. Per the thesis, git's commit-per-write is deliberately too coarse for that; AIDA uses the SQLite cache + file handshakes there. The mailbox is durable message-passing, not a hot loop.

## Reproducible recipe

For each target agent (`<vendor>` ∈ {claude, codex, antigravity}; `<handle>` is its mailbox identity, e.g. `codex`, `agy`):

```bash
# 1. Stage a message addressed to the agent's mailbox handle
aida mailbox send --to <handle> "hello world — reply to confirm the cross-vendor mailbox works"

# 2. Launch the agent so it reads its OWN inbox (see caveat: AIDA_USER prefix)
AIDA_USER=<handle> aida agent new <vendor>

# 3. Brief the agent (paste): read `aida mailbox inbox`, write a proof file,
#    then `aida mailbox send --to joe "hello back from <vendor> — confirmed"`

# 4. Confirm the round-trip from the operator side
aida mailbox inbox            # the reply lands here; the unread-mail hook also fires
```

The 2026-06-14 run produced reply message ids `ccc38b23` (codex → joe) and `41bf0e58` (agy → joe), plus proof files `/tmp/codex-acted.txt` and `/tmp/agy-acted.txt` (demo artifacts since cleaned up). The earlier Claude/`demo-bot` leg used the same shape.

## Caveat surfaced by the demo — BUG-558

The demo required an `AIDA_USER=<handle>` prefix on the launch. Root cause: **`aida agent new` exports `AIDA_AGENT_NAME` / `AIDA_SESSION_ROLE` but not `AIDA_USER`**, and the mailbox/queue identity resolves the shell `USER`/`AIDA_USER` (BUG-89). So a launched agent otherwise reads the *launching human's* inbox, not its own — briefs route by agent-name while mailbox routes by shell-user, and the two disagree. Filed as **BUG-558** with the design resolution (a unique per-instance stable-name identity owns the private inbox; type/role remain non-unique group aliases for broadcast/any-of routing). Until fixed, the `AIDA_USER` prefix is the one-line workaround — it does not weaken the substrate claim, only the out-of-the-box ergonomics of the launcher.

## Bottom line

The strongest, truest line in the substrate thesis — *multi-vendor coordination on neutral git-canonical ground* — now has a re-runnable demonstration behind it across three independent vendors. Positioning may cite this as evidence rather than assertion, with the BUG-558 ergonomics caveat noted honestly.
